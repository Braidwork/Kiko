use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::vec::Vec;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::stream::DeltaStream;
use crate::ai::huggingface::{HuggingFaceModel, LoadedHuggingFaceModel};
use crate::flow::rt::dto::StreamingResponseData;
use crate::man::settings;
use crate::result::{Error, Result};

/// 一条对话消息。历史、单个提示词、以及 `/ai/text/generation` 收上来的
/// JSON 提示词数组都是这个形状。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Prompt {
    pub(crate) role: String,
    pub(crate) content: String,
}

/// 本地 HuggingFace 模型的采样参数。原来住在 `completion.rs`——那个模块是
/// 给"文本生成"用的第二套 provider 配置，现在并进了这里，所以这几个常量也
/// 跟着搬（`gemma` / `llama` / `phi3` / `moondream` 都在用）。
pub(crate) const TEMPERATURE: f64 = 0.7;
pub(crate) const REPEAT_PENALTY: f32 = 1.1;
pub(crate) const REPEAT_LAST_N: usize = 64;

/// 已经装好的本地模型，按 `robot_id` 一个实例。
///
/// 两把锁，职责不同：
///
/// - **外层**（`Mutex`）只保护这张表本身（查/插入），拿到 `Arc` 就释放；
/// - **内层**（`Mutex`）保护那个模型，生成期间一直持有。
///
/// 这样两个机器人用不同模型时可以**并行**生成；同一个机器人的请求仍在它的
/// 内层锁上串行 —— 这是必须的，因为 KV cache 是模型自身的可变状态，两次生成
/// 同时改它必然互相污染。
///
/// 以前是 `Mutex<HashMap<String, LoadedHuggingFaceModel>>`，只有一把锁、且从
/// 加载模型一直持有到生成结束，于是**所有**本地模型请求被强制排成一队。
static LOADED_MODELS: LazyLock<Mutex<HashMap<String, Arc<Mutex<LoadedHuggingFaceModel>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::with_capacity(8)));

/// 模型的最近使用顺序，队首最新、队尾最旧。
///
/// 本地模型很占内存（Qwen3-1.7B 权重约 1.1GB，再加约 470MB 的 KV cache；
/// 8B 是 5GB + 1.1GB），所以必须有上限，否则每个用过的机器人都会常驻一份。
static MODEL_LRU: LazyLock<Mutex<std::collections::VecDeque<String>>> =
    LazyLock::new(|| Mutex::new(std::collections::VecDeque::with_capacity(8)));

/// 最多同时常驻几个本地模型。超了就淘汰最久未用的那个。
const MAX_LOADED_MODELS: usize = 5;

/// 把 `robot_id` 记为最近使用。
fn touch_lru(lru: &mut std::collections::VecDeque<String>, robot_id: &str) {
    if let Some(i) = lru.iter().position(|k| k == robot_id) {
        lru.remove(i);
    }
    lru.push_front(String::from(robot_id));
}

/// 超过上限时淘汰最久未使用的模型，返回被淘汰的 key。
///
/// 淘汰**正在生成**的条目是安全的：在飞请求手里攥着 `Arc` 快照，会自己跑完。
/// 但那会在下次请求时白白重载，所以这里优先挑空闲的（`try_lock` 能拿到锁说明
/// 没人在生成）。全都忙就挑最旧的那个 —— 正确性不受影响，只是可能多一次加载。
///
/// 对 `T` 泛型只是为了单测能用占位类型（不必加载真实权重）。
fn evict_one_idle<T>(
    models: &mut HashMap<String, Arc<Mutex<T>>>,
    lru: &mut std::collections::VecDeque<String>,
) -> Option<String> {
    if let Some(free) = lru
        .iter()
        .rev()
        .find(|k| models.get(*k).map(|e| e.try_lock().is_ok()).unwrap_or(false))
    {
        let key = free.clone();
        models.remove(&key);
        if let Some(i) = lru.iter().position(|k| *k == key) {
            lru.remove(i);
        }
        return Some(key);
    }
    // 全都在生成中：退而求其次，淘汰最旧的那个。
    if let Some(key) = lru.pop_back() {
        models.remove(&key);
        return Some(key);
    }
    None
}

/// 正在加载中的机器人，防止同一个模型被并发加载两遍。
///
/// 用 `tokio::sync::Mutex` 是因为加载要跨 `.await` 持有它。每个 key 一把锁，
/// 所以一个机器人的冷加载不会挡住别的机器人。
static LOADING: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::with_capacity(8)));

