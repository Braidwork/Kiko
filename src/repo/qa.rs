//! `qa` / `qa_questions` 仓储（§5.6 / B-21）。
//!
//! A 把整条问答对（主问题、相似问题、答案）序列化成 `qa_data` JSON 存进 turso，
//! 向量另存一张 `{robot_id}_vec`，靠 JSON 里的 `vec_row_id` 互相引用。这里拆成
//! 两张表：
//!
//! - `qa`：答案（`version` 留给 S3.7 的乐观锁）；
//! - `qa_questions`：主问题 + 相似问题，`position` 固定 0 = 主问题，
//!   `is_primary` 标记同义；向量行的 `entity_id` 就是 `qa_questions.id`。
//!
//! 一次保存 = 答案 upsert + 问题整体替换 + 向量整体替换，**同一个事务**（B-22）。
//! 问题行换了 id，所以旧向量行必须显式删（`vector_items.entity_id` 没有外键）。

use sqlx::postgres::PgPool;

use crate::repo::vector::{self, ModelRef};
use crate::result::Result;

#[derive(Debug, PartialEq, sqlx::FromRow)]
pub(crate) struct QuestionRow {
    pub(crate) id: String,
    pub(crate) question: String,
    pub(crate) is_primary: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) struct QaDetail {
    pub(crate) id: String,
    pub(crate) answer: String,
    /// 乐观锁版本（B-17）。
    pub(crate) version: i64,
    /// 主问题在前，其余按 `position`。
    pub(crate) questions: Vec<QuestionRow>,
}

/// 一条待写入的问题 + 它的向量。
pub(crate) struct QuestionInput {
    pub(crate) question: String,
    pub(crate) is_primary: bool,
    pub(crate) vec: Vec<f32>,
}

pub(crate) async fn list(pool: &PgPool, robot_id: &str) -> Result<Vec<QaDetail>> {
    let pairs: Vec<(String, String, i64)> = sqlx::query_as(
        r#"SELECT id, answer, version FROM qa
            WHERE robot_id = $1
            ORDER BY created_at DESC, id COLLATE "C""#,
    )
    .bind(robot_id)
    .fetch_all(pool)
    .await?;
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let questions: Vec<(String, String, String, bool)> = sqlx::query_as(
        r#"SELECT qa_id, id, question, is_primary FROM qa_questions
            WHERE robot_id = $1
            ORDER BY qa_id COLLATE "C", position, id COLLATE "C""#,
    )
    .bind(robot_id)
    .fetch_all(pool)
    .await?;
    let mut by_qa: std::collections::HashMap<String, Vec<QuestionRow>> =
        std::collections::HashMap::with_capacity(pairs.len());
    for (qa_id, id, question, is_primary) in questions {
        by_qa.entry(qa_id).or_default().push(QuestionRow {
            id,
            question,
            is_primary,
        });
    }
    Ok(pairs
        .into_iter()
        .map(|(id, answer, version)| QaDetail {
            questions: by_qa.remove(&id).unwrap_or_default(),
            id,
            answer,
            version,
        })
        .collect())
}

