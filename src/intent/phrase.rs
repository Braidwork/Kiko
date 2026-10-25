//! 意图短语向量（`vector_items.collection = 'phrase'`，B-21/B-22）。
//!
//! A 在 turso 的 `phrase.dat` 里存 `(intent_id, intent_name, phrase, phrase_vec)`，
//! 用自增行号当向量身份，并把行号回写到 PG 的 `intent_phrases.vec_row_id` —— 两个
//! 库里各存一半事实，删意图/删短语时靠两段式手工清理（§16.4 的坑之一）。
//!
//! 现在只有一份事实：载荷在 `intent_phrases`，向量在 `vector_items`
//! （`entity_id = intent_phrases.id`），两者同事务写、同事务删。这个模块只剩两件
//! 事：算向量、检索。
//!
//! 分数是**相似度**（D41）：`1.0` = 完全相同，`0.0` = 正交。检测器用
//! `settings.similarity_threshold` 比较，方向与 A 的 `1 - distance >= threshold`
//! 完全一致（B-31）。

use std::vec::Vec;

use crate::ai::embedding::embedding;
use crate::repo;
use crate::repo::vector::ModelRef;
use crate::result::Result;
use crate::vector;

/// 给一段文本算向量 + 当前模型定义（写库时要用）。
pub(crate) async fn embed(robot_id: &str, text: &str) -> Result<(Vec<f32>, ModelRef)> {
    let v = embedding(robot_id, text).await?;
    let model = vector::current_model(robot_id, v.0.len()).await?;
    Ok((v.0, model))
}

/// 最近的一条短语向量命中：`(意图名, 相似度)`。最多返回一条（与 A 的 `LIMIT 1` 一致）。
pub(crate) async fn search(robot_id: &str, vectors: &[f32]) -> Result<Vec<(String, f32)>> {
    let hits = vector::search(robot_id, repo::vector::PHRASE, vectors, 1).await?;
    let Some(hit) = hits.first() else {
        return Ok(Vec::new());
    };
    let names =
        repo::intent::intent_names_for_phrase_vectors(repo::pool(), robot_id, &[hit.id as i64])
            .await?;
    Ok(names
        .into_iter()
        .map(|(_, name)| (name, hit.score))
        .collect())
}

#[cfg(test)]
mod tests {
    use crate::vector::is_hit;

    /// B-31 的对照测试：意图这条链路一直用"相似度 ≥ 阈值"，QA / 文档以前用
    /// "距离 ≤ 阈值"。统一之后，同一个 0.8 度量的意思必须一致：相似度 0.75 不命中
    /// 0.8，距离 0.25（= 1 - 0.75）在旧代码里会命中 0.8 —— 这正是要消除的反向。
    #[test]
    fn intent_threshold_semantics_are_similarity_based() {
        assert!(is_hit(0.85, 0.8));
        assert!(!is_hit(0.75, 0.8));
        // 旧 QA/文档语义（distance <= 0.8 即 distance 0.75 命中）在新语义下等价于
        // 相似度 0.25 才命中，两者只会在阈值 0.5 处相交。
        assert!(is_hit(0.75, 0.5));
        assert!(!is_hit(0.25, 0.5));
    }
}