/// 取出 `robot_id` 对应的模型，没有就加载一个。
///
/// 每次都更新 LRU 顺序；超过 `MAX_LOADED_MODELS` 时淘汰最久未用的那个。
async fn get_or_load_model(
    robot_id: &str,
    m: &HuggingFaceModel,
) -> Result<Arc<Mutex<LoadedHuggingFaceModel>>> {
    // 快路径：已装好就直接拿，完全不碰加载锁。
    {
        let models = LOADED_MODELS.lock()?;
        if let Some(existing) = models.get(robot_id) {
            let entry = existing.clone();
            let mut lru = MODEL_LRU.lock()?;
            touch_lru(&mut lru, robot_id);
            return Ok(entry);
        }
    }
    // 取该机器人的加载锁；同一个模型的并发冷启动在这里排队，只有第一个真正加载。
    // 注意这个 guard 要活到插入之后：否则排队者会在插入前溜进来重复加载。
    let init = {
        let mut loading = LOADING.lock()?;
        loading
            .entry(String::from(robot_id))
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _guard = init.lock().await;
    // 拿到加载锁后复查：排在前面的那个可能已经装好了。
    {
        let models = LOADED_MODELS.lock()?;
        if let Some(existing) = models.get(robot_id) {
            let entry = existing.clone();
            let mut lru = MODEL_LRU.lock()?;
            touch_lru(&mut lru, robot_id);
            return Ok(entry);
        }
    }
    // 锁外加载（只持有该机器人的加载锁）。生成用的是 `Arc` 快照，所以这里
    // 不会再阻塞任何已经在跑的生成。
    let loaded = LoadedHuggingFaceModel::load(m)?;

    cache_loaded_model(robot_id, loaded)
}

/// 把一个**已经装好**的模型放进缓存，并按 LRU 记账，返回放进去的那个句柄。
///
/// 插入和淘汰**必须在同一处**。只在 [`get_or_load_model`] 里记账的话，从别的入口
/// （保存设置的后台预热走 [`load_model_into_cache`]）插进来的条目不会进 `MODEL_LRU`
/// 队列：它永远不会被淘汰，`lru.len()` 也算不到它，常驻模型数就能悄悄超过
/// `MAX_LOADED_MODELS`。
fn cache_loaded_model(
    robot_id: &str,
    model: LoadedHuggingFaceModel,
) -> Result<Arc<Mutex<LoadedHuggingFaceModel>>> {
    // 换成一个新的 `Arc`：正在生成的请求手里还攥着旧的那个，会安全地跑完并被
    // 释放，不会被这里拦住，也不会被我们改到。
    let entry = Arc::new(Mutex::new(model));
    let evicted = {
        let mut models = LOADED_MODELS.lock()?;
        models.insert(String::from(robot_id), entry.clone());
        let mut lru = MODEL_LRU.lock()?;
        touch_lru(&mut lru, robot_id);
        if lru.len() > MAX_LOADED_MODELS {
            evict_one_idle(&mut models, &mut lru)
        } else {
            None
        }
    };
    if let Some(key) = evicted {
        // 顺手清掉加载锁条目，别让它无界增长。
        if let Ok(mut loading) = LOADING.lock() {
            loading.remove(&key);
        }
        log::info!(
            "Evicted local model for robot {key} (limit {MAX_LOADED_MODELS}); \
             it will be reloaded on its next request"
        );
    }
    Ok(entry)
}

/// The sink one answer's deltas go to.
pub(crate) struct SenderWrapper<D> {
    sender: UnboundedSender<D>,
    content_seq: usize,
}

impl SenderWrapper<StreamingResponseData> {
    pub(crate) fn new(sender: UnboundedSender<StreamingResponseData>, content_seq: usize) -> Self {
        Self {
            sender,
            content_seq,
        }
    }

    /// Returns `false` once the receiver is gone, which is how a client hanging
    /// up reaches the generation loops: they stop rather than keep producing
    /// into a channel nobody reads.
    ///
    /// The channel is unbounded, so this never blocks and never reorders — the
    /// old per-token `spawn_blocking` send could do both.
    pub(crate) fn send(&self, content: String) -> bool {
        self.sender
            .send(StreamingResponseData {
                content_seq: Some(self.content_seq),
                content,
            })
            .is_ok()
    }
}

/// Where a generated answer goes. Both variants accumulate the full text, so a
/// node still has the complete answer to reason about afterwards; they differ
/// only in whether each delta is also pushed to the client on arrival.
pub(crate) enum ResultSender<'r, D> {
    ChannelSender(SenderWrapper<D>, &'r mut String),
    StrBuf(&'r mut String),
}

impl ResultSender<'_, StreamingResponseData> {
    /// Whether the caller wants deltas as they are produced. This also decides
    /// whether a provider asks its backend to stream at all.
    pub(crate) fn is_streaming(&self) -> bool {
        matches!(self, Self::ChannelSender(..))
    }

    /// Appends a delta to the answer and, when streaming, pushes it out.
    /// Returns `false` once the client is gone and generation should stop.
    pub(crate) fn push_delta(&mut self, delta: String) -> bool {
        match self {
            Self::ChannelSender(sender, answer) => {
                answer.push_str(&delta);
                sender.send(delta)
            }
            Self::StrBuf(answer) => {
                answer.push_str(&delta);
                true
            }
        }
    }
}

impl<'r> ResultSender<'r, StreamingResponseData> {
    /// 把"借用调用方的缓冲区"拆开：流式那条 sender 变成可以 `move` 走的所有权值，
    /// 缓冲区还给调用方。
    ///
    /// 需要它是因为本地模型的推理必须搬到 `spawn_blocking` 上跑（原因见 `chat()`
    /// 里 HuggingFace 分支的说明），而阻塞任务的闭包要求 `'static`，`&'r mut String`
    /// 借不进去。于是文本先由阻塞任务攒在自己的缓冲里，跑完再交回调用方，
    /// 由调用方写回原来那块 `&mut String` —— 调用方看到的最终结果不变。
    fn detach(self) -> (Option<SenderWrapper<StreamingResponseData>>, &'r mut String) {
        match self {
            Self::ChannelSender(sender, answer) => (Some(sender), answer),
            Self::StrBuf(answer) => (None, answer),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "id", content = "model")]
pub(crate) enum ChatProvider {
    HuggingFace(HuggingFaceModel),
    /// 兼容老记录的两个名字：
    /// - `OpenAI`：2026-09-17 之前这个变体叫这个名字；
    /// - `Ollama`：UI 里曾经有第三个"Ollama"选项，它指向 Ollama 的原生端点
    ///   （`/api/chat`）。原生路径已经并进这条统一的 OpenAI 兼容路径，老记录里
    ///   的那种地址由 `settings::unify_legacy_api_url` 在读写设置时挪到同一个
    ///   host 上的 `/v1/chat/completions`，所以请求形状仍然正确。
    ///
    /// 两个别名都可以在所有实例保存过一次设置后删掉。
    #[serde(alias = "OpenAI", alias = "Ollama")]
    OpenAICompatible(String),
}

/// 从磁盘装载 `m`，装好后按 LRU 放进缓存。
///
/// **同步且耗时**：要把整个权重文件读进来再建 tensor，GGUF 动辄几 GB 到几十 GB。
/// 调用方必须把它放进 `spawn_blocking`，否则会占住一个 tokio worker 几十秒
/// （设置保存的后台预热就是这么用的）。
pub(crate) fn load_model_into_cache(robot_id: &str, m: &HuggingFaceModel) -> Result<()> {
    let m = LoadedHuggingFaceModel::load(m)?;
    cache_loaded_model(robot_id, m)?;
    Ok(())
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    /// 并发共享的"模型句柄"。用 `String` 占位就够了 —— 要验证的是**锁的粒度**，
    /// 不是模型本身，这样单测不必加载任何权重。
    fn handle(name: &str) -> Arc<Mutex<String>> {
        Arc::new(Mutex::new(String::from(name)))
    }

    fn model_map() -> &'static Mutex<HashMap<String, Arc<Mutex<String>>>> {
        // 与 LOADED_MODELS 同形的一张独立表，避免污染真实缓存。
        static TEST_MAP: LazyLock<Mutex<HashMap<String, Arc<Mutex<String>>>>> =
            LazyLock::new(|| Mutex::new(HashMap::new()));
        &TEST_MAP
    }

    /// 生成期间**绝不能**持有 `LOADED_MODELS`，否则所有本地模型请求（哪怕用的是
    /// 完全不同的模型）都会被排成一队 —— 这正是之前的性能问题：一把大锁从加载
    /// 模型一直持有到生成结束。
    ///
    /// 判定方式：模拟"机器人 A 正在生成"（持有 A 的内层锁），此时另一个线程必须
    /// 仍能拿到表锁、并能锁住 B 的模型。拿不到就说明粒度不对。
    #[test]
    fn one_robots_generation_does_not_block_another_robot() {
        let models = model_map();
        {
            let mut m = models.lock().unwrap();
            m.insert(String::from("t1"), handle("a"));
            m.insert(String::from("t2"), handle("b"));
        }

        // 取出 A 的快照后立刻放开表锁，然后"长时间生成"。
        let entry_a = { models.lock().unwrap().get("t1").unwrap().clone() };
        let guard_a = entry_a.lock().unwrap();
        assert_eq!(*guard_a, "a");

        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            // 只依赖 model_map() 的 'static 引用，不需要跨越借用。
            let models = model_map();
            let entry_b = { models.lock().unwrap().get("t2").unwrap().clone() };
            let g = entry_b.lock().unwrap();
            tx.send(g.clone()).unwrap();
        });

        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).expect(
                "another robot's model was unreachable while one generation ran: the map \
                 (or a shared lock) is still held across generation"
            ),
            "b"
        );
        drop(guard_a);
        worker.join().unwrap();
    }

    /// 同一个机器人的两次生成必须串行：KV cache 是模型自身的可变状态，并发改写
    /// 会互相污染 —— 这正是"粘在上一个话题"那个 bug 的成因。
    #[test]
    fn the_same_model_is_serialized() {
        let entry = handle("only");

        let guard = entry.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let entry2 = entry.clone();
        let worker = std::thread::spawn(move || {
            let _g = entry2.lock().unwrap();
            tx.send(()).unwrap();
        });

        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300)).is_err(),
            "a second generation on the same model must wait for the first"
        );
        drop(guard);
        assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok());
        worker.join().unwrap();
    }

    /// 本地模型必须有上限：每个都是几百 MB 到几 GB 的常驻内存。
    /// 超过 `MAX_LOADED_MODELS` 时淘汰最久未用的那个。
    #[test]
    fn the_least_recently_used_model_is_evicted() {
        let mut models: HashMap<String, Arc<Mutex<String>>> = HashMap::new();
        let mut lru = std::collections::VecDeque::new();

        // 装 6 个（上限 5），key 按使用顺序 t1..t6。
        for i in 1..=6 {
            let key = format!("t{i}");
            models.insert(key.clone(), handle(&key));
            touch_lru(&mut lru, &key);
            if lru.len() > MAX_LOADED_MODELS {
                evict_one_idle(&mut models, &mut lru);
            }
        }
        assert_eq!(models.len(), MAX_LOADED_MODELS);
        assert!(!models.contains_key("t1"), "t1 is the least recently used");
        for i in 2..=6 {
            assert!(models.contains_key(&format!("t{i}")), "t{i} should survive");
        }

        // 再碰一次 t2，它就不再是最旧的了 —— 下一个被淘汰的应该是 t3。
        touch_lru(&mut lru, "t2");
        models.insert(String::from("t7"), handle("t7"));
        touch_lru(&mut lru, "t7");
        evict_one_idle(&mut models, &mut lru);
        assert!(models.contains_key("t2"), "t2 was touched, so it stays");
        assert!(!models.contains_key("t3"), "t3 became the least recently used");
        assert!(models.contains_key("t7"), "the newest entry must never be evicted");
    }

    /// 正在生成的模型优先不被淘汰：`try_lock` 拿不到锁说明有人占着。
    #[test]
    fn eviction_prefers_an_idle_model() {
        let mut models: HashMap<String, Arc<Mutex<String>>> = HashMap::new();
        let mut lru = std::collections::VecDeque::new();
        for i in 1..=3 {
            let key = format!("t{i}");
            models.insert(key.clone(), handle(&key));
            touch_lru(&mut lru, &key);
        }
        // t1 最旧，但正在生成；t2 空闲。
        let busy = models.get("t1").unwrap().clone();
        let _busy_guard = busy.lock().unwrap();

        let evicted = evict_one_idle(&mut models, &mut lru);
        assert_eq!(
            evicted.as_deref(),
            Some("t2"),
            "an idle model should be evicted before a busy one"
        );
        assert!(models.contains_key("t1"), "the busy model stays");
    }
}

