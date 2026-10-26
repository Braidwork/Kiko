//! 本地千问（Qwen3）推理：GGUF 量化权重 + 流式生成。
//!
//! 参考上游示例：
//! - `candle-examples/examples/quantized-qwen3`
//! - `candle-examples/examples/quantized-qwen3-moe`
//!
//! 两个模型的 `forward(&Tensor, usize) -> Tensor` 形状一致，差异只在具体类型，
//! 所以用一个私有 trait 把生成循环的重复代码收在一处 —— 而不是抄两遍循环。
//!
//! 另：`ModelWeights` 派生了 `Clone`，但 `GGUFQWenMoE` **没有**，所以 MoE 路径
//! 只能原地借用模型。两者都由 `chat.rs` 里 `LOADED_MODELS` 的互斥锁保护，同一
//! 时刻只会有一个生成在跑，原地借用不会串数据。

use candle::{DType, Device, Tensor, quantized::gguf_file};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::quantized_qwen3::ModelWeights as Qwen3;
use candle_transformers::models::quantized_qwen3_moe::GGUFQWenMoE as Qwen3Moe;
use frand::Rand;
use tokenizers::Tokenizer;

use super::chat::ResultSender;
use super::huggingface::{HuggingFaceModelInfo, device};
use crate::flow::rt::dto::StreamingResponseData;
use crate::result::{Error, Result};

/// Qwen 系列的会话结束标记。`convert_prompt` 用它闭合每个回合。
const IM_END: &str = "<|im_end|>";

/// 两个 Qwen3 变体的共同接口：跑一步前向，拿到**最后一个位置**的 logits。
///
/// 上游的 `forward` 内部已经做了 `narrow(1, l - 1, 1)`，所以返回值是
/// `[b, vocab]`，调用方 `squeeze(0)` 之后就是一维 logits。
trait NextLogits {
    fn next_logits(&mut self, input: &Tensor, offset: usize) -> candle::Result<Tensor>;

    /// 丢掉上一次推理留下的 KV cache。
    ///
    /// **每次回答之前都必须调。** `LOADED_MODELS` 会把模型实例缓存下来复用，
    /// 而 cache 里的 K/V 是上一次请求的内容：不清理的话，新提示词虽然从 offset 0
    /// 开始，注意力却还会读到上一次的 token。
    ///
    /// 症状非常有迷惑性 —— 模型"粘"在上一个话题上：连问五个互不相关的新话题
    /// （时尚 / 美食 / 音乐 / 航空母舰 / 园艺），答案全部还是第一个话题"时尚"。
    /// 新建实例就没有这个问题，所以一度被误判成"小模型能力不行"。
    fn clear_cache(&mut self);
}

impl NextLogits for Qwen3 {
    fn next_logits(&mut self, input: &Tensor, offset: usize) -> candle::Result<Tensor> {
        self.forward(input, offset)
    }

    fn clear_cache(&mut self) {
        self.clear_kv_cache();
    }
}

impl NextLogits for Qwen3Moe {
    fn next_logits(&mut self, input: &Tensor, offset: usize) -> candle::Result<Tensor> {
        self.forward(input, offset)
    }

    fn clear_cache(&mut self) {
        // Comment out the following line if this PR has been merged: https://github.com/huggingface/candle/pull/4010.
        // self.clear_kv_cache();
    }
}

/// 读 GGUF。`Content::read` 只解析头部元数据，权重仍是懒加载的。
fn read_gguf(info: &HuggingFaceModelInfo) -> Result<(std::fs::File, gguf_file::Content)> {
    let path = match info.gguf_model_path() {
        Some(p) => p,
        None => {
            return Err(Error::WithMessage(format!(
                "Model {:?} is not a GGUF model.",
                info.repository
            )));
        }
    };
    let start = std::time::Instant::now();
    let mut file = std::fs::File::open(&path)?;
    let ct = gguf_file::Content::read(&mut file)?;
    log::info!(
        "Loaded GGUF {:?} ({} tensors) in {:.2}s",
        path,
        ct.tensor_infos.len(),
        start.elapsed().as_secs_f32(),
    );
    Ok((file, ct))
}

