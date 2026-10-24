use base64::Engine as _;
use candle::{DType, Device, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::gemma::Model as GemmaModel;
use candle_transformers::models::gemma4::Model as Gemma4Model;
// use crossbeam_channel::Sender;
use frand::Rand;
use tokenizers::Tokenizer;

use super::chat::ResultSender;
use super::token_output_stream::TokenOutputStream;
use crate::flow::rt::dto::StreamingResponseData;
use crate::result::{Error, Result};

// static TEXT_GENERATION_MODEL: OnceLock<Mutex<HashMap<String, (GemmaModel, Tokenizer)>>> =
//     OnceLock::new();

// pub(super) fn replace_model_cache(robot_id: &str, info: &HuggingFaceModelInfo) -> Result<()> {
//     let device = device()?;
//     let c = load_gemma_model_files(info, &device)?;
//     if let Some(lock) = TEXT_GENERATION_MODEL.get() {
//         if let Ok(mut cache) = lock.lock() {
//             cache.insert(String::from(robot_id), c);
//         }
//     }
//     Ok(())
// }

/// Gemma 4 的回合标记。官方模板（`chat_template.jinja`）用 `<|turn>role\n` 开一个
/// 回合、`<turn|>\n` 收尾，生成前缀是 `<|turn>model\n`。**不是** gemma-2b/7b 的
/// `<start_of_turn>` / `<end_of_turn>`，两者不能混用。
pub(super) const TURN_START: &str = "<|turn>";
pub(super) const TURN_END: &str = "<turn|>\n";

/// 思考频道。开着 thinking 时系统回合里先注入 `<|think|>`，模型随后把推理过程包在
/// `<|channel>thought\n` … `<channel|>` 里（见 `tokenizer_config.json` 的
/// `response_template.fields.thinking`）。
pub(super) const THINK_TOKEN: &str = "<|think|>";
pub(super) const THINKING_OPEN: &str = "<|channel>";
const THINKING_CLOSE: &str = "<channel|>";

/// Gemma 4 一张图预处理后的**软 token 上限**，来自仓库里的 `processor_config.json`
/// （`image_processor.max_soft_tokens = 280` / `image_seq_length = 280`）。
///
/// 注意这是**上限**而不是固定值：官方（transformers / vLLM / MAX）的做法是保持
/// 长宽比缩放到"最多 280×3² 个 patch、且两边都是 3×16 的整数倍"，所以一张图实际
/// 产出的软 token 数 = patch 数 / 9，按长宽比取整后通常小于 280。提示词里的
/// `<|image|>` 占位符个数必须与之**完全相等**，否则 `forward_multimodal` 在
/// `broadcast_embed_to_mask` 里就会因形状不一致报错 —— 所以占位符个数是运行时按
/// 每张图算出来的，不能写死 280。
const GEMMA4_MAX_SOFT_TOKENS: usize = 280;
/// 视觉塔的 patch 边长（`vision_config.patch_size`）。
const GEMMA4_PATCH_SIZE: usize = 16;
/// 池化核边长（`vision_config.pooling_kernel_size`）：3×3 个 patch 合成一个软 token。
const GEMMA4_POOLING_KERNEL: usize = 3;

/// Gemma 4 的输入：文本提示词，外加可选的图片（data URI 或裸 base64，和
/// moondream 那条路径同一形状）。
pub(super) struct Gemma4Input<'a> {
    pub(super) prompt: &'a str,
    pub(super) images_base64: &'a [String],
}

/// 图片缩放到目标边长时的重采样方式。官方处理器用 `PIL.Image.Resampling.BICUBIC`
/// （见 `aspect_ratio_preserving_resize`），这里对齐它的意图；`image` crate 的
/// CatmullRom 与 PIL 的 bicubic 系数不完全相同，但"缩到同一尺寸"这件事一致。
const GEMMA4_RESAMPLE: image::imageops::FilterType = image::imageops::FilterType::CatmullRom;