pub(crate) async fn chat(
    robot_id: &str,
    // prompt: &str,
    chat_history: Option<Vec<Prompt>>,
    media: Option<&crate::ai::dto::UserMediaData>,
    connect_timeout: Option<u32>,
    read_timeout: Option<u32>,
    // Qwen3 的思考模式开关。只对本地 Qwen3 生效，其余后端（含在线模型）忽略。
    enable_thinking: bool,
    result_sender: ResultSender<'_, StreamingResponseData>,
) -> Result<()> {
    if let Some(settings) = settings::get_settings(robot_id).await? {
        // log::info!("{:?}", &settings.chat_provider.provider);
        match settings.chat_provider.provider {
            ChatProvider::HuggingFace(m) => {
                // 本地模型是**同步**的 CPU 推理：一旦开始就占着当前线程不放，
                // 整段回答期间都不会让出。所以它必须搬去阻塞线程池，不能在 async
                // 任务里直接调用 —— 否则流式输出会被攒到最后一次性吐给客户端。
                //
                // 攒住 delta 的不是我们这层，是 tokio 多线程调度器的 LIFO 槽：
                // `SenderWrapper::send` 是在**正在跑推理的那个 worker 线程**上
                // 唤醒 SSE 连接任务的，而 `Handle::schedule_task` 对"worker 线程
                // 上的唤醒"走 `schedule_local`，把任务塞进**当前 worker 的 LIFO
                // 槽**；LIFO 槽是刻意不参与工作窃取的（tokio `worker.rs`：除 LIFO
                // 槽里的任务外都可被偷），所以连接任务要等推理结束、worker 回到
                // 调度器才第一次被轮询到 —— 表现就是"干等十几秒，然后整段回答
                // 一下子全冒出来"，本地模型越慢越明显，而 Ollama 那类在线后端
                // 每收一个 delta 都会 await 让出线程，所以看不出问题。
                //
                // 在阻塞线程池上唤醒则走 `push_remote_task`（inject 队列），任何
                // 空闲 worker 都能立刻接手，每个 delta 都会即时刷出去。
                let (sender, answer) = result_sender.detach();
                let model = m.clone();
                let media = media.cloned();
                let robot_id = String::from(robot_id);
                let sample_len = settings.chat_provider.max_response_token_length as usize;
                // 冷加载必须在 async 上下文里做：`spawn_blocking` 的闭包是同步
                // 的，`.await` 进不去；而加载要按机器人串行（`get_or_load_model`
                // 里那把按 key 分的加载锁）。放在这里也让**加载不占用模型锁**，
                // 已经在生成的机器人不受影响。
                let entry = get_or_load_model(&robot_id, &model).await?;
                let (r, generated) = tokio::task::spawn_blocking(move || {
                    let mut generated = String::with_capacity(1024);
                    let r = {
                        let sender = match sender {
                            Some(sender) => ResultSender::ChannelSender(sender, &mut generated),
                            None => ResultSender::StrBuf(&mut generated),
                        };
                        huggingface(
                            &robot_id,
                            &model,
                            entry,
                            chat_history,
                            media.as_ref(),
                            sample_len,
                            enable_thinking,
                            sender,
                        )
                    };
                    (r, generated)
                })
                .await
                .map_err(|e| Error::WithMessage(format!("Local model task failed: {e}")))?;
                // 失败时也把已生成的部分写回：调用方靠这块缓冲判断要不要兜底
                // （见 `flow/rt/node.rs`），中途出错时它和以前一样能看到半截回答。
                answer.push_str(&generated);
                r?;
                Ok(())
            }
            ChatProvider::OpenAICompatible(m) => {
                open_ai_compatible(
                    &m,
                    chat_history,
                    media,
                    &settings.chat_provider.api_url,
                    &settings.chat_provider.api_key,
                    settings.chat_provider.max_response_token_length,
                    connect_timeout.unwrap_or(settings.chat_provider.connect_timeout_millis),
                    read_timeout.unwrap_or(settings.chat_provider.read_timeout_millis),
                    &settings.chat_provider.proxy_url,
                    result_sender,
                )
                .await?;
                Ok(())
            }
        }
    } else {
        Err(Error::WithMessage(format!(
            "Can NOT retrieve settings from robot_id: {robot_id}"
        )))
    }
}

/// `/ai/text/generation` 的实现：对话节点里那个"生成文本"按钮。
///
/// 它原来走 `completion.rs`——一份独立的 provider 配置加一套独立的请求构造，
/// 于是同一个机器人要在设置页配两遍模型；而在线模型那条分支把调用方给过来的
/// JSON 提示词数组当成纯文本塞进一条 user 消息，界面里填的 system 提示词根本
/// 到不了模型。现在它和对话节点走**完全相同**的路径、用同一份配置。
///
/// `enable_thinking` 由调用方给（见 `dto::Request`），缺省为 `false`：这个接口
/// 是"生成一段文字"，思考过程的推理轨迹对调用方没有意义，而本地小模型上它会
/// 先烧掉几十秒才吐第一个字。
pub(crate) async fn gen_text(
    robot_id: &str,
    prompt: &str,
    enable_thinking: bool,
    sender: UnboundedSender<StreamingResponseData>,
) -> Result<()> {
    let history = parse_prompt(prompt);
    // 这里的应答只往通道里推，没人读累积的文本，但 `ResultSender` 两种变体都
    // 需要一块缓冲，就随它留着。
    let mut answer = String::with_capacity(1024);
    chat(
        robot_id,
        Some(history),
        None,
        None,
        None,
        enable_thinking,
        ResultSender::ChannelSender(SenderWrapper::new(sender, 0), &mut answer),
    )
    .await
}