/// 加载稠密（非 MoE）Qwen3 的 GGUF 权重与分词器。
pub(super) fn load_qwen3_model_files(
    info: &HuggingFaceModelInfo,
) -> Result<(Device, Qwen3, Tokenizer)> {
    let device = device()?;
    let (mut file, ct) = read_gguf(info)?;
    let model = Qwen3::from_gguf(ct, &mut file, &device)?;
    let tokenizer = super::huggingface::init_model_tokenizer(info)?;
    Ok((device, model, tokenizer))
}

/// 加载 MoE 版 Qwen3（如 `Qwen3-30B-A3B`）的 GGUF 权重与分词器。
///
/// dtype 只在这里需要：MoE 的 `from_gguf` 要显式传；稠密版自己从 GGUF 元数据
/// 的 `general.dtype` 推断。
pub(super) fn load_qwen3_moe_model_files(
    info: &HuggingFaceModelInfo,
) -> Result<(Device, Qwen3Moe, Tokenizer)> {
    let device = device()?;
    let dtype = if device.is_cuda() {
        DType::BF16
    } else {
        DType::F32
    };
    let (mut file, ct) = read_gguf(info)?;
    let model = Qwen3Moe::from_gguf(ct, &mut file, &device, dtype)?;
    let tokenizer = super::huggingface::init_model_tokenizer(info)?;
    Ok((device, model, tokenizer))
}

/// 把提示词编码成 token id。空提示词在这里就报错，别让生成循环空转。
fn encode_prompt(tokenizer: &Tokenizer, prompt: &str) -> Result<Vec<u32>> {
    let tokens = match tokenizer.encode(prompt, true) {
        Ok(t) => t.get_ids().to_vec(),
        Err(e) => return Err(Error::WithMessage(format!("{e}"))),
    };
    if tokens.is_empty() {
        return Err(Error::WithMessage(String::from(
            "Empty prompts are not supported in the Qwen3 model.",
        )));
    }
    Ok(tokens)
}

/// 解析 EOS。取不到时明确报错 —— 上游示例这里是 `unwrap()`，一个不匹配的
/// tokenizer 会让整个进程 panic，而我们只想让这一次回答失败。
fn eos_token_of(tokenizer: &super::token_output_stream::TokenOutputStream) -> Result<u32> {
    match tokenizer.get_token(IM_END) {
        Some(t) => Ok(t),
        None => Err(Error::WithMessage(format!(
            "cannot find the {IM_END} token in the Qwen3 tokenizer"
        ))),
    }
}

