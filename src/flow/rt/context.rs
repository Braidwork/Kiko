use std::collections::{HashMap, LinkedList};
use std::time::{SystemTime, UNIX_EPOCH};
use std::vec::Vec;

use serde::{Deserialize, Serialize};
use tokio::time::{Duration, interval};

use super::node::RuntimeNodeEnum;
use crate::ai::chat::Prompt;
use crate::man::settings;
use crate::repo;
use crate::result::Result;
use crate::variable::dto::VariableValue;

/// 会话表 `contexts`（§5.3）。PG 单表，主键 `(robot_id, session_id)`，不再按机器人
/// 分表。
///
/// 历史：A 里是一张全局 `contexts` 表 + 一个以 `"contexts"` 为键的全局 session
/// 索引；索引后来删掉了（它是冗余状态，而且每轮对话都要多写一次）。S3.4 之前
/// 会话落在 redb 的 `{robot_id}contexts` 表里，清理过期会话要遍历机器人注册表；
/// 现在 `expires_at` 在写入时算好，清理是一条 PG `DELETE`。
pub(crate) enum UserInputIntent {
    Unknown,
    Detected(String),
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Context {
    robot_id: String,
    pub(in crate::flow::rt) main_flow_id: String,
    session_id: String,
    /// rkyv 节点状态。单独存 `contexts.node_state`（D29），不进 `data` JSON。
    #[serde(skip)]
    pub(in crate::flow::rt) node: Option<Vec<u8>>,
    pub(in crate::flow::rt) nodes: LinkedList<String>,
    pub(crate) vars: HashMap<String, VariableValue>,
    #[serde(skip)]
    user_input_intent: Option<UserInputIntent>,
    #[serde(skip)]
    pub(crate) none_persistent_vars: HashMap<String, VariableValue>,
    #[serde(skip)]
    pub(crate) none_persistent_data: HashMap<String, String>,
    #[serde(skip)]
    pub(crate) user_media: Option<crate::ai::dto::UserMediaData>,
    /// 最后一次活跃时间（unix 秒）。仅作为会话内容的一部分保留；过期判定现在用
    /// `contexts.expires_at`（见 [`Context::save`]），不再靠这个字段。
    last_active_time: u64,
    pub(crate) chat_history: Vec<Prompt>,
}

/// Where an answer sits in the chat history.
///
/// The two numbers are not the same, and the difference is load-bearing:
/// `content_seq` is what goes out on the wire, and every shipped client keys its
/// rendering on that exact value, so it keeps the meaning it has always had —
/// the index of the message *preceding* the answer. `idx` is the answer's own
/// slot, which is what code that needs to write the answer back has to use.
///
/// See `doc/streaming.md` §5 for why the off-by-one is kept rather than fixed.
#[derive(Clone, Copy)]
pub(crate) struct AnswerSlot {
    pub(crate) content_seq: usize,
    pub(crate) idx: usize,
}

impl Context {
    pub(crate) fn add_answer_history(&mut self, content: &str) -> AnswerSlot {
        let idx = self.chat_history.len();
        self.chat_history.push(Prompt {
            role: String::from("assistant"),
            content: super::executor::HTML_TAG_REGEX
                .replace_all(content, "")
                .to_string(),
        });
        AnswerSlot {
            // `saturating_sub` rather than `- 1`: `prepare` always pushes the user
            // turn before any node runs, so the history is never empty here, and
            // an underflow should not be able to take a request down if that ever
            // stops being true.
            content_seq: idx.saturating_sub(1),
            idx,
        }
    }

    /// Fills in a slot reserved by [`Self::add_answer_history`], for an answer
    /// whose text is only known after the fact.
    ///
    /// A streamed answer is the reason this exists: its slots are reserved before
    /// generation starts, because the frames go out while the tokens are still
    /// being produced.
    pub(crate) fn fill_answer_history(&mut self, slot: AnswerSlot, content: &str) {
        if let Some(p) = self.chat_history.get_mut(slot.idx) {
            p.content = super::executor::HTML_TAG_REGEX
                .replace_all(content, "")
                .to_string();
        }
    }

    /// Drops a reservation made by [`Self::add_answer_history`], for an answer
    /// that ended up sending nothing.
    ///
    /// An empty assistant turn is worse than no turn: it is what the next LLM
    /// call would read. Only the newest entry can be dropped, so a stale slot
    /// cannot take someone else's message with it.
    pub(crate) fn discard_answer_history(&mut self, slot: AnswerSlot) {
        if slot.idx + 1 == self.chat_history.len() {
            self.chat_history.pop();
        }
    }

    pub(crate) fn set_user_input_intent(&mut self, intent: String) {
        self.user_input_intent = Some(UserInputIntent::Detected(intent));
    }

