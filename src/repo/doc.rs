//! `documents` / `doc_chunks` 仓储（§5.6 / B-21）。
//!
//! A 把文档正文放在 turso 的 `{robot_id}` 表、分块原文 + 向量放在 `{robot_id}_vec`，
//! 两半**不在一个事务里**，也没有外键。搬进 PG 后：
//!
//! - 文档行与分块行是父子表（`ON DELETE CASCADE`），分块带 `position`；
//! - `chunk_text` 存**原文**（要拼进 LLM prompt，不能小写化），
//!   `chunk_text_norm` 存 `to_lowercase()` 的归一化文本，只给关键词 `LIKE` 用
//!   （§16.3 / §15.4 S4.6：不依赖数据库 collation）；
//! - 分块的向量行（`vector_items.collection = 'doc'`，`entity_id = doc_chunks.id`）
//!   与分块**同事务**写入；`vector_items.entity_id` 没有外键，所以替换/删除分块时
//!   必须显式删向量行。
//!
//! 关键词召回在这里（SQL 只能在 `src/repo/`）：返回命中分块的 id / 原文 / 归一化
//! 文本，打分的语义留在 `kb::doc`。

use sqlx::postgres::PgPool;

use crate::repo::vector::{self, ModelRef};
use crate::result::Result;

/// 列表接口要的东西（不带分块，只要正文）。
#[derive(Debug, PartialEq, sqlx::FromRow)]
pub(crate) struct DocRow {
    pub(crate) id: String,
    pub(crate) file_name: String,
    pub(crate) file_size: i64,
    pub(crate) doc_content: String,
    /// 乐观锁版本（B-17）。
    pub(crate) version: i64,
}

/// 一篇文档的正文（重建索引用）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct DocumentPayload {
    pub(crate) id: String,
    pub(crate) doc_content: String,
}

/// 新上传的文档元数据。
pub(crate) struct NewDocument {
    pub(crate) file_name: String,
    pub(crate) file_size: i64,
    pub(crate) mime_type: Option<String>,
    /// 相对 `data/` 的路径，如 `uploads/<robot>/<doc>.<ext>`。
    pub(crate) original_path: Option<String>,
    /// SHA-256 十六进制。
    pub(crate) content_hash: Option<String>,
    pub(crate) doc_content: String,
}

/// 一个分块 + 它的向量。分块 id 由仓储生成（调用方不关心）。
pub(crate) struct ChunkInput {
    pub(crate) position: i32,
    pub(crate) text: String,
    pub(crate) norm: String,
    pub(crate) vec: Vec<f32>,
}

/// 关键词命中：分块 id、原文、归一化文本。
#[derive(Debug, PartialEq, sqlx::FromRow)]
pub(crate) struct KeywordHit {
    pub(crate) id: String,
    pub(crate) chunk_text: String,
    pub(crate) chunk_text_norm: String,
}

pub(crate) async fn list(pool: &PgPool, robot_id: &str) -> Result<Vec<DocRow>> {
    Ok(sqlx::query_as(
        r#"SELECT id, file_name, file_size, doc_content, version
             FROM documents
            WHERE robot_id = $1
            ORDER BY created_at DESC, id COLLATE "C""#,
    )
    .bind(robot_id)
    .fetch_all(pool)
    .await?)
}

/// 读一篇文档的正文；已被删掉时返回 `None`（重建时跳过，不终止整轮）。
pub(crate) async fn get_payload(
    pool: &PgPool,
    robot_id: &str,
    doc_id: &str,
) -> Result<Option<DocumentPayload>> {
    Ok(
        sqlx::query_as("SELECT id, doc_content FROM documents WHERE id = $1 AND robot_id = $2")
            .bind(doc_id)
            .bind(robot_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 插入文档 + 分块 + 向量（**一个事务**，B-21/B-22）。
pub(crate) async fn create(
    pool: &PgPool,
    robot_id: &str,
    doc_id: &str,
    doc: &NewDocument,
    chunks: &[ChunkInput],
    model: &ModelRef,
) -> Result<Vec<i64>> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO documents
             (id, robot_id, file_name, file_size, mime_type, original_path,
              content_hash, doc_content)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(doc_id)
    .bind(robot_id)
    .bind(&doc.file_name)
    .bind(doc.file_size)
    .bind(&doc.mime_type)
    .bind(&doc.original_path)
    .bind(&doc.content_hash)
    .bind(&doc.doc_content)
    .execute(&mut *tx)
    .await?;
    let vec_ids = insert_chunks(&mut tx, robot_id, doc_id, chunks, model, None).await?;
    tx.commit().await?;
    Ok(vec_ids)
}

/// 改正文：更新 `documents` 并**整篇替换**分块与向量（同一个事务）。
///
/// `expected_version = Some(v)` 时做乐观锁（B-17）：版本不符返回
/// [`crate::repo::version_conflict`]，不会覆盖别人的编辑。`None` 是旧客户端/
/// 内部调用的"最后写入胜出"路径。
///
/// 返回 `None` 表示文档已经不存在（调用方按"找不到文档"处理）。
pub(crate) async fn update_content(
    pool: &PgPool,
    robot_id: &str,
    doc_id: &str,
    doc_content: &str,
    chunks: &[ChunkInput],
    model: &ModelRef,
    expected_version: Option<i64>,
) -> Result<Option<Vec<i64>>> {
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE documents
            SET doc_content = $3, updated_at = now(), version = version + 1
          WHERE id = $1 AND robot_id = $2
            AND ($4::BIGINT IS NULL OR version = $4)",
    )
    .bind(doc_id)
    .bind(robot_id)
    .bind(doc_content)
    .bind(expected_version)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        if expected_version.is_none() {
            return Ok(None);
        }
        let exists: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM documents WHERE id = $1 AND robot_id = $2")
                .bind(doc_id)
                .bind(robot_id)
                .fetch_optional(pool)
                .await?;
        return if exists.is_some() {
            Err(crate::repo::version_conflict("Document"))
        } else {
            Ok(None)
        };
    }
    let vec_ids = replace_chunks(&mut tx, robot_id, doc_id, chunks, model, None).await?;
    tx.commit().await?;
    Ok(vec_ids)
}