/// 调用方发来的提示词：`[{"role":"user","content":"…"}]`。不是 JSON 就当成
/// 一条 user 消息，纯文本的老调用方也不至于直接失败。
///
/// system 消息统一提到最前。调用方是按 `[user, system]` 拼的（`DialogNode.vue`
/// 先 push user 再 push system），而 system 落在 user 之后对部分端点是非法顺序；
/// 本地 HF 那条路径本来也是把它拆出来放最前的，这样三个后端行为才一致。
fn parse_prompt(s: &str) -> Vec<Prompt> {
    let mut prompts: Vec<Prompt> = serde_json::from_str(s).unwrap_or_else(|_| {
        vec![Prompt {
            role: String::from("user"),
            content: String::from(s),
        }]
    });
    // `sort_by_key` 是稳定排序，system 之间的相对顺序不变。
    prompts.sort_by_key(|p| !p.role.eq("system"));
    prompts
}

fn huggingface(
    _robot_id: &str,
    m: &HuggingFaceModel,
    entry: Arc<Mutex<LoadedHuggingFaceModel>>,
    chat_history: Option<Vec<Prompt>>,
    media: Option<&crate::ai::dto::UserMediaData>,
    sample_len: usize,
    enable_thinking: bool,
    mut result_sender: ResultSender<'_, StreamingResponseData>,
) -> Result<()> {
    let info = m.get_info();
    // log::info!("model_type={:?}", &info.model_type);
    let empty_media = crate::ai::dto::UserMediaData::default();
    let media = media.unwrap_or(&empty_media);
    if !media.is_empty() && !info.supports_vision() {
        return Err(Error::WithMessage(format!(
            "Model {:?} doesn't support image input, please configure a vision model (e.g. Moondream2, or OpenAI/Ollama vision models).",
            &info.model_type
        )));
    }
    let new_prompt = info.convert_prompt("", chat_history.clone(), enable_thinking)?;
    log::info!("Prompt: {}", &new_prompt);
    // 只锁这一个模型：不同机器人之间互不阻塞，同一机器人的请求在此串行
    // （KV cache 是模型自身的可变状态，必须串行）。
    //
    // `entry` 是 `chat()` 拿到的 `Arc` 快照：即使期间有人 `load_model_into_cache`
    // 换了新实例，我们手上这个依然有效，不会被改动或提前释放。
    let mut guard = entry.lock().unwrap_or_else(|e| {
        log::warn!("{:#?}", &e);
        e.into_inner()
    });
    let loaded_model = &mut *guard;
    match loaded_model {
        LoadedHuggingFaceModel::Gemma(m) => super::gemma::gen_text(
            &m.0,
            &m.1,
            &m.2,
            &new_prompt,
            sample_len,
            Some(0.5),
            &mut result_sender,
        ),
        LoadedHuggingFaceModel::Gemma4((device, model, tokenizer)) => {
            // Gemma 4 的 prompt 由 `convert_prompt` 拼成 `<|turn>…`，图片占位符
            // （`<|image|>`）由 `gen_text_gemma4` 按每张图的软 token 数补在后面。
            // 模型只有一个形状（多模态），纯文本请求也走它。
            super::gemma::gen_text_gemma4(
                device,
                &mut model.0,
                tokenizer,
                super::gemma::Gemma4Input {
                    prompt: &new_prompt,
                    images_base64: &media.images,
                },
                sample_len,
                Some(0.5),
                &mut result_sender,
            )
        }
        LoadedHuggingFaceModel::Llama(m) => super::llama::gen_text(
            &m.0,
            &m.1,
            &m.2,
            &m.3,
            &m.4,
            &new_prompt,
            sample_len,
            Some(25),
            Some(0.5),
            &mut result_sender,
        ),
        LoadedHuggingFaceModel::Phi3(m) => super::phi3::gen_text(
            &m.0,
            &m.1,
            &m.2,
            &new_prompt,
            sample_len,
            Some(0.5),
            &mut result_sender,
        ),
        LoadedHuggingFaceModel::Moondream((device, model, tokenizer)) => {
            // Moondream's text backbone doesn't take a chat-formatted prompt,
            // the user question is the last user message.
            let question = chat_history
                .as_ref()
                .and_then(|h| {
                    h.iter()
                        .rev()
                        .find(|p| p.role.eq("user") && !p.content.is_empty())
                })
                .map(|p| p.content.clone())
                .unwrap_or_default();
            super::moondream::gen_text(
                device,
                model,
                tokenizer,
                super::moondream::MoondreamInput {
                    prompt: &question,
                    images_base64: &media.images,
                },
                sample_len,
                Some(0.5),
                &mut result_sender,
            )
        }
        LoadedHuggingFaceModel::Qwen3((device, model, tokenizer)) => super::qwen3::gen_text(
            device,
            model,
            tokenizer,
            &new_prompt,
            sample_len,
            Some(0.5),
            &mut result_sender,
        ),
        LoadedHuggingFaceModel::Qwen3Moe((device, model, tokenizer)) => {
            super::qwen3::gen_text_moe(
                device,
                model,
                tokenizer,
                &new_prompt,
                sample_len,
                Some(0.5),
                &mut result_sender,
            )
        }
        LoadedHuggingFaceModel::Bert(_m) => Err(Error::WithMessage(format!(
            "Unsuported model type {:?}.",
            &info.model_type
        ))),
    }
    // Ok(())
}

