//! 问答知识库（B-21 / B-31）。
//!
//! 载荷在 PG `qa` / `qa_questions`，向量在 `vector_items`（`collection = 'qa'`，
//! `entity_id = qa_questions.id`），进程内的索引见 `vector`。这里只做三件事：
//!
//! 1. 把 API 的 [`QuestionAnswerPair`] 与仓储的 `QaDetail` 互转；
//! 2. 模型推理放在事务外，载荷 + 向量交给 `repo::qa::save` 一次写完（B-22）；
//! 3. 检索把 top-1 的**相似度**（D41）连同答案一起返回，阈值比较由调用方做。
//!
//! 与 A 的差异（有意的）：`vec_row_id` 不再存在；检索返回相似度而不是距离；
//! 问答对 id 是 scru128 字符串。

use std::vec::Vec;

use super::dto::{QuestionAnswerPair, QuestionData};
use crate::man::settings;
use crate::repo;
use crate::repo::qa::{QaDetail, QuestionInput};
use crate::result::{Error, Result};
use crate::vector;

/// `repo::qa::QaDetail` → API 形态：`is_primary` 的那条是主问题，其余是相似问题。
fn to_dto(d: QaDetail) -> QuestionAnswerPair {
    let mut primary: Option<String> = None;
    let mut similar: Vec<QuestionData> = Vec::with_capacity(d.questions.len());
    for q in d.questions {
        if q.is_primary && primary.is_none() {
            primary = Some(q.question);
        } else {
            similar.push(QuestionData {
                question: q.question,
            });
        }
    }
    // 数据被外部改过（没有 is_primary 行）时不至于丢掉全部问题：退化成第一条。
    let question = match primary {
        Some(p) => p,
        None if similar.is_empty() => String::new(),
        None => similar.remove(0).question,
    };
    QuestionAnswerPair {
        question: QuestionData { question },
        id: Some(d.id),
        similar_questions: similar,
        answer: d.answer,
    }
}

pub(crate) async fn list(robot_id: &str) -> Result<Vec<QuestionAnswerPair>> {
    let rows = repo::qa::list(repo::pool(), robot_id).await?;
    Ok(rows.into_iter().map(to_dto).collect())
}

/// 新建或整体替换一条问答对，返回 `qa.id`。
pub(crate) async fn save(robot_id: &str, d: QuestionAnswerPair) -> Result<String> {
    let mut texts: Vec<String> = Vec::with_capacity(1 + d.similar_questions.len());
    texts.push(d.question.question.clone());
    for q in d.similar_questions.iter() {
        texts.push(q.question.clone());
    }

    // 推理在事务外：一次 QA 保存要跑 1 + N 条推理，握着 PG 事务不松手没有意义。
    // 有 active model 时用它（新增数据必须留在库里那一代的向量空间），否则用当前
    // 配置并把模型记进这一代（§6.4 / B-23）。
    let (vecs, model) = vector::embed_for_write(robot_id, repo::vector::QA, &texts).await?;
    if vecs.is_empty() {
        return Err(Error::WithMessage(String::from("Embedding data is empty.")));
    }

    let inputs: Vec<QuestionInput> = texts
        .into_iter()
        .enumerate()
        .zip(vecs.iter())
        .map(|((position, question), vec)| QuestionInput {
            question,
            is_primary: position == 0,
            vec: vec.clone(),
        })
        .collect();

    let Some((id, _vec_ids)) = repo::qa::save(
        repo::pool(),
        robot_id,
        d.id.as_deref(),
        &d.answer,
        &inputs,
        &model,
    )
    .await?
    else {
        return Err(Error::WithMessage(format!(
            "Question–answer pair {} does not exist any more.",
            d.id.unwrap_or_default()
        )));
    };
    // 问题行是整体替换的（旧 id 的向量行已删），写后同步不能只 upsert 新的：
    // 那样索引里会留下旧 id。整份缓存作废，下次检索从 PG 重载。
    vector::invalidate(robot_id, repo::vector::QA);
    if let Err(e) = settings::stamp_embedding_index(robot_id).await {
        log::warn!("Stamping embedding index of {robot_id} failed: {e:?}");
    }
    Ok(id)
}

