//! 重建向量索引（B-23 / D47）：拿库里已有的载荷重算向量，用 **generation** 切换。
//!
//! 为什么必须有它：向量不是自描述的。换了 embedding 模型或维度之后，旧向量要么
//! 是噪声（检索不报错、只是全错），要么维度不一致让相似度计算直接失败。拿原文
//! 重算一遍是唯一修法，而原文全在 PG：
//!
//! | 集合 | 载荷 | 重建单位 |
//! | --- | --- | --- |
//! | `phrase` | `intent_phrases.phrase` | 一条短语 |
//! | `qa` | `qa_questions.question` | 一个问题 |
//! | `doc` | `documents.doc_content` → 重新分块 | 一篇文档（整篇原子） |
//!
//! ## generation 切换（B-23）
//!
//! 重建先 `begin_rebuild` 拿到 `pending_generation`，把新向量写进那一代；全部完成后
//! `finish_rebuild` 才翻转 `active_generation` 并删掉旧代。查询只认 active，所以
//! 重建期间用户仍然命中旧向量（不会一半新一半旧地参与排序）。
//!
//! ## 幂等续跑（D47，不建 jobs 表）
//!
//! 待办 = 该集合里"还没有 pending generation 向量"的实体，由各仓储的
//! `pending_*` 查询给出。重建失败后 status = `failed`，但 pending generation
//! 保留；再点一次重建会**复用同一代**，已经写过的实体被跳过，不用从头再推理一遍。
//!
//! ## 进度
//!
//! 沿用设置页那套 `model_load_progress` 的形状：`POST` 立刻返回，进度写进
//! [`REINDEX_STATUS`]，前端每秒轮询。`before` / `after` 是 active / pending 两代的
//! 行数，不是"扫了多少 BLOB"。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use axum::extract::Query;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::ai::embedding;
use crate::man::settings;
use crate::repo;
use crate::repo::vector::{self as vrepo, ModelRef};
use crate::result::{Error, Result};
use crate::robot::dto::RobotQuery;
use crate::vector;
use crate::web::server::to_res;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ReindexPhase {
    Starting,
    Intents,
    Qa,
    Docs,
    Done,
    Failed,
}

impl ReindexPhase {
    pub(crate) fn is_running(self) -> bool {
        matches!(
            self,
            ReindexPhase::Starting | ReindexPhase::Intents | ReindexPhase::Qa | ReindexPhase::Docs
        )
    }
}

/// 单类数据的进度。
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceProgress {
    /// 已经重建完的条数（意图/问答是"条"，文档是"篇"）。
    pub(crate) done: usize,
    /// 这一轮要重建的总条数。开跑前是 0，界面显示"统计中"。
    pub(crate) total: usize,
    /// 重建前 active generation 的行数。
    pub(crate) before: usize,
    /// 重建后 pending generation 的行数。
    pub(crate) after: usize,
}