/// 重建一篇文档的分块向量（正文不动）。返回写入的分块数。
///
/// 分块会按**当前** provider 的参数重新切（换模型后 chunk 数可能变），所以这里
/// 是"整篇替换"而不是原地更新：旧分块与它们的向量行（任意 generation）一起删掉，
/// 新分块与新 generation 的向量行在同一个事务里写入。单篇文档因此是原子的 ——
/// 重建中途失败时，这篇要么整篇旧、要么整篇新，不会半篇。
pub(crate) async fn reindex_document(
    pool: &PgPool,
    robot_id: &str,
    doc_id: &str,
    generation: i64,
    chunks: &[ChunkInput],
    model: &ModelRef,
) -> Result<usize> {
    let mut tx = pool.begin().await?;
    if replace_chunks(&mut tx, robot_id, doc_id, chunks, model, Some(generation))
        .await?
        .is_none()
    {
        tx.rollback().await?;
        return Ok(0);
    }
    tx.commit().await?;
    Ok(chunks.len())
}

/// 删除文档：分块向量 → 文档行（分块由 FK 级联）。返回原文件相对路径。
pub(crate) async fn delete(pool: &PgPool, robot_id: &str, doc_id: &str) -> Result<Option<String>> {
    let mut tx = pool.begin().await?;
    let original_path: Option<String> = sqlx::query_scalar(
        "SELECT original_path FROM documents WHERE id = $1 AND robot_id = $2 FOR UPDATE",
    )
    .bind(doc_id)
    .bind(robot_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let chunk_ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM doc_chunks WHERE document_id = $1 AND robot_id = $2")
            .bind(doc_id)
            .bind(robot_id)
            .fetch_all(&mut *tx)
            .await?;
    vector::delete_entities(&mut tx, robot_id, vector::DOC, &chunk_ids).await?;
    sqlx::query("DELETE FROM documents WHERE id = $1 AND robot_id = $2")
        .bind(doc_id)
        .bind(robot_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(original_path)
}

/// 关键词召回：`chunk_text_norm LIKE` 任一模式，最多 `limit` 行。
///
/// `patterns` 是已经拼好的 **LIKE 模式**（含 `%`，且通配符已转义）；归一化与
/// 转义都在 `kb::doc` 里做，这里只负责拼 OR 链。没有 FTS、没有分词器（§16.3）——
/// 单机器人 10^4–10^5 行全表扫是可接受的。
pub(crate) async fn keyword_recall(
    pool: &PgPool,
    robot_id: &str,
    patterns: &[String],
    limit: i64,
) -> Result<Vec<KeywordHit>> {
    if patterns.is_empty() {
        return Ok(Vec::new());
    }
    let cond = patterns
        .iter()
        .enumerate()
        .map(|(i, _)| format!("c.chunk_text_norm LIKE ${} ESCAPE '\\'", i + 3))
        .collect::<Vec<_>>()
        .join(" OR ");
    let sql = format!(
        "SELECT c.id, c.chunk_text, c.chunk_text_norm
           FROM doc_chunks c
          WHERE c.robot_id = $1 AND ({cond})
          ORDER BY c.document_id COLLATE \"C\", c.position
          LIMIT $2"
    );
    let mut q = sqlx::query_as::<_, KeywordHit>(sqlx::AssertSqlSafe(sql))
        .bind(robot_id)
        .bind(limit);
    for p in patterns {
        q = q.bind(p);
    }
    Ok(q.fetch_all(pool).await?)
}

/// 用 `vector_items.id` 取回分块原文（索引 top-k → 回 PG 取载荷）。
///
/// 返回 `(vector_items.id, doc_chunks.id, chunk_text)`，顺序不保证；调用方按索引
/// 分数重排。关键词召回和向量召回要用 `doc_chunks.id` 作为同一个键做 RRF。
pub(crate) async fn chunks_for_vectors(
    pool: &PgPool,
    robot_id: &str,
    vector_ids: &[i64],
) -> Result<Vec<(i64, String, String)>> {
    if vector_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT v.id, c.id, c.chunk_text
           FROM vector_items v
           JOIN doc_chunks c ON c.id = v.entity_id
          WHERE v.robot_id = $1 AND v.collection = $2 AND v.id = ANY($3)",
    )
    .bind(robot_id)
    .bind(vector::DOC)
    .bind(vector_ids)
    .fetch_all(pool)
    .await?)
}