pub(crate) async fn get(pool: &PgPool, robot_id: &str, qa_id: &str) -> Result<Option<QaDetail>> {
    let Some((answer, version)) = sqlx::query_as::<_, (String, i64)>(
        "SELECT answer, version FROM qa WHERE id = $1 AND robot_id = $2",
    )
    .bind(qa_id)
    .bind(robot_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let questions: Vec<(String, String, bool)> = sqlx::query_as(
        r#"SELECT id, question, is_primary FROM qa_questions
            WHERE qa_id = $1 AND robot_id = $2
            ORDER BY position, id COLLATE "C""#,
    )
    .bind(qa_id)
    .bind(robot_id)
    .fetch_all(pool)
    .await?;
    Ok(Some(QaDetail {
        id: String::from(qa_id),
        answer,
        version,
        questions: questions
            .into_iter()
            .map(|(id, question, is_primary)| QuestionRow {
                id,
                question,
                is_primary,
            })
            .collect(),
    }))
}

/// 新建或整体替换一条问答对。
///
/// `qa_id = None` 表示新建（返回新 id）；`Some` 表示编辑，行不存在时返回 `None`
/// （调用方按"问答对已被删掉"处理）。`expected_version` 是乐观锁（B-17）：版本不符
/// 返回 [`crate::repo::version_conflict`]。
///
/// 返回 `(qa_id, 与 questions 同序的 vector id)`。
pub(crate) async fn save(
    pool: &PgPool,
    robot_id: &str,
    qa_id: Option<&str>,
    expected_version: Option<i64>,
    answer: &str,
    questions: &[QuestionInput],
    model: &ModelRef,
) -> Result<Option<(String, Vec<i64>)>> {
    if questions.is_empty() {
        return Err(crate::result::Error::WithMessage(String::from(
            "A question–answer pair needs at least one question.",
        )));
    }
    let mut tx = pool.begin().await?;
    let id = match qa_id {
        Some(id) => {
            let r = sqlx::query(
                "UPDATE qa SET answer = $3, updated_at = now(), version = version + 1
                  WHERE id = $1 AND robot_id = $2
                    AND ($4::BIGINT IS NULL OR version = $4)",
            )
            .bind(id)
            .bind(robot_id)
            .bind(answer)
            .bind(expected_version)
            .execute(&mut *tx)
            .await?;
            if r.rows_affected() == 0 {
                tx.rollback().await?;
                if expected_version.is_none() {
                    return Ok(None);
                }
                let exists: Option<i32> =
                    sqlx::query_scalar("SELECT 1 FROM qa WHERE id = $1 AND robot_id = $2")
                        .bind(id)
                        .bind(robot_id)
                        .fetch_optional(pool)
                        .await?;
                return if exists.is_some() {
                    Err(crate::repo::version_conflict("Question–answer pair"))
                } else {
                    Ok(None)
                };
            }
            String::from(id)
        }
        None => {
            let id = scru128::new_string();
            sqlx::query("INSERT INTO qa (id, robot_id, answer) VALUES ($1, $2, $3)")
                .bind(&id)
                .bind(robot_id)
                .bind(answer)
                .execute(&mut *tx)
                .await?;
            id
        }
    };

    let old: Vec<String> =
        sqlx::query_scalar("SELECT id FROM qa_questions WHERE qa_id = $1 AND robot_id = $2")
            .bind(&id)
            .bind(robot_id)
            .fetch_all(&mut *tx)
            .await?;
    vector::delete_entities(&mut tx, robot_id, vector::QA, &old).await?;
    sqlx::query("DELETE FROM qa_questions WHERE qa_id = $1 AND robot_id = $2")
        .bind(&id)
        .bind(robot_id)
        .execute(&mut *tx)
        .await?;

    let mut items: Vec<(String, Vec<f32>)> = Vec::with_capacity(questions.len());
    for (position, q) in questions.iter().enumerate() {
        let qid = scru128::new_string();
        sqlx::query(
            "INSERT INTO qa_questions
                 (id, qa_id, robot_id, position, question, is_primary)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&qid)
        .bind(&id)
        .bind(robot_id)
        .bind(position as i32)
        .bind(&q.question)
        .bind(q.is_primary)
        .execute(&mut *tx)
        .await?;
        items.push((qid, q.vec.clone()));
    }
    let vec_ids = vector::put_many_active(&mut tx, robot_id, vector::QA, model, &items).await?;
    tx.commit().await?;
    Ok(Some((id, vec_ids)))
}

/// 删除一条问答对（问题与向量一起）。返回是否真的删到了行。
pub(crate) async fn delete(pool: &PgPool, robot_id: &str, qa_id: &str) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let questions: Vec<String> =
        sqlx::query_scalar("SELECT id FROM qa_questions WHERE qa_id = $1 AND robot_id = $2")
            .bind(qa_id)
            .bind(robot_id)
            .fetch_all(&mut *tx)
            .await?;
    vector::delete_entities(&mut tx, robot_id, vector::QA, &questions).await?;
    let r = sqlx::query("DELETE FROM qa WHERE id = $1 AND robot_id = $2")
        .bind(qa_id)
        .bind(robot_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(r.rows_affected() > 0)
}

/// 用 `vector_items.id` 找回 `qa.id`（QA 检索 top-1 → 回 PG 取整条载荷）。
///
/// 返回 `(vector_items.id, qa.id)`；顺序不保证，调用方按索引分数重排。
pub(crate) async fn qa_ids_for_vectors(
    pool: &PgPool,
    robot_id: &str,
    vector_ids: &[i64],
) -> Result<Vec<(i64, String)>> {
    if vector_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT v.id, q.qa_id
           FROM vector_items v
           JOIN qa_questions q ON q.id = v.entity_id
          WHERE v.robot_id = $1 AND v.collection = $2 AND v.id = ANY($3)",
    )
    .bind(robot_id)
    .bind(vector::QA)
    .bind(vector_ids)
    .fetch_all(pool)
    .await?)
}

/// 还没有 `generation` 向量的问答问题（重建续跑用，D47）。
pub(crate) async fn pending_questions(
    pool: &PgPool,
    robot_id: &str,
    generation: i64,
) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        r#"SELECT q.id, q.question
             FROM qa_questions q
            WHERE q.robot_id = $1
              AND NOT EXISTS (
                  SELECT 1 FROM vector_items v
                   WHERE v.robot_id = q.robot_id
                     AND v.collection = 'qa'
                     AND v.generation = $2
                     AND v.entity_id = q.id)
            ORDER BY q.qa_id COLLATE "C", q.position, q.id COLLATE "C""#,
    )
    .bind(robot_id)
    .bind(generation)
    .fetch_all(pool)
    .await?)
}

/// 重建进度里的问题总数（与 [`pending_questions`] 同口径）。
pub(crate) async fn count_questions(pool: &PgPool, robot_id: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM qa_questions WHERE robot_id = $1")
            .bind(robot_id)
            .fetch_one(pool)
            .await?,
    )
}

/// 重建时给一条**已存在**的问题补写一个 generation 的向量。
///
/// 与 `save` 的区别：载荷不动、问题 id 不动，只写向量行（幂等 upsert）。
pub(crate) async fn put_question_vector(
    pool: &PgPool,
    robot_id: &str,
    question_id: &str,
    generation: i64,
    vec: Vec<f32>,
    model: &ModelRef,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    vector::put_many(
        &mut tx,
        robot_id,
        vector::QA,
        generation,
        model,
        &[(String::from(question_id), vec)],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
