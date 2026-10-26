//! Phi-3-mini 与 Phi-4-mini（V4Mini）的本地推理。
//!
//! 这两个模型在 candle 里是**同一个实现**：`microsoft/Phi-4-mini-instruct` 的
//! `config.json` 里 `architectures` 是 `Phi3ForCausalLM`、`model_type` 是 `phi3`，
//! 所以参考例子 `candle-examples/examples/phi/main.rs` 把 V3 / V3-medium / V4Mini
//! 一起放进 `Model::Phi3` 那一支。下面这个循环就是那一支，逐项对应：
//!
//! - `model.forward(&input, pos)?.i((.., 0, ..))?`：candle 的 `forward` 只返回**最后
//!   一个位置**的 logits（形状 `(b, 1, vocab)`），取 `(.., 0, ..)` 才是下一步的分布；
//! - `pos` 就是 `seqlen_offset`：预填充那一步传 0，之后每步只喂一个新 token 并
//!   `+= context_size`，KV cache 靠它对齐；
//! - 停止符 `<|endoftext|>` **按字符串查词表**（见 [`eos_token_id`]），不写死 id：
//!   Phi-3-mini 是 32000，V4Mini（Phi-4-mini）是 199999。
//!
//! 与 V4Mini 有关、但不在这个文件里的两件事：
//!
//! - 权重 dtype：`huggingface.rs::load_phi3_model_files` 对 V4Mini 用
//!   `device.bf16_default_to_f32()`（照参考例子），因为它的权重是 BF16；
//! - 提示词：`HuggingFaceModelInfo::convert_prompt` 里 Phi-4-mini 走自己的官方
//!   chat 模板（开头没有 `<s>`、角色标记后没有换行），和 Phi-3 不同。

