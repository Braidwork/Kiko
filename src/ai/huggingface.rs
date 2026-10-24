use core::time::Duration;
use std::fs::OpenOptions as StdOpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::vec::Vec;

use candle::{DType, Device};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use candle_transformers::models::gemma::{Config as GemmaConfig, Model as GemmaModel};
use candle_transformers::models::gemma4::config::Gemma4Config;
use candle_transformers::models::gemma4::Model as Gemma4Model;
use candle_transformers::models::llama::{Cache as LlamaCache, Llama, LlamaConfig, LlamaEosToks};
use candle_transformers::models::moondream::{Config as MoondreamConfig, Model as MoondreamModel};
use candle_transformers::models::parler_tts::{Config as ParlerTtsConfig, Model as ParlerTtsModel};
use candle_transformers::models::phi3::{Config as Phi3Config, Model as Phi3};
use candle_transformers::models::quantized_qwen3::ModelWeights as Qwen3;
use candle_transformers::models::quantized_qwen3_moe::GGUFQWenMoE as Qwen3Moe;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use tokenizers::{AddedToken, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use crate::result::{Error, Result};

/// 本地的 HuggingFace 模型。`PartialEq` 是给"保存设置时模型换没换"比的：只有换了
/// 才需要重新装（见 `man::settings::models_to_reload`）。
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(crate) enum HuggingFaceModel {
    AllMiniLML6V2,
    ParaphraseMLMiniLML12V2,
    ParaphraseMLMpnetBaseV2,
    BgeSmallEnV1_5,
    BgeBaseEnV1_5,
    BgeLargeEnV1_5,
    BgeM3,
    NomicEmbedTextV1_5,
    MultilingualE5Small,
    MultilingualE5Base,
    MultilingualE5Large,
    MxbaiEmbedLargeV1,
    /// Microsoft Phi-3-mini-4k-instruct（3.8B，分片 safetensors）。
    Phi3Mini4kInstruct,
    /// Microsoft Phi-4-mini-instruct（V4Mini，3.8B，BF16 分片 safetensors）。
    ///
    /// 架构和 Phi-3 是同一个（仓库 `config.json` 里 `architectures` 是
    /// `Phi3ForCausalLM`、`model_type` 是 `phi3`），candle 里也共用 `phi3::Model`
    /// —— 参考 `candle-examples/examples/phi/main.rs`，V3 / V3-medium / V4Mini 都走
    /// `Model::Phi3` 那一支，所以本项目的推理也直接复用 `phi3.rs`。
    ///
    /// 与 Phi-3 的差别只有三处，都不在这个枚举里：BF16 权重与 longrope
    /// （131072 上下文）在 `load_phi3_model_files` 里按 `model_type` 选 dtype，
    /// chat 模板在 `convert_prompt` 里单独拼。
    Phi4MiniInstruct,
    TinyLlama1_1bChatV1_0,
    Gemma2bInstruct,
    Gemma7bInstruct,
    // 本地 Gemma 4（多模态）。两个档位都是单文件 BF16 safetensors，Apache-2.0、
    // 不需要访问令牌。权重里同时含文本/视觉/音频三部分，我们只用文本和视觉。
    // E2B/E4B 的 "E" 是 effective parameters，和 2B/4B 的稠密模型不是一回事：
    // 实际权重是 51 亿 / 80 亿参数（BF16 下约 10.2GB / 16GB）。
    Gemma4E2BIt,
    Gemma4E4BIt,
    Gemma412BIt,
    Moondream2,
    ParlerTtsMiniV1,
    ParlerTtsLargeV1,
    WhisperLargeV3,
    // 本地千问（Qwen3）。权重与分词器分别来自两个仓库，见
    // `HuggingFaceModelInfo::tokenizer_repository`。
    Qwen3_0_6B,
    Qwen3_1_7B,
    Qwen3_4B,
    Qwen3_8B,
    Qwen3_14B,
    Qwen3_32B,
    Qwen3_30bA3bInstruct2507,
}

pub(crate) enum LoadedHuggingFaceModel {
    Bert((BertModel, Tokenizer)),
    Llama((Device, Llama, LlamaCache, Tokenizer, Option<LlamaEosToks>)),
    Gemma((Device, GemmaModel, Tokenizer)),
    /// Gemma 4 的两种加载形状（纯文本 / 多模态）。`pub(crate)` 是为了让它能出现在
    /// `LoadedHuggingFaceModel` 这个 crate 级枚举里；具体形状见 `gemma.rs`。
    Gemma4((Device, super::gemma::Gemma4Loaded, Tokenizer)),
    /// Phi-3-mini 与 Phi-4-mini（V4Mini）**共用**这个变体：两者在 candle 里是同一个
    /// `phi3::Model`，形状完全一致，差别只在权重 dtype 与 chat 模板。
    Phi3((Device, Phi3, Tokenizer)),
    Moondream((Device, MoondreamModel, Tokenizer)),
    Qwen3((Device, Qwen3, Tokenizer)),
    Qwen3Moe((Device, Qwen3Moe, Tokenizer)),
}

impl LoadedHuggingFaceModel {
    pub(super) fn load(m: &HuggingFaceModel) -> Result<LoadedHuggingFaceModel> {
        let info = m.get_info();
        let m = match info.model_type {
            HuggingFaceModelType::Llama => {
                LoadedHuggingFaceModel::Llama(load_llama_model_files(&info)?)
            }
            HuggingFaceModelType::Gemma => {
                LoadedHuggingFaceModel::Gemma(load_gemma_model_files(&info)?)
            }
            HuggingFaceModelType::Gemma4 => {
                LoadedHuggingFaceModel::Gemma4(load_gemma4_model_files(&info)?)
            }
            HuggingFaceModelType::Phi3 => {
                LoadedHuggingFaceModel::Phi3(load_phi3_model_files(&info)?)
            }
            HuggingFaceModelType::Phi4Mini => {
                LoadedHuggingFaceModel::Phi3(load_phi3_model_files(&info)?)
            }
            HuggingFaceModelType::Moondream => {
                LoadedHuggingFaceModel::Moondream(load_moondream_model_files(&info)?)
            }
            HuggingFaceModelType::Bert => {
                LoadedHuggingFaceModel::Bert(load_bert_model_files(&info)?)
            }
            HuggingFaceModelType::Qwen3 => {
                LoadedHuggingFaceModel::Qwen3(super::qwen3::load_qwen3_model_files(&info)?)
            }
            HuggingFaceModelType::Qwen3Moe => {
                LoadedHuggingFaceModel::Qwen3Moe(super::qwen3::load_qwen3_moe_model_files(&info)?)
            }
        };
        Ok(m)
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub(crate) enum HuggingFaceModelType {
    Bert,
    Llama,
    Gemma,
    Gemma4,
    Phi3,
    /// Phi-4-mini（V4Mini）。与 `Phi3` 共用 candle 的 `phi3::Model` 与
    /// `LoadedHuggingFaceModel::Phi3`，单列出来是为了让它拿到自己的**加载 dtype**
    /// （BF16）和**官方 chat 模板** —— 这两件事都与 Phi-3 不同。
    Phi4Mini,
    Moondream,
    Qwen3,
    Qwen3Moe,
}

// enum LoadedHfModel {
//     Bert(BertModel, Tokenizer),
//     Phi3(Phi3),
// }

pub(crate) struct HuggingFaceModelInfo {
    pub(super) model_type: HuggingFaceModelType,
    pub(crate) repository: &'static str,
    mirror: &'static str,
    /// 非权重文件清单（`config.json`、`tokenizer.json`…）。`pub(super)` 是为了让
    /// `chat.rs` 里的单测能断言下载清单的组成。
    pub(super) model_files: Vec<&'static str>,
    /// 分片权重的索引文件名；空串表示只有一个 `model.safetensors`。同样是
    /// `pub(super)` 以便断言"单文件模型不许去找索引"。
    pub(super) model_index_file: &'static str,
    tokenizer_filename: &'static str,
    dimenssions: u32,
    /// GGUF 权重文件名（相对于 `mirror` 仓库根目录）。空串表示这个模型不是
    /// GGUF —— 那时权重由 `model_files` / `model_index_file` 描述。
    pub(super) gguf_model_filename: &'static str,
    /// 分词器与 `config.json` 所在的仓库。空串表示就在 `mirror` 里。
    ///
    /// 需要这个字段是因为 GGUF 权重仓库是**纯权重仓库**：`unsloth/*-GGUF` 与
    /// `Qwen/Qwen3-*-GGUF` 都只有 `.gguf`（外加 README/params），没有
    /// `tokenizer.json`，而 base 模型仓库（`Qwen/Qwen3-8B`）才有。上游 candle
    /// 示例也是分两处下载的。
    tokenizer_repository: &'static str,
}

impl HuggingFaceModelInfo {
    /// 分词器（以及 `config.json`）所在仓库；未单独指定时回退到权重仓库。
    pub(super) fn tokenizer_repository(&self) -> &'static str {
        if self.tokenizer_repository.is_empty() {
            self.mirror
        } else {
            self.tokenizer_repository
        }
    }

    /// GGUF 权重文件的完整本地路径。非 GGUF 模型返回 None。
    pub(super) fn gguf_model_path(&self) -> Option<String> {
        if self.gguf_model_filename.is_empty() {
            None
        } else {
            Some(construct_model_file_path(
                self.local_directory(),
                self.gguf_model_filename,
            ))
        }
    }

    /// 这个模型的本地根目录名（相对 `HUGGING_FACE_MODEL_ROOT`）。
    ///
    /// 下载与加载**必须**都从这里取。以前下载用 `repository`、加载用 `mirror`，
    /// 两个字段一旦不一致，文件就被下载到一个目录、却去另一个目录加载。
    ///
    /// 取 `repository` 而不是 `mirror`：目录名要跟着**保存时**用的那个仓库走，
    /// 这样已经下载好的模型原地可用。`mirror` 只表示"从哪个镜像/量化仓库取文件"，
    /// 它会随量化档位变（`unsloth/Qwen3-0.6B-GGUF`），不该决定本地目录。
    pub(super) fn local_directory(&self) -> &'static str {
        self.repository
    }

    /// `local_directory()` 的完整路径。
    pub(super) fn local_directory_path(&self) -> String {
        format!("{HUGGING_FACE_MODEL_ROOT}{}", self.local_directory())
    }

    /// 本地目录的完整路径，以及这个目录**是否已经存在**。
    ///
    /// 设置页要回答两个问题："模型会落到哪个目录"（显示给用户）和"下载过没有"
    /// （要不要提示下载）。目录在不在就够回答后者了，不必逐个 stat 模型文件、
    /// 更不必解析 `tokenizer.json`（几兆到几十兆）——那是 [`check_model_files`]
    /// 的活，成本高，应该由用户主动触发。
    ///
    /// 路径**必须**来自 [`Self::local_directory_path`]：它和真正写文件的下载路径
    /// 同源，这样显示给用户的目录不可能是编出来的。`pub(crate)` 就是为了让
    /// `man::settings` 走这条路，而不是自己拼一个可能漂移的字符串。
    pub(crate) fn local_directory_status(&self) -> (String, bool) {
        let path = self.local_directory_path();
        let exists = Path::new(&path).is_dir();
        (path, exists)
    }

    /// 本地 `tokenizer.json` 的完整路径。
    pub(super) fn tokenizer_path(&self) -> String {
        construct_model_file_path(self.local_directory(), "tokenizer.json")
    }

    pub(super) fn supports_vision(&self) -> bool {
        matches!(
            self.model_type,
            HuggingFaceModelType::Moondream | HuggingFaceModelType::Gemma4
        )
    }

    /// 把对话历史拼成模型要的提示词。
    ///
    /// `enable_thinking` 只对 Qwen3 有意义（`false` 时注入官方模板的 `/no_think`）；
    /// 其余模型完全忽略它 —— 给 llama/gemma/phi3 塞 `/no_think` 只会污染它们的
    /// 提示词，所以这个开关必须在这里按模型类型判断，不能让调用方自己拼字符串。
    pub(super) fn convert_prompt(
        &self,
        s: &str,
        history: Option<Vec<crate::ai::chat::Prompt>>,
        enable_thinking: bool,
    ) -> Result<String> {
        let mut system = String::new();
        let mut user = String::new();
        if !s.is_empty() && s.starts_with("[") {
            let mut prompts: Vec<super::chat::Prompt> = serde_json::from_str(s)?;
            for p in prompts.iter_mut() {
                if p.role.eq("system") {
                    std::mem::swap(&mut system, &mut p.content);
                } else if p.role.eq("user") {
                    std::mem::swap(&mut user, &mut p.content);
                }
            }
        }
        match self.model_type {
            HuggingFaceModelType::Bert => {
                let m = String::from("Bert model doesn't support prompt.");
                log::warn!("{}", &m);
                Err(Error::WithMessage(m))
            }
            HuggingFaceModelType::Llama => {
                let mut p = String::with_capacity(s.len());
                if !system.is_empty() {
                    p.push_str("<|system|>\n");
                    p.push_str(&system);
                    p.push_str("</s>\n");
                }
                if let Some(h) = history {
                    for i in h.iter() {
                        if i.content.is_empty() {
                            continue;
                        }
                        p.push_str("<|");
                        p.push_str(&i.role);
                        p.push_str("|>\n");
                        p.push_str(&i.content);
                        p.push_str("</s>\n");
                    }
                }
                if !user.is_empty() {
                    p.push_str("<|user|>\n");
                    p.push_str(&user);
                    p.push_str("</s>\n");
                }
                p.push_str("<|assistant|>");
                // p.push_str("<|begin_of_text|>");
                // if !system.is_empty() {
                //     p.push_str("<|start_header_id|>system<|end_header_id|>");
                //     p.push_str(&system);
                //     p.push_str("<|eot_id|>");
                // }
                // p.push_str("<|start_header_id|>user<|end_header_id|>");
                // p.push_str(&user);
                // p.push_str("<|eot_id|><|start_header_id|>assistant<|end_header_id|><|eot_id|>");
                Ok(p)
            }
            HuggingFaceModelType::Gemma => {
                let mut p = String::with_capacity(s.len());
                // p.push_str("<bos>");
                if let Some(h) = history {
                    for i in h.iter() {
                        p.push_str("<start_of_turn>");
                        if i.role.eq("assistant") {
                            p.push_str("model");
                        } else {
                            p.push_str(&i.role);
                        }
                        p.push('\n');
                        p.push_str(&i.content);
                        p.push_str("<end_of_turn>\n");
                    }
                }
                p.push_str("<start_of_turn>user\n");
                p.push_str(&user);
                p.push_str("<end_of_turn>\n<start_of_turn>model");
                Ok(p)
            }
            // Gemma 4 换了回合标记：`<|turn>role\n … <turn|>\n`，生成前缀是
            // `<|turn>model\n`（见仓库里的 `chat_template.jinja`）。system 只在
            // **有 system 消息**时开一个系统回合；`enable_thinking` 打开时官方模板
            // 会在系统回合最前面注入 `<|think|>`。两者是独立的：有系统提示词但没开
            // 思考时，照样要有系统回合，只是不带 `<|think|>`。
            //
            // system / user 的正文按官方模板 trim，assistant 的历史正文先过一次
            // `strip_thinking`（上一轮回答可能带着 `<|channel>` 推理块回传）。
            HuggingFaceModelType::Gemma4 => {
                use super::gemma::{THINK_TOKEN, TURN_END, TURN_START};
                let mut p = String::with_capacity(s.len() + 64);
                // 模板以 `{{- bos_token }}` 开头，`<bos>` 由 tokenizer 的
                // post-processor 自己加，这里不能重复。
                if enable_thinking || !system.is_empty() {
                    p.push_str(TURN_START);
                    p.push_str("system\n");
                    if enable_thinking {
                        p.push_str(THINK_TOKEN);
                        p.push('\n');
                    }
                    if !system.is_empty() {
                        p.push_str(system.trim());
                    }
                    p.push_str(TURN_END);
                }
                if let Some(h) = history {
                    for i in h.iter() {
                        if i.content.is_empty() {
                            continue;
                        }
                        let role = if i.role.eq("assistant") {
                            "model"
                        } else {
                            i.role.as_str()
                        };
                        p.push_str(TURN_START);
                        p.push_str(role);
                        p.push('\n');
                        if role.eq("model") {
                            p.push_str(&super::gemma::strip_thinking(&i.content));
                        } else {
                            p.push_str(i.content.trim());
                        }
                        p.push_str(TURN_END);
                    }
                }
                // 两个调用方都传 `s = ""`，最新的那条 user 消息在 history 末尾，
                // 所以 `user` 通常是空的 —— 空回合要跳过。
                if !user.is_empty() {
                    p.push_str(TURN_START);
                    p.push_str("user\n");
                    p.push_str(user.trim());
                    p.push_str(TURN_END);
                }
                p.push_str(TURN_START);
                p.push_str("model\n");
                Ok(p)
            }
            HuggingFaceModelType::Phi3 => {
                let mut p = String::with_capacity(s.len());
                p.push_str("<s>");
                if !system.is_empty() {
                    p.push_str("<|system|>\n");
                    p.push_str(&system);
                    p.push_str("<|end|>\n");
                }
                if let Some(h) = history {
                    for i in h.iter() {
                        p.push_str("<|");
                        p.push_str(&i.role);
                        p.push_str("|>\n");
                        p.push_str(&i.content);
                        p.push_str("<|end|>\n");
                    }
                }
                p.push_str("<|user|>\n");
                p.push_str(&user);
                p.push_str("<|end|>\n<|assistant|>");
                Ok(p)
            }
            // Phi-4-mini（V4Mini）的官方模板，照 `tokenizer_config.json` 里的
            // `chat_template` 逐字搬过来：
            //   `{{ '<|' + message['role'] + '|>' + message['content'] + '<|end|>' }}`
            //   最后是 `{{ '<|assistant|>' }}`。
            //
            // 和上面 Phi-3 那一支有**两处不能照抄**的差别：
            //   * 开头**没有** `<s>`（Phi-3 模板以 `{{ bos_token }}` 起头，V4Mini 的
            //     tokenizer 是 `add_bos_token: false`）；
            //   * 角色标记后面**没有** `\n`，正文直接接 `<|end|>`。
            // 多出来的换行虽然会被 `<|end|>` 这个 rstrip 的 added token 吃掉一部分，
            // 但把官方形状原样拼出来才是安全的。
            //
            // 空的 history 回合要跳过（`<|user|><|end|>` 不是模板会产出的形状）；
            // 两个调用方都传 `s = ""`，最新那条 user 消息在 history 末尾，所以
            // 上面那个单独的 `user` 分支通常为空 —— 这里**不**像 Phi-3 那一支那样
            // 无条件补一个空回合，官方模板也不会产出它。
            HuggingFaceModelType::Phi4Mini => {
                let mut p = String::with_capacity(s.len() + 64);
                if !system.is_empty() {
                    p.push_str("<|system|>");
                    p.push_str(&system);
                    p.push_str("<|end|>");
                }
                if let Some(h) = history {
                    for i in h.iter() {
                        if i.content.is_empty() {
                            continue;
                        }
                        p.push_str("<|");
                        p.push_str(&i.role);
                        p.push_str("|>");
                        p.push_str(&i.content);
                        p.push_str("<|end|>");
                    }
                }
                if !user.is_empty() {
                    p.push_str("<|user|>");
                    p.push_str(&user);
                    p.push_str("<|end|>");
                }
                p.push_str("<|assistant|>");
                Ok(p)
            }
            // Qwen3 用 ChatML。和上面几个一样，只有 system 和我们自己拼的
            // assistant 开头是固定的，history 原样保留。
            //
            // 历史里的 `tool` 回合先开一个 `<|im_start|>user`；本项目的
            // `Prompt.role` 目前只有 system/user/assistant，走不到这个分支，
            // 但保持一致，将来加角色时不会静默丢弃内容。
            HuggingFaceModelType::Qwen3 | HuggingFaceModelType::Qwen3Moe => {
                let mut p = String::with_capacity(s.len() + 64);
                if !system.is_empty() {
                    p.push_str("<|im_start|>system\n");
                    p.push_str(&system);
                    p.push_str("<|im_end|>\n");
                }
                if let Some(h) = history {
                    for i in h.iter() {
                        if i.content.is_empty() {
                            continue;
                        }
                        if i.role.eq("tool") {
                            p.push_str("<|im_start|>user\n<tool_response>\n");
                            p.push_str(&i.content);
                            p.push_str("\n</tool_response><|im_end|>\n");
                        } else {
                            p.push_str("<|im_start|>");
                            p.push_str(&i.role);
                            p.push('\n');
                            p.push_str(&i.content);
                            p.push_str("<|im_end|>\n");
                        }
                    }
                }
                // 两个调用方都传 `s = ""`，最新的那条 user 消息在 history 末尾，
                // 所以 `user` 通常是空的。空回合要跳过 —— `<|im_start|>user\n
                // <|im_end|>` 不是模板会产出的东西，白送一个空回合只会让模型困惑。
                // （其它分支会原样输出一个空回合，那是既有的小瑕疵，不在本次范围内。）
                if !user.is_empty() {
                    p.push_str("<|im_start|>user\n");
                    p.push_str(&user);
                    p.push_str("<|im_end|>\n");
                }
                // 生成前缀。关闭思考时照着 Qwen3 自己的 `chat_template` 来：
                // 它的 `add_generation_prompt` 分支在 `enable_thinking == false`
                // 时就追加一个**空的 think 块**，模型随后直接给答案、不会再吐
                // 思考内容。
                //
                // 早先这里发的是 `/no_think`（Qwen2.5 的软开关）。实测不行：
                // 0.6B 不认它，照样输出 `<think>\n\n</think>\n\n` 前缀，
                // 那个 `</think>` 会被当成正文流到用户面前。
                //
                // 实测对比（`Qwen3-0.6B-Q4_K_M`，同一 seed）：
                //   `/no_think` 版 → `<think>\n\n</think>\n\n当然可以！…`
                //   空 think 块版 → `当然可以！…`            ← 模型跳过了思考
                p.push_str("<|im_start|>assistant\n");
                if !enable_thinking {
                    p.push_str("<think>\n\n</think>\n\n");
                }
                Ok(p)
            }
            HuggingFaceModelType::Moondream => todo!()
        }
    }
}

fn get_common_model_files() -> Vec<&'static str> {
    vec![
        "model.safetensors",
        "tokenizer.json",
        "config.json",
        "special_tokens_map.json",
        "tokenizer_config.json",
    ]
}