/// 还没有 `generation` 向量的文档（重建续跑用，D47）。
///
/// 待办 = 有分块、但存在某个分块在该 generation 下没有向量行的文档。
/// 没有分块的空文档不算待办（它本来就没有向量可写）。
pub(crate) async fn pending_documents(
    pool: &PgPool,
    robot_id: &str,
    generation: i64,
) -> Result<Vec<DocumentPayload>> {
    Ok(sqlx::query_as(
        r#"SELECT d.id, d.doc_content
             FROM documents d
            WHERE d.robot_id = $1
              AND EXISTS (SELECT 1 FROM doc_chunks c WHERE c.document_id = d.id)
              AND EXISTS (
                  SELECT 1 FROM doc_chunks c
                   WHERE c.document_id = d.id
                     AND NOT EXISTS (
                         SELECT 1 FROM vector_items v
                          WHERE v.robot_id = d.robot_id
                            AND v.collection = 'doc'
                            AND v.generation = $2
                            AND v.entity_id = c.id))
            ORDER BY d.created_at, d.id COLLATE "C""#,
    )
    .bind(robot_id)
    .bind(generation)
    .fetch_all(pool)
    .await?)
}

/// 重建进度里的"总文档数"（只数有分块的文档，与 [`pending_documents`] 同口径）。
pub(crate) async fn count_chunked_documents(pool: &PgPool, robot_id: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM documents d
          WHERE d.robot_id = $1
            AND EXISTS (SELECT 1 FROM doc_chunks c WHERE c.document_id = d.id)",
    )
    .bind(robot_id)
    .fetch_one(pool)
    .await?)
}

/// 插入分块 + 向量；返回与 `chunks` 同序的 `vector_items.id`。
async fn insert_chunks(
    tx: &mut sqlx::PgConnection,
    robot_id: &str,
    doc_id: &str,
    chunks: &[ChunkInput],
    model: &ModelRef,
    generation: Option<i64>,
) -> Result<Vec<i64>> {
    let mut ids: Vec<String> = Vec::with_capacity(chunks.len());
    let mut items: Vec<(String, Vec<f32>)> = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let cid = scru128::new_string();
        sqlx::query(
            "INSERT INTO doc_chunks
                 (id, document_id, robot_id, position, chunk_text, chunk_text_norm)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&cid)
        .bind(doc_id)
        .bind(robot_id)
        .bind(chunk.position)
        .bind(&chunk.text)
        .bind(&chunk.norm)
        .execute(&mut *tx)
        .await?;
        items.push((cid.clone(), chunk.vec.clone()));
        ids.push(cid);
    }
    match generation {
        Some(g) => vector::put_many(&mut *tx, robot_id, vector::DOC, g, model, &items).await,
        None => vector::put_many_active(&mut *tx, robot_id, vector::DOC, model, &items).await,
    }
}

/// 删掉该文档现有分块（连同向量行），再插入新的。
///
/// 返回 `None` 表示文档已经不存在；否则返回与 `chunks` 同序的 `vector_items.id`。
async fn replace_chunks(
    tx: &mut sqlx::PgConnection,
    robot_id: &str,
    doc_id: &str,
    chunks: &[ChunkInput],
    model: &ModelRef,
    generation: Option<i64>,
) -> Result<Option<Vec<i64>>> {
    let exists: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM documents WHERE id = $1 AND robot_id = $2")
            .bind(doc_id)
            .bind(robot_id)
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_none() {
        return Ok(None);
    }
    let old: Vec<String> = sqlx::query_scalar("SELECT id FROM doc_chunks WHERE document_id = $1")
        .bind(doc_id)
        .fetch_all(&mut *tx)
        .await?;
    vector::delete_entities(&mut *tx, robot_id, vector::DOC, &old).await?;
    sqlx::query("DELETE FROM doc_chunks WHERE document_id = $1")
        .bind(doc_id)
        .execute(&mut *tx)
        .await?;
    let ids = insert_chunks(&mut *tx, robot_id, doc_id, chunks, model, generation).await?;
    Ok(Some(ids))
}