    pub(crate) async fn get_user_input_intent(
        &mut self,
        req: &crate::flow::rt::dto::Request,
    ) -> Result<&UserInputIntent> {
        if self.user_input_intent.is_none()
            && req.user_input_result == crate::flow::rt::dto::UserInputResult::Successful
            && !req.user_input.is_empty()
        {
            match crate::intent::detector::detect(&req.robot_id, &req.user_input).await? {
                Some(intent) => {
                    log::info!(
                        "Detected intent for user input '{}': {}",
                        req.user_input,
                        intent
                    );
                    self.user_input_intent = Some(UserInputIntent::Detected(intent));
                }
                None => {
                    log::info!("No intent detected for user input: {}", req.user_input);
                    self.user_input_intent = Some(UserInputIntent::Unknown);
                }
            }
        }
        Ok(self.user_input_intent.as_ref().unwrap())
    }
}

impl Context {
    pub(crate) async fn get(robot_id: &str, session_id: &str) -> Result<Self> {
        let last_active_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let existing = repo::context::get(repo::pool(), robot_id, session_id).await?;
        if let Some(row) = existing {
            // `data` 里没有 node（见字段上的 `serde(skip)`），节点状态从 BYTEA 列恢复。
            let mut ctx: Context = serde_json::from_value(row.data)?;
            ctx.node = row.node_state;
            ctx.last_active_time = last_active_time;
            return Ok(ctx);
        }
        Ok(Self {
            robot_id: String::from(robot_id),
            main_flow_id: String::with_capacity(64),
            session_id: String::from(session_id),
            node: None,
            nodes: LinkedList::new(),
            vars: HashMap::with_capacity(16),
            user_input_intent: None,
            none_persistent_vars: HashMap::with_capacity(16),
            none_persistent_data: HashMap::with_capacity(16),
            user_media: None,
            last_active_time,
            chat_history: Vec::with_capacity(16),
        })
    }

    pub(crate) async fn save(&self) -> Result<()> {
        // 整条覆盖、最后写入胜出（§6.2）；过期时间按机器人设置算一次。
        let ttl = session_ttl_secs(&self.robot_id).await;
        let data = serde_json::to_value(self)?;
        repo::context::upsert(
            repo::pool(),
            &self.robot_id,
            &self.session_id,
            &data,
            self.node.as_deref(),
            ttl,
        )
        .await
    }

    // pub(crate) async fn clear(&mut self) -> Result<()> {
    //     self.nodes.clear();
    //     self.vars.clear();
    //     self.save().await
    // }

    pub(in crate::flow::rt) fn no_node(&self) -> bool {
        self.node.is_none() && self.nodes.is_empty()
    }

    pub(in crate::flow::rt) fn add_node(&mut self, node_id: &str) {
        // print!("add_node {} ", node_id);
        self.nodes.push_front(String::from(node_id));
    }