/// GGUF 模型要下载的**非权重**文件。
///
/// 不能复用 `get_common_model_files()`：权重仓库里没有 `model.safetensors`，
/// 而 `unsloth/*-GGUF` 也没有 `special_tokens_map.json`，列进去只会拿到 404。
/// GGUF 文件本身由 `gguf_model_filename` 单独描述。
fn qwen3_model_files() -> Vec<&'static str> {
    vec!["tokenizer.json", "tokenizer_config.json", "config.json"]
}

/// Gemma 4 要下载的**非权重**文件。
///
/// 不能直接复用 `get_common_model_files()`：权重是单文件 `model.safetensors`，
/// 而那个列表里就带着 "model.safetensors"，会被 `get_model_files` 当成额外文件
/// 再拼一遍。`processor_config.json` 必须下 —— 图像预处理的
/// `max_soft_tokens = 280` / patch / 池化参数都是照它来的。
fn gemma4_model_files() -> Vec<&'static str> {
    vec![
        "tokenizer.json",
        "tokenizer_config.json",
        "config.json",
        "processor_config.json",
    ]
}

impl HuggingFaceModel {
    pub(crate) fn get_info(&self) -> HuggingFaceModelInfo {
        match self {
            HuggingFaceModel::AllMiniLML6V2 => HuggingFaceModelInfo {
                repository: "sentence-transformers/all-MiniLM-L6-v2",
                mirror: "sentence-transformers/all-MiniLM-L6-v2",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 384,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::ParaphraseMLMiniLML12V2 => HuggingFaceModelInfo {
                repository: "sentence-transformers/paraphrase-MiniLM-L12-v2",
                mirror: "sentence-transformers/paraphrase-MiniLM-L12-v2",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 384,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::ParaphraseMLMpnetBaseV2 => HuggingFaceModelInfo {
                repository: "sentence-transformers/paraphrase-multilingual-mpnet-base-v2",
                mirror: "sentence-transformers/paraphrase-multilingual-mpnet-base-v2",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 768,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::BgeSmallEnV1_5 => HuggingFaceModelInfo {
                repository: "BAAI/bge-small-en-v1.5",
                mirror: "BAAI/bge-small-en-v1.5",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 384,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::BgeBaseEnV1_5 => HuggingFaceModelInfo {
                repository: "BAAI/bge-base-en-v1.5",
                mirror: "BAAI/bge-base-en-v1.5",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 768,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::BgeLargeEnV1_5 => HuggingFaceModelInfo {
                repository: "BAAI/bge-large-en-v1.5",
                mirror: "BAAI/bge-large-en-v1.5",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::BgeM3 => HuggingFaceModelInfo {
                repository: "BAAI/bge-m3",
                mirror: "BAAI/bge-m3",
                model_files: vec!["onnx/model.onnx", "onnx/model.onnx_data"],
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::NomicEmbedTextV1_5 => HuggingFaceModelInfo {
                repository: "nomic-ai/nomic-embed-text-v1.5",
                mirror: "nomic-ai/nomic-embed-text-v1.5",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 768,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::MultilingualE5Small => HuggingFaceModelInfo {
                repository: "intfloat/multilingual-e5-small",
                mirror: "intfloat/multilingual-e5-small",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 384,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::MultilingualE5Base => HuggingFaceModelInfo {
                repository: "intfloat/multilingual-e5-base",
                mirror: "intfloat/multilingual-e5-base",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 768,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::MultilingualE5Large => HuggingFaceModelInfo {
                repository: "intfloat/multilingual-e5-large",
                mirror: "intfloat/multilingual-e5-large",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::MxbaiEmbedLargeV1 => HuggingFaceModelInfo {
                repository: "mixedbread-ai/mxbai-embed-large-v1",
                mirror: "mixedbread-ai/mxbai-embed-large-v1",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Bert,
            },
            HuggingFaceModel::Phi3Mini4kInstruct => HuggingFaceModelInfo {
                repository: "microsoft/Phi-3-mini-4k-instruct",
                mirror: "microsoft/Phi-3-mini-4k-instruct",
                model_files: {
                    let mut v = get_common_model_files();
                    let mut idx = 0usize;
                    for &f in v.iter() {
                        if f.eq("model.safetensors") {
                            break;
                        }
                        idx += 1;
                    }
                    v.remove(idx);
                    v
                },
                model_index_file: "model.safetensors.index.json",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Phi3,
            },
            // Phi-4-mini 的权重同样是分片 safetensors（`model-00001-of-00002` 两片 +
            // 索引），所以和上面 Phi-3 一样：`model_files` 里去掉
            // "model.safetensors"、由 `model_index_file` 描述权重。
            // `tie_word_embeddings: true`，没有单独的 `lm_head` 权重。
            HuggingFaceModel::Phi4MiniInstruct => HuggingFaceModelInfo {
                repository: "microsoft/Phi-4-mini-instruct",
                mirror: "microsoft/Phi-4-mini-instruct",
                model_files: {
                    let mut v = get_common_model_files();
                    let mut idx = 0usize;
                    for &f in v.iter() {
                        if f.eq("model.safetensors") {
                            break;
                        }
                        idx += 1;
                    }
                    v.remove(idx);
                    v
                },
                model_index_file: "model.safetensors.index.json",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 3072,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Phi4Mini,
            },
            HuggingFaceModel::TinyLlama1_1bChatV1_0 => HuggingFaceModelInfo {
                repository: "TinyLlama/TinyLlama-1.1B-Chat-v1.0",
                mirror: "TinyLlama/TinyLlama-1.1B-Chat-v1.0",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Llama,
            },
            HuggingFaceModel::Gemma2bInstruct => HuggingFaceModelInfo {
                repository: "google/gemma-2b-it",
                mirror: "google/gemma-2b-it",
                model_files: {
                    let mut v = get_common_model_files();
                    let mut idx = 0usize;
                    for &f in v.iter() {
                        if f.eq("model.safetensors") {
                            break;
                        }
                        idx += 1;
                    }
                    v.remove(idx);
                    v
                },
                model_index_file: "model.safetensors.index.json",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Gemma,
            },
            HuggingFaceModel::Gemma7bInstruct => HuggingFaceModelInfo {
                repository: "google/gemma-7b-it",
                mirror: "google/gemma-7b-it",
                model_files: {
                    let mut v = get_common_model_files();
                    let mut idx = 0usize;
                    for &f in v.iter() {
                        if f.eq("model.safetensors") {
                            break;
                        }
                        idx += 1;
                    }
                    v.remove(idx);
                    v
                },
                model_index_file: "model.safetensors.index.json",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Gemma,
            },
            // ---- 本地 Gemma 4（多模态，文本 + 视觉） ----
            //
            // 两个档位都是**单文件** `model.safetensors`（没有分片索引），所以
            // `model_files` 里要把自带的 "model.safetensors" 去掉、并让
            // `model_index_file` 留空，`get_model_files` 才会去找那一个文件。
            //
            // `processor_config.json` 要一起下：它带着 `max_soft_tokens = 280` 和
            // patch/池化参数，是图像预处理（`gemma.rs`）的依据。
            HuggingFaceModel::Gemma4E2BIt => HuggingFaceModelInfo {
                repository: "google/gemma-4-E2B-it",
                mirror: "google/gemma-4-E2B-it",
                model_files: gemma4_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Gemma4,
            },
            HuggingFaceModel::Gemma4E4BIt => HuggingFaceModelInfo {
                repository: "google/gemma-4-E4B-it",
                mirror: "google/gemma-4-E4B-it",
                model_files: gemma4_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Gemma4,
            },
            HuggingFaceModel::Gemma412BIt => HuggingFaceModelInfo {
                repository: "google/gemma-4-12B-it",
                mirror: "google/gemma-4-12B-it",
                model_files: gemma4_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Gemma4,
            },
            HuggingFaceModel::Moondream2 => HuggingFaceModelInfo {
                repository: "vikhyatk/moondream2",
                mirror: "vikhyatk/moondream2",
                model_files: {
                    let mut v = get_common_model_files();
                    let mut idx = 0usize;
                    for &f in v.iter() {
                        if f.eq("model.safetensors") {
                            break;
                        }
                        idx += 1;
                    }
                    v.remove(idx);
                    v
                },
                model_index_file: "model.safetensors.index.json",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Moondream,
            },
            HuggingFaceModel::ParlerTtsMiniV1 => HuggingFaceModelInfo {
                repository: "parler-tts/parler-tts-mini-v1",
                mirror: "parler-tts/parler-tts-mini-v1",
                model_files: get_common_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Llama,
            },
            HuggingFaceModel::ParlerTtsLargeV1 => HuggingFaceModelInfo {
                repository: "parler-tts/parler-tts-large-v1",
                mirror: "parler-tts/parler-tts-large-v1",
                model_files: {
                    let mut v = get_common_model_files();
                    let mut idx = 0usize;
                    for &f in v.iter() {
                        if f.eq("model.safetensors") {
                            break;
                        }
                        idx += 1;
                    }
                    v.remove(idx);
                    v
                },
                model_index_file: "model.safetensors.index.json",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "",
                tokenizer_repository: "",
                model_type: HuggingFaceModelType::Gemma,
            },
            // ---- 本地千问 Qwen3（GGUF 量化） ----
            //
            // 权重来自 unsloth 的 GGUF 仓库，分词器来自官方 base 仓库 —— 前者是
            // 纯权重仓库（只有 .gguf + README/params），没有 tokenizer.json。
            // 选 unsloth 而不是 `Qwen/Qwen3-*-GGUF` 是因为量化档位全得多：
            // 官方 8B 最小只到 Q4_K_M（约 5GB），而低配机器真正能跑的
            // Q2_K / IQ4_XS / UD-Q2_K_XL 只有 unsloth 有。这与上游 candle 的
            // quantized-qwen3 示例一致。
            //
            // 这些 GGUF 都是**单文件**（分片只出现在 BF16/ 子目录和 UD-* 大分片
            // 里），这点很重要：candle 的 `from_gguf` 只接受一个 reader，不支持
            // 多分片。
            HuggingFaceModel::Qwen3_0_6B => HuggingFaceModelInfo {
                repository: "Qwen/Qwen3-0.6B",
                mirror: "unsloth/Qwen3-0.6B-GGUF",
                model_files: qwen3_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "Qwen3-0.6B-Q4_K_M.gguf",
                tokenizer_repository: "Qwen/Qwen3-0.6B",
                model_type: HuggingFaceModelType::Qwen3,
            },
            HuggingFaceModel::Qwen3_1_7B => HuggingFaceModelInfo {
                repository: "Qwen/Qwen3-1.7B",
                mirror: "unsloth/Qwen3-1.7B-GGUF",
                model_files: qwen3_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "Qwen3-1.7B-Q4_K_M.gguf",
                tokenizer_repository: "Qwen/Qwen3-1.7B",
                model_type: HuggingFaceModelType::Qwen3,
            },
            HuggingFaceModel::Qwen3_4B => HuggingFaceModelInfo {
                repository: "Qwen/Qwen3-4B",
                mirror: "unsloth/Qwen3-4B-GGUF",
                model_files: qwen3_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "Qwen3-4B-Q4_K_M.gguf",
                tokenizer_repository: "Qwen/Qwen3-4B",
                model_type: HuggingFaceModelType::Qwen3,
            },
            HuggingFaceModel::Qwen3_8B => HuggingFaceModelInfo {
                repository: "Qwen/Qwen3-8B",
                mirror: "unsloth/Qwen3-8B-GGUF",
                model_files: qwen3_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "Qwen3-8B-Q4_K_M.gguf",
                tokenizer_repository: "Qwen/Qwen3-8B",
                model_type: HuggingFaceModelType::Qwen3,
            },
            HuggingFaceModel::Qwen3_14B => HuggingFaceModelInfo {
                repository: "Qwen/Qwen3-14B",
                mirror: "unsloth/Qwen3-14B-GGUF",
                model_files: qwen3_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "Qwen3-14B-Q4_K_M.gguf",
                tokenizer_repository: "Qwen/Qwen3-14B",
                model_type: HuggingFaceModelType::Qwen3,
            },
            HuggingFaceModel::Qwen3_32B => HuggingFaceModelInfo {
                repository: "Qwen/Qwen3-32B",
                mirror: "unsloth/Qwen3-32B-GGUF",
                model_files: qwen3_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "Qwen3-32B-Q4_K_M.gguf",
                tokenizer_repository: "Qwen/Qwen3-32B",
                model_type: HuggingFaceModelType::Qwen3,
            },
            // MoE：30B 总参数 / 3B 激活。GGUF 仓库里**没有** config.json，
            // 正好由 tokenizer_repository 一起兜住。
            HuggingFaceModel::Qwen3_30bA3bInstruct2507 => HuggingFaceModelInfo {
                repository: "Qwen/Qwen3-30B-A3B-Instruct-2507",
                mirror: "unsloth/Qwen3-30B-A3B-Instruct-2507-GGUF",
                model_files: qwen3_model_files(),
                model_index_file: "",
                tokenizer_filename: "tokenizer.json",
                dimenssions: 1024,
                gguf_model_filename: "Qwen3-30B-A3B-Instruct-2507-Q4_K_M.gguf",
                tokenizer_repository: "Qwen/Qwen3-30B-A3B-Instruct-2507",
                model_type: HuggingFaceModelType::Qwen3Moe,
            },
            HuggingFaceModel::WhisperLargeV3 => todo!(),
        }
    }
}

impl std::fmt::Display for HuggingFaceModel {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
        // or, alternatively:
        // fmt::Debug::fmt(self, f)
    }
}

/// 本地模型的根目录。下载、加载、校验、以及设置页显示给用户的路径全部从这里来。
///
/// 用**复数** `models`：这是同类工具的通行写法（ollama 的 `~/.ollama/models`、
/// LM Studio / llama.cpp 的 `models`），而且设置页里显示的文案本来就是
/// `./data/models`。以前这里是单数 `./data/model/`，前端那句提示就一直是错的，
/// 用户照着提示去目录里找根本找不到文件。
const HUGGING_FACE_MODEL_ROOT: &str = "./data/models/";

#[derive(Clone, Serialize)]
pub(crate) struct DownloadStatus {
    pub(crate) downloading: bool,
    #[serde(rename = "totalLen")]
    pub(crate) total_len: u64,
    #[serde(rename = "downloadedLen")]
    pub(crate) downloaded_len: u64,
    pub(crate) url: String,
    pub(crate) err: String,
}

pub(crate) static DOWNLOAD_STATUS: OnceLock<Mutex<DownloadStatus>> = OnceLock::new();

pub(crate) fn get_download_status() -> Option<DownloadStatus> {
    if let Some(op) = DOWNLOAD_STATUS.get() {
        return match op.lock() {
            Ok(s) => Some(s.clone()),
            Err(e) => {
                log::error!("{:?}", &e);
                None
            }
        };
    }
    None
}

fn download_status<'l>() -> Result<std::sync::MutexGuard<'l, DownloadStatus>> {
    let locker = match DOWNLOAD_STATUS
        .get_or_init(|| {
            Mutex::new(DownloadStatus {
                downloading: false,
                total_len: 1,
                downloaded_len: 0,
                url: String::new(),
                err: String::new(),
            })
        })
        .try_lock()
    {
        Ok(l) => l,
        Err(e) => match e {
            std::sync::TryLockError::Poisoned(pe) => {
                log::warn!("{:?}", &pe);
                pe.into_inner()
            }
            std::sync::TryLockError::WouldBlock => {
                return Err(Error::WithMessage(String::from(
                    "Model files are downloading.",
                )));
            }
        },
    };
    Ok(locker)
}

pub(crate) async fn download_hf_models(
    info: &HuggingFaceModelInfo,
    huggingface_token: &str,
    connect_timeout: u64,
    read_timeout: u64,
) -> Result<()> {
    // if let Ok(v) = DOWNLOAD_STATUS
    //     .get_or_init(|| {
    //         Mutex::new(DownloadStatus {
    //             downloading: false,
    //             total_len: 1,
    //             downloaded_len: 0,
    //             url: String::new(),
    //         })
    //     })
    //     .lock()
    // {
    //     if v.downloading {
    //         return Err(Error::ErrorWithMessage(String::from(
    //             "Model files are downloading.",
    //         )));
    //     }
    // }
    {
        let mut status = download_status()?;
        if status.downloading {
            return Err(Error::WithMessage(String::from(
                "Model files are downloading.",
            )));
        }
        status.downloading = true;
    }
    // 目录名走 `local_directory()`，和加载路径（`construct_model_file_path` /
    // `gguf_model_path`）同源。以前这里用 `info.repository`，于是 GGUF 模型
    // （repository 是所有权仓库、mirror 是量化仓库）下载与加载各去一个目录。
    let root_path = info.local_directory_path();
    tokio::fs::create_dir_all(&root_path).await?;

    let mut headers = HeaderMap::new();
    let user_agent = format!(
        "unkown/None; Braidwork/Kiko/{}; rust/unknown",
        crate::web::server::VERSION
    );
    headers.insert("User-Agent", HeaderValue::from_str(&user_agent)?);
    if !huggingface_token.is_empty() {
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {huggingface_token}"))?,
        );
    }

    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(connect_timeout))
        .read_timeout(Duration::from_millis(read_timeout))
        .default_headers(headers);
    if let Ok(proxy) = std::env::var("https_proxy") {
        if !proxy.is_empty() {
            log::info!("Detected proxy setting: {}", &proxy);
            builder = builder.proxy(reqwest::Proxy::https(&proxy)?)
        }
    }
    let client = builder.build()?;
    // 每个文件只归**一个**仓库。曾经这里是"两个仓库各推一份"，依赖 `download_hf_file`
    // 跳过已存在的文件来收敛 —— 于是顺序成了行为的一部分，而且第一个被尝试的是
    // 错的仓库（`unsloth/Qwen3-0.6B-GGUF/tokenizer.json`，那个仓库里根本没这个
    // 文件），每次下载都先白挨一个 404。现在路由一次定死（`download_file_list`），
    // 顺序无关紧要。
    let mut files = download_file_list(info);
    let mut r: Result<_> = Ok(());
    if !info.model_index_file.is_empty() {
        let dir = info.local_directory();
        let model_index_file = construct_model_file_path(dir, info.model_index_file);
        let path = std::path::Path::new(&model_index_file);
        if !path.exists() {
            r = download_hf_file(&client, dir, &root_path, info.model_index_file).await;
        }
        if r.is_ok() {
            let f = load_safetensors(dir, info.model_index_file)?;
            files.extend(f.into_iter().map(|v| (dir, v)));
        }
    };
    if r.is_ok() {
        for (repository, f) in files.iter() {
            r = download_hf_file(&client, repository, &root_path, f).await;
            if r.is_err() {
                break;
            }
        }
    }
    {
        let mut status = download_status()?;
        if r.is_err() {
            status.err = format!("Download failed,err: {:?}", r.as_ref().err());
        }
        status.downloading = false;
    }

    r
}

/// 下载 `repository` 仓库里的 `f`。仓库必须显式传入，不能假设是 `info.mirror`：
/// GGUF 模型的分词器来自另一个仓库。
async fn download_hf_file(
    client: &reqwest::Client,
    repository: &str,
    root_path: &str,
    f: &str,
) -> Result<()> {
    let file_path_str = format!("{root_path}/{f}");
    let file_path = std::path::Path::new(&file_path_str);
    if tokio::fs::try_exists(file_path).await? {
        return Ok(());
    }
    let u = format!("https://huggingface.co/{repository}/resolve/main/{f}");
    if let Some(s) = DOWNLOAD_STATUS.get() {
        if let Ok(mut v) = s.lock() {
            v.url = String::from(f);
            // 进度是**按文件**的，每个文件从头计。原来只重置上限、不重置已下载
            // 字节数，于是多文件模型（现在所有 GGUF 模型都是）的进度条会累计上
            // 一个文件的字节数，看着像卡住或倒退。
            v.downloaded_len = 0;
        }
    }
    let res = client.get(&u).query(&[("download", "true")]).send().await?;
    let status = res.status();
    if !status.is_success() {
        // 以前这里不看状态码，404 的响应体会被当成模型文件写进磁盘，直到加载
        // 时才以「GGUF 魔数不对」之类的怪错爆出来。带上仓库名，因为现在同一个
        // 模型可能来自两个仓库。
        let body = res.text().await.unwrap_or_default();
        return Err(Error::WithMessage(format!(
            "Download {repository}/{f} failed with {status}: {}",
            body.chars().take(256).collect::<String>()
        )));
    }
    let total_size = res.content_length().unwrap_or(0);
    // println!("Downloading {f}, total size {total_size}");
    if let Some(s) = DOWNLOAD_STATUS.get() {
        if let Ok(mut v) = s.lock() {
            v.total_len = total_size;
        }
    }
    // let b = res.bytes().await?;
    // fs::write("./temp.file", b.as_ref()).await?;
    // let mut downloaded = 0u64;
    let mut stream = res.bytes_stream();
    let mut file = OpenOptions::new()
        .read(false)
        .write(true)
        .truncate(false)
        .create_new(true)
        .open(file_path)
        .await?;
    // let mut file = File::create("./temp.file").await?;

    let mut downloaded = 0u64;
    while let Some(item) = stream.next().await {
        let chunk = item?;
        file.write_all(&chunk).await?;
        downloaded = std::cmp::min(downloaded + (chunk.len() as u64), total_size);
        if let Some(s) = DOWNLOAD_STATUS.get() {
            if let Ok(mut v) = s.lock() {
                // log::info!("Downloaded {new}");
                v.downloaded_len = downloaded;
            }
        }
    }
    Ok(())
}

pub(super) fn construct_model_file_path(mirror: &str, f: &str) -> String {
    format!("{HUGGING_FACE_MODEL_ROOT}{mirror}/{f}")
}

pub(super) fn device() -> Result<Device> {
    if candle::utils::cuda_is_available() {
        Ok(Device::new_cuda(0)?)
    } else if candle::utils::metal_is_available() {
        Ok(Device::new_metal(0)?)
    } else {
        Ok(Device::Cpu)
    }
}

// type TokenizerImpl = tokenizers::TokenizerImpl<
//     tokenizers::ModelWrapper,
//     tokenizers::NormalizerWrapper,
//     tokenizers::PreTokenizerWrapper,
//     tokenizers::PostProcessorWrapper,
//     tokenizers::DecoderWrapper,
// >;

pub(crate) fn check_model_files(info: &HuggingFaceModelInfo) -> Result<()> {
    // let f = construct_model_file_path(repo, "config.json");
    // let config = std::fs::read(&f)?;
    // let config: serde_json::Value = serde_json::from_slice(&config)?;
    // let arch = &config["architectures"];
    // if !arch.is_array() {
    //     return Ok(false)
    // }
    // let architectures=arch.as_array().unwrap();
    // if architectures.len() <1{
    //     return Ok(false)
    // }
    // let arch=architectures.get(0).unwrap();
    // if !arch.is_string() {
    //     return Ok(false)
    // }
    // if arch.as_str().unwrap().starts_with("Bert") {
    let files = get_model_files(info)?;
    for f in files.iter() {
        let p = Path::new(f);
        if !p.exists() {
            return Err(Error::WithMessage(format!("Path {:?} is not exist.", p)));
        }
        let ext = p.extension();
        if ext.is_none() {
            return Err(Error::WithMessage(format!(
                "{:?} doesn't have extension.",
                p
            )));
        }
        let ext = ext.unwrap();
        if ext.eq("json") {
            // https://github.com/serde-rs/json/issues/160
            // let file = StdOpenOptions::new().read(true).write(false).create(false).open(&f)?;
            // let br = std::io::BufReader::with_capacity(4096, file);
            // let _ = serde_json::from_reader(br)?;
            let mut file = StdOpenOptions::new()
                .read(true)
                .write(false)
                .create(false)
                .open(f)?;
            let mut bytes = Vec::with_capacity(4096);
            file.read_to_end(&mut bytes)?;
            let _: serde::de::IgnoredAny = serde_json::from_slice(&bytes)?;
        } else if ext.eq("safetensors") {
            let metadata = std::fs::metadata(p)?;
            if metadata.len() < 62914560u64 {
                return Err(Error::WithMessage(format!(
                    "{:?} file size is too small.",
                    p
                )));
            }
        } else if ext.eq("gguf") {
            // 校验魔数。这一步不能省：下载失败时旧代码不看状态码，404 的响应体
            // 会被原样写进文件，直到 `Content::read` 才报一个和"文件不对"毫不
            // 相干的错。这里提前把话说明白。
            let mut file = StdOpenOptions::new()
                .read(true)
                .write(false)
                .create(false)
                .open(p)?;
            let mut magic = [0u8; 4];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut magic)?;
            if &magic != b"GGUF" {
                return Err(Error::WithMessage(format!(
                    "{:?} is not a GGUF file (magic: {:?}).",
                    p, magic
                )));
            }
        } else {
            // 原来是"未知后缀静默通过"：列错文件也检查不出来。
            return Err(Error::WithMessage(format!(
                "{:?} has an unsupported extension.",
                p
            )));
        }
    }
    Ok(())
    // match info.model_type {
    //     HuggingFaceModelType::Bert => load_bert_model_files(&info.repository)
    //         .map(|_| true)
    //         .or_else(|e| {
    //             log::warn!("Check bert model files failed,err: {:?}", &e);
    //             Ok(false)
    //         }),
    //     HuggingFaceModelType::Gemma => {
    //         let device = device()?;
    //         load_gemma_model_files(&info, &device)
    //             .map(|_| true)
    //             .or_else(|e| {
    //                 log::warn!("Check gemma model files failed,err: {:?}", &e);
    //                 Ok(false)
    //             })
    //     }
    //     HuggingFaceModelType::Llama => {
    //         let device = device()?;
    //         load_llama_model_files(&info, &device)
    //             .map(|_| true)
    //             .or_else(|e| {
    //                 log::warn!("Check llama model files failed,err: {:?}", &e);
    //                 Ok(false)
    //             })
    //     }
    //     HuggingFaceModelType::Phi3 => {
    //         let device = device()?;
    //         load_phi3_model_files(&info, &device)
    //             .map(|_| true)
    //             .or_else(|e| {
    //                 log::warn!("Check phi3 model files failed,err: {:?}", &e);
    //                 Ok(false)
    //             })
    //     }
    // }
}

/// 读已下载的 `tokenizer.json`。
///
/// 收的是**文件路径**而不是仓库名：分词器是两仓库下载的产物，落在
/// `local_directory()` 里（`Qwen/Qwen3-0.6B`），而不是它在 HuggingFace 上的
/// 来源仓库（`mirror`）。按仓库名去找会指到一个不存在的目录。
///
/// 只是"路径"这个口子本身太容易喂错——装载器请一律走 [`init_model_tokenizer`]。
pub(super) fn init_tokenizer(tokenizer_path: &str) -> Result<Tokenizer> {
    match Tokenizer::from_file(tokenizer_path) {
        Ok(t) => Ok(t),
        Err(e) => Err(Error::WithMessage(format!("{tokenizer_path}: {e}"))),
    }
}

/// 读 `info` 对应的本地 `tokenizer.json`（装载器统一入口）。
///
/// 收模型信息而不是路径，是因为**这个错误真的发生过**：
/// `load_llama_model_files` / `load_gemma_model_files` / `load_moondream_model_files`
/// / `load_parler_tts_model_files` 四处都写成过 `init_tokenizer(info.repository)`，
/// 把仓库名当文件路径去打开，于是这四个模型家族**永远加载失败**，用户看到的还是
/// 一句看不懂的 `TinyLlama/TinyLlama-1.1B-Chat-v1.0: 系统找不到指定的路径。`。
/// 不接受路径参数，这类错误就不可能再写出来。
pub(super) fn init_model_tokenizer(info: &HuggingFaceModelInfo) -> Result<Tokenizer> {
    init_tokenizer(&info.tokenizer_path())
}

fn set_tokenizer_config(
    tokenizer_path: &str,
    mut tokenizer: Tokenizer,
    pad_token_id: u32,
) -> Result<Tokenizer> {
    let dir = std::path::Path::new(tokenizer_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let f = dir.join("tokenizer_config.json");
    let p = f.as_path();
    let t = if p.exists() {
        let j: serde_json::Value = serde_json::from_slice(std::fs::read(&f)?.as_slice())?;
        let model_max_length = j["model_max_length"]
            .as_f64()
            .expect("Error reading model_max_length from tokenizer_config.json")
            as f32;
        let max_length = 8192.min(model_max_length as usize);
        let pad_token = j["pad_token"]
            .as_str()
            .expect("Error reading pad_token from tokenier_config.json")
            .into();
        // log::info!("p1 {}", tokenizer.get_padding().unwrap().pad_token);
        // log::info!("t1 {}", tokenizer.get_truncation().unwrap().max_length);
        tokenizer
            .with_padding(Some(PaddingParams {
                strategy: PaddingStrategy::BatchLongest,
                pad_token,
                pad_id: pad_token_id,
                ..Default::default()
            }))
            .with_truncation(Some(TruncationParams {
                max_length,
                ..Default::default()
            }))
    } else {
        tokenizer.with_padding(None).with_truncation(None)
    };
    let t = match t {
        Ok(t) => t.clone().into(),
        Err(e) => {
            log::warn!("{:?}", &e);
            tokenizer
        }
    };

    Ok(t)
    // log::info!("p2 {}", tokenizer.get_padding().unwrap().pad_token);
    // log::info!("t2 {}", tokenizer.get_truncation().unwrap().max_length);
}

fn set_special_tokens_map(tokenizer_path: &str, tokenizer: &mut Tokenizer) -> Result<()> {
    let dir = std::path::Path::new(tokenizer_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let f = dir.join("special_tokens_map.json");
    let p = f.as_path();
    if !p.exists() {
        return Ok(());
    }
    if let serde_json::Value::Object(root_object) =
        serde_json::from_slice(std::fs::read(&f)?.as_slice())?
    {
        for (_, value) in root_object.iter() {
            if value.is_string() {
                if let Err(e) = tokenizer.add_special_tokens([AddedToken {
                    content: value.as_str().unwrap().into(),
                    special: true,
                    ..Default::default()
                }]) {
                    log::warn!("Error adding special token: {}", e);
                }
            } else if value.is_object() {
                if let Err(e) = tokenizer.add_special_tokens([AddedToken {
                    content: value["content"].as_str().unwrap().into(),
                    special: true,
                    single_word: value["single_word"].as_bool().unwrap(),
                    lstrip: value["lstrip"].as_bool().unwrap(),
                    rstrip: value["rstrip"].as_bool().unwrap(),
                    normalized: value["normalized"].as_bool().unwrap(),
                }]) {
                    log::warn!("Error adding special token: {}", e);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn load_bert_model_files(info: &HuggingFaceModelInfo) -> Result<(BertModel, Tokenizer)> {
    // 本地目录一律走 `local_directory()`：下载落在那里，加载也从那里读。
    let dir = info.local_directory();
    let f = construct_model_file_path(dir, "config.json");
    let config = std::fs::read_to_string(&f)?;
    let config: serde_json::Value = serde_json::from_str(&config)?;
    let pad_token_id = config["pad_token_id"].as_u64().unwrap_or(0) as u32;
    let config: Config = serde_json::from_value(config)?;
    let tokenizer_path = info.tokenizer_path();
    let tokenizer = init_tokenizer(&tokenizer_path)?;
    let mut tokenizer = set_tokenizer_config(&tokenizer_path, tokenizer, pad_token_id)?;
    set_special_tokens_map(&tokenizer_path, &mut tokenizer)?;
    let f = construct_model_file_path(dir, "model.safetensors");
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[&f], DTYPE, &device()?)? };
    let model = BertModel::load(vb, &config)?;
    Ok((model, tokenizer))
}

fn load_safetensors(mirror: &str, json_file: &str) -> Result<Vec<String>> {
    let json_file = construct_model_file_path(mirror, json_file);
    let json_file = std::fs::File::open(json_file)?;
    let json: serde_json::Value =
        serde_json::from_reader(&json_file).map_err(candle::Error::wrap)?;
    let weight_map = match json.get("weight_map") {
        None => {
            return Err(Error::WithMessage(format!(
                "no weight map in {json_file:?}"
            )));
        }
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => {
            return Err(Error::WithMessage(format!(
                "weight map in {json_file:?} is not a map"
            )));
        }
    };
    let mut safetensors_files = std::collections::HashSet::new();
    for value in weight_map.values() {
        if let Some(file) = value.as_str() {
            safetensors_files.insert(file.to_string());
        }
    }
    Ok(Vec::from_iter(safetensors_files))
}

pub(crate) fn load_phi3_model_files(
    info: &HuggingFaceModelInfo,
) -> Result<(Device, Phi3, Tokenizer)> {
    let device = device()?;
    // V4Mini（Phi-4-mini）的权重是 BF16：照参考例子 `candle-examples/examples/phi/
    // main.rs` 的做法（V3 / V3-medium / V4Mini 都走这一支）用
    // `device.bf16_default_to_f32()` —— 支持 BF16 的设备（CUDA / Metal）用 BF16，
    // CPU 自动落回 F32。Phi-3-mini 保持原来的取舍不变。
    let dtype = match info.model_type {
        HuggingFaceModelType::Phi4Mini => device.bf16_default_to_f32(),
        _ => {
            if device.is_cuda() {
                DType::BF16
            } else {
                DType::F32
            }
        }
    };
    let dir = info.local_directory();
    let filenames = load_safetensors(dir, info.model_index_file)?
        .iter()
        .map(|v| std::path::PathBuf::from(construct_model_file_path(dir, v)))
        .collect::<Vec<_>>();
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&filenames, dtype, &device)? };
    let config_filename = construct_model_file_path(dir, "config.json");
    let config = std::fs::read_to_string(config_filename)?;
    let config: Phi3Config = serde_json::from_str(&config)?;
    let phi3 = Phi3::new(&config, vb)?;
    let tokenizer = init_model_tokenizer(info)?;
    Ok((device, phi3, tokenizer))
}

/// 需要校验/下载的权重文件路径。
///
/// 注意每个路径都按**它自己所属的仓库**来拼：GGUF 模型的 tokenizer 在另一个
/// 仓库，`info.model_files` 里既有本仓库的文件（`config.json`）也有 tokenizer
/// 仓库的文件（`tokenizer.json`）。
fn get_model_files(info: &HuggingFaceModelInfo) -> Result<Vec<String>> {
    // 本地路径全部按 `local_directory()` 拼 —— 下载落在那里，校验也从那里看。
    let dir = info.local_directory();
    let mut f = if info.model_index_file.is_empty() {
        vec![construct_model_file_path(dir, "model.safetensors")]
    } else {
        load_safetensors(dir, info.model_index_file)?
            .iter()
            .map(|v| construct_model_file_path(dir, v))
            .collect::<Vec<_>>()
    };
    if !info.gguf_model_filename.is_empty() {
        // GGUF 模型没有 model.safetensors —— 上面那个占位路径要把权重换成 GGUF。
        f.clear();
        f.push(construct_model_file_path(dir, info.gguf_model_filename));
    }
    for &name in info.model_files.iter() {
        f.push(construct_model_file_path(dir, name));
    }
    Ok(f)
}

/// 要下载的文件清单：`(仓库, 文件名)`。
///
/// 拆成独立函数是为了能被单测覆盖 —— 这里出过一次真实的错（同一份
/// `tokenizer.json` 被同时指向 GGUF 权重仓库和 base 模型仓库，于是每次下载都先
/// 向一个没有该文件的仓库发一次请求）。不联网也能断言路由结果。
fn download_file_list(info: &HuggingFaceModelInfo) -> Vec<(&'static str, String)> {
    let mut files: Vec<(&'static str, String)> = info
        .model_files
        .iter()
        .map(|&name| (file_repository(info, name), String::from(name)))
        .collect();
    if !info.gguf_model_filename.is_empty() {
        files.push((file_repository(info, info.gguf_model_filename), String::from(info.gguf_model_filename)));
    }
    files
}

/// `model_files` 里的某个文件该从哪个**远端**仓库取。
///
/// **下载与校验两条路径都必须走这里。** 之前下载那边是自己写的一套"两个仓库各
/// 推一份"的逻辑，于是每次下载都先往 GGUF 权重仓库要一次 `tokenizer.json`
/// （那里没有这个文件），只是靠"文件已存在就跳过"才没真正失败。
///
/// 依据：GGUF 权重仓库只有 `.gguf` + `config.json`（外加 README/params），
/// `tokenizer.json` / `tokenizer_config.json` 在 base 模型仓库。非 GGUF 模型
/// 两边是同一个仓库，走哪个分支都一样。
///
/// 这里回答的是"从哪儿下"，不是"存到哪儿" —— 本地一律进 `local_directory()`。
fn file_repository(info: &HuggingFaceModelInfo, name: &str) -> &'static str {
    let tokenizer_repository = info.tokenizer_repository();
    if tokenizer_repository != info.mirror && is_tokenizer_file(name) {
        tokenizer_repository
    } else {
        info.mirror
    }
}

/// `model_files` 里哪些属于分词器仓库。
fn is_tokenizer_file(name: &str) -> bool {
    name.starts_with("tokenizer") || name.eq("special_tokens_map.json") || name.eq("vocab.json")
}

pub(crate) fn load_llama_model_files(
    info: &HuggingFaceModelInfo,
) -> Result<(Device, Llama, LlamaCache, Tokenizer, Option<LlamaEosToks>)> {
    log::info!("load_llama_model_files start");
    let tokenizer = init_model_tokenizer(info)?;
    let device = device()?;

    let config_filename = construct_model_file_path(info.repository, "config.json");
    let config: LlamaConfig = serde_json::from_slice(&std::fs::read(config_filename)?)?;
    let config = config.into_config(device.is_cuda());
    let dtype = DType::F16;
    let cache = LlamaCache::new(true, dtype, &config, &device)?;
    let filenames = get_model_files(info)?;
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&filenames, dtype, &device)? };
    let m = Llama::load(vb, &config)?;
    let eos_token_id = config
        .eos_token_id
        .or_else(|| tokenizer.token_to_id("</s>").map(LlamaEosToks::Single));
    log::info!("load_llama_model_files end");
    Ok((device, m, cache, tokenizer, eos_token_id))
}

pub(crate) fn load_gemma_model_files(
    info: &HuggingFaceModelInfo,
) -> Result<(Device, GemmaModel, Tokenizer)> {
    let tokenizer = init_model_tokenizer(info)?;
    let device = device()?;

    let config_filename = construct_model_file_path(info.repository, "config.json");
    let config: GemmaConfig = serde_json::from_reader(std::fs::File::open(config_filename)?)?;
    let dtype = if device.is_cuda() {
        DType::BF16
    } else {
        DType::F32
    };
    let filenames = get_model_files(info)?;
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&filenames, dtype, &device)? };
    let model = GemmaModel::new(device.is_cuda(), &config, vb)?;
    Ok((device, model, tokenizer))
}

/// 读并解析 Gemma 4 的 `config.json`。
///
/// 多模态形状：`text_config` / `vision_config` / `audio_config` 三段并列，candle 的
/// `Gemma4Config` 就是照这个结构反序列化的。缺 `vision_config` 时给出**针对性的**
/// 提示，而不是让 serde 只报一句 "missing field"：那种检查点没有视觉塔，我们跑不了。
fn read_gemma4_config(config_filename: &str) -> Result<Gemma4Config> {
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config_filename)?).map_err(|e| {
            Error::WithMessage(format!("{config_filename} is not valid JSON: {e}"))
        })?;
    if raw.get("text_config").is_none() {
        return Err(Error::WithMessage(format!(
            "{config_filename} has no text_config: this does not look like a Gemma 4 config"
        )));
    }
    if raw.get("vision_config").is_none() {
        return Err(Error::WithMessage(format!(
            "{config_filename} has no vision_config: this Gemma 4 checkpoint carries no \
             vision tower, and this build only runs the multimodal ones \
             (google/gemma-4-E2B-it or -E4B-it)."
        )));
    }
    serde_json::from_value(raw).map_err(|e| {
        Error::WithMessage(format!(
            "cannot parse the Gemma 4 config in {config_filename}: {e}"
        ))
    })
}

/// 加载 Gemma 4 的权重与分词器。
///
/// 只支持**多模态**检查点：google 官方的 `gemma-4-E2B-it` / `-E4B-it` 都是这个形状，
/// 而我们要的正是文本 + 图像。配置里没有 `vision_config` 的检查点直接报错，而不是
/// 悄悄降级成纯文本 —— candle 0.11.0 的 `gemma4::text::TextModel` 确实是纯文本入口，
/// 但同一个模型实例是跨请求复用的（`chat.rs` 的 `LOADED_MODELS`），一旦按纯文本建
/// 出来，之后的带图请求就永远出不了图，那种"有时能收图有时不能"的行为比直接失败
/// 更难查。
pub(crate) fn load_gemma4_model_files(
    info: &HuggingFaceModelInfo,
) -> Result<(Device, super::gemma::Gemma4Loaded, Tokenizer)> {
    let start = std::time::Instant::now();
    let tokenizer = init_model_tokenizer(info)?;
    let device = device()?;
    let dtype = if device.is_cuda() {
        DType::BF16
    } else {
        DType::F32
    };
    let config_filename = construct_model_file_path(info.repository, "config.json");
    let config = read_gemma4_config(&config_filename)?;
    log::info!(
        "Gemma4: {} hidden size, {} text layers, {dtype:?}",
        config.text_config.hidden_size,
        config.text_config.num_hidden_layers,
    );
    let filenames = get_model_files(info)?;
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&filenames, dtype, &device)? };

    // 前缀是**检查点**决定的，不是我们决定的：多模态权重挂在
    // `model.language_model.*` / `model.vision_tower.*` 下。candle 的 `Model` 内部
    // 还会自己再 `pp("model")`，所以这里多包的那层就是最外层那个 `model`。
    if !vb.contains_tensor("model.language_model.embed_tokens.weight") {
        return Err(Error::WithMessage(format!(
            "{config_filename}: cannot find model.language_model.embed_tokens.weight in {}; \
             this does not look like a multimodal Gemma 4 checkpoint",
            filenames.join(", ")
        )));
    }
    let vb = vb.pp("model");

    // 注意 `Model::new` 会连**音频塔**一起建（配置里有 `audio_config` 就会
    // `vb.get` 它的权重），而我们只推送文本和图像。google/gemma-4-*-it 是 any-to-any
    // 检查点、权重里带着音频塔，所以正常；真遇到"只有文本+视觉"的检查点，这里会在
    // 加载时报找不到 `model.audio_tower.*` —— 那时的修法是先把
    // `raw_config["audio_config"]` 置成 `null` 再构造（`forward_multimodal` 本来就把
    // 音频塔当可选的，`None` 会直接跳过音频注入）。
    let model = super::gemma::Gemma4Loaded(Gemma4Model::new(&config, vb)?);
    log::info!("Gemma4: weights loaded in {:.2}s", start.elapsed().as_secs_f32());
    Ok((device, model, tokenizer))
}

pub(crate) fn load_parler_tts_model_files(
    info: &HuggingFaceModelInfo,
) -> Result<(Device, ParlerTtsModel, Tokenizer)> {
    let tokenizer = init_model_tokenizer(info)?;
    let device = device()?;
    let filenames = get_model_files(info)?;
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&filenames, DType::F32, &device)? };
    let config_filename = construct_model_file_path(info.repository, "config.json");
    let config: ParlerTtsConfig = serde_json::from_reader(std::fs::File::open(config_filename)?)?;
    let model = ParlerTtsModel::new(&config, vb)?;
    Ok((device, model, tokenizer))
}