/// 按 Gemma 4 官方处理器的方式，算出保持长宽比的缩放尺寸。
///
/// 规则（照 `processing_utils.py::aspect_ratio_preserving_resize` 逐条搬过来）：
///
/// 1. 先把面积缩到 `max_patches * patch_size²` 以内：`factor = sqrt(target_px / 原面积)`；
/// 2. 再把两边**向下取整**到 `pooling_kernel_size * patch_size`（=48）的整数倍 ——
///    这样池化时 3×3 的块永远完整，软 token 数恰好是 `patch 数 / 9`；
/// 3. 只缩不放：尺寸已经合规时不放大（边长向上取整保证 `factor ≥ 1` 时结果不变）。
fn gemma4_vision_size(width: u32, height: u32) -> (u32, u32) {
    let ps = GEMMA4_PATCH_SIZE;
    let pool = GEMMA4_POOLING_KERNEL;
    let side_mult = (pool * ps) as f64; // 48
    let max_patches = GEMMA4_MAX_SOFT_TOKENS * pool * pool; // 2520
    let target_px = (max_patches * ps * ps) as f64; // 2520 × 256 = 645120

    let (width, height) = (width.max(1) as f64, height.max(1) as f64);
    let factor = (target_px / (width * height)).sqrt();
    let mut target_h = ((factor * height) / side_mult).floor() * side_mult;
    let mut target_w = ((factor * width) / side_mult).floor() * side_mult;
    // 单边最长能占多少 patch，退化分支用它免得另一边超预算。
    let max_side_length = ((max_patches / (pool * pool)) * (pool * ps)) as f64;

    if target_h == 0.0 && target_w == 0.0 {
        // 极端小图也要给一个合法尺寸，不能缩成 0（前向会直接报错）。
        target_h = side_mult;
        target_w = side_mult;
    } else if target_h == 0.0 {
        target_h = side_mult;
        target_w = ((width / height).floor() * side_mult).min(max_side_length);
    } else if target_w == 0.0 {
        target_w = side_mult;
        target_h = ((height / width).floor() * side_mult).min(max_side_length);
    }
    (target_w.max(1.0) as u32, target_h.max(1.0) as u32)
}

/// 一张图预处理后实际会产出多少个软 token（= patch 数 / 9）。
///
/// 前向里的占位符个数由它决定，所以这个函数必须和 resize + 池化的口径完全一致。
fn gemma4_soft_tokens(width: u32, height: u32) -> usize {
    let ps = GEMMA4_PATCH_SIZE as u32;
    let (w, h) = gemma4_vision_size(width, height);
    ((w / ps) * (h / ps)) as usize / (GEMMA4_POOLING_KERNEL * GEMMA4_POOLING_KERNEL)
}

/// 去掉可选的 data URI 前缀：`data:image/png;base64,xxxx`。
fn strip_data_uri(data_uri_or_b64: &str) -> &str {
    match data_uri_or_b64.split_once(",") {
        Some((prefix, rest)) if prefix.starts_with("data:") => rest,
        _ => data_uri_or_b64,
    }
}