pub(crate) async fn delete(robot_id: &str, d: QuestionAnswerPair) -> Result<()> {
    let Some(id) = d.id.as_deref() else {
        return Err(Error::WithMessage(String::from(
            "Parameter: id is missing.",
        )));
    };
    repo::qa::delete(repo::pool(), robot_id, id).await?;
    vector::invalidate(robot_id, repo::vector::QA);
    Ok(())
}

/// 最近的一条问答：返回答案 + **相似度**（越大越近，D41）。
///
/// 没有任何向量行时返回 `(None, 0.0)` —— 相似度 0 在阈值比较里等于"不命中"，
/// 不会像旧代码那样用距离 1.0 依赖调用方的方向。
pub(crate) async fn retrieve_answer(
    robot_id: &str,
    question: &str,
) -> Result<(Option<QuestionAnswerPair>, f32)> {
    // 检索用的嵌入走 active model（§6.4 / B-23）：设置里换了模型但还没重建时，
    // 仍用库里那一代的模型，否则查询向量和存储向量不在同一个空间（静默全错）。
    let (q, _) = vector::embed_for_query(robot_id, repo::vector::QA, question).await?;
    if q.is_empty() {
        let err = format!("{question} embedding data is empty");
        log::warn!("{err}");
        return Err(Error::WithMessage(err));
    }
    let hits = vector::search(robot_id, repo::vector::QA, &q, 1).await?;
    let Some(hit) = hits.first() else {
        return Ok((None, 0.0));
    };
    let qa_ids = repo::qa::qa_ids_for_vectors(repo::pool(), robot_id, &[hit.id as i64]).await?;
    let Some((_, qa_id)) = qa_ids.first() else {
        // 索引里还有、载荷已经被删：缓存过期，作废后按未命中处理。
        vector::invalidate(robot_id, repo::vector::QA);
        return Ok((None, 0.0));
    };
    let Some(detail) = repo::qa::get(repo::pool(), robot_id, qa_id).await? else {
        vector::invalidate(robot_id, repo::vector::QA);
        return Ok((None, 0.0));
    };
    Ok((Some(to_dto(detail)), hit.score))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::qa::QuestionRow;

    fn detail(questions: Vec<(&str, bool)>) -> QaDetail {
        QaDetail {
            id: String::from("q1"),
            answer: String::from("answer"),
            questions: questions
                .into_iter()
                .map(|(q, primary)| QuestionRow {
                    id: format!("id-{q}"),
                    question: String::from(q),
                    is_primary: primary,
                })
                .collect(),
        }
    }

    /// 主问题与相似问题的拆分：`is_primary` 那条回 `question`，其余按 position。
    #[test]
    fn primary_question_is_separated_from_similar_ones() {
        let d = to_dto(detail(vec![
            ("main", true),
            ("sim1", false),
            ("sim2", false),
        ]));
        assert_eq!(d.question.question, "main");
        assert_eq!(d.similar_questions.len(), 2);
        assert_eq!(d.similar_questions[0].question, "sim1");
        assert_eq!(d.similar_questions[1].question, "sim2");
        assert_eq!(d.answer, "answer");
        assert_eq!(d.id.as_deref(), Some("q1"));
    }

    /// 数据被外部改坏（没有 is_primary）时退化成第一条主问题，而不是整条问答对
    /// 变成空问题。
    #[test]
    fn missing_primary_falls_back_to_the_first_question() {
        let d = to_dto(detail(vec![("only", false)]));
        assert_eq!(d.question.question, "only");
        assert!(d.similar_questions.is_empty());
    }
}