async fn open_ai_compatible(
    m: &str,
    chat_history: Option<Vec<Prompt>>,
    media: Option<&crate::ai::dto::UserMediaData>,
    api_url: &str,
    api_key: &str,
    max_response_token_length: u32,
    connect_timeout_millis: u32,
    read_timeout_millis: u32,
    proxy_url: &str,
    mut result_sender: ResultSender<'_, StreamingResponseData>,
) -> Result<()> {
    // let client = HTTP_CLIENTS.lock()?.entry(String::from("value")).or_insert(crate::external::http::get_client(connect_timeout_millis.into(), read_timeout_millis.into(), proxy_url)?);
    let client = crate::external::http::get_client(
        connect_timeout_millis.into(),
        read_timeout_millis.into(),
        proxy_url,
    )?;
    let mut req_body = Map::new();
    req_body.insert(String::from("model"), Value::from(m));

    let mut sys_message = Map::new();
    sys_message.insert(String::from("role"), Value::from("system"));
    sys_message.insert(
        String::from("content"),
        Value::from("You are a helpful assistant."),
    );
    let empty_media = crate::ai::dto::UserMediaData::default();
    let media = media.unwrap_or(&empty_media);
    // The system message is pushed rather than written into a pre-sized slot:
    // indexing a `Vec::with_capacity(1)` that was never pushed to panics, which
    // is what an empty or absent history used to do.
    let mut messages: Vec<Value> = Vec::with_capacity(1 + chat_history.as_ref().map_or(0, Vec::len));
    messages.push(Value::Object(sys_message));
    if let Some(h) = chat_history {
        for p in h.into_iter() {
            // 键必须是 `role` 和 `content`。这里原来写的是
            // `map.insert(p.role, Value::String(p.content))`，即把角色名当成了
            // 键（`{"user":"hi"}`），任何端点都会以"缺少 role"驳回；视觉那段
            // 靠 `get("role")` 找最后一条 user 消息，也因此永远找不到。
            let mut map = Map::new();
            map.insert(String::from("role"), Value::String(p.role));
            map.insert(String::from("content"), Value::String(p.content));
            messages.push(Value::Object(map));
        }
    }
    // OpenAI-compatible vision format: the last user message's content becomes
    // an array of typed parts (text + image_url) when images are present.
    // Works with OpenAI, vLLM and other OpenAI-compatible servers.
    if !media.is_empty() {
        let last_user_idx = messages.iter().rposition(|m| {
            m.get("role").and_then(|r| r.as_str()) == Some("user")
        });
        if let Some(idx) = last_user_idx {
            let msg = messages[idx].as_object().unwrap();
            let text = msg
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string();
            let mut parts: Vec<Value> = Vec::with_capacity(media.images.len() + 1);
            if !text.is_empty() {
                let mut text_part = Map::new();
                text_part.insert(String::from("type"), Value::from("text"));
                text_part.insert(String::from("text"), Value::String(text));
                parts.push(Value::Object(text_part));
            }
            for img in media.images.iter() {
                let mut image_url = Map::new();
                image_url.insert(String::from("url"), Value::String(img.clone()));
                let mut part = Map::new();
                part.insert(String::from("type"), Value::from("image_url"));
                part.insert(String::from("image_url"), Value::Object(image_url));
                parts.push(Value::Object(part));
            }
            let mut new_msg = Map::new();
            new_msg.insert(String::from("role"), Value::from("user"));
            new_msg.insert(String::from("content"), Value::Array(parts));
            messages[idx] = Value::Object(new_msg);
        }
    }
    let messages = Value::Array(messages);
    req_body.insert(String::from("messages"), messages);

    // The backend is asked to stream only when someone is reading frame by
    // frame; otherwise one document is simpler for both sides.
    req_body.insert(
        String::from("stream"),
        Value::Bool(result_sender.is_streaming()),
    );
    // 这个上限原来在本路径上被完全忽略（只有本地和 Ollama 分支用了），
    // 所以界面上那个"最大响应 token 长度"对任何在线模型都是摆设。
    req_body.insert(
        String::from("max_tokens"),
        Value::from(max_response_token_length),
    );
    let obj = Value::Object(req_body);
    // 不再回退到 api.openai.com：用户填了别家的 key 却漏了地址时，
    // 静默把请求（和 key）打向 OpenAI 不是我们该做的选择。
    if api_url.is_empty() {
        return Err(Error::WithMessage(String::from(
            "OpenAI-compatible API URL is empty, please configure it in settings.",
        )));
    }
    let u = String::from(api_url);
    let req = client
        .post(&u)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .body(serde_json::to_string(&obj)?);
    let res = req.send().await?;
    let status = res.status();
    if !status.is_success() {
        // Without this a rejected key or an unknown model shows up as a
        // silently empty answer.
        let body = res.text().await.unwrap_or_default();
        // 报错里带上地址：这是排查任意第三方端点的唯一线索，
        // 而用户往往同时配了好几个 provider。
        return Err(Error::WithMessage(format!(
            "OpenAI-compatible endpoint {u} returned {status}: {body}"
        )));
    }
    if result_sender.is_streaming() {
        // `bytes_stream()` yields arbitrary fragments: several events, half an
        // event, or one ending mid-character. DeltaStream buffers them so only
        // complete payloads are dispatched.
        let mut deltas = DeltaStream::new();
        let mut stream = res.bytes_stream();
        while let Some(item) = stream.next().await {
            for delta in deltas.push(item?.as_ref())? {
                if !result_sender.push_delta(delta) {
                    log::warn!("OpenAI stream receiver is gone, stopping generation.");
                    return Ok(());
                }
            }
            if deltas.is_done() {
                break;
            }
        }
        for delta in deltas.finish()? {
            if !result_sender.push_delta(delta) {
                return Ok(());
            }
        }
    } else {
        let v: Value = serde_json::from_slice(res.bytes().await?.as_ref())?;
        if let Some(content) = v
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
        {
            log::info!("OpenAI returned {content}");
            result_sender.push_delta(String::from(content));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::ai::test_support::{drain, prompt, serve, serve_capturing, sse_event, sse_head};

    use super::*;

    /// 本地模型那条路要把 delta 的 sender 交给阻塞线程池（靠 [`ResultSender::detach`]），
    /// 缓冲区留在调用方手里。这一步不能把"要不要流式"弄丢 —— 丢了就退化成
    /// 一次性应答；缓冲也必须还是调用方那块，否则 `node.rs` 拿到的回答会是空的。
    #[tokio::test]
    async fn detach_keeps_the_stream_and_hands_the_buffer_back() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut answer = String::new();
        let (sender, buf) =
            ResultSender::ChannelSender(SenderWrapper::new(tx, 7), &mut answer).detach();
        assert!(
            sender.is_some(),
            "a streaming call must keep its channel: the local-model path hands this \
             sender to the blocking pool"
        );
        buf.push_str("hi");
        assert_eq!(buf.len(), 2, "the buffer handed back is the caller's");
        assert_eq!(answer, "hi", "the caller's buffer is the one that moves");

        // 非流式的调用不能被凭空塞一个通道进去。
        let rs: ResultSender<StreamingResponseData> = ResultSender::StrBuf(&mut answer);
        let (sender, buf) = rs.detach();
        assert!(sender.is_none());
        buf.push_str("!");
        assert_eq!(buf.len(), 3, "the buffer handed back is the caller's");
        assert_eq!(answer, "hi!");
        assert!(rx.try_recv().is_err());
    }

    /// What actually goes on the wire: the configured URL, the key, and the
    /// token cap. The cap used to be dropped entirely on this path, and an
    /// empty URL used to silently become OpenAI's.
    #[tokio::test]
    async fn open_ai_compatible_sends_url_key_and_token_cap() {
        let body = format!("{}data: [DONE]\n\n", sse_event("hi"));
        let (url, request) =
            serve_capturing("/custom/llm/chat/completions", sse_head(), &body).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut answer = String::new();
        open_ai_compatible(
            "deepseek-chat",
            prompt(),
            None,
            &url,
            "test-key",
            4_242,
            3_323,
            9_923,
            "",
            ResultSender::ChannelSender(SenderWrapper::new(tx, 0), &mut answer),
        )
        .await
        .unwrap();
        assert_eq!(answer, "hi");
        assert_eq!(drain(&mut rx), vec!["hi"]);

        let request = request.await.unwrap();
        assert!(
            request.starts_with("POST /custom/llm/chat/completions "),
            "the configured path must be used, not a hardcoded one: {request}"
        );
        // Header names arrive lowercased over HTTP/1.1.
        assert!(
            request.to_lowercase().contains("authorization: bearer test-key"),
            "the configured key must be sent: {request}"
        );
        let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
        assert!(
            body.contains("\"max_tokens\":4242"),
            "the configured token cap must be sent: {body}"
        );
        // 消息必须是 `{"role":…,"content":…}`。这里曾经写成 `{"user":"hi"}`——
        // 把角色名当成了键，任何端点都会以"缺少 role"驳回，视觉那段靠
        // `get("role")` 找最后一条 user 消息也因此永远找不到。
        assert!(
            body.contains("\"role\":\"user\"") && body.contains("\"content\":\"hi\""),
            "every message needs role and content keys: {body}"
        );
        assert!(
            !body.contains("\"hi\":") && !body.contains("\"user\":\"hi\""),
            "the role must not be used as the key: {body}"
        );
    }

    /// `/ai/text/generation` 收到的提示词是 JSON 数组，且调用方按
    /// `[user, system]` 的顺序拼（`DialogNode.vue`）。system 必须被提到最前，
    /// 否则部分端点会直接拒绝这个顺序；纯文本则退化成一条 user 消息。
    #[test]
    fn parse_prompt_puts_system_first_and_accepts_plain_text() {
        let p = parse_prompt(
            r#"[{"role":"user","content":"写一句问候"},{"role":"system","content":"你是客服"}]"#,
        );
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].role, "system");
        assert_eq!(p[0].content, "你是客服");
        assert_eq!(p[1].role, "user");
        assert_eq!(p[1].content, "写一句问候");

        let p = parse_prompt("hello");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].role, "user");
        assert_eq!(p[0].content, "hello");
    }

    /// Qwen3 用 ChatML。这里锁住几个容易写错的细节：
    /// - 回合结束标记是 `<|im_end|>`，不是 llama 的 `</s>`；
    /// - 每个回合都必须闭合（漏掉 `<|im_end|>` 会让模型把下一段当成同一回合）；
    /// - 最后必须以 `<|im_start|>assistant\n` 结尾，模型才知道该它说话了。
    ///
    /// 期望值对照 HF 上 `Qwen/Qwen3-0.6B` 的 `chat_template` 输出。history 就是
    /// 完整的消息列表（含最新那条 user）—— `chat()` 与 `gen_text()` 都是这么传的
    /// （`s` 为空串）。
    ///
    /// 这里传 `enable_thinking = true`，所以不该出现 `/no_think`；关掉的那个
    /// 由 `qwen3_thinking_off_injects_no_think` 覆盖。
    #[test]
    fn qwen3_prompt_is_chatml_and_stays_closed() {
        let info = HuggingFaceModel::Qwen3_0_6B.get_info();
        let history = vec![
            Prompt {
                role: String::from("system"),
                content: String::from("你是客服"),
            },
            Prompt {
                role: String::from("user"),
                content: String::from("你好"),
            },
            Prompt {
                role: String::from("assistant"),
                content: String::from("你好，有什么可以帮你？"),
            },
            Prompt {
                role: String::from("user"),
                content: String::from("退货运费谁出"),
            },
        ];
        assert_eq!(
            info.convert_prompt("", Some(history), true).unwrap(),
            "<|im_start|>system\n你是客服<|im_end|>\n\
             <|im_start|>user\n你好<|im_end|>\n\
             <|im_start|>assistant\n你好，有什么可以帮你？<|im_end|>\n\
             <|im_start|>user\n退货运费谁出<|im_end|>\n\
             <|im_start|>assistant\n"
        );
    }

    /// 没有历史时，`user` 为空必须**跳过**整个 user 回合：`<|im_start|>user\n
    /// <|im_end|>` 不是 Qwen3 模板会产出的形状，而 `chat()` 的第一轮就是
    /// "history 里有 user + `s` 为空"，不注意就会多送一个空回合。
    #[test]
    fn qwen3_prompt_skips_an_empty_user_turn() {
        let info = HuggingFaceModel::Qwen3_8B.get_info();
        assert_eq!(
            info.convert_prompt("", None, true).unwrap(),
            "<|im_start|>assistant\n"
        );
        // 只有 system、没有 user 时同理，也不能留下空回合。
        let history = vec![Prompt {
            role: String::from("system"),
            content: String::from("你是客服"),
        }];
        assert_eq!(
            info.convert_prompt("", Some(history), true).unwrap(),
            "<|im_start|>system\n你是客服<|im_end|>\n<|im_start|>assistant\n"
        );
    }

    /// MoE 档位走同一套 ChatML；两条分支不能有一条漏掉。
    #[test]
    fn qwen3_moe_uses_the_same_chatml() {
        let dense = HuggingFaceModel::Qwen3_0_6B.get_info();
        let moe = HuggingFaceModel::Qwen3_30bA3bInstruct2507.get_info();
        let history = Some(vec![Prompt {
            role: String::from("user"),
            content: String::from("hi"),
        }]);
        assert_eq!(
            dense.convert_prompt("", history.clone(), true).unwrap(),
            moe.convert_prompt("", history, true).unwrap()
        );
    }

    /// 关闭思考：照 Qwen3 自己 `chat_template` 的 `enable_thinking == false`
    /// 分支，在生成前缀后追加一个**空的 think 块**。
    ///
    /// 早先这里发的是 `/no_think`（Qwen2.5 的软开关）。实测 0.6B 根本不认它：
    /// 照样输出 `<think>\n\n</think>\n\n` 前缀，而那个 `</think>` 会作为正文流到
    /// 用户面前 —— 就是"回答以 `</think>` 开头"这个现象的来源。
    #[test]
    fn qwen3_thinking_off_appends_an_empty_think_block() {
        let info = HuggingFaceModel::Qwen3_0_6B.get_info();
        let history = Some(vec![
            Prompt {
                role: String::from("system"),
                content: String::from("你是客服"),
            },
            Prompt {
                role: String::from("user"),
                content: String::from("你好"),
            },
        ]);
        assert_eq!(
            info.convert_prompt("", history.clone(), false).unwrap(),
            "<|im_start|>system\n你是客服<|im_end|>\n\
             <|im_start|>user\n你好<|im_end|>\n\
             <|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        // 开着思考时不能有这个空块（那等于替模型把思考跳过了）。
        assert_eq!(
            info.convert_prompt("", history, true).unwrap(),
            "<|im_start|>system\n你是客服<|im_end|>\n\
             <|im_start|>user\n你好<|im_end|>\n\
             <|im_start|>assistant\n"
        );
    }

    /// MoE 那档（Instruct-2507）的模板里本来就没有 `<think>`，但开关走的是同一条
    /// 分支，不能出现"密集模型注入、MoE 不注入"这种不一致。
    #[test]
    fn qwen3_moe_honours_the_thinking_switch() {
        let moe = HuggingFaceModel::Qwen3_30bA3bInstruct2507.get_info();
        assert_eq!(
            moe.convert_prompt("", None, false).unwrap(),
            "<|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        assert_eq!(
            moe.convert_prompt("", None, true).unwrap(),
            "<|im_start|>assistant\n"
        );
    }

    /// 空 think 块是 Qwen3 专有的：**绝不能**注入到 llama/gemma/phi3/phi4-mini 的
    /// 提示词里，那会把它们的提示词弄脏。这个开关必须由 `convert_prompt` 按模型类型
    /// 判断，而不是让调用方自己拼字符串。
    #[test]
    fn the_thinking_markers_never_leak_into_other_models() {
        let history = Some(vec![Prompt {
            role: String::from("user"),
            content: String::from("hi"),
        }]);
        // `(模型, 它认不认这个思考开关)`。Gemma 4 用的是自己那套 `<|think|>` /
        // `<|channel>`，认这个开关 —— 具体形状由下面 `gemma4_prompt_*` 那条测试管；
        // 这里只要求它们都别拿到 Qwen 的 `<think>` / `/no_think`。
        for (m, honours_thinking) in [
            (HuggingFaceModel::TinyLlama1_1bChatV1_0, false),
            (HuggingFaceModel::Gemma2bInstruct, false),
            (HuggingFaceModel::Phi3Mini4kInstruct, false),
            (HuggingFaceModel::Phi4MiniInstruct, false),
            (HuggingFaceModel::Gemma4E4BIt, true),
        ] {
            let info = m.get_info();
            for enable in [false, true] {
                let p = info.convert_prompt("", history.clone(), enable).unwrap();
                assert!(
                    !p.contains("<think>") && !p.contains("/no_think"),
                    "{m:?} (thinking={enable}) must not get Qwen thinking markers: {p}"
                );
            }
            if !honours_thinking {
                // 两种取值必须产出完全一样的提示词。
                assert_eq!(
                    info.convert_prompt("", history.clone(), false).unwrap(),
                    info.convert_prompt("", history.clone(), true).unwrap(),
                    "{m:?} ignores the thinking switch, so both settings must match"
                );
            }
        }
    }

    /// Gemma 4 换了回合标记：`<|turn>role\n … <turn|>\n`，生成前缀
    /// `<|turn>model\n`。这里锁住几个容易写错的地方：
    /// - 不能用 gemma2 的 `<start_of_turn>` / `<end_of_turn>`；
    /// - assistant 的回合名是 `model`，不是 `assistant`；
    /// - 有 system 消息就要开系统回合（哪怕没开思考）；
    /// - 思考开关只在系统回合最前面加 `<|think|>`，并且此时**空**系统提示词也要
    ///   开系统回合 —— 官方模板的条件是 `enable_thinking or tools or system`；
    /// - 空 user 回合必须跳过。
    #[test]
    fn gemma4_prompt_uses_turn_markers_and_gates_thinking() {
        let info = HuggingFaceModel::Gemma4E4BIt.get_info();
        let history = Some(vec![
            Prompt {
                role: String::from("system"),
                content: String::from("你是客服"),
            },
            Prompt {
                role: String::from("user"),
                content: String::from("你好"),
            },
            Prompt {
                role: String::from("assistant"),
                content: String::from("你好，有什么可以帮你？"),
            },
            Prompt {
                role: String::from("user"),
                content: String::from("退货运费谁出"),
            },
        ]);
        // 没开思考：系统回合照开，但没有 `<|think|>`。
        assert_eq!(
            info.convert_prompt("", history.clone(), false).unwrap(),
            "<|turn>system\n你是客服<turn|>\n\
             <|turn>user\n你好<turn|>\n\
             <|turn>model\n你好，有什么可以帮你？<turn|>\n\
             <|turn>user\n退货运费谁出<turn|>\n\
             <|turn>model\n"
        );
        // 开了思考：多出来的系统回合在**最前面**、内容为空 —— 因为 `s` 是空的，
        // system 正文只存在于 history 里，这和官方模板只认 `messages[0]` 的口径
        // 一致，于是 history 里那条 system 又各自成了一个系统回合。
        assert_eq!(
            info.convert_prompt("", history, true).unwrap(),
            "<|turn>system\n<|think|>\n<turn|>\n\
             <|turn>system\n你是客服<turn|>\n\
             <|turn>user\n你好<turn|>\n\
             <|turn>model\n你好，有什么可以帮你？<turn|>\n\
             <|turn>user\n退货运费谁出<turn|>\n\
             <|turn>model\n"
        );
        // 没有历史、没有系统提示词：只有一个空的生成前缀（`chat()` 的第一轮就是
        // "history 为空、`s` 为空"这个形状，不能多送空回合）。
        assert_eq!(
            info.convert_prompt("", None, false).unwrap(),
            "<|turn>model\n"
        );
        // 开了思考但没有系统提示词：仍要有系统回合来承载 `<|think|>`。
        assert_eq!(
            info.convert_prompt("", None, true).unwrap(),
            "<|turn>system\n<|think|>\n<turn|>\n<|turn>model\n"
        );
        // 上一轮回答里带思考块时，拼进提示词前要被摘掉（官方 `strip_thinking`）。
        let with_thought = Some(vec![Prompt {
            role: String::from("assistant"),
            content: String::from("<|channel>thought\n内部推理<channel|>正式回答"),
        }]);
        assert_eq!(
            info.convert_prompt("", with_thought, false).unwrap(),
            "<|turn>model\n正式回答<turn|>\n<|turn>model\n"
        );
        // system / user 同时出现在 **history** 和 `s` 两条路上时，两边的内容都会被
        // 渲染：system 变成两个系统回合、user 变成两个 user 回合。这是官方模板的
        // 口径（它只把 `messages[0]` 当系统消息、其余原样渲染），而两个调用方传的
        // `s` 都是空串（`chat()` 的最新 user 消息在 history 末尾），所以线上走不到
        // 这个组合。这里把既成事实钉住，免得以后误以为是新引入的 bug。
        let system_and_user_in_history = Some(vec![
            Prompt {
                role: String::from("system"),
                content: String::from("你是客服"),
            },
            Prompt {
                role: String::from("user"),
                content: String::from("你好"),
            },
        ]);
        assert_eq!(
            info.convert_prompt(
                r#"[{"role":"user","content":"你好"},{"role":"system","content":"你是客服"}]"#,
                system_and_user_in_history,
                false
            )
            .unwrap(),
            "<|turn>system\n你是客服<turn|>\n\
             <|turn>system\n你是客服<turn|>\n\
             <|turn>user\n你好<turn|>\n\
             <|turn>user\n你好<turn|>\n\
             <|turn>model\n"
        );
    }

    /// Gemma 4 的权重和分词器在**同一个**仓库里（不像 Qwen3 的 GGUF 是双仓库），
    /// 而且是**单文件** `model.safetensors`：`model_index_file` 必须留空，否则
    /// `get_model_files` 会去找不存在的 `model.safetensors.index.json`。
    /// `processor_config.json` 必须在下载清单里 —— 图像预处理的参数来自它。
    #[test]
    fn gemma4_is_a_single_repository_single_file_model() {
        for m in [
            HuggingFaceModel::Gemma4E2BIt,
            HuggingFaceModel::Gemma4E4BIt,
            HuggingFaceModel::Gemma412BIt,
        ] {
            let info = m.get_info();
            assert_eq!(
                info.model_type,
                crate::ai::huggingface::HuggingFaceModelType::Gemma4
            );
            assert!(info.supports_vision(), "{m:?} takes images");
            assert_eq!(info.tokenizer_repository(), info.repository);
            assert!(
                info.model_index_file.is_empty(),
                "{m:?} has a single weights file, so it must not look for an index"
            );
            assert!(
                info.model_files.contains(&"processor_config.json"),
                "{m:?} needs processor_config.json: the image preprocessing reads it"
            );
            assert!(
                !info.model_files.contains(&"model.safetensors"),
                "{m:?}: the weights file must not be listed twice (get_model_files \
                 already adds it)"
            );
        }
        assert_eq!(
            HuggingFaceModel::Gemma4E4BIt.get_info().local_directory(),
            "google/gemma-4-E4B-it"
        );
    }

    /// Gemma 4 走的是**文本 + 图像**那条路：加载器只接受带视觉塔的检查点，所以
    /// 这两个档位必须真的能收图，而且都不需要访问令牌（Apache-2.0、非 gated）。
    #[test]
    fn gemma4_models_each_declare_a_vision_tower() {
        for (m, repo) in [
            (HuggingFaceModel::Gemma4E2BIt, "google/gemma-4-E2B-it"),
            (HuggingFaceModel::Gemma4E4BIt, "google/gemma-4-E4B-it"),
            (HuggingFaceModel::Gemma412BIt, "google/gemma-4-12B-it"),
        ] {
            let info = m.get_info();
            assert_eq!(info.repository, repo);
            assert_eq!(
                info.model_type,
                crate::ai::huggingface::HuggingFaceModelType::Gemma4
            );
            assert!(info.supports_vision(), "{m:?} must accept images");
        }
    }

    /// GGUF 模型是**双仓库**的：权重从 unsloth 的 GGUF 仓库取，分词器从官方
    /// base 仓库取（GGUF 仓库里没有 tokenizer.json，已核对过文件清单）。
    /// 这里锁住两个**远端来源**，防止有人"顺手统一"成同一个而把下载跑成 404。
    ///
    /// 注意本地目录不在这两个字段里：它跟着保存时用的 `repository` 走，
    /// 所以 GGUF 落在 `data/models/Qwen/Qwen3-4B/`，而不是量化仓库名下。
    #[test]
    fn qwen3_fetches_weights_and_tokenizer_from_two_repositories() {
        let info = HuggingFaceModel::Qwen3_4B.get_info();
        assert_eq!(info.tokenizer_repository(), "Qwen/Qwen3-4B");
        assert_eq!(info.local_directory(), "Qwen/Qwen3-4B");
        assert_eq!(
            info.gguf_model_path().unwrap(),
            "./data/models/Qwen/Qwen3-4B/Qwen3-4B-Q4_K_M.gguf"
        );
        // 非 GGUF 模型不受影响：分词器仓库回退到权重仓库。
        let bert = HuggingFaceModel::AllMiniLML6V2.get_info();
        assert!(bert.gguf_model_path().is_none());
        assert_eq!(
            bert.tokenizer_repository(),
            "sentence-transformers/all-MiniLM-L6-v2"
        );
    }

    /// The point of the streaming path: provider output arrives as arbitrary
    /// byte fragments and the answer still has to come out whole and in order.
    /// The cuts below land inside a multi-byte character and between an event's
    /// lines, which is where the old code gave up and lost the whole answer.
    #[tokio::test]
    async fn open_ai_compatible_streams_deltas_whatever_the_chunk_boundaries() {
        let body = format!(
            ": OPENROUTER PROCESSING\n\n{}{}{}data: [DONE]\n\n{}",
            sse_event("Hello, "),
            sse_event("世"),
            sse_event("界"),
            sse_event("NEVER"),
        );
        // A cut in the middle of an event's payload, then one and two bytes into
        // 世, so a multi-byte character is split across three reads and the
        // fragments in between are not valid UTF-8 on their own.
        let hello = body.find("Hello").expect("the body contains Hello") + 2;
        let world = body.find('世').expect("the body contains 世") + 1;
        let cuts = [0, hello, world, world + 1];
        let url = serve(sse_head(), &body, &cuts).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut answer = String::new();
        open_ai_compatible(
            "test-model",
            prompt(),
            None,
            &url,
            "test-key",
            1_000,
            3_311,
            9_913,
            "",
            ResultSender::ChannelSender(SenderWrapper::new(tx, 0), &mut answer),
        )
        .await
        .unwrap();
        assert_eq!(answer, "Hello, 世界", "text after [DONE] must not appear");
        assert_eq!(drain(&mut rx), vec!["Hello, ", "世", "界"]);
    }

    /// A client that hangs up stops the generation, and stopping is not an
    /// error: the run just ends.
    #[tokio::test]
    async fn open_ai_compatible_stops_when_the_client_is_gone() {
        let body = format!("{}{}{}", sse_event("a"), sse_event("b"), sse_event("c"));
        let url = serve(sse_head(), &body, &[]).await;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let mut answer = String::new();
        open_ai_compatible(
            "test-model",
            prompt(),
            None,
            &url,
            "test-key",
            1_000,
            3_313,
            9_917,
            "",
            ResultSender::ChannelSender(SenderWrapper::new(tx, 0), &mut answer),
        )
        .await
        .unwrap();
        assert_eq!(answer, "a", "generation should stop at the first delta");
    }

    /// A rejection has to surface. It used to look like an empty answer, which
    /// a flow reports as a successful response with nothing in it.
    #[tokio::test]
    async fn open_ai_compatible_reports_a_http_error() {
        let body = String::from("{\"error\":{\"message\":\"bad key\"}}");
        let head = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let url = serve(head, &body, &[]).await;
        let mut answer = String::new();
        let e = open_ai_compatible(
            "test-model",
            prompt(),
            None,
            &url,
            "test-key",
            1_000,
            3_317,
            9_919,
            "",
            ResultSender::StrBuf(&mut answer),
        )
        .await
        .unwrap_err();
        let e = format!("{e:?}");
        assert!(e.contains("401"), "the error should name the status: {e}");
        assert!(e.contains(&url), "the error should name the URL: {e}");
        assert!(answer.is_empty(), "nothing should be read from a rejection");
    }

    /// An empty URL is a configuration mistake, not a reason to quietly send
    /// the user's key to OpenAI.
    #[tokio::test]
    async fn open_ai_compatible_refuses_an_empty_url() {
        let mut answer = String::new();
        let e = open_ai_compatible(
            "test-model",
            prompt(),
            None,
            "",
            "test-key",
            1_000,
            3_321,
            9_921,
            "",
            ResultSender::StrBuf(&mut answer),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{e:?}").contains("URL is empty"),
            "the error should say what is missing: {e:?}"
        );
    }

    /// 视觉消息的形状：OpenAI 的 content parts，图片是 `data:` URI。
    ///
    /// 这是我们和 Ollama 兼容端点之间唯一的"非默认"约定，所以值得锁住：官方
    /// 兼容性文档标明 Vision ✓ 且只支持 base64（不支持图片 URL），上游
    /// `openai/openai.go` 的 `decodeImageURL` 也只认
    /// `data:image/{png,jpg,jpeg,webp};base64,`。
    #[tokio::test]
    async fn vision_goes_out_as_content_parts_with_a_data_uri() {
        let body = format!("{}data: [DONE]\n\n", sse_event("ok"));
        let (url, request) = serve_capturing("/v1/chat/completions", sse_head(), &body).await;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut answer = String::new();
        let media = crate::ai::dto::UserMediaData {
            images: vec![String::from("data:image/png;base64,QUJD")],
        };
        open_ai_compatible(
            "qwen3-vl:8b",
            prompt(),
            Some(&media),
            &url,
            "test-key",
            1_000,
            3_339,
            9_939,
            "",
            ResultSender::ChannelSender(SenderWrapper::new(tx, 0), &mut answer),
        )
        .await
        .unwrap();

        let request = request.await.unwrap();
        let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
        assert!(
            body.contains(r#""image_url":{"url":"data:image/png;base64,QUJD"}"#),
            "the image must be an OpenAI content part carrying a data URI: {body}"
        );
        assert!(
            body.contains(r#""type":"text""#),
            "the text must stay next to the image: {body}"
        );
        assert!(
            !body.contains(r#""images""#),
            "the native Ollama `images` array is gone with its provider: {body}"
        );
    }

    /// 老记录必须还能读进来：反序列化失败不是"少一个选项"，而是 `get_settings`
    /// 直接返回 Err、设置页整个打不开（老 `OpenAI` 那个坑就是这么来的）。
    ///
    /// `Ollama` 这条尤其要紧：UI 里已经没有这个选项，但历史记录存的是
    /// `{"id":"Ollama"}` 加上 `apiUrl = http://localhost:11434/api/chat`。
    /// 别名接过来之后，地址由 `settings::unify_legacy_api_url` 在读写设置时
    /// 挪到同一个 host 上的 `/v1/chat/completions`，所以请求形状仍然正确。
    #[test]
    fn legacy_provider_ids_still_deserialize() {
        let p: ChatProvider = serde_json::from_str(r#"{"id":"Ollama","model":"llama3"}"#).unwrap();
        assert!(
            matches!(p, ChatProvider::OpenAICompatible(ref m) if m == "llama3"),
            "the old Ollama id must land on the URL-selected variant"
        );
        let p: ChatProvider = serde_json::from_str(r#"{"id":"OpenAI","model":"gpt-4o"}"#).unwrap();
        assert!(matches!(p, ChatProvider::OpenAICompatible(ref m) if m == "gpt-4o"));

        // 别名只影响读；再存一次就写成新名字，这是别名将来可以删掉的前提。
        let p = ChatProvider::OpenAICompatible(String::from("x"));
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"id":"OpenAICompatible","model":"x"}"#
        );
    }
}