/// 解码一张 base64 图片，按 Gemma 4 的视觉预处理缩放并转成 `[1, 3, H, W]`。
///
/// 归一化只做 `/255`（`do_rescale: true`、`do_normalize: false`，`image_mean/std`
/// 都是 0/1）：视觉塔的 `PatchEmbedder` 内部才把 `[0,1]` 映到 `[-1,1]`
/// （`(patch - 0.5) * 2`），这里再减均值就等于连乘两遍。
pub(super) fn load_image_from_base64_for_gemma4(
    data_uri_or_b64: &str,
    device: &Device,
) -> Result<Tensor> {
    if data_uri_or_b64.starts_with("http://") || data_uri_or_b64.starts_with("https://") {
        return Err(Error::WithMessage(String::from(
            "Gemma 4 needs the image itself (base64), not a URL.",
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(strip_data_uri(data_uri_or_b64))
        .map_err(|e| Error::WithMessage(format!("Failed to decode image base64, err: {e}")))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| Error::WithMessage(format!("Failed to decode image, err: {e}")))?;
    let (w, h) = gemma4_vision_size(img.width(), img.height());
    let data = img.resize_exact(w, h, GEMMA4_RESAMPLE).to_rgb8().into_raw();
    let t = Tensor::from_vec(data, (h as usize, w as usize, 3), &Device::Cpu)?
        .permute((2, 0, 1))?
        .to_dtype(DType::F32)?
        .affine(1.0 / 255.0, 0.0)?
        .unsqueeze(0)?
        .to_device(device)?;
    Ok(t)
}

/// 一张图会占多少个 `<|image|>` 占位符。
fn gemma4_soft_tokens_of_base64(data_uri_or_b64: &str) -> Result<usize> {
    if data_uri_or_b64.starts_with("http://") || data_uri_or_b64.starts_with("https://") {
        return Err(Error::WithMessage(String::from(
            "Gemma 4 needs the image itself (base64), not a URL.",
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(strip_data_uri(data_uri_or_b64))
        .map_err(|e| Error::WithMessage(format!("Failed to decode image base64, err: {e}")))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| Error::WithMessage(format!("Failed to decode image, err: {e}")))?;
    Ok(gemma4_soft_tokens(img.width(), img.height()))
}

/// 提示词里的图片占位符。张数与每个视觉塔产出的嵌入个数严格相等。
fn gemma4_image_placeholder(count: usize) -> String {
    "<|image|>".repeat(count)
}

/// 把 `<|channel>thought\n…<channel|>` 这段推理过程从流出去的内容里摘掉。
///
/// 为什么必须摘：`skip_special_tokens` **不会**滤掉这两个标记（`token_output_stream`
/// 里那条 Qwen3 的测试已经把这件事钉住了），tokenizer 的 `response_schema` 也把它们
/// 当普通文本。官方的 `strip_thinking` 宏对 model 正文做同样的事：丢掉 `<|channel>`
/// 之前的内容和整个频道块。
///
/// 标记可能被切在多个 token 里（`<|channel>` 未必是一个 token），所以不能"看到一个
/// token 就判断"，得像流式解析器那样把可能是标记开头的尾巴先留在 `pending` 里，
/// 等后续 token 到齐再决定。
#[derive(Debug, Default)]
struct ThoughtChannelFilter {
    pending: String,
    /// 当前是否已经进了思考频道（进了就丢弃内容，直到看到 `<channel|>`）。
    in_channel: bool,
}

impl ThoughtChannelFilter {
    /// 产出 `pending[..end]` 并把它从缓冲里摘掉。
    fn drain_upto(&mut self, end: usize) -> Vec<String> {
        if end == 0 {
            return Vec::new();
        }
        let rest = self.pending.split_off(end);
        let head = std::mem::replace(&mut self.pending, rest);
        if head.is_empty() {
            Vec::new()
        } else {
            vec![head]
        }
    }

    /// `pending[at..]` 是否可能是一个停止标记的开头（用来决定"最多能安全吐出到哪"）。
    fn split_at_partial(&self, stop: &str) -> usize {
        let s = self.pending.as_bytes();
        let t = stop.as_bytes();
        for k in (1..t.len()).rev() {
            if s.len() >= k && &s[s.len() - k..] == &t[..k] {
                return self.pending.len() - k;
            }
        }
        self.pending.len()
    }

    /// 喂一段刚从 token 解出来的文本，返回可以立刻发给客户端的部分。
    fn feed(&mut self, text: &str) -> Vec<String> {
        self.pending.push_str(text);
        let mut out = Vec::new();
        loop {
            let stop = if self.in_channel {
                THINKING_CLOSE
            } else {
                THINKING_OPEN
            };
            match self.pending.find(stop) {
                Some(at) => {
                    // 频道块整体丢掉：不管开还是闭，`at` 之前的正文也算推理
                    // （官方 `strip_thinking` 宏同样只保留 `<|channel>` 之前那截）。
                    self.pending.drain(..at + stop.len());
                    self.in_channel = !self.in_channel;
                }
                None => {
                    if self.in_channel {
                        // 频道里还没结束：内容全部留在缓冲里等闭合标记，真正的
                        // 推理文本因此不会漏出去。
                        break;
                    }
                    let up_to = self.split_at_partial(stop);
                    out.extend(self.drain_upto(up_to));
                    break;
                }
            }
        }
        out
    }

    /// 生成结束时的收尾：没闭合的思考块直接丢弃（推理过程不该给用户看），
    /// 正常状态下把压在缓冲里的尾巴吐出来。
    fn finish(&mut self) -> Option<String> {
        if self.in_channel {
            self.pending.clear();
            return None;
        }
        self.drain_upto(self.pending.len()).pop()
    }
}

/// 历史里的 assistant 正文可能带思考块（多轮对话时上一轮的回答由客户端回传），
/// 拼进提示词前按官方 `strip_thinking` 宏的口径去掉。
///
/// 官方宏把正文按 `<channel|>` 切开，每段里**只保留 `<|channel>` 之前的那截**：
/// 于是"`<|channel>thought\n推理<channel|>正式回答`"会变成"正式回答"（开头那截在
/// 第一个开标记之前是空的），而开标记之前的正常正文会保留。`<channel|>` 从未出现
/// 的那段（`split_once` 返回 None）原样保留 —— 没闭合的思考块把整段正文吞掉反而
/// 更糟。
pub(super) fn strip_thinking(text: &str) -> String {
    // 没有闭合标记：看不出思考块到哪儿结束，原样保留（宁可把脏文本拼进提示词，
    // 也不要把整段回答吞成空）。
    if !text.contains(THINKING_CLOSE) {
        return text.trim().to_string();
    }
    let mut out = String::with_capacity(text.len());
    for part in text.split(THINKING_CLOSE) {
        match part.split_once(THINKING_OPEN) {
            // `<|channel>` 之前的正文保留，频道内容（连同标记）丢掉。
            Some((head, _)) => out.push_str(head),
            None => out.push_str(part),
        }
    }
    out.trim().to_string()
}

pub(super) fn gen_text(
    device: &Device,
    model: &GemmaModel,
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
    //     let r = load_gemma_model_files(info, &device)?;
    //     model.insert(String::from(robot_id), r);
    // };
    // let (model, tokenizer) = model.get(robot_id).unwrap();
    let mut tokens = match tokenizer.encode(prompt, true) {
        Ok(t) => t.get_ids().to_vec(),
        Err(e) => return Err(Error::WithMessage(format!("{}", &e))),
    };
    let mut tokenizer = super::token_output_stream::TokenOutputStream::new(tokenizer.clone());
    let eos_token = match tokenizer.get_token("<eos>") {
        Some(token) => token,
        None => {
            return Err(Error::WithMessage(String::from(
                "cannot find the <eos> token",
            )));
        }
    };
    let mut generated_tokens = 0usize;
    let start_gen = std::time::Instant::now();
    let mut model = model.clone();
    model.clear_kv_cache();
    // let rr = Rc::new(result_sender);
    for index in 0..sample_len {
        let context_size = if index > 0 { 1 } else { tokens.len() };
        let start_pos = tokens.len().saturating_sub(context_size);
        let ctxt = &tokens[start_pos..];
        let input = Tensor::new(ctxt, device)?.unsqueeze(0)?;
        let logits = model.forward(&input, start_pos)?;
        let logits = logits.squeeze(0)?.squeeze(0)?.to_dtype(DType::F32)?;
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

        let mut rng = Rand::new();
        let mut logits_processor = LogitsProcessor::new(
            rng.r#gen::<u64>(),
            Some(super::chat::TEMPERATURE),
            top_p,
        );
        let next_token = logits_processor.sample(&logits)?;
        tokens.push(next_token);
        generated_tokens += 1;
        if next_token == eos_token {
            break;
        }
        // let rr: ResultReceiver;
        if let Some(t) = tokenizer.next_token(next_token)? {
            // Stops only when the client is gone. The old `try_send` also
            // aborted on a full queue, which cut generation short.
            if !result_sender.push_delta(t) {
                log::info!("Gemma receiver is gone, stopping generation.");
                break;
            }
        }
    }
    let dt = start_gen.elapsed();
    if let Some(rest) = tokenizer.decode_rest()? {
        result_sender.push_delta(rest);
    }
    // std::io::stdout().flush()?;
    println!(
        "\n{generated_tokens} tokens generated ({:.2} token/s)",
        generated_tokens as f64 / dt.as_secs_f64(),
    );
    Ok(())
}

/// 已加载的 Gemma 4 模型（多模态那一份）。
///
/// 为什么不是 `gemma4::text::TextModel`：candle 0.11.0 里**只有** `gemma4::Model`
/// 公开了 `forward_multimodal`，`text::TextModel` 没有图像入口。既然下拉框里两个
/// 档位（`google/gemma-4-E2B-it` / `-E4B-it`）的配置都带 `vision_config`、我们又要
/// 图像，就统一用多模态模型：纯文本请求同样走 `forward_multimodal`，只是
/// `pixel_values` 传 `None`，它内部直接退化成语言模型前向。
///
/// 代价是多了视觉塔的内存（音频塔在配置里有 `audio_config` 时也会一起建，我们不用），
/// 换来的是"同一实例既能收文本也能收图" —— `chat.rs` 的 `LOADED_MODELS` 是跨请求
/// 复用实例的，两种请求形状必须共用一个模型。
pub(crate) struct Gemma4Loaded(pub(crate) Gemma4Model);

/// 解析 EOS。取不到时明确报错 —— 上游示例这里是 `unwrap()`，一个不匹配的 tokenizer
/// 会让整个进程 panic，而我们只想让这一次回答失败。
fn gemma4_eos_token_of(tokenizer: &TokenOutputStream) -> Result<u32> {
    match tokenizer.get_token("<eos>") {
        Some(token) => Ok(token),
        None => Err(Error::WithMessage(String::from(
            "cannot find the <eos> token in the Gemma 4 tokenizer",
        ))),
    }
}

/// Gemma 4（文本 + 图像）的流式生成入口。
///
/// 与 gemma-2b/7b 那条路径的区别：
///
/// - 提示词格式是 `<|turn>…<turn|>`（见 `huggingface.rs` 的 `Gemma4` 分支）；
/// - 有图片时走 `forward_multimodal`，在 `<|image|>` 的位置把视觉塔的嵌入换进
///   词嵌入序列，**只有预填充那一步需要**，之后逐 token 生成和纯文本一样；
/// - 推理过程包在 `<|channel>` 里，出流前由 `ThoughtChannelFilter` 摘掉。
#[allow(clippy::too_many_arguments)]
pub(super) fn gen_text_gemma4(
    device: &Device,
    model: &mut Gemma4Model,
    tokenizer: &Tokenizer,
    input: Gemma4Input<'_>,
    sample_len: usize,
    top_p: Option<f64>,
    result_sender: &mut ResultSender<'_, StreamingResponseData>,
) -> Result<()> {
    if input.prompt.is_empty() {
        return Err(Error::WithMessage(String::from(
            "Empty prompts are not supported in the Gemma 4 model.",
        )));
    }
    // 每张图先算出它会产出多少软 token，再据此拼出**个数吻合**的占位符。
    // 不能假设 280：缩放后 patch 数是按长宽比取整的，窄图/横图都少于 280。
    let mut image_tokens = 0usize;
    for img in input.images_base64.iter() {
        image_tokens += gemma4_soft_tokens_of_base64(img)?;
    }
    let prompt = format!("{}{}", input.prompt, gemma4_image_placeholder(image_tokens));
    let mut tokens = match tokenizer.encode(prompt.as_str(), true) {
        Ok(t) => t.get_ids().to_vec(),
        Err(e) => return Err(Error::WithMessage(format!("{e}"))),
    };
    let prompt_len = tokens.len();
    let mut tokenizer = TokenOutputStream::new(tokenizer.clone());
    // Gemma 4 的回合结束标记是 `<eos>`（`tokenizer_config.json` 的 `eos_token`），
    // 既不是 `<|turn>` 也不是 gemma2 的 `</s>`。
    let eos_token = gemma4_eos_token_of(&tokenizer)?;
    let mut rng = Rand::new();
    let mut logits_processor = LogitsProcessor::new(
        rng.r#gen::<u64>(),
        Some(super::chat::TEMPERATURE),
        top_p,
    );
    let mut thoughts = ThoughtChannelFilter::default();
    let start_gen = std::time::Instant::now();
    let mut generated_tokens = 0usize;

    log::info!(
        "Gemma4: prefill {prompt_len} tokens ({} images, {image_tokens} image tokens) on {device:?} (max {sample_len} new tokens)...",
        input.images_base64.len(),
    );

    // 模型实例跨请求复用，上一轮的 KV 必须先清掉，否则会"粘"在上一个话题上
    // （cache 里留着上一次请求的 K/V，新提示词从 offset 0 开始也会读到旧内容）。
    model.clear_kv_cache();
    // 图片像素只在这里加载一次：预填充用，后面的解码步骤不再需要。
    let pixel_values = if input.images_base64.is_empty() {
        None
    } else {
        let dtype = if device.is_cuda() {
            DType::BF16
        } else {
            DType::F32
        };
        let mut images = Vec::with_capacity(input.images_base64.len());
        for img in input.images_base64.iter() {
            images.push(load_image_from_base64_for_gemma4(img, device)?.to_dtype(dtype)?);
        }
        Some(images)
    };

    // 预填充：整段提示词一次过，offset 从 0 开始。`forward_multimodal` 在
    // `pixel_values` 为 `None` 时就是普通语言模型前向，纯文本请求走的是同一条。
    let mut logits = {
        let input_tensor = Tensor::new(tokens.as_slice(), device)?.unsqueeze(0)?;
        model.forward_multimodal(&input_tensor, pixel_values.as_deref(), None, None, 0)?
    };
    let prefill_dt = start_gen.elapsed();
    log::info!(
        "Gemma4: prefill done in {:.2}s ({:.2} tok/s), generating...",
        prefill_dt.as_secs_f64(),
        prompt_len as f64 / prefill_dt.as_secs_f64(),
    );

    for _ in 0..sample_len {
        let step_logits = logits.squeeze(0)?.squeeze(0)?.to_dtype(DType::F32)?;
        // 重复惩罚只看**已生成**的 token —— 与上游示例的 `all_tokens` 一致。
        let step_logits = if super::chat::REPEAT_PENALTY == 1. {
            step_logits
        } else {
            let start_at = generated_tokens.saturating_sub(super::chat::REPEAT_LAST_N);
            candle_transformers::utils::apply_repeat_penalty(
                &step_logits,
                super::chat::REPEAT_PENALTY,
                &tokens[prompt_len + start_at..],
            )?
        };
        let next_token = logits_processor.sample(&step_logits)?;
        tokens.push(next_token);
        generated_tokens += 1;

        if next_token == eos_token {
            break;
        }
        if let Some(t) = tokenizer.next_token(next_token)? {
            for piece in thoughts.feed(&t) {
                if !result_sender.push_delta(piece) {
                    log::info!("Gemma4 receiver is gone, stopping generation.");
                    return Ok(());
                }
            }
        }
        // 每 8 个 token 报一次进度：debug 构建下每 token 可能要好几秒，不报进度
        // 看起来就像卡死了。
        if generated_tokens % 8 == 0 {
            let dt = start_gen.elapsed().as_secs_f64();
            let rate = generated_tokens as f64 / (dt - prefill_dt.as_secs_f64()).max(1e-9);
            log::info!(
                "Gemma4: {generated_tokens}/{sample_len} tokens, {rate:.2} tok/s, {dt:.0}s elapsed"
            );
        }
        // 下一个位置：offset 是**当前**已缓存的 token 数（`tokens` 已含刚采样的
        // 那个，所以减一）。解码阶段不需要再传图像。
        let offset = tokens.len() - 1;
        let input_tensor = Tensor::new(&[next_token], device)?.unsqueeze(0)?;
        logits = model.forward_multimodal(&input_tensor, None, None, None, offset)?;
    }

    // 无论从哪个分支退出都要冲刷尾巴：`next_token` 只在解码文本以字母数字结尾时
    // 才吐字，中文回答的句号、换行都会压在 `TokenOutputStream` 里。
    if let Some(t) = tokenizer.decode_rest()? {
        for piece in thoughts.feed(&t) {
            if !result_sender.push_delta(piece) {
                return Ok(());
            }
        }
    }
    if let Some(rest) = thoughts.finish() {
        result_sender.push_delta(rest);
    }

    let dt = start_gen.elapsed();
    log::info!(
        "Gemma4: {prompt_len} prompt tokens, {generated_tokens} tokens generated ({:.2} token/s)",
        generated_tokens as f64 / dt.as_secs_f64(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 图片预处理的硬约束：缩放后两边都是 `pooling_kernel * patch_size` 的整数倍
    /// （否则池化会丢掉残缺的块，软 token 数就对不上），且 patch 数不超预算。
    #[test]
    fn gemma4_image_target_size_respects_the_patch_budget() {
        let side_mult = (GEMMA4_POOLING_KERNEL * GEMMA4_PATCH_SIZE) as u32; // 48
        let max_patches = GEMMA4_MAX_SOFT_TOKENS * GEMMA4_POOLING_KERNEL.pow(2); // 2520
        for (w, h) in [
            (448u32, 448u32),
            (1024, 768),
            (5000, 3000),
            (300, 1200),
            (1200, 300),
            (47, 47),
            (1, 1),
        ] {
            let (tw, th) = gemma4_vision_size(w, h);
            assert!(
                tw % side_mult == 0 && th % side_mult == 0,
                "{w}x{h} -> {tw}x{th} must keep both sides divisible by {side_mult}"
            );
            assert!(tw >= 1 && th >= 1, "{w}x{h} -> {tw}x{th} must not collapse");
            let patches = (tw as usize / GEMMA4_PATCH_SIZE) * (th as usize / GEMMA4_PATCH_SIZE);
            assert!(
                patches <= max_patches,
                "{w}x{h} -> {tw}x{th} gives {patches} patches, over the {max_patches} budget"
            );
            assert!(
                patches >= GEMMA4_POOLING_KERNEL * GEMMA4_POOLING_KERNEL,
                "{w}x{h} -> {tw}x{th} must leave at least one full pooling window"
            );
        }
        // 448×448 会被**放大**到 768×768：官方的 `max_patches` 是 280×3² = 2520 个
        // patch 的预算，而 448×448 只占 784 个，`factor = sqrt(2520/784) ≈ 1.79`，
        // 两边取整到 48 的倍数就是 768（2304 个 patch，仍在预算内）。
        assert_eq!(gemma4_vision_size(448, 448), (768, 768));
        assert_eq!(gemma4_soft_tokens(448, 448), 256);
        // 超过预算的才缩：1024×768 的 factor < 1。
        let (tw, th) = gemma4_vision_size(1024, 768);
        assert!(tw < 1024 && th < 768, "1024x768 must shrink: got {tw}x{th}");
        // 小图同样按预算放大：96×96 的 `sqrt(645120 / 9216) ≈ 8.37`，也落到 768。
        assert_eq!(gemma4_vision_size(96, 96), (768, 768));
        assert_eq!(gemma4_soft_tokens(96, 96), 256);
    }

    /// 一张真实图片经预处理后，软 token 数必须与像素张量的形状一致 —— 提示词里的
    /// 占位符个数就是按前者算的，差一个都会让 `forward_multimodal` 报形状错误。
    #[test]
    fn gemma4_image_tensor_shape_matches_the_soft_token_count() {
        for (w, h) in [(448u32, 448u32), (1200, 300), (300, 1200), (900, 900)] {
            let uri = solid_png_uri(w, h);
            let tensor = load_image_from_base64_for_gemma4(&uri, &Device::Cpu).unwrap();
            let (b, c, th, tw) = tensor.dims4().unwrap();
            assert_eq!((b, c), (1, 3));
            let soft = gemma4_soft_tokens_of_base64(&uri).unwrap();
            assert_eq!(
                soft,
                (tw / GEMMA4_PATCH_SIZE) * (th / GEMMA4_PATCH_SIZE)
                    / (GEMMA4_POOLING_KERNEL * GEMMA4_POOLING_KERNEL),
                "{w}x{h}: placeholder count and pixel tensor disagree"
            );
            assert_eq!(
                gemma4_image_placeholder(soft).matches("<|image|>").count(),
                soft
            );
            // 归一化只做 /255：像素应当落在 [0,1]，而不是被均值方差搬到负值区。
            let flat = tensor.flatten_all().unwrap();
            let min = flat.min(0).unwrap().to_scalar::<f32>().unwrap();
            let max = flat.max(0).unwrap().to_scalar::<f32>().unwrap();
            assert!(
                (0.0..=1.0).contains(&min) && (0.0..=1.0).contains(&max),
                "{w}x{h}: pixels must stay in [0,1] (got {min}..{max}); the vision tower \
                 does the -0.5*2 scaling itself"
            );
        }
    }

    /// 造一张纯色 PNG（data URI），省得测试依赖图片文件。
    fn solid_png_uri(w: u32, h: u32) -> String {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 30]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        )
    }

    /// 思考频道必须被摘干净：开标记、频道内容、闭标记都不能漏给用户；频道之前的
    /// 文本同样属于推理，按官方 `strip_thinking` 的口径一起丢掉。
    #[test]
    fn gemma4_thought_channel_is_stripped() {
        let mut f = ThoughtChannelFilter::default();
        let mut out = String::new();
        for piece in f.feed("<|channel>thought\n我在想用户是不是要退货<channel|>答案在这里") {
            out.push_str(&piece);
        }
        if let Some(rest) = f.finish() {
            out.push_str(&rest);
        }
        assert_eq!(
            out, "答案在这里",
            "only the text outside the thinking channel may reach the user"
        );
    }

    /// 流式场景下标记会被切在多个 token 中间。任意切法都必须得到同一个结果 ——
    /// 这正是"看到半个标记就先别吐"的原因。
    #[test]
    fn gemma4_thought_channel_survives_any_chunk_boundary() {
        let full = "<|channel>thought\n先想一想<channel|>好的，可以退货。";
        let expected = "好的，可以退货。";
        for cut in 0..=full.len() {
            if !full.is_char_boundary(cut) {
                continue;
            }
            let mut f = ThoughtChannelFilter::default();
            let mut out = String::new();
            for piece in f.feed(&full[..cut]) {
                out.push_str(&piece);
            }
            for piece in f.feed(&full[cut..]) {
                out.push_str(&piece);
            }
            if let Some(rest) = f.finish() {
                out.push_str(&rest);
            }
            assert_eq!(out, expected, "cut at {cut} produced {out:?}");
        }
    }

    /// 没闭合的思考块（跑满 token 上限时很常见）不能漏出去；正常文本则必须完整
    /// 保留，包括结尾的标点。
    #[test]
    fn gemma4_unterminated_thinking_is_dropped_not_leaked() {
        let mut f = ThoughtChannelFilter::default();
        let mut out = String::new();
        for piece in f.feed("<|channel>thought\n想了半天没想完") {
            out.push_str(&piece);
        }
        assert_eq!(out, "", "an unterminated channel must not stream out");
        assert_eq!(f.finish(), None);

        let mut f = ThoughtChannelFilter::default();
        assert_eq!(f.feed("正常回答。").concat(), "正常回答。");
        assert_eq!(f.finish(), None);
    }

    /// 多轮对话里上一轮的回答由客户端回传，可能带思考块；拼进提示词前要按官方宏
    /// 的 `strip_thinking` 丢掉。口径是"按 `<channel|>` 切开，每段只保留第一个
    /// `<|channel>` **之前**的正文"：所以开头的纯推理那截（在第一个开标记之前）
    /// 会被丢掉，而开标记之前的正常正文会保留。
    #[test]
    fn gemma4_strip_thinking_matches_the_official_macro() {
        // 开标记之前是纯推理 → 丢掉；闭标记之后是正文 → 保留。
        assert_eq!(
            strip_thinking("<|channel>thought\n推理<channel|>正式回答"),
            "正式回答"
        );
        // 正文里混着一段思考块 → 思考块自己那截丢掉，其余正文保留。
        assert_eq!(
            strip_thinking("前面<|channel>thought\n推理<channel|>后面  "),
            "前面后面"
        );
        // 没有频道块时只做 trim。
        assert_eq!(strip_thinking(" 你好 "), "你好");
        // 没闭合的思考块：`<channel|>` 从未出现，整段原样保留（宁可留脏文本，
        // 也不要把正文整段吞掉）。
        assert_eq!(
            strip_thinking("<|channel>thought\n没闭合"),
            "<|channel>thought\n没闭合"
        );
    }

    /// 常量是模板和过滤器共用的口径，写错一个字符就会让模型收到脏提示词。
    #[test]
    fn gemma4_markers_match_the_official_template() {
        assert_eq!(TURN_START, "<|turn>");
        assert_eq!(TURN_END, "<turn|>\n");
        assert_eq!(THINK_TOKEN, "<|think|>");
        assert_eq!(THINKING_OPEN, "<|channel>");
        assert_eq!(THINKING_CLOSE, "<channel|>");
    }

    /// 提示词的骨架是这些特殊标记，它们必须各自是**一个** token，否则模型看到的
    /// 就是被切碎的 `<|turn>`，格式一说就散。
    ///
    /// 顺带核对 `<|image|>` 的 id 与 `config.json` 里的 `image_token_id`
    /// （258880）：多模态前向就是按这个 id 找占位符位置的，对不上等于没有图。
    ///
    /// 模型文件不在时跳过 —— 与 `token_output_stream.rs` 里同样的取舍：CI 上通常
    /// 没有 16GB 的权重，这条断言只在本地装了模型时才有意义。
    #[test]
    fn gemma4_special_markers_are_single_tokens() {
        const DIR: &str = "data/models/google/gemma-4-E4B-it";
        let path = format!("{DIR}/tokenizer.json");
        if !std::path::Path::new(&path).exists() {
            eprintln!("skipping: {path} not present");
            return;
        }
        let tokenizer = Tokenizer::from_file(&path).unwrap();
        let vocab = tokenizer.get_vocab(true);
        for marker in [
            TURN_START,
            "<turn|>",
            THINK_TOKEN,
            THINKING_OPEN,
            THINKING_CLOSE,
            "<|image|>",
        ] {
            let id = vocab.get(marker).copied();
            assert!(
                id.is_some(),
                "{marker} must be in the Gemma 4 vocabulary: the prompt format \
                 and the thinking filter both depend on it"
            );
            let ids = tokenizer
                .encode(marker, false)
                .unwrap()
                .get_ids()
                .to_vec();
            assert_eq!(
                ids,
                vec![id.unwrap()],
                "{marker} must encode to exactly one token"
            );
        }
        assert_eq!(
            vocab.get("<|image|>").copied(),
            Some(258880),
            "the image placeholder id must match config.json's image_token_id"
        );
    }
}
