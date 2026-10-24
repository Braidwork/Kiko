use std::collections::HashMap;
use std::default::Default;
use std::net::SocketAddr;
use std::sync::{LazyLock, Mutex};
use std::vec::Vec;

use axum::Json;
use axum::body::Bytes;
use axum::extract::Query;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::huggingface::HuggingFaceModel;
use crate::ai::{asr, chat, embedding, huggingface, tts};
use crate::db;
use crate::db_executor;
use crate::result::{Error, Result};
use crate::robot::dto::RobotQuery;
use crate::web::server::{self, to_res};

/// 全局设置表 —— **跨机器人**，只有它和机器人注册表是全局的。
///
/// 名字保持 `settings`：全局的那几个键（[`SETTINGS_KEY`] / `db_init_time` /
/// `version`）原来就住在这里。
pub(crate) const TABLE: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("settings");
/// 每机器人设置表的后缀，表名是 `{robot_id}settings`。
///
/// 原来每机器人的设置以 `robot_id` 为键混在上面的全局表里，是仓库里唯一一张
/// GLOBAL 与 PER-ROBOT 混住的表；拆开以后机器人导出/删除都只碰自己那张。
pub(crate) const TABLE_SUFFIX: &str = "settings";
pub(crate) const SETTINGS_KEY: &str = "global-settings";

/// `db_init_time` 写库用的时间格式。
///
/// 抽成常量是为了能被单测覆盖：`parse_borrowed` 是**运行期**解析，格式串里多一个
/// `]` 之类的笔误编译期查不出来，只会在启动写初始化时间时 panic（曾经就是这样）。
const TIME_FORMAT: &str = "[year]-[month]-[day] [hour]:[minute]:[second]";

static SETTINGS_CACHE: LazyLock<Mutex<HashMap<String, Settings>>> =
    LazyLock::new(|| Mutex::new(HashMap::with_capacity(32)));

/// 后台预热本地模型的状态，按机器人存。
///
/// 保存设置**不再**在请求里现装模型：几十 GB 的权重能把接口卡几十秒，而且那是在
/// async 任务里做同步重活，会占住一个 tokio worker。改成 `spawn_blocking` 后台装，
/// 这里记状态给设置页轮询：`loading` 显示"正在后台加载模型"，`err` 把失败原因
/// （哪个文件不对、为什么）原样带给用户。
///
/// 键是 `robot_id`，和 [`SETTINGS_CACHE`] / `LOADED_MODELS` 一样，机器人删掉后没
/// 有人来清（那两张表也一样）；一条记录很小，不值得为它单独加清理钩子。
static MODEL_LOAD_STATUS: LazyLock<Mutex<HashMap<String, ModelLoadStatus>>> =
    LazyLock::new(|| Mutex::new(HashMap::with_capacity(8)));

#[derive(Clone, Default, Serialize)]
pub(crate) struct ModelLoadStatus {
    pub(crate) chat: ModelLoadProgress,
    pub(crate) embedding: ModelLoadProgress,
}

#[derive(Clone, Default, Serialize)]
pub(crate) struct ModelLoadProgress {
    pub(crate) loading: bool,
    /// 正在装（或最后装过）的模型名，给界面显示用。
    pub(crate) model: String,
    /// 上一次装载失败的原因；成功、或还没跑过时是空串。
    pub(crate) err: String,
}

/// 「重新加载模型」按钮的入参：装哪个机器人的哪一块。
#[derive(Deserialize)]
pub(crate) struct LoadModelQuery {
    #[serde(rename = "robotId")]
    pub(crate) robot_id: String,
    /// `chat` 或 `embedding`，见 [`ModelKind::from_request`]。
    pub(crate) kind: String,
}

/// 后台要装的是哪一块的模型。
#[derive(Clone, Copy)]
enum ModelKind {
    Chat,
    Embedding,
}

impl ModelKind {
    /// 请求里写的 `kind`。**不认识的取值一律拒绝**：写错了就当 chat 处理，会
    /// 把另一个模型装进缓存，运行中的对话会被换成错的模型。
    fn from_request(kind: &str) -> Option<ModelKind> {
        match kind {
            "chat" => Some(ModelKind::Chat),
            "embedding" => Some(ModelKind::Embedding),
            _ => None,
        }
    }
}

/// 改一格装载状态。拿不到锁（中毒）就什么都不做：这只是给界面看的状态，
/// 不值得让保存/查询跟着失败。
fn update_load_status(robot_id: &str, kind: ModelKind, f: impl FnOnce(&mut ModelLoadProgress)) {
    let Ok(mut map) = MODEL_LOAD_STATUS.lock() else {
        return;
    };
    let status = map.entry(String::from(robot_id)).or_default();
    let slot = match kind {
        ModelKind::Chat => &mut status.chat,
        ModelKind::Embedding => &mut status.embedding,
    };
    f(slot);
}

/// 把"装载这个本地模型"丢到后台，并**立刻**把状态置成"加载中"。
///
/// 状态必须先写：保存接口一返回，设置页马上就会来轮询；晚一步它会先看到一次
/// "没在加载"，提示语就会闪一下才出现。
///
/// 为什么必须 `spawn_blocking`：装载是同步的纯 CPU/IO 重活（GGUF 要把整个文件读
/// 进内存再反量化），丢在 async 任务里会占住一个 tokio worker 几十秒。失败原因
/// 原样存下来给前端弹，而不是只写日志。
fn spawn_model_load(robot_id: &str, kind: ModelKind, model: HuggingFaceModel) {
    update_load_status(robot_id, kind, |p| {
        p.loading = true;
        p.model = model.to_string();
        // 上一次的失败原因必须先清掉：否则轮询会在"加载中"的时候还把旧错误挂着。
        p.err = String::new();
    });
    let robot_id = String::from(robot_id);
    tokio::task::spawn_blocking(move || {
        let r = match kind {
            ModelKind::Chat => chat::load_model_into_cache(&robot_id, &model),
            ModelKind::Embedding => embedding::load_model_into_cache(&robot_id, &model),
        };
        match r {
            Ok(_) => {
                log::info!("Loaded local model {model} into cache for robot {robot_id}");
                update_load_status(&robot_id, kind, |p| {
                    p.loading = false;
                    p.err = String::new();
                });
            }
            Err(e) => {
                let err = e.message();
                log::warn!("Loading local model {model} for robot {robot_id} failed. Err: {err}");
                update_load_status(&robot_id, kind, |p| {
                    p.loading = false;
                    p.err = err;
                });
            }
        }
    });
}

/// 这份设置里，这一块用的本地（HuggingFace）模型；在线模型是 `None`。
fn local_model_for(settings: &Settings, kind: ModelKind) -> Option<HuggingFaceModel> {
    match kind {
        ModelKind::Chat => match &settings.chat_provider.provider {
            chat::ChatProvider::HuggingFace(m) => Some(m.clone()),
            chat::ChatProvider::OpenAICompatible(_) => None,
        },
        ModelKind::Embedding => match &settings.sentence_embedding_provider.provider {
            embedding::SentenceEmbeddingProvider::HuggingFace(m) => Some(m.clone()),
            embedding::SentenceEmbeddingProvider::OpenAICompatible(_) => None,
        },
    }
}