use candle::{DType, Device, IndexOp, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::phi3::Model;
use frand::Rand;
use tokenizers::Tokenizer;

use super::chat::ResultSender;
use crate::flow::rt::dto::StreamingResponseData;
use crate::result::{Error, Result};

// static TEXT_GENERATION_MODEL: OnceLock<Mutex<HashMap<String, (Model, Tokenizer)>>> =
//     OnceLock::new();

// pub(super) fn replace_model_cache(robot_id: &str, info: &HuggingFaceModelInfo) -> Result<()> {
//     let device = device()?;
//     let c = load_phi3_model_files(info, &device)?;
//     if let Some(lock) = TEXT_GENERATION_MODEL.get() {
//         if let Ok(mut cache) = lock.lock() {
//             cache.insert(String::from(robot_id), c);
//         }
//     }
//     Ok(())
// }

/// Phi 系列的停止符，**按字符串**从词表里查，不写死 id。
///
/// 同一个 `<|endoftext|>` 在两个分词器里 id 不同：Phi-3-mini 是 32000，
/// Phi-4-mini（V4Mini）是 199999，也就是各自 `config.json` 里的 `eos_token_id`。
/// 参考例子本来就是按字符串查的（`tokenizer.get_token("<|endoftext|>")`），这一点
/// 正是同一份循环能同时服务 V3 和 V4Mini 的原因：写死任何一个 id，另一个模型要么
/// 一路生成到 `sample_len`，要么第一个 token 就停。
///
/// 查的是 `get_vocab(true)`（词表 + added tokens）而不是 `token_to_id`：这两个
/// 模型里的 `<|endoftext|>` 都是 added token，两边都得覆盖到。
fn eos_token_id(tokenizer: &Tokenizer) -> Result<u32> {
    tokenizer
        .get_vocab(true)
        .get("<|endoftext|>")
        .copied()
        .ok_or_else(|| {
            Error::WithMessage(String::from(
                "cannot find the <|endoftext|> token in the Phi tokenizer",
            ))
        })
}

pub(super) fn gen_text(
    device: &Device,
    model: &Model,
    tokenizer: &Tokenizer,
    prompt: &str,
    sample_len: usize,
    top_p: Option<f64>,
    result_sender: &mut ResultSender<'_, StreamingResponseData>,
) -> Result<()> {
    // let device = device()?;
    // let lock = TEXT_GENERATION_MODEL.get_or_init(|| Mutex::new(HashMap::with_capacity(32)));
    // let mut model = lock.lock().unwrap_or_else(|e| {
    //     log::warn!("{:#?}", &e);
    //     e.into_inner()
    // });
    // if !model.contains_key(robot_id) {
    //     let r = load_phi3_model_files(info, &device)?;
    //     model.insert(String::from(robot_id), r);
    // };
    // let (model, tokenizer) = model.get(robot_id).unwrap();

    // log::info!("starting the inference loop");
    let mut tokenizer = super::token_output_stream::TokenOutputStream::new(tokenizer.clone());
    let mut tokens = match tokenizer.tokenizer().encode(prompt, true) {
        Ok(t) => t.get_ids().to_vec(),
        Err(e) => return Err(Error::WithMessage(format!("{}", &e))),
    };
    if tokens.is_empty() {
        return Err(Error::WithMessage(String::from(
            "Empty prompts are not supported in the phi model.",
        )));
    }
    let mut generated_tokens = 0usize;
    let eos_token = eos_token_id(tokenizer.tokenizer())?;
    // log::info!("{prompt}");
    // std::io::stdout().flush()?;
    let start_gen = std::time::Instant::now();
    let mut pos = 0;
    let mut rng = Rand::new();
    let mut model = model.clone();
    model.clear_kv_cache();
    let mut logits_processor = LogitsProcessor::new(
        rng.r#gen::<u64>(),
        Some(super::chat::TEMPERATURE),
        top_p,
    );
    for index in 0..sample_len {
        let context_size = if index > 0 { 1 } else { tokens.len() };
        let ctxt = &tokens[tokens.len().saturating_sub(context_size)..];
        let input = Tensor::new(ctxt, device)?.unsqueeze(0)?;
        let logits = model.forward(&input, pos)?.i((.., 0, ..))?;
        let logits = logits.squeeze(0)?.to_dtype(DType::F32)?;
        let logits = if super::chat::REPEAT_PENALTY == 1. {
            logits
        } else {
            let start_at = tokens
                .len()
                .saturating_sub(super::chat::REPEAT_LAST_N);
            candle_transformers::utils::apply_repeat_penalty(
                &logits,
                super::chat::REPEAT_PENALTY,
                &tokens[start_at..],
            )?
        };

        let next_token = logits_processor.sample(&logits)?;
        tokens.push(next_token);
        generated_tokens += 1;
        if next_token == eos_token {
            break;
        }
        if let Some(t) = tokenizer.next_token(next_token)? {
            // Stops as soon as the client is gone. This used to have no check
            // at all, so a closed connection still ran the whole generation.
            if !result_sender.push_delta(t) {
                log::info!("Phi3 receiver is gone, stopping generation.");
                break;
            }
        }
        pos += context_size;
    }
    // 无论从哪个分支退出都要冲刷尾巴：`next_token` 只在解码文本以字母数字结尾
    // 时才吐字，最后几个 token 若以标点/换行收尾就会一直压在流里。原来只在 EOS
    // 分支里 `decode_rest()`，"跑满上限"或"客户端断开"退出时会静默丢掉结尾。
    // （`gemma.rs` / `llama.rs` 在循环之后无条件调用了它，这里跟它们对齐。）
    if let Some(t) = tokenizer.decode_rest()? {
        result_sender.push_delta(t);
    }
    let dt = start_gen.elapsed();
    log::info!(
        "\n{generated_tokens} tokens generated ({:.2} token/s)",
        generated_tokens as f64 / dt.as_secs_f64(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个只有词表的最小分词器。真实分词器（几 MB 的 `tokenizer.json`）不在仓库里，
    /// 而这里要钉住的只是"停止符按字符串查"这一件事。
    fn word_level_tokenizer(vocab: &str) -> Tokenizer {
        let json = format!(
            r#"{{"version":"1.0","model":{{"type":"WordLevel","vocab":{{{vocab}}},"unk_token":"<unk>"}}}}"#
        );
        json.parse().expect("a minimal WordLevel tokenizer")
    }

    /// V4Mini 必须停得下来。停止符按**字符串**查，所以 Phi-3-mini 的 32000 和
    /// Phi-4-mini 的 199999 都能查到；写死任何一个 id，另一个模型就会一路生成到
    /// `sample_len`（它的 `<|endoftext|>` 永远匹配不上）。
    #[test]
    fn the_stop_token_is_looked_up_by_string_for_both_phi_families() {
        for id in [32000u32, 199999u32] {
            let tokenizer = word_level_tokenizer(&format!("\"<|endoftext|>\": {id}"));
            assert_eq!(
                eos_token_id(&tokenizer).unwrap(),
                id,
                "the stop token must come from the tokenizer, not a hardcoded id"
            );
        }
    }

    /// 查不到时报一条能看懂的错，而不是退回 0 号 token —— 那会让生成在第一个
    /// token 上就停下，看起来像"模型没有输出"。
    #[test]
    fn a_missing_stop_token_is_an_error() {
        let tokenizer = word_level_tokenizer("\"<s>\": 1");
        let err = eos_token_id(&tokenizer).unwrap_err().to_string();
        assert!(err.contains("<|endoftext|>"), "{err}");
    }
}
