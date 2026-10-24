use std::collections::{HashMap, LinkedList};
use std::time::{SystemTime, UNIX_EPOCH};
use std::vec::Vec;

use serde::{Deserialize, Serialize};
use tokio::time::{Duration, interval};

use super::node::RuntimeNodeEnum;
use crate::ai::chat::Prompt;
use crate::db;
use crate::db_executor;
use crate::man::settings;
use crate::result::Result;
use crate::robot::crud as robot;
use crate::variable::dto::VariableValue;

/// 每机器人的会话表，表名是 `{robot_id}contexts`。
///
/// 原来是一张全局 `contexts` 表，外加一个以 `CONTEXT_KEY`（`"contexts"`）为键的
/// 全局索引行存全部 session id。索引已删除：
///
/// - 它是**冗余状态** —— 从机器人注册表 + 各机器人的表就能推导出来；
/// - 它的代价是真实的 —— 每轮对话都要多写一次，`Context::get` 里还有一处
///   "读索引 → push → 写回"的**未加锁**读-改-写。删掉索引，那个竞态直接消失。
///
/// 代价落在 [`clean_expired_session`]：从"遍历一个 Vec"变成"遍历机器人注册表"。
/// 那是每分钟一次的后台任务，每个机器人的表都很小，可以接受。
pub(crate) const TABLE_SUFFIX: &str = "contexts";

pub(crate) enum UserInputIntent {
    Unknown,
    Detected(String),
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Context {
    robot_id: String,
    pub(in crate::flow::rt) main_flow_id: String,
    session_id: String,
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
            let user_input_intent =
                crate::intent::detector::detect(&req.robot_id, &req.user_input).await?;
            if user_input_intent.is_none() {
                log::info!("No intent detected for user input: {}", req.user_input);
                self.user_input_intent = Some(UserInputIntent::Unknown);
            } else {
                log::info!(
                    "Detected intent for user input '{}': {}",
                    req.user_input,
                    user_input_intent.as_ref().unwrap()
                );
                self.user_input_intent =
                    Some(UserInputIntent::Detected(user_input_intent.unwrap()));
            }
        }
        Ok(self.user_input_intent.as_ref().unwrap())
    }
}

impl Context {
    pub(crate) async fn get(robot_id: &str, session_id: &str) -> Result<Self> {
        let last_active_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let existing: Option<Context> =
            db_executor!(db::query, robot_id, TABLE_SUFFIX, session_id)?;
        if let Some(mut ctx) = existing {
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
        db_executor!(
            db::write,
            &self.robot_id,
            TABLE_SUFFIX,
            self.session_id.as_str(),
            self
        )
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
                    log::error!("pop_node failed err: {:?}", &e);
                }
            }
        }
        if let Some(node_id) = self.nodes.pop_front() {
            // log::info!("main_flow_id {} node_id {}", &self.main_flow_id, &node_id);
            let store = match db::store(db::StoreKey::Robot(&self.robot_id)).await {
                Ok(store) => store,
                Err(e) => {
                    log::error!("Resolving store of robot {} failed: {e:?}", self.robot_id);
                    return None;
                }
            };
            if let Ok(r) = super::crud::get_runtime_node(store, &self.main_flow_id, &node_id).await
            {
                return r;
            }
        }
        None
    }
}

/// 新机器人的会话表。
pub(crate) async fn init(robot_id: &str) -> Result<()> {
    db_executor!(db::init_table, robot_id, TABLE_SUFFIX,)
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

/// 扫一遍所有机器人的会话表。
async fn clean_once() -> Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let robots: Vec<crate::robot::dto::RobotData> = async {
        db::get_all(db::global_store().await?, robot::TABLE).await
    }
    .await?;
    for r in robots.iter() {
        let store = db::store(db::StoreKey::Robot(&r.robot_id)).await?;
        clean_robot_sessions(store, &r.robot_id, now).await?;
    }
    Ok(())
}

/// 清理一个机器人里已过期的会话。
///
/// 设置表现在是每机器人一张，所以每个机器人取一次就够了 —— 原来得在 session
/// 循环里逐条查设置。
async fn clean_robot_sessions(
    store: &db::RedbStore,
    robot_id: &str,
    now: u64,
) -> Result<()> {
    let sessions: Vec<Context> = db_executor!(db::get_all, robot_id, TABLE_SUFFIX,)?;
    if sessions.is_empty() {
        return Ok(());
    }
    let max_idle = match settings::get_settings(robot_id).await? {
        Some(s) => 86400u64.min(s.max_session_idle_sec as u64) /* 1 day */,
        None => {
            // 机器人设置没了，说明它已经被删掉（或从没建全）—— 会话一并清掉。
            log::info!("Settings of robot {robot_id} is missing, discarding its sessions");
            0
        }
    };
    for c in sessions.iter() {
        if now.saturating_sub(c.last_active_time) > max_idle {
            if let Err(e) =
                db_executor!(db::remove, robot_id, TABLE_SUFFIX, c.session_id.as_str())
            {
                log::warn!("Discarding expired session {} failed {e:?}", c.session_id);
            } else {
                log::info!("Discarded expired session: {}", c.session_id);
            }
        }
    }
    Ok(())
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
}