/// 这次保存需要在后台重装哪些本地模型（`None` = 不用装）。
///
/// **只有模型真的换了才装**：改个 SMTP 超时、动一下会话时长，本来不该把几 GB 到
/// 几十 GB 的权重重新读一遍（原来每次保存都读）。判据是本地模型本身，所以
/// "在线模型 → 在线模型"、"本地模型原地不动"都不装；换成在线模型时也没得装。
fn models_to_reload(
    previous: Option<&Settings>,
    current: &Settings,
) -> (Option<HuggingFaceModel>, Option<HuggingFaceModel>) {
    let changed = |kind| {
        let previous = previous.and_then(|p| local_model_for(p, kind));
        let current = local_model_for(current, kind);
        match current {
            Some(current) if previous.as_ref() != Some(&current) => Some(current),
            _ => None,
        }
    };
    (changed(ModelKind::Chat), changed(ModelKind::Embedding))
}

/// 这个机器人这一块是不是正在装（`spawn_model_load` 已经把状态置成"加载中"）。
fn load_in_progress(robot_id: &str, kind: ModelKind) -> bool {
    MODEL_LOAD_STATUS
        .lock()
        .ok()
        .and_then(|m| m.get(robot_id).map(|s| progress_of(s, kind).loading))
        .unwrap_or(false)
}