pub(crate) fn load_moondream_model_files(
    info: &HuggingFaceModelInfo,
) -> Result<(Device, MoondreamModel, Tokenizer)> {
    let tokenizer = init_model_tokenizer(info)?;
    let device = device()?;
    let config = MoondreamConfig::v2();
    let dtype = if device.is_cuda() {
        DType::F16
    } else {
        DType::F32
    };
    let filenames = get_model_files(info)?;
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&filenames, dtype, &device)? };
    let model = MoondreamModel::new(&config, vb)?;
    Ok((device, model, tokenizer))
}

pub(crate) fn load_pytorch_mode_files(info: &HuggingFaceModelInfo, device: &Device) -> Result<()> {
    let weights_filename = construct_model_file_path(info.repository, "pytorch_model.bin");
    let vb = VarBuilder::from_pth(&weights_filename, DType::BF16, device)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GGUF model as it is actually configured: `repository` is both the
    /// local directory and the origin of the weights, while `tokenizer_repository`
    /// names a different repo that holds the tokenizer.
    fn gguf_info(local: &'static str, tokenizer_repository: &'static str) -> HuggingFaceModelInfo {
        HuggingFaceModelInfo {
            model_type: HuggingFaceModelType::Qwen3,
            repository: local,
            mirror: local,
            model_files: qwen3_model_files(),
            model_index_file: "",
            tokenizer_filename: "tokenizer.json",
            dimenssions: 1024,
            gguf_model_filename: "model-Q4_K_M.gguf",
            tokenizer_repository,
        }
    }

    /// Regression test: `tokenizer.json` and `tokenizer_config.json` may only be
    /// fetched from the tokenizer repo; `config.json` and the GGUF weights only
    /// from the weights repo.
    ///
    /// This used to be broken in a way that only showed up at runtime: the
    /// download list pushed *both* repos for every file, so each download first
    /// asked the quant repo for `tokenizer.json` (which is not there) and only
    /// survived because "file already exists" skipped it. The user reported
    /// exactly that wrong URL.
    #[test]
    fn gguf_files_each_go_to_exactly_one_repository() {
        let info = gguf_info("unsloth/Qwen3-0.6B-GGUF", "Qwen/Qwen3-0.6B");
        let files = download_file_list(&info);

        let repo_of = |name: &str| {
            files
                .iter()
                .find(|(_, f)| f == name)
                .map(|(r, _)| *r)
                .unwrap_or_else(|| panic!("{name} is missing from the download list"))
        };
        assert_eq!(repo_of("tokenizer.json"), "Qwen/Qwen3-0.6B");
        assert_eq!(repo_of("tokenizer_config.json"), "Qwen/Qwen3-0.6B");
        assert_eq!(repo_of("config.json"), "unsloth/Qwen3-0.6B-GGUF");
        assert_eq!(repo_of("model-Q4_K_M.gguf"), "unsloth/Qwen3-0.6B-GGUF");
        // The tokenizer repo must be taken from config, never guessed from the
        // weights repo name (the 30B-A3B entry differs).
        let moe = gguf_info(
            "unsloth/Qwen3-30B-A3B-Instruct-2507-GGUF",
            "Qwen/Qwen3-30B-A3B-Instruct-2507",
        );
        assert!(
            download_file_list(&moe)
                .iter()
                .any(|(r, f)| *r == "Qwen/Qwen3-30B-A3B-Instruct-2507" && f == "tokenizer.json"),
            "the tokenizer must come from the configured repository"
        );
    }

    /// The same file must never be listed twice: duplicate entries are the
    /// direct symptom of the bug above.
    #[test]
    fn download_list_has_no_duplicate_file_names() {
        let info = gguf_info("unsloth/Qwen3-0.6B-GGUF", "Qwen/Qwen3-0.6B");
        let files = download_file_list(&info);
        let mut seen = std::collections::HashSet::new();
        for (repository, f) in files.iter() {
            assert!(seen.insert(f.clone()), "{f} is listed more than once");
            assert!(!repository.is_empty(), "{f} has no repository");
        }
        assert_eq!(seen.len(), files.len());
    }

    /// The check pass and the download pass must name the same set of files.
    ///
    /// These two paths used to each carry their own routing logic, and their
    /// disagreement is what let the wrong-URL bug slip past `cargo check`.
    /// Download paths are remote (`unsloth/...`), check paths are local
    /// (`Qwen/Qwen3-0.6B/...`), so they are compared by file name — the file
    /// *set* is what has to match.
    #[test]
    fn check_and_download_paths_cover_the_same_files() {
        let info = HuggingFaceModel::Qwen3_0_6B.get_info();
        let mut checked: Vec<String> = get_model_files(&info)
            .unwrap()
            .iter()
            .map(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let mut downloaded: Vec<String> = download_file_list(&info)
            .into_iter()
            .map(|(_, f)| f)
            .collect();
        checked.sort();
        downloaded.sort();
        assert_eq!(checked, downloaded);
    }

    /// Where each file is fetched from, for a real entry. The weights repo and
    /// the local directory are deliberately different fields: the files are
    /// saved under the base-model directory (`Qwen/Qwen3-0.6B`) but the GGUF
    /// comes from unsloth's quant repo, and the tokenizer from the base repo.
    ///
    /// If the weights source were derived from `repository`, the 0.6B entry
    /// would pull `Qwen/Qwen3-0.6B-GGUF` (Q8_0 only, ~2x bigger) and the MoE
    /// entry would point at `Qwen/Qwen3-30B-A3B-Instruct-2507`, which has no
    /// GGUF repo at all.
    #[test]
    fn files_are_fetched_from_the_configured_repositories() {
        let info = HuggingFaceModel::Qwen3_0_6B.get_info();
        let repos: std::collections::HashMap<String, &str> = download_file_list(&info)
            .into_iter()
            .map(|(r, f)| (f, r))
            .collect();
        assert_eq!(repos["tokenizer.json"], "Qwen/Qwen3-0.6B");
        assert_eq!(repos["tokenizer_config.json"], "Qwen/Qwen3-0.6B");
        assert_eq!(repos["config.json"], "unsloth/Qwen3-0.6B-GGUF");
        assert_eq!(repos["Qwen3-0.6B-Q4_K_M.gguf"], "unsloth/Qwen3-0.6B-GGUF");

        let moe = HuggingFaceModel::Qwen3_30bA3bInstruct2507.get_info();
        let repos: std::collections::HashMap<String, &str> = download_file_list(&moe)
            .into_iter()
            .map(|(r, f)| (f, r))
            .collect();
        assert_eq!(
            repos["Qwen3-30B-A3B-Instruct-2507-Q4_K_M.gguf"],
            "unsloth/Qwen3-30B-A3B-Instruct-2507-GGUF"
        );
        assert_eq!(
            repos["tokenizer.json"],
            "Qwen/Qwen3-30B-A3B-Instruct-2507"
        );
    }

    /// The load path must match where the files are actually saved.
    ///
    /// `Qwen3_0_6B` used to download into `data/models/Qwen/Qwen3-0.6B/` (built
    /// from `repository`) while loading from `data/models/unsloth/Qwen3-0.6B-GGUF/`
    /// (built from `mirror`), so an already-downloaded model could never load.
    /// The local directory is now `repository` and both sides use it.
    #[test]
    fn load_paths_match_the_download_directory() {
        for (m, dir) in [
            (HuggingFaceModel::Qwen3_0_6B, "Qwen/Qwen3-0.6B"),
            (HuggingFaceModel::Qwen3_1_7B, "Qwen/Qwen3-1.7B"),
            (HuggingFaceModel::Qwen3_4B, "Qwen/Qwen3-4B"),
            (HuggingFaceModel::Qwen3_8B, "Qwen/Qwen3-8B"),
            (HuggingFaceModel::Qwen3_14B, "Qwen/Qwen3-14B"),
            (HuggingFaceModel::Qwen3_32B, "Qwen/Qwen3-32B"),
            (
                HuggingFaceModel::Qwen3_30bA3bInstruct2507,
                "Qwen/Qwen3-30B-A3B-Instruct-2507",
            ),
        ] {
            let info = m.get_info();
            assert_eq!(info.local_directory(), dir, "{m:?} local directory");
            assert_eq!(
                info.local_directory_path(),
                format!("./data/models/{dir}"),
                "{m:?} local directory path"
            );
            // The exact path the user reported, pinned so it cannot drift again.
            assert_eq!(
                info.gguf_model_path().unwrap(),
                format!("./data/models/{dir}/{}", info.gguf_model_filename)
            );
            assert_eq!(
                info.tokenizer_path(),
                format!("./data/models/{dir}/tokenizer.json")
            );
        }
        assert_eq!(
            HuggingFaceModel::Qwen3_0_6B.get_info().gguf_model_path().unwrap(),
            "./data/models/Qwen/Qwen3-0.6B/Qwen3-0.6B-Q4_K_M.gguf"
        );
    }

    /// The reported directory must be the one the downloader writes into.
    ///
    /// The settings page puts this string in front of the user ("Model will be
    /// downloaded locally at ..."), so a second, drifting computation of the path
    /// would point them at a directory that never exists.
    #[test]
    fn local_directory_status_reports_the_download_directory() {
        for m in [
            HuggingFaceModel::Qwen3_0_6B,
            HuggingFaceModel::BgeSmallEnV1_5,
            HuggingFaceModel::Gemma4E2BIt,
        ] {
            let info = m.get_info();
            let (path, exists) = info.local_directory_status();
            assert_eq!(path, info.local_directory_path(), "{m:?}");
            // Whether the model is already downloaded depends on this machine, so
            // pin the comparison instead of a fixed answer.
            assert_eq!(
                exists,
                std::path::Path::new(&path).is_dir(),
                "{m:?} existence"
            );
        }
    }

    /// 每个装载器都必须按**本地路径**去找 `tokenizer.json`，而不是拿仓库名当路径。
    ///
    /// 这个错误真的发生过：llama / gemma / moondream / parler_tts 四个装载器都写成
    /// 了 `init_tokenizer(info.repository)`，于是这四个模型家族永远加载失败，用户看到
    /// 的是一句看不懂的 `TinyLlama/TinyLlama-1.1B-Chat-v1.0: 系统找不到指定的路径。`。
    ///
    /// 用一个**不存在的仓库名**调用就够把路径钉住：这四个装载器都是先读分词器，所以
    /// 它们必定在分词器这一步失败，不会碰到任何权重（这条测试不许依赖磁盘上有模型）。
    #[test]
    fn loaders_look_for_the_tokenizer_in_the_local_directory() {
        let info = HuggingFaceModelInfo {
            model_type: HuggingFaceModelType::Llama,
            repository: "__no_such_repository__/nope",
            mirror: "__no_such_repository__/nope",
            model_files: Vec::new(),
            model_index_file: "",
            tokenizer_filename: "tokenizer.json",
            dimenssions: 0,
            gguf_model_filename: "",
            tokenizer_repository: "",
        };
        let expected = format!("{HUGGING_FACE_MODEL_ROOT}__no_such_repository__/nope/tokenizer.json");

        let cases: [(&str, Result<()>); 4] = [
            ("llama", load_llama_model_files(&info).map(|_| ())),
            ("gemma", load_gemma_model_files(&info).map(|_| ())),
            (
                "moondream",
                load_moondream_model_files(&info).map(|_| ()),
            ),
            (
                "parler_tts",
                load_parler_tts_model_files(&info).map(|_| ()),
            ),
        ];
        for (name, r) in cases {
            let err = r.expect_err("a repository that does not exist must fail").to_string();
            assert!(
                err.contains(&expected),
                "{name} looked for the tokenizer somewhere else: {err}"
            );
        }
    }

    /// Every path the check pass looks at must live in the local directory.
    ///
    /// Only GGUF models are exercised here: `get_model_files` returns paths
    /// without touching the disk for them. A model with sharded weights would
    /// need its index file to already be downloaded, which a unit test must not
    /// depend on.
    #[test]
    fn all_local_paths_live_in_the_local_directory() {
        for m in [
            HuggingFaceModel::Qwen3_0_6B,
            HuggingFaceModel::Qwen3_8B,
            HuggingFaceModel::Qwen3_30bA3bInstruct2507,
        ] {
            let info = m.get_info();
            let prefix = format!("{}/", info.local_directory_path());
            let files = get_model_files(&info).unwrap();
            assert!(!files.is_empty(), "{m:?} listed no files to check");
            for p in files.iter() {
                assert!(
                    p.starts_with(&prefix),
                    "{m:?}: {p} is outside {prefix} (download and load would disagree)"
                );
            }
            // The tokenizer is fetched from another repo but saved locally,
            // so the check pass must look for it in the local directory too.
            assert!(
                files.contains(&info.tokenizer_path()),
                "{m:?}: {} is missing from {files:?}",
                info.tokenizer_path()
            );
        }
    }

    /// Non-GGUF models keep a single repository, so nothing changes for them.
    ///
    /// Uses real entries rather than hand-built structs: their `model_files`
    /// deliberately omit `model.safetensors` (weights come from
    /// `model_index_file` shards) and hand-building that is easy to get wrong.
    /// Phi-4-mini (V4Mini) is the same shape as Phi-3-mini, so it must be
    /// covered too — its weights are also two shards behind an index file.
    #[test]
    fn non_gguf_models_keep_a_single_repository() {
        for m in [
            HuggingFaceModel::Phi3Mini4kInstruct,
            HuggingFaceModel::Phi4MiniInstruct,
        ] {
            let info = m.get_info();
            assert_eq!(info.tokenizer_repository(), info.mirror);
            assert_eq!(info.local_directory(), info.repository);
            // For every non-GGUF entry these three coincide, so the local directory,
            // the download origin and the tokenizer origin are all the same repo.
            assert_eq!(info.local_directory(), info.mirror);
            assert_eq!(
                info.model_index_file, "model.safetensors.index.json",
                "{m:?} weights are sharded, so the index file has to be named"
            );
            let files = download_file_list(&info);
            for (repository, _) in files.iter() {
                assert_eq!(*repository, info.mirror);
            }
            assert!(
                files.iter().all(|(_, f)| f != "model.safetensors"),
                "{m:?} must not look for a single-file checkpoint: {files:?}"
            );
            assert!(
                files.iter().any(|(_, f)| f == "tokenizer.json"),
                "the tokenizer still has to be in the list: {files:?}"
            );
        }
    }

    /// Phi-4-mini（V4Mini）的提示词照官方 `chat_template` 拼：**没有**开头 `<s>`、
    /// 角色标记后**没有**换行，每条消息以 `<|end|>` 收尾，最后是 `<|assistant|>`。
    ///
    /// 它和 Phi-3-mini 的模板长得很像，但这两处差别不能照抄 —— 所以这里把两个模型的
    /// 期望值放在一起对照，谁被改成对方的样子都会红。
    #[test]
    fn phi4_mini_uses_its_own_chat_template() {
        let history = Some(vec![
            crate::ai::chat::Prompt {
                role: String::from("system"),
                content: String::from("你是客服"),
            },
            crate::ai::chat::Prompt {
                role: String::from("user"),
                content: String::from("你好"),
            },
        ]);
        // 走 `chat()` / `gen_text()` 的真实形状：`s` 为空、最新一条 user 在 history 末尾。
        assert_eq!(
            HuggingFaceModel::Phi4MiniInstruct
                .get_info()
                .convert_prompt("", history.clone(), false)
                .unwrap(),
            "<|system|>你是客服<|end|><|user|>你好<|end|><|assistant|>"
        );
        // Phi-4-mini 忽略思考开关：两种取值必须完全一样。
        assert_eq!(
            HuggingFaceModel::Phi4MiniInstruct
                .get_info()
                .convert_prompt("", history.clone(), true)
                .unwrap(),
            "<|system|>你是客服<|end|><|user|>你好<|end|><|assistant|>"
        );
        // Phi-3 那一支不受影响。注意它比官方模板多一个**空回合**：`s` 是空的
        // （最新那条 user 消息在 history 里），而这一支仍无条件补
        // `<|user|>\n<|end|>\n`。这是既有的小瑕疵（`convert_prompt` 里 Qwen3 那段
        // 注释也提到过），这里只把它钉住，Phi-4-mini 的分支不重复这个形状。
        assert_eq!(
            HuggingFaceModel::Phi3Mini4kInstruct
                .get_info()
                .convert_prompt("", history, false)
                .unwrap(),
            "<s><|system|>\n你是客服<|end|>\n<|user|>\n你好<|end|>\n\
             <|user|>\n<|end|>\n<|assistant|>"
        );
        // 没有历史、也没有单独 user 文本时也不能留下空回合。
        assert_eq!(
            HuggingFaceModel::Phi4MiniInstruct
                .get_info()
                .convert_prompt("", None, false)
                .unwrap(),
            "<|assistant|>"
        );
    }
}