/// Qwen3 的生成循环。`Qwen3` 与 `Qwen3Moe` 共用它。
#[allow(clippy::too_many_arguments)]
fn generate<M: NextLogits>(
    device: &Device,
    model: &mut M,
    tokenizer: &Tokenizer,
    prompt: &str,
    sample_len: usize,
    top_p: Option<f64>,
    result_sender: &mut ResultSender<'_, StreamingResponseData>,
    backend: &str,
) -> Result<()> {
    let mut tokens = encode_prompt(tokenizer, prompt)?;
    let prompt_len = tokens.len();
    let mut tokenizer = super::token_output_stream::TokenOutputStream::new(tokenizer.clone());
    let eos_token = eos_token_of(&tokenizer)?;

    let mut rng = Rand::new();
    let mut logits_processor = LogitsProcessor::new(
        rng.r#gen::<u64>(),
        Some(super::chat::TEMPERATURE),
        top_p,
    );

    let start_gen = std::time::Instant::now();
    let mut generated = 0usize;

    log::info!(
        "{backend}: prefill {} tokens on {:?} (max {sample_len} new tokens)...",
        prompt_len,
        device,
    );

    // 先清掉上一次回答留下的 KV。模型实例是跨请求复用的（`LOADED_MODELS`），
    // 不清就会"粘"在上一个话题上 —— 详见 `NextLogits::clear_cache`。
    model.clear_cache();

    // 预填充：整段提示词一次过，offset 从 0 开始。
    let mut logits = {
        let input = Tensor::new(tokens.as_slice(), device)?.unsqueeze(0)?;
        model.next_logits(&input, 0)?
    };
    let prefill_dt = start_gen.elapsed();
    log::info!(
        "{backend}: prefill done in {:.2}s ({:.2} tok/s), generating...",
        prefill_dt.as_secs_f64(),
        prompt_len as f64 / prefill_dt.as_secs_f64(),
    );

    for _ in 0..sample_len {
        let step_logits = logits.squeeze(0)?.to_dtype(DType::F32)?;
        // 重复惩罚只看**已生成**的 token —— 与上游示例的 `all_tokens` 一致。
        let step_logits = if super::chat::REPEAT_PENALTY == 1. {
            step_logits
        } else {
            let start_at = generated.saturating_sub(super::chat::REPEAT_LAST_N);
            candle_transformers::utils::apply_repeat_penalty(
                &step_logits,
                super::chat::REPEAT_PENALTY,
                &tokens[prompt_len + start_at..],
            )?
        };
        let next_token = logits_processor.sample(&step_logits)?;
        tokens.push(next_token);
        generated += 1;

        // 每 8 个 token 报一次进度。原来这里只在整段生成**结束后**才打日志，
        // 于是在慢机器上（debug 构建下一 token 要二十多秒）看起来像是卡死了。
        if generated % 8 == 0 {
            let dt = start_gen.elapsed().as_secs_f64();
            let rate = (generated as f64) / (dt - prefill_dt.as_secs_f64()).max(1e-9);
            log::info!(
                "{backend}: {generated}/{sample_len} tokens, {rate:.2} tok/s, {:.0}s elapsed",
                dt,
            );
        }

        if next_token == eos_token {
            break;
        }
        if let Some(t) = tokenizer.next_token(next_token)? {
            // 客户端断开就停止生成，而不是继续往没人读的通道里塞。
            if !result_sender.push_delta(t) {
                log::info!("{backend} receiver is gone, stopping generation.");
                break;
            }
        }

        // 下一个位置：offset 是**当前**已缓存的 token 数（`tokens` 已含新
        // 采样的那个，所以减一）。
        let offset = tokens.len() - 1;
        let input = Tensor::new(&[next_token], device)?.unsqueeze(0)?;
        logits = model.next_logits(&input, offset)?;
    }

    // 无论从哪个分支退出都要**冲刷尾巴**。`next_token` 只在解码出的文本以
    // 字母数字结尾时才吐字，所以只要最后几个 token 是标点/换行（中文回答的
    // 结尾几乎总是"。"），它们还压在 `TokenOutputStream` 里。原来只在 EOS
    // 分支里调 `decode_rest()`，于是"跑满 token 上限"或"客户端断开"退出时，
    // 结尾会被静默丢掉。
    if let Some(t) = tokenizer.decode_rest()? {
        result_sender.push_delta(t);
    }

    let dt = start_gen.elapsed();
    log::info!(
        "{backend}: {prompt_len} prompt tokens, {generated} tokens generated ({:.2} token/s)",
        generated as f64 / dt.as_secs_f64(),
    );
    Ok(())
}

/// 稠密 Qwen3 的流式生成入口。
pub(super) fn gen_text(
    device: &Device,
    model: &mut Qwen3,
    tokenizer: &Tokenizer,
    prompt: &str,
    sample_len: usize,
    top_p: Option<f64>,
    result_sender: &mut ResultSender<'_, StreamingResponseData>,
) -> Result<()> {
    generate(
        device,
        model,
        tokenizer,
        prompt,
        sample_len,
        top_p,
        result_sender,
        "Qwen3",
    )
}

/// MoE 版 Qwen3 的流式生成入口。
pub(super) fn gen_text_moe(
    device: &Device,
    model: &mut Qwen3Moe,
    tokenizer: &Tokenizer,
    prompt: &str,
    sample_len: usize,
    top_p: Option<f64>,
    result_sender: &mut ResultSender<'_, StreamingResponseData>,
) -> Result<()> {
    generate(
        device,
        model,
        tokenizer,
        prompt,
        sample_len,
        top_p,
        result_sender,
        "Qwen3Moe",
    )
}