/// 取状态里对应那一格的进度。
fn progress_of(status: &ModelLoadStatus, kind: ModelKind) -> &ModelLoadProgress {
    match kind {
        ModelKind::Chat => &status.chat,
        ModelKind::Embedding => &status.embedding,
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct HfModelDownload {
    #[serde(rename = "connectTimeoutMillis")]
    pub(crate) connect_timeout_millis: u32,
    #[serde(rename = "readTimeoutMillis")]
    pub(crate) read_timeout_millis: u32,
    #[serde(rename = "accessToken")]
    pub(crate) access_token: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct GlobalSettings {
    pub(crate) ip: String,
    pub(crate) port: u16,
    #[serde(rename = "selectRandomPortWhenConflict")]
    pub(crate) select_random_port_when_conflict: bool,
    #[serde(rename = "hfModelDownload")]
    pub(crate) hf_model_download: HfModelDownload,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Settings {
    settings_version: u8,
    #[serde(rename = "maxSessionIdleSec")]
    pub(crate) max_session_idle_sec: u32,
    #[serde(rename = "chatProvider")]
    pub(crate) chat_provider: ChatProvider,
    #[serde(rename = "sentenceEmbeddingProvider")]
    pub(crate) sentence_embedding_provider: SentenceEmbeddingProvider,
    #[serde(rename = "asrProvider")]
    pub(crate) asr_provider: AsrProvider,
    #[serde(rename = "ttsProvider")]
    pub(crate) tts_provider: TtsProvider,
    #[serde(rename = "smtpHost")]
    pub(crate) smtp_host: String,
    #[serde(rename = "smtpUsername")]
    pub(crate) smtp_username: String,
    #[serde(rename = "smtpPassword")]
    pub(crate) smtp_password: String,
    #[serde(rename = "smtpTimeoutSec")]
    pub(crate) smtp_timeout_sec: u16,
    #[serde(rename = "emailVerificationRegex")]
    pub(crate) email_verification_regex: String,
}

// #[test]
// fn deser() {
//     let s = Settings {
//         ip: String::new(),
//         port: 12715,
//         max_session_duration_min: 60,
//         embedding_provider: EmbeddingProvider {
//             provider: crate::intent::embedding::EmbeddingProvider::HuggingFace(
//                 crate::intent::embedding::HuggingFaceModel::AllMiniLML6V2,
//             ),
//             api_url: String::new(),
//             api_key: String::new(),
//             model: String::new(),
//             connect_timeout_millis: 1000,
//             read_timeout_millis: 5000,
//         },
//         smtp_host: String::new(),
//         smtp_username: String::new(),
//         smtp_password: String::new(),
//         smtp_timeout_sec: 30,
//         email_verification_regex: String::new(),
//         select_random_port_when_conflict: false,
//     };
//     let j = serde_json::to_string(&s);
//     assert!(j.is_ok());
//     println!("{}", j.unwrap());
//     let j = "{\"ip\":\"127.0.0.1\",\"port\":12715,\"selectRandomPortWhenConflict\":false,\"maxSessionDurationMin\":30,\"smtpHost\":\"\",\"smtpUsername\":\"\",\"smtpPassword\":\"\",\"smtpTimeoutSec\":60,\"emailVerificationRegex\":\"[-\\w\\.\\+]{1,100}@[A-Za-z0-9]{1,30}[A-Za-z\\.]{2,30}\",\"embeddingProvider\":{\"provider\":\"HuggingFace\",\"apiUrl\":\"Model will be downloaded locally at ./data/models\",\"apiKey\":\"\",\"model\":\"AllMiniLML6V2\",\"apiUrlDisabled\":true,\"showApiKeyInput\":false}}";
//     let r = serde_json::from_str(j);
//     assert!(r.is_ok());
//     let v: serde_json::Value = r.unwrap();
//     assert_eq!(v["embeddingProvider"]["provider"], "HuggingFace");
// }

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ChatProvider {
    pub(crate) provider: chat::ChatProvider,
    #[serde(rename = "apiUrl")]
    pub(crate) api_url: String,
    #[serde(rename = "apiKey")]
    pub(crate) api_key: String,
    pub(crate) model: String,
    #[serde(rename = "connectTimeoutMillis")]
    pub(crate) connect_timeout_millis: u32,
    #[serde(rename = "readTimeoutMillis")]
    pub(crate) read_timeout_millis: u32,
    #[serde(rename = "maxResponseTokenLength")]
    pub(crate) max_response_token_length: u32,
    #[serde(rename = "proxyUrl")]
    pub(crate) proxy_url: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct SentenceEmbeddingProvider {
    pub(crate) provider: embedding::SentenceEmbeddingProvider,
    #[serde(rename = "similarityThreshold")]
    pub(crate) similarity_threshold: f32,
    #[serde(rename = "apiUrl")]
    pub(crate) api_url: String,
    #[serde(rename = "apiKey")]
    pub(crate) api_key: String,
    pub(crate) model: String,
    #[serde(rename = "connectTimeoutMillis")]
    pub(crate) connect_timeout_millis: u32,
    #[serde(rename = "readTimeoutMillis")]
    pub(crate) read_timeout_millis: u32,
    #[serde(rename = "proxyUrl")]
    pub(crate) proxy_url: String,
    /// 期望的向量维度，`None` = 由模型自己决定（绝大多数情况）。
    ///
    /// 这是**当前 provider 生效的那一份**，后端只用它（见 [`crate::ai::embedding`]）。
    /// 它和 [`Self::dimensions_by_provider`] 的关系：后者是给设置页在"本地/在线"之间
    /// 来回切换时记住各自那一份用的，设置页保存前会把当前 provider 的值同步到这里。
    ///
    /// 只有实现了 Matryoshka 截断的模型才认这个参数，而且字段名各家不同：
    /// OpenAI / DashScope 兼容模式 / 硅基流动用 `dimensions`，Cohere / Voyage /
    /// Mistral 用 `output_dimension`，Gemini 用 `output_dimensionality`。我们对
    /// 外只发 `dimensions`（见 [`crate::ai::embedding`] 里那段注释），所以这里填的
    /// 值在别的厂商那里可能被无视——请求发出去之后会**核对返回向量的长度**，
    /// 没生效就直接报错，而不是把不一致的向量写进库。
    ///
    /// 保留 `dimensions` 这个名字（而不是只留下面那张表）：它是**后端唯一读的那个
    /// 字段**，接口形状和"缺省即自动"这个语义都不变，老记录也照旧能读。
    ///
    /// `#[serde(default)]` 是必需的：老记录里没有这个键，没有它整个设置都读不回来。
    #[serde(default)]
    pub(crate) dimensions: Option<u32>,
    /// 每个 provider（本地 / 在线）各自那份维度，`{"HuggingFace": 8192, ...}`。
    ///
    /// 为什么不能只留一个 `dimensions`：本地模型和在线模型是两类互不相干的模型，
    /// 维度跟着模型走。只存一份的话，"在本地填 8192、保存、切到在线"会把 8192
    /// 当成在线那一份，用户看到的是自己从没填过的值。
    ///
    /// 服务端不解析它，只是存下来再原样返回给设置页。
    ///
    /// `allow(dead_code)`：Rust 这边确实一个字段都不读（读写都靠 serde），但它绝不
    /// 是死代码——删掉它，设置页每打开一次，"本地 8192 / 在线 16"就只剩当前生效的
    /// 那一份，用户在本地填的值会跑到在线那一栏去。
    #[allow(dead_code)]
    #[serde(rename = "dimensionsByProvider", default)]
    pub(crate) dimensions_by_provider: Option<serde_json::Value>,
    /// **已经写进向量表的那份索引**用的是哪个模型/维度。
    ///
    /// 它和当前配置是两件事：换掉模型之后，库里旧向量还在，要等重新索引（或重建）
    /// 才会变。设置页拿它和当前配置比，不一致就警告（见 `Settings.vue` 的
    /// `embeddingIndexWarning`）：换了模型的话向量空间整个不同，检索**不会报错**、
    /// 只会全是噪声；只换了维度的话 `vector_distance_cos` 会让整条检索直接失败。
    #[serde(rename = "indexedEmbedding", default)]
    pub(crate) indexed_embedding: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct AsrProvider {
    pub(crate) provider: asr::AsrProvider,
    #[serde(rename = "apiUrl")]
    pub(crate) api_url: String,
    #[serde(rename = "apiKey")]
    pub(crate) api_key: String,
    pub(crate) model: String,
    #[serde(rename = "connectTimeoutMillis")]
    pub(crate) connect_timeout_millis: u32,
    #[serde(rename = "readTimeoutMillis")]
    pub(crate) read_timeout_millis: u32,
    #[serde(rename = "proxyUrl")]
    pub(crate) proxy_url: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct TtsProvider {
    pub(crate) provider: tts::TtsProvider,
    #[serde(rename = "apiUrl")]
    pub(crate) api_url: String,
    #[serde(rename = "apiKey")]
    pub(crate) api_key: String,
    pub(crate) model: String,
    #[serde(rename = "connectTimeoutMillis")]
    pub(crate) connect_timeout_millis: u32,
    #[serde(rename = "readTimeoutMillis")]
    pub(crate) read_timeout_millis: u32,
    #[serde(rename = "proxyUrl")]
    pub(crate) proxy_url: String,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        GlobalSettings {
            ip: String::from("127.0.0.1"),
            port: 12715,
            select_random_port_when_conflict: false,
            hf_model_download: HfModelDownload {
                connect_timeout_millis: 2000,
                read_timeout_millis: 10000,
                access_token: String::new(),
            },
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            settings_version: 1u8,
            max_session_idle_sec: 1800,
            chat_provider: ChatProvider {
                provider: chat::ChatProvider::HuggingFace(
                    huggingface::HuggingFaceModel::TinyLlama1_1bChatV1_0,
                ),
                api_url: String::new(),
                api_key: String::new(),
                model: String::new(),
                connect_timeout_millis: 5000,
                read_timeout_millis: 10000,
                max_response_token_length: 200,
                proxy_url: String::new(),
            },
            sentence_embedding_provider: SentenceEmbeddingProvider {
                provider: embedding::SentenceEmbeddingProvider::HuggingFace(
                    huggingface::HuggingFaceModel::AllMiniLML6V2,
                ),
                similarity_threshold: 0.85f32,
                api_url: String::new(),
                api_key: String::new(),
                model: String::new(),
                dimensions: None,
                dimensions_by_provider: None,
                indexed_embedding: None,
                connect_timeout_millis: 5000,
                read_timeout_millis: 10000,
                proxy_url: String::new(),
            },
            asr_provider: AsrProvider {
                provider: asr::AsrProvider::HuggingFace(
                    huggingface::HuggingFaceModel::WhisperLargeV3,
                ),
                api_url: String::new(),
                api_key: String::new(),
                model: String::new(),
                connect_timeout_millis: 5000,
                read_timeout_millis: 10000,
                proxy_url: String::new(),
            },
            tts_provider: TtsProvider {
                provider: tts::TtsProvider::HuggingFace(
                    huggingface::HuggingFaceModel::ParlerTtsMiniV1,
                ),
                api_url: String::new(),
                api_key: String::new(),
                model: String::new(),
                connect_timeout_millis: 5000,
                read_timeout_millis: 10000,
                proxy_url: String::new(),
            },
            smtp_host: String::new(),
            smtp_username: String::new(),
            smtp_password: String::new(),
            smtp_timeout_sec: 60u16,
            email_verification_regex: String::new(),
        }
    }
}

pub(crate) async fn init_table(store: &db::RedbStore) -> Result<()> {
    db::init_table(store, TABLE).await
}

pub(crate) async fn exists(store: &db::RedbStore) -> Result<bool> {
    let cnt = db::count(store, TABLE).await?;
    Ok(cnt > 0)
}

pub(crate) async fn init_global(store: &db::RedbStore) -> Result<GlobalSettings> {
    let settings = GlobalSettings::default();
    db::write(store, TABLE, SETTINGS_KEY, &settings).await?;
    let format = time::format_description::parse_borrowed::<3>(TIME_FORMAT)
        .expect("Invalid format description");
    let t = time::OffsetDateTime::now_utc();
    let t_str = t
        .format(&format)
        .map_err(|e| Error::TimeFormat(Box::new(e)))?;
    db::write(store, TABLE, "db_init_time", &t_str).await?;
    db::write(store, TABLE, "version", &String::from(server::VERSION)).await?;
    Ok(settings)
}

/// 新机器人的默认设置，写在**它自己**那张表里。
pub(crate) async fn init(robot_id: &str) -> Result<Settings> {
    let settings = Settings::default();
    db_executor!(db::write, robot_id, TABLE_SUFFIX, robot_id, &settings)?;
    Ok(settings)
}

pub(crate) async fn get_global_settings(store: &db::RedbStore) -> Result<Option<GlobalSettings>> {
    db::query(store, TABLE, SETTINGS_KEY).await
}

pub(crate) async fn rest_get_global_settings() -> impl IntoResponse {
    let r = match db::store(db::StoreKey::Global).await {
        Ok(store) => get_global_settings(store).await,
        Err(e) => Err(e),
    };
    to_res(r)
}

pub(crate) async fn get_settings(robot_id: &str) -> Result<Option<Settings>> {
    // 缓存锁必须在 await 之前放开：std 的 MutexGuard 不是 Send。
    {
        let l = SETTINGS_CACHE.lock()?;
        if let Some(s) = l.get(robot_id) {
            return Ok(Some(s.clone()));
        }
    }
    let r: Option<Settings> = db_executor!(db::query, robot_id, TABLE_SUFFIX, robot_id)?;
    Ok(r.map(unify_legacy_api_urls))
}

/// 老记录里指向 Ollama **原生**端点的地址。
///
/// 原生路径这次并进了统一的 OpenAI 兼容路径，所以这些地址要挪到同一个 host 上的
/// 兼容端点：不挪的话，老记录会把 OpenAI 形状的请求发到原生端点，回来的是 NDJSON
/// 或 `{"embedding":[...]}`，表现为"应答是空的"，比报错更难查。
///
/// 只在读写设置时归一化（[`get_settings`] / [`save_settings`]），所以设置页里显示
/// 的就是真正会被使用的地址，用户下一次保存就落库。等所有实例都保存过一次之后，
/// 这个函数可以连同两个 serde 别名一起删掉。
fn unify_legacy_api_url(u: &str) -> String {
    let t = u.trim().trim_end_matches('/');
    let rewritten = ["/api/chat", "/api/embed", "/api/embeddings"]
        .iter()
        .find_map(|suffix| t.strip_suffix(suffix).map(|base| (suffix, base)));
    match rewritten {
        Some((suffix, base)) if *suffix == "/api/chat" => format!("{base}/v1/chat/completions"),
        Some((_, base)) => format!("{base}/v1/embeddings"),
        None => String::from(u),
    }
}

fn unify_legacy_api_urls(mut s: Settings) -> Settings {
    s.chat_provider.api_url = unify_legacy_api_url(&s.chat_provider.api_url);
    s.sentence_embedding_provider.api_url =
        unify_legacy_api_url(&s.sentence_embedding_provider.api_url);
    s
}

/// 「这份向量是用哪个模型、多少维算出来的」的指纹。
///
/// 格式固定成 `v1|kind|model|dims`，前端按同样规则拼一份来比对（见
/// `Settings.vue` 的 `currentEmbeddingIdentity`），中间的 `|` 分隔是为了让前端
/// 能安全地 `split("|")` 取模型名和维度。
///
/// **必须包含模型名，不只是维度**：两个模型维度都是 1024 也照样不可比——它们把
/// 同一个句子映射到两个毫不相干的坐标系里，`vector_distance_cos` 算出来的余弦
/// 距离全是噪声（而且**不会报错**，比报错更难查）。维度则是因为同一模型可以按
/// Matryoshka 截断成不同长度，而 turso 的 `vector_distance_cos` 遇到维度不一致
/// 会直接返回错误，整条检索整体失败。
pub(crate) fn embedding_identity(
    provider: &embedding::SentenceEmbeddingProvider,
    model: &str,
    dims: usize,
) -> String {
    let kind = match provider {
        embedding::SentenceEmbeddingProvider::HuggingFace(_) => "huggingface",
        embedding::SentenceEmbeddingProvider::OpenAICompatible(_) => "openai-compatible",
    };
    format!("v1|{kind}|{model}|{dims}")
}

/// 这份设置实际用的模型名。
///
/// **本地模型的真名在枚举里**（如 `BgeM3`）：设置里那个 `model` 字段对本地模型
/// 没有意义（前端显示的是枚举名，用户改不了）。在线模型则只能取
/// `OpenAICompatible(名字)` 里那个名字 —— 这是真正发给端点的字符串，而不是外层
/// 那个可能没人维护的 `model` 字段。
pub(crate) fn embedding_model_name(provider: &embedding::SentenceEmbeddingProvider) -> String {
    match provider {
        embedding::SentenceEmbeddingProvider::HuggingFace(m) => m.to_string(),
        embedding::SentenceEmbeddingProvider::OpenAICompatible(m) => m.clone(),
    }
}

impl Settings {
    /// 当前配置对应的索引指纹。维度用配置里写的那个（没写就是"还不知道"，
    /// 用 0 占位），所以它和「实际写进库的指纹」只在用户设了 dimensions 时
    /// 才严格可比。设置页自己按同一规则拼一份来比对（见 `Settings.vue` 的
    /// `currentEmbeddingIdentity`）。
    pub(crate) fn current_embedding_identity(&self) -> String {
        let p = &self.sentence_embedding_provider;
        embedding_identity(
            &p.provider,
            &embedding_model_name(&p.provider),
            p.dimensions.unwrap_or(0) as usize,
        )
    }
}

/// 把「写进库的那份向量是哪来的」记下来。
///
/// **必须在向量真正落库之后调用**（见 `kb::qa::save` / `kb::doc` / `intent::phrase`），
/// 而且只在写入路径上调：查询路径也会算向量，但查询不改变库里有什么。
///
/// 指纹描述的是**库里的东西**，不是用户当前的配置，所以换模型本身不会改它——
/// 这正是设置页能发现"换了模型但还没重新索引"的原因。
///
/// 返回 `Result` 只是为了把"设置读不出来/写不回去"的细节留给调用方决定：打标失败
/// 最多是下次少一条警告，而向量已经写成功了，所以调用方通常只记一笔日志，不把
/// 整个请求判失败。
pub(crate) async fn stamp_embedding_index(robot_id: &str) -> Result<()> {
    let Some(mut s) = get_settings(robot_id).await? else {
        return Ok(());
    };
    s.sentence_embedding_provider.indexed_embedding = Some(s.current_embedding_identity());
    db_executor!(db::write, robot_id, TABLE_SUFFIX, robot_id, &s)?;
    let mut l = SETTINGS_CACHE.lock()?;
    l.insert(String::from(robot_id), s);
    drop(l);
    Ok(())
}

pub(crate) async fn get(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    to_res::<Option<Settings>>(get_settings(&q.robot_id).await)
}

pub(crate) async fn save(
    Query(q): Query<RobotQuery>,
    Json(data): Json<Settings>,
) -> impl IntoResponse {
    to_res(save_settings(&q.robot_id, data).await)
}

pub(crate) async fn save_global_settings(
    store: &db::RedbStore,
    data: &GlobalSettings,
) -> Result<()> {
    let addr = format!("{}:{}", data.ip, data.port);
    let _: SocketAddr = addr.parse().map_err(|_| {
        log::error!("Saving invalid listen IP: {}", &addr);
        Error::WithMessage(String::from("lang.settings.invalidIp"))
    })?;
    db::write(store, TABLE, SETTINGS_KEY, data).await
}

pub(crate) async fn rest_save_global_settings(
    Json(data): Json<GlobalSettings>,
) -> impl IntoResponse {
    let r = match db::store(db::StoreKey::Global).await {
        Ok(store) => save_global_settings(store, &data).await,
        Err(e) => Err(e),
    };
    to_res(r)
}

pub(crate) async fn save_settings(robot_id: &str, data: Settings) -> Result<()> {
    // 保存时也归一化，这样老地址会被真正写掉（读时归一化只保证运行期正确，
    // 存储里的旧值要等一次保存才能收敛）。
    let data = unify_legacy_api_urls(data);
    // 和**存库里的那份**比：缓存里装的正是它对应的模型，所以"这份设置和上一份
    // 是不是同一个本地模型"就是"缓存里的东西还能不能接着用"。
    //
    // 读不回来（Err）就当"不知道之前是什么"，按换了处理：宁可在后台多装一次，
    // 也不要因为一次读失败把整个保存拒掉——真要是库坏了，下面的写会自己报错。
    let previous = get_settings(robot_id).await.ok().flatten();
    let (reload_chat, reload_embedding) = models_to_reload(previous.as_ref(), &data);

    db_executor!(db::write, robot_id, TABLE_SUFFIX, robot_id, &data)?;
    let mut l = SETTINGS_CACHE.lock()?;
    l.insert(String::from(robot_id), data);
    drop(l);

    // 装模型放在写完库之后：写失败就不该去装。装的过程在后台跑，接口立刻返回，
    // 进度和失败原因由 [`model_load_progress`] 给设置页（见 [`spawn_model_load`]）。
    //
    // 这里原来是在请求里同步装，而且**每次保存都装**（同一个模型再存一次也一样）：
    // 改个 SMTP 超时也要把整个模型从磁盘读一遍、重新建 tensor。
    if let Some(m) = reload_chat {
        spawn_model_load(robot_id, ModelKind::Chat, m);
    }
    if let Some(m) = reload_embedding {
        spawn_model_load(robot_id, ModelKind::Embedding, m);
    }
    Ok(())
}

/// 后台预热本地模型的进度/结果，给设置页轮询。
///
/// 没有记录的机器人返回一份默认值（都没在加载、也没有错误），前端不必区分
/// "没跑过"和"跑完了"。
pub(crate) async fn model_load_progress(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    let status = MODEL_LOAD_STATUS
        .lock()
        .ok()
        .and_then(|m| m.get(&q.robot_id).cloned())
        .unwrap_or_default();
    to_res(Ok(status))
}

/// 「重新加载模型」按钮：手动把本地模型重新装进内存。
///
/// 用在"文件坏了 → 补好 → 不想等下一次对话、也不想重启进程"这条路上。装载动作和
/// 保存时的预热完全一样（后台 `spawn_blocking` + [`model_load_progress`] 可查），
/// 所以前端复用同一套进度轮询。
///
/// 装哪个模型**以存库的设置为准**，不接受前端传模型名。缓存是按 `robot_id` 存的，
/// 而运行中的对话读的也是存库的那份设置：允许"装一个还没保存的选择"会把正在用的
/// 模型换成另一个，错得很难查。
pub(crate) async fn load_model_now(Query(q): Query<LoadModelQuery>) -> impl IntoResponse {
    let Some(kind) = ModelKind::from_request(&q.kind) else {
        return to_res(Err(Error::WithMessage(format!(
            "Unknown model kind {:?}, expected \"chat\" or \"embedding\".",
            &q.kind
        ))));
    };
    let settings = match get_settings(&q.robot_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return to_res(Err(Error::WithMessage(String::from(
                "Can NOT find settings of this robot.",
            ))));
        }
        Err(e) => return to_res(Err(e)),
    };
    let Some(model) = local_model_for(&settings, kind) else {
        return to_res(Err(Error::WithMessage(String::from(
            "This robot does not use a local HuggingFace model here.",
        ))));
    };
    // 已经在装了就直接返回：重复点（或者双击）不该同时装两遍，那既费内存也费 IO。
    // 状态里已经有"加载中"，前端会继续显示进度，所以这里静默成功是对的。
    if !load_in_progress(&q.robot_id, kind) {
        spawn_model_load(&q.robot_id, kind, model);
    }
    to_res(Ok(()))
}

pub(crate) async fn smtp_test(Json(settings): Json<Settings>) -> impl IntoResponse {
    to_res(check_smtp_settings(&settings))
}

pub(crate) fn check_smtp_settings(settings: &Settings) -> Result<bool> {
    use lettre::SmtpTransport;
    use lettre::transport::smtp::authentication::Credentials;
    let creds = Credentials::new(
        settings.smtp_username.to_owned(),
        settings.smtp_password.to_owned(),
    );

    let mailer = SmtpTransport::relay(&settings.smtp_host)?
        .credentials(creds)
        .timeout(Some(core::time::Duration::from_secs(
            settings.smtp_timeout_sec as u64,
        )))
        .build();

    Ok(mailer.test_connection()?)
}

pub(crate) async fn download_model_files(Json(m): Json<HuggingFaceModel>) -> impl IntoResponse {
    let store = match db::store(db::StoreKey::Global).await {
        Ok(store) => store,
        Err(e) => return to_res(Err(e)),
    };
    let global_settings = get_global_settings(store).await;
    if global_settings.is_err() {
        return to_res(Err(Error::WithMessage(String::from(
            "Load global settings failed.",
        ))));
    }
    let global_settings = global_settings.unwrap();
    if global_settings.is_none() {
        return to_res(Err(Error::WithMessage(String::from(
            "Global settings not found.",
        ))));
    }
    let global_settings = global_settings.unwrap();
    tokio::spawn(async move {
        match huggingface::download_hf_models(
            &m.get_info(),
            &global_settings.hf_model_download.access_token,
            global_settings.hf_model_download.connect_timeout_millis as u64,
            global_settings.hf_model_download.read_timeout_millis as u64,
        )
        .await
        {
            Ok(_) => log::info!("All model files download successfully."),
            Err(e) => log::error!("Model file downloaded failed, err: {:?}", &e),
        }
        // if let Some(s) = huggingface::DOWNLOAD_STATUS.get() {
        //     if let Ok(mut v) = s.lock() {
        //         v.downloading = false;
        //     }
        // }
    });
    to_res(Ok(()))
}

pub(crate) async fn download_model_progress() -> impl IntoResponse {
    let r = huggingface::get_download_status();
    to_res(Ok(r))
}

/// 一个模型文件级校验的结果。
///
/// `err` 不是可选的装饰：只回一个 bool，用户看到的就只是"坏了"，既不知道哪个
/// 文件不对、也不知道该删什么重下。校验失败的原因（`Path "..." is not exist.`、
/// `... is not a GGUF file` 之类）要原样带回前端弹出来。
#[derive(Serialize)]
pub(crate) struct ModelFileCheck {
    pub(crate) ok: bool,
    /// 校验通过时为空串。
    pub(crate) err: String,
}

/// 「检测已存向量维度」的响应。
///
/// 为什么要单独看"库里已经存了什么"：设置页那一栏 `dimensions` 说的是"**以后**按
/// 几维算"，和库里已有的向量是两件事——改过设置又还没重新索引时，两者不一致，
/// 而 turso 的 `vector_distance_cos` 遇到维度不一致会让**整条检索报错**。所以用户
/// 需要一个"不看设置、只看数据"的答案。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VectorDimensions {
    /// 当前配置会算出来的维度（设置里填了就是它，没填就是 `null` = 由模型决定）。
    /// 只用于界面提示，不是事实来源。
    pub(crate) configured: Option<u32>,
    /// 库里实际存着的维度（每处向量列一条）。`exists = false` 表示"还没写过向量"。
    pub(crate) stored: Vec<crate::man::vector_report::VectorColumn>,
}

/// 只读地报告"库里已经存了的向量是多少维"。
///
/// 命令行版本见 `man::vector_report` 的 `report_vector_dimensions` 测试；这条接口是
/// 给设置页那个按钮用的。它**不写任何东西**，所以可以随便点。
pub(crate) async fn vector_dimensions(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    let configured = get_settings(&q.robot_id)
        .await
        .ok()
        .flatten()
        .and_then(|s| s.sentence_embedding_provider.dimensions);
    let stored = crate::man::vector_report::robot_vector_report(&q.robot_id).await;
    to_res(Ok(VectorDimensions { configured, stored }))
}

/// 批量校验本地模型文件：请求体是模型名数组，响应是 `模型名 -> {ok, err}`。
///
/// 这是**文件级**校验：逐个 stat、解析 `config.json` / `tokenizer.json`、核对
/// safetensors 体积和 GGUF 魔数。比 [`local_model_paths`] 只 stat 目录贵得多
/// （tokenizer.json 有几兆到几十兆），所以设置页把它放在用户主动点的
/// 「检测模型」按钮后面，而不是每次打开页面都跑一遍。
pub(crate) async fn check_model_files(bytes: Bytes) -> impl IntoResponse {
    match serde_json::from_slice::<Vec<HuggingFaceModel>>(bytes.as_ref()) {
        Ok(models) => {
            let mut map: HashMap<String, ModelFileCheck> = HashMap::new();
            for model in models.iter() {
                let info = model.get_info();
                let check = match huggingface::check_model_files(&info) {
                    Ok(_) => ModelFileCheck {
                        ok: true,
                        err: String::new(),
                    },
                    Err(e) => {
                        log::warn!(
                            "Hugging face model {} files incorrect. Err: {:?}",
                            info.repository,
                            &e
                        );
                        ModelFileCheck {
                            ok: false,
                            err: e.message(),
                        }
                    }
                };
                map.insert(model.to_string(), check);
            }
            to_res(Ok(map))
        }
        Err(e) => to_res(Err(Error::WithMessage(format!(
            "Invalid request body, err {:?}",
            &e
        )))),
    }
}

/// 本地 HuggingFace 模型的落盘位置，以及那个目录是否已经存在。
///
/// 设置页把它拼在"Model will be downloaded locally at ..."后面显示给用户。以前
/// 这句话整个是前端硬编码的（`./data/models`），和真正写文件的根目录
/// （`HUGGING_FACE_MODEL_ROOT`）对不上，用户照着提示去目录里找是找不到文件的；
/// 路径改由后端给出，两边就不可能再漂移。
///
/// `path` 只是"要放到哪个目录"，不含模型文件名。目录本身也算信息：设置页据此
/// 决定要不要提示"模型缺失/需要下载"。
#[derive(Serialize)]
pub(crate) struct LocalModelPath {
    pub(crate) path: String,
    pub(crate) exists: bool,
}

/// 批量查本地模型目录：请求体是模型名数组，响应是 `模型名 -> {path, exists}`。
///
/// 只 `stat` **目录**，不逐个检查模型文件——`check/files` 那套要读并解析
/// `tokenizer.json`（几兆到几十兆，bge-m3 这类模型更大），进一次设置页就做一遍
/// 并不划算，而"目录在不在"已经足够回答"要不要提示用户下载"。文件级有效性校验
/// 留给用户主动触发的按钮（接口仍是 [`check_model_files`]）。
///
/// 形状刻意和 [`check_model_files`] 对齐（同样是模型名数组进、以模型名为键的
/// map 出），这样前端两处调用可以共用同一个收集模型名的流程。
pub(crate) async fn local_model_paths(bytes: Bytes) -> impl IntoResponse {
    match serde_json::from_slice::<Vec<HuggingFaceModel>>(bytes.as_ref()) {
        Ok(models) => {
            // 用 std 的 `HashMap` 而不是 `serde_json::Map`：后者只在 value 是
            // `serde_json::Value` 时才可序列化，而这里的 value 是上面那个结构体。
            // 两者的 JSON 形状一样（`{模型名: {...}}`），前端按键取值。
            let mut map: HashMap<String, LocalModelPath> = HashMap::new();
            for model in models.iter() {
                let (path, exists) = model.get_info().local_directory_status();
                map.insert(model.to_string(), LocalModelPath { path, exists });
            }
            to_res(Ok(map))
        }
        Err(e) => to_res(Err(Error::WithMessage(format!(
            "Invalid request body, err {:?}",
            &e
        )))),
    }
}

pub(crate) async fn check_embedding_model(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    let r = if let Ok(r) = get_settings(&q.robot_id).await {
        if let Some(settings) = r {
            match settings.sentence_embedding_provider.provider {
                embedding::SentenceEmbeddingProvider::HuggingFace(m) => {
                    let info = m.get_info();
                    huggingface::check_model_files(&info)
                }
                embedding::SentenceEmbeddingProvider::OpenAICompatible(_) => {
                    if settings.sentence_embedding_provider.api_url.is_empty() {
                        Err(Error::WithMessage(String::from(
                            "lang.settings.apiUrlEmpty",
                        )))
                    } else if settings.sentence_embedding_provider.api_key.is_empty() {
                        Err(Error::WithMessage(String::from("OPENAI_API_KEY is empty.")))
                    } else {
                        Ok(())
                    }
                }
            }
        } else {
            Err(Error::WithMessage(String::from(
                "Can NOT find settings of this robot.",
            )))
        }
    } else {
        Err(Error::WithMessage(String::from(
            "Failed to get settings of this robot.",
        )))
    };
    to_res(r)
}

/// 列出 OpenAI 兼容端点的模型，供前端"获取模型列表"按钮使用。
///
/// 参数走 query 而不是 body：模型名现在由用户手输，拼错只会得到一个看不出
/// 原因的 404，这个接口就是为了消掉那个首次使用的障碍。
pub(crate) async fn list_openai_models(
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(url) = q.get("url") else {
        return to_res(Err(Error::WithMessage(String::from(
            "API URL parameter not found",
        ))));
    };
    if url.trim().is_empty() {
        return to_res(Err(Error::WithMessage(String::from(
            "API URL parameter is empty",
        ))));
    }
    let api_key = q.get("apiKey").map(String::as_str).unwrap_or_default();
    let proxy_url = q.get("proxyUrl").map(String::as_str).unwrap_or_default();
    let connect_timeout_millis = q
        .get("connectTimeoutMillis")
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000u64);
    let read_timeout_millis = q
        .get("readTimeoutMillis")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000u64);
    to_res(
        retrieve_openai_models(
            url,
            api_key,
            connect_timeout_millis,
            read_timeout_millis,
            proxy_url,
        )
        .await,
    )
}

/// 模型列表就在用户配置的那个端点隔壁：`.../v1/chat/completions` 和
/// `.../v1/embeddings` 都在 `.../v1/models` 下。Ollama 的兼容端点形状相同，
/// 所以原生 `/api/tags` 那套 `rfind('/')` 派生方式在这里不适用。
///
/// 例外是 Ollama 的原生对话端点 `/api/chat`：它的模型列表在 `/api/tags`，
/// 不带 `/v1`（形状也不同，`retrieve_openai_models` 两种都认）。
fn models_url(u: &str) -> String {
    let base = u.trim().trim_end_matches('/');
    if let Some(base) = base.strip_suffix("/api/chat") {
        return format!("{}/api/tags", base.trim_end_matches('/'));
    }
    let base = ["/chat/completions", "/completions", "/embeddings"]
        .iter()
        .find_map(|suffix| base.strip_suffix(suffix))
        .unwrap_or(base);
    format!("{}/models", base.trim_end_matches('/'))
}

async fn retrieve_openai_models(
    url: &str,
    api_key: &str,
    connect_timeout_millis: u64,
    read_timeout_millis: u64,
    proxy_url: &str,
) -> Result<Vec<String>> {
    // 走共享 client 而不是裸 reqwest::get，这样用户的代理和超时设置才生效。
    let client = crate::external::http::get_client(
        connect_timeout_millis,
        read_timeout_millis,
        proxy_url,
    )?;
    let mut req = client.get(models_url(url));
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {api_key}"));
    }
    let res = req.send().await?;
    let status = res.status();
    let bytes = res.bytes().await?;
    if !status.is_success() {
        return Err(Error::WithMessage(format!(
            "Model list endpoint returned {status}: {}",
            String::from_utf8_lossy(bytes.as_ref())
        )));
    }
    let json: Value = serde_json::from_slice(bytes.as_ref())?;
    let mut result: Vec<String> = Vec::with_capacity(16);
    // `{"data":[{"id":...}]}` 是 OpenAI 的形状；裸数组和 Ollama 原生的
    // `{"models":[{"id":...}]}` 顺带接受，代价极小。
    let items = json
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| json.get("models").and_then(Value::as_array))
        .or_else(|| json.as_array());
    if let Some(items) = items {
        for item in items {
            if let Some(id) = item
                .get("id")
                .or_else(|| item.get("model"))
                .and_then(Value::as_str)
            {
                result.push(String::from(id));
            }
        }
    }
    result.sort();
    result.dedup();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 格式串必须是**可解析**的：`init_global` 里那句 `.expect(...)` 会在启动时
    /// 直接 panic，所以这里把解析和格式化都跑一遍。
    #[test]
    fn db_init_time_format_is_parseable() {
        let format = time::format_description::parse_borrowed::<3>(TIME_FORMAT)
            .expect("TIME_FORMAT must be a valid time format description");
        let t = time::OffsetDateTime::now_utc();
        let s = t.format(&format).expect("formatting must succeed");
        assert_eq!(s.len(), "1970-01-01 00:00:00".len(), "unexpected: {s}");
        assert!(s.as_bytes()[4] == b'-' && s.as_bytes()[10] == b' ');
    }

    #[test]
    fn models_url_sits_next_to_the_configured_endpoint() {
        assert_eq!(
            models_url("https://api.deepseek.com/v1/chat/completions"),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            models_url("https://api.openai.com/v1/embeddings"),
            "https://api.openai.com/v1/models"
        );
        // Ollama's compatibility endpoint, which is why the native `/api/tags`
        // derivation cannot be reused here.
        assert_eq!(
            models_url("http://localhost:11434/v1/chat/completions"),
            "http://localhost:11434/v1/models"
        );
        // Ollama's native chat endpoint instead: the model list is `/api/tags`.
        assert_eq!(
            models_url("http://localhost:11434/api/chat"),
            "http://localhost:11434/api/tags"
        );
        assert_eq!(
            models_url("http://192.168.1.9:11434/api/chat/"),
            "http://192.168.1.9:11434/api/tags"
        );
        // Trailing slashes and stray whitespace are the common paste accidents.
        assert_eq!(
            models_url("  https://api.moonshot.cn/v1/chat/completions/  "),
            "https://api.moonshot.cn/v1/models"
        );
        // A bare host is a documented wrong guess: the field stays editable.
        assert_eq!(
            models_url("https://api.deepseek.com"),
            "https://api.deepseek.com/models"
        );
    }

    /// 老记录里指向 Ollama 原生端点的地址必须被挪到兼容端点，否则老机器人会在
    /// 运行期把 OpenAI 形状的请求打过去，表现为"应答是空的"。
    #[test]
    fn legacy_ollama_urls_are_moved_to_the_compatible_endpoints() {
        assert_eq!(
            unify_legacy_api_url("http://localhost:11434/api/chat"),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            unify_legacy_api_url("  http://192.168.1.9:11434/api/chat/  "),
            "http://192.168.1.9:11434/v1/chat/completions"
        );
        assert_eq!(
            unify_legacy_api_url("http://localhost:11434/api/embeddings"),
            "http://localhost:11434/v1/embeddings"
        );
        // `/api/embed` 是同一个原生端点的较新名字（请求体是 `input`）。
        assert_eq!(
            unify_legacy_api_url("http://localhost:11434/api/embed"),
            "http://localhost:11434/v1/embeddings"
        );

        // 已经是兼容端点、别的厂商、以及空地址都原样不动。
        for u in [
            "http://localhost:11434/v1/chat/completions",
            "https://api.deepseek.com/v1/chat/completions",
            "http://localhost:11434/v1/embeddings",
            "",
        ] {
            assert_eq!(unify_legacy_api_url(u), u, "{u} must be left alone");
        }
        // 只有**结尾**是那个路径才动；带查询串的不猜。
        assert_eq!(
            unify_legacy_api_url("http://localhost:11434/api/chat?x=1"),
            "http://localhost:11434/api/chat?x=1"
        );
    }

    /// 造一份只为"换模型"测试用的设置：`None` 表示这一块用在线模型。
    fn settings_with(
        chat: Option<HuggingFaceModel>,
        embedding: Option<HuggingFaceModel>,
    ) -> Settings {
        let mut s = Settings::default();
        s.chat_provider.provider = match chat {
            Some(m) => chat::ChatProvider::HuggingFace(m),
            None => chat::ChatProvider::OpenAICompatible(String::from("gpt-4o-mini")),
        };
        s.sentence_embedding_provider.provider = match embedding {
            Some(m) => embedding::SentenceEmbeddingProvider::HuggingFace(m),
            None => embedding::SentenceEmbeddingProvider::OpenAICompatible(String::from(
                "text-embedding-3-small",
            )),
        };
        s
    }

    /// 保存设置只重装**换了**的本地模型。
    ///
    /// 这条判据是"保存接口到底慢不慢"的全部依据：以前每次保存（哪怕只改一个
    /// SMTP 超时）都会把整个模型从磁盘读一遍。
    #[test]
    fn only_a_changed_local_model_is_reloaded() {
        use HuggingFaceModel::{BgeSmallEnV1_5, Qwen3_0_6B, Qwen3_8B};

        // 第一次保存（库里还没有东西）：本地模型都要装。
        let both_local = settings_with(Some(Qwen3_0_6B), Some(BgeSmallEnV1_5));
        assert_eq!(
            models_to_reload(None, &both_local),
            (Some(Qwen3_0_6B), Some(BgeSmallEnV1_5))
        );

        // 一模一样的设置再存一次：什么都不用装。
        assert_eq!(
            models_to_reload(Some(&both_local), &both_local),
            (None, None),
            "re-saving the same model must not re-read the weights"
        );

        // 只换了对话模型：只装对话那一个。
        let new_chat = settings_with(Some(Qwen3_8B), Some(BgeSmallEnV1_5));
        assert_eq!(
            models_to_reload(Some(&both_local), &new_chat),
            (Some(Qwen3_8B), None)
        );

        // 只换了句向量模型：只装句向量那一个。
        let new_embedding = settings_with(Some(Qwen3_0_6B), Some(Qwen3_8B));
        assert_eq!(
            models_to_reload(Some(&both_local), &new_embedding),
            (None, Some(Qwen3_8B))
        );

        // 本地模型换成在线模型：没有东西可装（旧缓存留给懒加载/LRU 处理）。
        let all_online = settings_with(None, None);
        assert_eq!(models_to_reload(Some(&both_local), &all_online), (None, None));

        // 在线模型之间、以及在线换回本地：后者要装。
        assert_eq!(
            models_to_reload(Some(&all_online), &all_online),
            (None, None)
        );
        assert_eq!(
            models_to_reload(Some(&all_online), &both_local),
            (Some(Qwen3_0_6B), Some(BgeSmallEnV1_5))
        );
    }

    /// 改的是和模型无关的设置时，不该触发任何装载。
    #[test]
    fn unrelated_settings_changes_do_not_reload_models() {
        let before = settings_with(Some(HuggingFaceModel::Qwen3_0_6B), None);
        let mut after = settings_with(Some(HuggingFaceModel::Qwen3_0_6B), None);
        after.smtp_host = String::from("smtp.example.com");
        after.max_session_idle_sec = 60;
        after.chat_provider.api_url = String::from("https://api.openai.com/v1/chat/completions");

        assert_eq!(models_to_reload(Some(&before), &after), (None, None));
    }

    /// 「重新加载模型」按钮的入参解析，以及"装哪个模型"的解析。
    ///
    /// `kind` 写错**必须拒绝**：要是把不认识的值当成 chat，就可能在用户想重装
    /// 句向量模型时把另一个对话模型装进缓存，运行中的对话被悄悄换掉。
    #[test]
    fn reload_targets_come_from_the_stored_settings() {
        use HuggingFaceModel::{BgeSmallEnV1_5, Qwen3_0_6B};

        assert!(matches!(
            ModelKind::from_request("chat"),
            Some(ModelKind::Chat)
        ));
        assert!(matches!(
            ModelKind::from_request("embedding"),
            Some(ModelKind::Embedding)
        ));
        for bad in ["", "Chat", "embeddings", " chat", "all"] {
            assert!(
                ModelKind::from_request(bad).is_none(),
                "{bad:?} must not be accepted"
            );
        }

        // 存库的设置决定装哪个：本地模型给出模型本身，在线模型给出 None
        // （按钮那边会明确报错，而不是随便装一个）。
        let local_both = settings_with(Some(Qwen3_0_6B), Some(BgeSmallEnV1_5));
        assert_eq!(
            local_model_for(&local_both, ModelKind::Chat),
            Some(Qwen3_0_6B)
        );
        assert_eq!(
            local_model_for(&local_both, ModelKind::Embedding),
            Some(BgeSmallEnV1_5)
        );

        let online_chat = settings_with(None, Some(BgeSmallEnV1_5));
        assert_eq!(local_model_for(&online_chat, ModelKind::Chat), None);
        assert_eq!(
            local_model_for(&online_chat, ModelKind::Embedding),
            Some(BgeSmallEnV1_5)
        );
    }

    /// 老记录里没有 `dimensions` / `dimensionsByProvider` / `indexedEmbedding`
    /// 这些键。少一个 `#[serde(default)]` 就会让**整个设置**读不回来 —— 那等于
    /// 用户升级之后机器人的配置全部消失，比缺少几个可选功能严重得多。
    #[test]
    fn settings_without_the_new_keys_still_load() {
        let mut v = serde_json::to_value(Settings::default()).unwrap();
        let obj = v.as_object_mut().unwrap();
        let p = obj
            .get_mut("sentenceEmbeddingProvider")
            .unwrap()
            .as_object_mut()
            .unwrap();
        assert!(p.remove("dimensions").is_some());
        assert!(p.remove("dimensionsByProvider").is_some());
        assert!(p.remove("indexedEmbedding").is_some());

        let s: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(s.sentence_embedding_provider.dimensions, None);
        assert_eq!(s.sentence_embedding_provider.dimensions_by_provider, None);
        assert_eq!(s.sentence_embedding_provider.indexed_embedding, None);
        assert!(s.sentence_embedding_provider.connect_timeout_millis > 0);
    }

    /// 每个 provider 各自那份维度必须原样进出：服务端不解析它，但也不能把它丢掉，
    /// 否则设置页每打开一次，"本地 8192 / 在线 16"就退化成只剩当前生效的那一份。
    #[test]
    fn per_provider_dimensions_round_trip_untouched() {
        let mut s = Settings::default();
        s.sentence_embedding_provider.dimensions = Some(8192);
        s.sentence_embedding_provider.dimensions_by_provider = Some(serde_json::json!({
            "HuggingFace": 8192,
            "OpenAICompatible": 16,
        }));

        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sentence_embedding_provider.dimensions, Some(8192));
        let map = back
            .sentence_embedding_provider
            .dimensions_by_provider
            .unwrap();
        assert_eq!(map["HuggingFace"], 8192);
        assert_eq!(map["OpenAICompatible"], 16);
    }

    /// 指纹必须同时带上模型名和维度，而且前端要能按 `|` 拆开它：
    /// 只比维度会让"换了模型但维度相同"蒙混过关，只比模型名则漏掉
    /// Matryoshka 截断（维度一变，`vector_distance_cos` 会让整条检索报错）。
    #[test]
    fn embedding_identity_names_both_the_model_and_the_dimensions() {
        let mut s = Settings::default();
        // 默认是本地 AllMiniLML6V2，没填维度。
        assert_eq!(s.current_embedding_identity(), "v1|huggingface|AllMiniLML6V2|0");
        s.sentence_embedding_provider.dimensions = Some(1024);
        assert_eq!(s.current_embedding_identity(), "v1|huggingface|AllMiniLML6V2|1024");

        // 在线模型用真正发给端点的那个名字（`OpenAICompatible(名字)`），
        // 外层那个 `model` 字段不参与 —— 前端存的是前者。
        s.sentence_embedding_provider.provider =
            embedding::SentenceEmbeddingProvider::OpenAICompatible(String::from("text-embedding-v4"));
        s.sentence_embedding_provider.model = String::from("stale-unused-field");
        let identity = s.current_embedding_identity();
        assert_eq!(identity, "v1|openai-compatible|text-embedding-v4|1024");
        let parts: Vec<&str> = identity.split('|').collect();
        assert_eq!(parts.len(), 4, "the frontend splits this on |: {identity}");
        assert_eq!(parts[2], "text-embedding-v4");
        assert_eq!(parts[3], "1024");

        // 同一个模型换维度、同一个维度换模型，指纹都必须变。
        let mut other = s.clone();
        other.sentence_embedding_provider.dimensions = Some(512);
        assert_ne!(other.current_embedding_identity(), identity);
        let mut third = s.clone();
        third.sentence_embedding_provider.provider =
            embedding::SentenceEmbeddingProvider::OpenAICompatible(String::from(
                "text-embedding-v3",
            ));
        assert_ne!(third.current_embedding_identity(), identity);
    }
}
