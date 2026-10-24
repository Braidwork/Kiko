use base64::Engine as _;
use candle::{DType, Device, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::moondream::{Config as MoondreamConfig, Model as MoondreamModel};
use frand::Rand;
use tokenizers::Tokenizer;

use super::chat::ResultSender;
use super::token_output_stream::TokenOutputStream;
use crate::flow::rt::dto::StreamingResponseData;
use crate::result::{Error, Result};

const IMAGE_SIZE: u32 = 378;
const BOS_EOS_TOKEN: &str = "</s>";

pub(super) struct MoondreamInput<'a> {
    pub(super) prompt: &'a str,
    pub(super) images_base64: &'a [String],
}

pub(super) fn load_image_from_base64(data_uri_or_b64: &str, device: &Device) -> Result<Tensor> {
    // strip an optional data URI prefix.
    let b64 = match data_uri_or_b64.split_once(",") {
        Some((prefix, rest)) if prefix.starts_with("data:") => rest,
        _ => data_uri_or_b64,
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64)
        .map_err(|e| Error::WithMessage(format!("Failed to decode image base64, err: {e}")))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| Error::WithMessage(format!("Failed to decode image, err: {e}")))?;
    let img = img.resize_to_fill(
        IMAGE_SIZE,
        IMAGE_SIZE,
        image::imageops::FilterType::Triangle,
    );
    let img = img.to_rgb8();
    let data = img.into_raw();
    let data = Tensor::from_vec(data, (IMAGE_SIZE as usize, IMAGE_SIZE as usize, 3), &Device::Cpu)?
        .permute((2, 0, 1))?;
    let mean = Tensor::new(&[0.5f32, 0.5, 0.5], &Device::Cpu)?.reshape((3, 1, 1))?;
    let std = Tensor::new(&[0.5f32, 0.5, 0.5], &Device::Cpu)?.reshape((3, 1, 1))?;
    let t = (data.to_dtype(DType::F32)? / 255.)?
        .broadcast_sub(&mean)?
        .broadcast_div(&std)?;
    Ok(t.to_device(device)?)
}

pub(super) fn gen_text(
    device: &Device,
    model: &mut MoondreamModel,
    tokenizer: &Tokenizer,
    input: MoondreamInput<'_>,
    sample_len: usize,
    top_p: Option<f64>,
    result_sender: &mut ResultSender<'_, StreamingResponseData>,
) -> Result<()> {
    let prompt = format!("\n\nQuestion: {0}\n\nAnswer:", input.prompt);
    let mut tokens = match tokenizer.encode(prompt.as_str(), true) {
        Ok(t) => t.get_ids().to_vec(),
        Err(e) => return Err(Error::WithMessage(format!("{}", &e))),
    };
    if tokens.is_empty() {
        return Err(Error::WithMessage(String::from(
            "Empty prompts are not supported in the Moondream model.",
        )));
    }
    let special_token = match tokenizer.get_vocab(true).get(BOS_EOS_TOKEN) {
        Some(token) => *token,
        None => {
            return Err(Error::WithMessage(format!(
                "cannot find the special token {BOS_EOS_TOKEN}"
            )))
        }
    };
    let (bos_token, eos_token) = (special_token, special_token);
    let mut tokenizer_stream = TokenOutputStream::new(tokenizer.clone());
    let mut rng = Rand::new();
    let mut logits_processor = LogitsProcessor::new(
        rng.r#gen::<u64>(),
        Some(super::chat::TEMPERATURE),
        top_p,
    );
    let mut generated_tokens = 0usize;
    let start_gen = std::time::Instant::now();
    for index in 0..sample_len {
        let context_size = if index > 0 { 1 } else { tokens.len() };
        let ctxt = &tokens[tokens.len().saturating_sub(context_size)..];
        let input_ids = Tensor::new(ctxt, device)?.unsqueeze(0)?;
        let logits = if index > 0 || input.images_base64.is_empty() {
            model.text_model.forward(&input_ids)?
        } else {
            let dtype = if device.is_cuda() {
                DType::F16
            } else {
                DType::F32
            };
            let image = load_image_from_base64(&input.images_base64[0], device)?.to_dtype(dtype)?;
            let image_embeds = image.unsqueeze(0)?.apply(model.vision_encoder())?;
            let bos_token = Tensor::new(&[bos_token], device)?.unsqueeze(0)?;
            model.text_model.forward_with_img(&bos_token, &input_ids, &image_embeds)?
        };
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
        if next_token == eos_token || tokens.ends_with(&[27, 10619, 29] /* <END> */) {
            break;
        }
        if let Some(t) = tokenizer_stream.next_token(next_token)? {
            // Stops only when the client is gone: the old `try_send` also
            // aborted on a full queue.
            if !result_sender.push_delta(t) {
                log::info!("Moondream receiver is gone, stopping generation.");
                break;
            }
        }
    }
    let dt = start_gen.elapsed();
    log::info!("Moondream generated {generated_tokens} tokens in {dt:?}");
    if let Some(rest) = tokenizer_stream.decode_rest()? {
        result_sender.push_delta(rest);
    }
    Ok(())
}
