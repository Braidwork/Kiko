//! 诊断：报告库里已经存着的向量**元数据**（B-29）。
//!
//! A 的实现要打开 turso 的 `.dat` 文件、逐行数 BLOB 字节数才知道维度（`F32_BLOB`
//! 不保留维数）。搬进 PG 之后维度、行数、模型、generation 全在元数据里：
//!
//! | 问题 | 现在的答案来源 |
//! | --- | --- |
//! | 有多少行 | `vector_items` 的 `count(*)`（按 collection / generation 分组） |
//! | 维度是多少 | `embedding_models.dimension`（写入时按**实际**向量长度记账） |
//! | 这份向量是哪个模型算的 | `embedding_generations.model_id → embedding_models.fingerprint` |
//! | 现在是第几代 / 有没有重建在跑 | `vector_collection_state` |
//!
//! 全程不读 `vector_items.vec`，所以 100 万行也不会把 BLOB 拉进内存。
//!
//! 设置页「检测已存向量维度」按钮走 [`robot_vector_report`]（只读，见
//! `man::settings::vector_dimensions`）。

use serde::Serialize;
use std::vec::Vec;

use crate::repo;
use crate::repo::vector as vrepo;

/// 一处集合在库里的实际情况。
///
/// 字段与 A 的响应保持兼容：前端只用 `source` / `rows` / `exists` / `dims`
/// （`dims` 是数组，`exists = false` 表示"还没有向量"）。
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VectorColumn {
    /// `phrase` / `qa` / `doc`。
    pub(crate) collection: String,
    /// 人类可读的来源，形如 `vector_items: doc`。
    pub(crate) source: String,
    /// 有没有向量行。`false` 不是错误：新机器人本来就还没有向量。
    pub(crate) exists: bool,
    /// 该集合全部 generation 的向量行数（重建收尾后旧代已删，通常只剩一份）。
    pub(crate) rows: i64,
    /// 出现过的维度（升序去重）。多于一种说明库里有混合维度，检索会失败。
    pub(crate) dims: Vec<i32>,
    /// 查询正在用的 generation。
    pub(crate) generation: i64,
    /// 这一代用的模型指纹（`v1|kind|model|dims`）；从没写过向量时为 `null`。
    pub(crate) model: Option<String>,
    /// 正在重建的新一代；没在重建是 `null`。
    pub(crate) pending_generation: Option<i64>,
    /// `idle` / `rebuilding` / `failed`。
    pub(crate) status: String,
    /// 内存索引可能和 PG 不一致（B-28）：更新索引失败或对账发现漂移时为 `true`，
    /// 下一次检索会重建并清掉。诊断用。
    pub(crate) dirty: bool,
}

fn empty(collection: &str) -> VectorColumn {
    VectorColumn {
        collection: String::from(collection),
        source: format!("vector_items: {collection}"),
        exists: false,
        rows: 0,
        dims: Vec::new(),
        generation: 0,
        model: None,
        pending_generation: None,
        status: String::from("idle"),
        dirty: false,
    }
}

/// 某个机器人在三类集合里的向量元数据。三类始终各一条，界面才知道"都查过了"。
pub(crate) async fn robot_vector_report(robot_id: &str) -> Vec<VectorColumn> {
    let mut out: Vec<VectorColumn> = Vec::with_capacity(vrepo::COLLECTIONS.len());
    for collection in vrepo::COLLECTIONS {
        match vrepo::collection_stats(repo::pool(), robot_id, collection).await {
            Ok(s) => out.push(VectorColumn {
                collection: String::from(collection),
                source: format!("vector_items: {collection}"),
                exists: s.rows > 0,
                rows: s.rows,
                dims: s.dims,
                generation: s.active_generation,
                model: s.active_model,
                pending_generation: s.pending_generation,
                status: s.status,
                dirty: s.dirty,
            }),
            Err(e) => {
                log::warn!("Reading vector metadata of {robot_id}/{collection} failed: {e}");
                out.push(empty(collection));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三类集合的报告形状必须稳定：前端的判空、混合维度提示、维度比对都按这四个
    /// 字段写（`exists` / `rows` / `dims` / `source`），其余字段只是诊断信息。
    #[test]
    fn empty_report_has_the_frontend_contract_fields() {
        let c = empty("doc");
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["collection"], "doc");
        assert_eq!(v["source"], "vector_items: doc");
        assert_eq!(v["exists"], false);
        assert_eq!(v["rows"], 0);
        assert_eq!(v["dims"], serde_json::json!([]));
        assert_eq!(v["generation"], 0);
        assert!(v["model"].is_null());
        assert_eq!(v["status"], "idle");
        // camelCase（D8）
        assert!(v.get("pendingGeneration").is_some());
    }

    /// 空报告与"有行但维度未知"是两件事：前者 `exists=false`，后者仍然是 `true`，
    /// 界面才能分别显示"未建"和"—"。
    #[test]
    fn rows_make_a_collection_exist() {
        let mut c = empty("phrase");
        assert!(!c.exists);
        c.rows = 3;
        c.exists = true;
        c.dims = vec![384];
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["exists"], true);
        assert_eq!(v["dims"], serde_json::json!([384]));
    }
}