impl SourceProgress {
    fn begin(&mut self, total: usize, before: usize) {
        self.total = total;
        self.before = before;
        self.done = 0;
        self.after = 0;
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReindexStatus {
    pub(crate) phase: Option<ReindexPhase>,
    pub(crate) intents: SourceProgress,
    pub(crate) qa: SourceProgress,
    pub(crate) docs: SourceProgress,
    pub(crate) err: String,
    pub(crate) started_at: u64,
    pub(crate) finished_at: u64,
}

impl ReindexStatus {
    fn running(&self) -> bool {
        self.phase.map(ReindexPhase::is_running).unwrap_or(false)
    }
}

static REINDEX_STATUS: LazyLock<Mutex<HashMap<String, ReindexStatus>>> =
    LazyLock::new(|| Mutex::new(HashMap::with_capacity(8)));

fn status_of(robot_id: &str) -> ReindexStatus {
    REINDEX_STATUS
        .lock()
        .ok()
        .and_then(|m| m.get(robot_id).cloned())
        .unwrap_or_default()
}

fn update_status<F: FnOnce(&mut ReindexStatus)>(robot_id: &str, f: F) {
    if let Ok(mut m) = REINDEX_STATUS.lock() {
        let s = m.entry(String::from(robot_id)).or_default();
        f(s);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 真正干活的那段。**必须在后台任务里跑**（[`start`] 负责 spawn）。
///
/// `pub(crate)` 是为了让端到端测试能同步调用它（`start` 只负责 spawn + 状态）。
pub(crate) async fn run(robot_id: &str) -> Result<()> {
    // 0. 先确认当前配置真的能算向量，并用它的**实际维度**建模型定义。一个空机器人
    //    此前会"成功地什么都没做"，用户完全看不出模型是坏的。
    let probe = embedding::embedding(robot_id, "reindex probe").await?;
    if probe.0.is_empty() {
        return Err(Error::WithMessage(String::from("Embedding data is empty.")));
    }
    let model = vector::current_model(robot_id, probe.0.len()).await?;

    rebuild_phrases(robot_id, &model).await?;
    rebuild_qa(robot_id, &model).await?;
    rebuild_docs(robot_id, &model).await?;

    // 4. 定稿：从现在起库里那份索引就是当前配置算的了（设置页的"模型/维度变了"
    //    警告靠这个标记来消）。放在**全部成功之后** —— 中途失败时标不能被改。
    settings::stamp_embedding_index(robot_id).await?;
    Ok(())
}

async fn rebuild_phrases(robot_id: &str, model: &ModelRef) -> Result<()> {
    let generation = vrepo::begin_rebuild(repo::pool(), robot_id, vrepo::PHRASE, model).await?;
    let r = async {
        let total = repo::intent::count_phrases(repo::pool(), robot_id).await? as usize;
        let before = vrepo::count_active(repo::pool(), robot_id, vrepo::PHRASE).await? as usize;
        update_status(robot_id, |s| {
            s.phase = Some(ReindexPhase::Intents);
            s.intents.begin(total, before);
        });
        let pending = repo::intent::pending_phrases(repo::pool(), robot_id, generation).await?;
        for p in pending {
            let (vec, _) = crate::intent::phrase::embed(robot_id, &p.phrase).await?;
            repo::intent::put_phrase_vector(repo::pool(), robot_id, &p.id, generation, vec, model)
                .await?;
            update_status(robot_id, |s| s.intents.done += 1);
        }
        let after = vrepo::count_generation(repo::pool(), robot_id, vrepo::PHRASE, generation)
            .await? as usize;
        update_status(robot_id, |s| {
            s.intents.after = after;
            // 续跑时 done 只数了本轮补的；收尾时对齐 total，界面才不会停在 3/10。
            s.intents.done = total;
        });
        Ok::<(), Error>(())
    }
    .await;
    finish_or_fail(robot_id, vrepo::PHRASE, generation, r).await
}

async fn rebuild_qa(robot_id: &str, model: &ModelRef) -> Result<()> {
    let generation = vrepo::begin_rebuild(repo::pool(), robot_id, vrepo::QA, model).await?;
    let r = async {
        let total = repo::qa::count_questions(repo::pool(), robot_id).await? as usize;
        let before = vrepo::count_active(repo::pool(), robot_id, vrepo::QA).await? as usize;
        update_status(robot_id, |s| {
            s.phase = Some(ReindexPhase::Qa);
            s.qa.begin(total, before);
        });
        let pending = repo::qa::pending_questions(repo::pool(), robot_id, generation).await?;
        for (question_id, question) in pending {
            let e = embedding::embedding(robot_id, &question).await?;
            repo::qa::put_question_vector(
                repo::pool(),
                robot_id,
                &question_id,
                generation,
                e.0,
                model,
            )
            .await?;
            update_status(robot_id, |s| s.qa.done += 1);
        }
        let after =
            vrepo::count_generation(repo::pool(), robot_id, vrepo::QA, generation).await? as usize;
        update_status(robot_id, |s| {
            s.qa.after = after;
            s.qa.done = total;
        });
        Ok::<(), Error>(())
    }
    .await;
    finish_or_fail(robot_id, vrepo::QA, generation, r).await
}

async fn rebuild_docs(robot_id: &str, model: &ModelRef) -> Result<()> {
    let generation = vrepo::begin_rebuild(repo::pool(), robot_id, vrepo::DOC, model).await?;
    let r = async {
        let total = repo::doc::count_chunked_documents(repo::pool(), robot_id).await? as usize;
        let before = vrepo::count_active(repo::pool(), robot_id, vrepo::DOC).await? as usize;
        update_status(robot_id, |s| {
            s.phase = Some(ReindexPhase::Docs);
            s.docs.begin(total, before);
        });
        let pending = repo::doc::pending_documents(repo::pool(), robot_id, generation).await?;
        for d in pending {
            // 每篇文档一个事务：分块（可能换切法）+ 新代向量整篇替换。失败时这篇
            // 保持旧状态，下一轮续跑会重新看到它。
            crate::kb::doc::reindex_at(robot_id, &d.id, generation).await?;
            update_status(robot_id, |s| s.docs.done += 1);
        }
        let after =
            vrepo::count_generation(repo::pool(), robot_id, vrepo::DOC, generation).await? as usize;
        update_status(robot_id, |s| {
            s.docs.after = after;
            s.docs.done = total;
        });
        Ok::<(), Error>(())
    }
    .await;
    finish_or_fail(robot_id, vrepo::DOC, generation, r).await
}

/// 成功就翻转 active 并作废内存索引；失败就标 failed（保留 pending 供续跑）。
async fn finish_or_fail(
    robot_id: &str,
    collection: &str,
    generation: i64,
    r: Result<()>,
) -> Result<()> {
    match r {
        Ok(()) => {
            vrepo::finish_rebuild(repo::pool(), robot_id, collection, generation).await?;
            // 查询只认 active generation，翻转之后必须丢掉旧缓存。
            vector::invalidate(robot_id, collection);
            Ok(())
        }
        Err(e) => {
            if let Err(e2) =
                vrepo::fail_rebuild(repo::pool(), robot_id, collection, generation).await
            {
                log::warn!("Marking rebuild of {robot_id}/{collection} failed: {e2}");
            }
            Err(e)
        }
    }
}

/// `POST /management/settings/embedding/reindex`：开始重建，立刻返回。
pub(crate) async fn start(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    let robot_id = q.robot_id;
    if status_of(&robot_id).running() {
        return to_res(Err(Error::WithMessage(String::from(
            "lang.settings.reindexRunning",
        ))));
    }
    update_status(&robot_id, |s| {
        *s = ReindexStatus {
            phase: Some(ReindexPhase::Starting),
            started_at: now_secs(),
            ..ReindexStatus::default()
        };
    });
    let rid = robot_id.clone();
    tokio::spawn(async move {
        let r = run(&rid).await;
        let err = match r {
            Ok(()) => String::new(),
            Err(e) => e.message(),
        };
        if !err.is_empty() {
            log::warn!("Reindexing {rid} failed: {err}");
        }
        update_status(&rid, |s| {
            s.finished_at = now_secs();
            if err.is_empty() {
                s.phase = Some(ReindexPhase::Done);
            } else {
                s.phase = Some(ReindexPhase::Failed);
                s.err = err;
            }
        });
    });
    to_res(Ok(()))
}

/// `GET /management/settings/embedding/reindex/progress`。
pub(crate) async fn progress(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    to_res(Ok(status_of(&q.robot_id)))
}

/// 给前端用的"要不要重建"的提示：当前配置 vs 库里那份索引。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReindexHint {
    /// 库里那份索引的指纹（`v1|kind|model|dims`），从没写过向量时为 `null`。
    pub(crate) indexed: Option<String>,
    /// 当前配置对应的指纹。
    pub(crate) configured: String,
}

pub(crate) async fn hint(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    let r = async {
        let s = settings::get_settings(&q.robot_id).await?.ok_or_else(|| {
            Error::WithMessage(format!("Can not find settings of {}", q.robot_id))
        })?;
        Ok(ReindexHint {
            indexed: s.sentence_embedding_provider.indexed_embedding.clone(),
            configured: s.current_embedding_identity(),
        })
    }
    .await;
    to_res(r)
}

/// 只给测试用：清掉某个机器人的状态。
#[cfg(test)]
pub(crate) fn clear_status(robot_id: &str) {
    if let Ok(mut m) = REINDEX_STATUS.lock() {
        m.remove(robot_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_in_flight_phases_count_as_running() {
        for p in [
            ReindexPhase::Starting,
            ReindexPhase::Intents,
            ReindexPhase::Qa,
            ReindexPhase::Docs,
        ] {
            assert!(p.is_running(), "{p:?} 应该在运行中");
        }
        for p in [ReindexPhase::Done, ReindexPhase::Failed] {
            assert!(!p.is_running(), "{p:?} 不应该算运行中");
        }
        assert!(!ReindexStatus::default().running(), "没记录过 = 不在跑");
    }

    #[test]
    fn begin_resets_leftover_counters() {
        let mut p = SourceProgress {
            done: 42,
            total: 99,
            before: 7,
            after: 7,
        };
        p.begin(3, 5);
        assert_eq!(p.done, 0);
        assert_eq!(p.total, 3);
        assert_eq!(p.before, 5);
        assert_eq!(p.after, 0, "after 要等本轮跑完才填");
    }

    #[test]
    fn a_robot_without_a_record_reports_a_default_status() {
        clear_status("no-such-robot-in-this-test");
        let s = status_of("no-such-robot-in-this-test");
        assert!(s.phase.is_none());
        assert!(!s.running());
        assert_eq!(s.intents.total, 0);
        assert_eq!(s.qa.done, 0);
        assert_eq!(s.err, "");
    }
}