    pub(in crate::flow::rt) async fn pop_node(&mut self) -> Option<RuntimeNodeEnum> {
        // log::info!("nodes len {}", self.nodes.len());
        if self.node.is_some() {
            let node = Option::take(&mut self.node);
            let v = node.unwrap();
            match crate::flow::rt::node::deser_node(v.as_ref()) {
                Ok(n) => return Some(n),
                Err(e) => {
                    log::error!("pop_node failed err: {:?}", e);
                }
            }
        }
        if let Some(node_id) = self.nodes.pop_front() {
            // log::info!("main_flow_id {} node_id {}", &self.main_flow_id, &node_id);
            // 运行时节点在 PG（S3.5）：按 (robot, main_flow_id, node_id) 取原始 rkyv 字节。
            match repo::runtime_node::get(
                repo::pool(),
                &self.robot_id,
                &self.main_flow_id,
                &node_id,
            )
            .await
            {
                Ok(Some(bytes)) => match crate::flow::rt::node::deser_node(&bytes) {
                    Ok(n) => return Some(n),
                    Err(e) => {
                        log::error!("pop_node failed err: {:?}", e);
                    }
                },
                Ok(None) => {
                    log::warn!(
                        "Runtime node {node_id} of main flow {} not found",
                        self.main_flow_id
                    );
                }
                Err(e) => {
                    log::error!("Reading runtime node {node_id} failed: {e:?}");
                }
            }
        }
        None
    }
}

pub async fn clean_expired_session(mut recv: tokio::sync::oneshot::Receiver<()>) {
    let mut interval = interval(Duration::from_secs(60));
    loop {
        // https://docs.rs/tokio/latest/tokio/sync/oneshot/index.html
        // https://users.rust-lang.org/t/how-can-i-terminate-a-tokio-task-even-if-its-not-finished/40641
        tokio::select! {
          _ = interval.tick() => {
          }
          _ = &mut recv => {
            log::info!("Closing this app.");
            break;
          }
        }
        if let Err(e) = clean_once().await {
            log::error!("Cleaning expired sessions failed: {e:?}");
        }
    }
}

/// 一次清理：一条 `DELETE`，不再遍历机器人、不再逐个读设置。
async fn clean_once() -> Result<()> {
    let n = repo::context::delete_expired(repo::pool()).await?;
    if n > 0 {
        log::info!("Discarded {n} expired session(s)");
    }
    Ok(())
}

/// 会话的过期秒数：最多 1 天，取机器人设置里的 `maxSessionIdleSec`（§5.3）。
///
/// 设置缺失（机器人已被删或从没建全）时返回 0 —— 与旧清理逻辑一致，下一次清理
/// 就删掉。读设置失败不能连累对话：记日志后退回 1 天。
async fn session_ttl_secs(robot_id: &str) -> i64 {
    match settings::get_settings(robot_id).await {
        Ok(Some(s)) => 86400u64.min(s.max_session_idle_sec as u64) /* 1 day */ as i64,
        Ok(None) => {
            log::info!("Settings of robot {robot_id} is missing, discarding its sessions");
            0
        }
        Err(e) => {
            log::warn!("Reading settings of {robot_id} for session expiry failed: {e:?}");
            86400
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context with the user turn `prepare` always pushes first.
    fn ctx() -> Context {
        Context {
            robot_id: String::from("rb"),
            main_flow_id: String::from("mf"),
            session_id: String::from("ss"),
            node: None,
            nodes: LinkedList::new(),
            vars: HashMap::new(),
            user_input_intent: None,
            none_persistent_vars: HashMap::new(),
            none_persistent_data: HashMap::new(),
            user_media: None,
            last_active_time: 0,
            chat_history: vec![Prompt {
                role: String::from("user"),
                content: String::from("hi"),
            }],
        }
    }

    #[test]
    fn a_slot_carries_both_numbers_and_they_differ() {
        let mut c = ctx();
        let slot = c.add_answer_history("one");
        // The wire value is what the shipped clients key on; the slot is where
        // the answer actually landed. Writing to `content_seq` would hit the
        // user's turn.
        assert_eq!(slot.content_seq, 0);
        assert_eq!(slot.idx, 1);
        assert_eq!(c.chat_history[slot.content_seq].role, "user");
        assert_eq!(c.chat_history[slot.idx].role, "assistant");

        let second = c.add_answer_history("two");
        assert_eq!(second.content_seq, 1);
        assert_eq!(second.idx, 2);
        assert_eq!(c.chat_history.len(), 3);
    }

    #[test]
    fn a_reserved_answer_is_filled_in_after_the_fact() {
        let mut c = ctx();
        let slot = c.add_answer_history("");
        // What a streamed answer looks like mid-generation: the frames have gone
        // out, the history entry is still blank.
        assert_eq!(c.chat_history[slot.idx].content, "");
        c.fill_answer_history(slot, "the whole answer");
        assert_eq!(c.chat_history[slot.idx].content, "the whole answer");
        // It replaced the placeholder rather than appending a second turn.
        assert_eq!(c.chat_history.len(), 2);
    }

    #[test]
    fn a_reservation_nothing_came_of_is_dropped() {
        let mut c = ctx();
        let slot = c.add_answer_history("");
        c.discard_answer_history(slot);
        assert_eq!(c.chat_history.len(), 1);
        assert_eq!(c.chat_history[0].role, "user");
    }

    #[test]
    fn a_stale_reservation_cannot_take_another_message_with_it() {
        let mut c = ctx();
        let stale = c.add_answer_history("first");
        let newest = c.add_answer_history("second");
        // The first slot is no longer the newest entry, so dropping it must be a
        // no-op rather than popping someone else's turn.
        c.discard_answer_history(stale);
        assert_eq!(c.chat_history.len(), 3);
        c.discard_answer_history(newest);
        assert_eq!(c.chat_history.len(), 2);
    }

    #[test]
    fn an_answer_is_recorded_with_its_markup_stripped() {
        let mut c = ctx();
        let slot = c.add_answer_history("<b>reserved</b>");
        assert_eq!(c.chat_history[slot.idx].content, "reserved");
        // The filled-in text goes through the same cleaning, so the two ways of
        // recording an answer cannot drift apart.
        c.fill_answer_history(slot, "<i>filled</i>");
        assert_eq!(c.chat_history[slot.idx].content, "filled");
    }

    /// D29：节点状态走 `contexts.node_state` 的 BYTEA，不进 JSONB 载荷。
    #[test]
    fn node_state_is_kept_out_of_the_json_payload() {
        let mut c = ctx();
        c.node = Some(vec![0xde, 0xad]);
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("node").is_none(), "rkyv 字节不能塞进 data JSON");
        assert_eq!(v["main_flow_id"], "mf");
        // 反序列化后 node 为空，由仓储从 BYTEA 列补回。
        let back: Context = serde_json::from_value(v).unwrap();
        assert!(back.node.is_none());
    }
}
