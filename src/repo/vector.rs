//! `vector_items` / `embedding_models` / `vector_collection_state` /
//! `embedding_generations` 仓储（§5.7 / B-22 / B-23）。
//!
//! 设计要点：
//!
//! - **实体与向量同事务**（B-22）：载荷表（`doc_chunks` / `qa_questions` /
//!   `intent_phrases`）和 `vector_items` 在同一个 PostgreSQL 事务里写，绝不出现
//!   "有记录无向量"或"有向量无记录"；`vector_items` 的 `entity_id` 是通用列、没有
//!   外键，所以删实体时必须**显式**在同一个事务里删向量行（[`delete_entities`]）。
//! - **generation**（B-23 / D47）：一份向量属于某个 generation。增量写进
//!   `active_generation`；重建写进 `pending_generation`，全部完成后再翻转 active 并
//!   删掉旧 generation。查询只认 active（见 `vector::ensure_loaded`）。
//! - **模型只记定义**：`embedding_models` 按 `fingerprint`（`v1|kind|model|维度`）
//!   去重，一行可被多个 generation 复用；`active_model_id` 记录"库里这份向量是哪个
//!   模型算的"，与用户当前配置是两件事。
//! - `vector_items.vec` 的格式就是 `vector::index::encode` 的小端 f32 拼接。

use sqlx::PgConnection;
use sqlx::postgres::PgPool;

use crate::result::Result;
use crate::vector::index::{decode, encode};

/// `vector_items.collection` 的三个取值（§5.7）。
pub(crate) const PHRASE: &str = "phrase";
pub(crate) const QA: &str = "qa";
pub(crate) const DOC: &str = "doc";

pub(crate) const COLLECTIONS: [&str; 3] = [PHRASE, QA, DOC];

/// 一份模型定义。`dimension` 是**实际算出来的**维度，不是设置里写的期望值。
pub(crate) struct ModelRef {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) dimension: i32,
    pub(crate) normalized: bool,
    pub(crate) fingerprint: String,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct CollectionState {
    pub(crate) active_generation: i64,
    pub(crate) active_model_id: Option<i16>,
    pub(crate) pending_generation: Option<i64>,
    pub(crate) status: String,
}

/// 一处集合的元数据（`man::vector_report` 用，B-29）。
///
/// 维度与行数全部来自元数据表，**不扫 BLOB**：`embedding_models.dimension` 是写入时
/// 记下的真实维度，`count(*)` 只碰索引。
#[derive(Debug, PartialEq)]
pub(crate) struct CollectionStats {
    pub(crate) collection: String,
    /// 该集合全部 generation 的向量行数（重建收尾后旧 generation 已删，通常只剩一份）。
    pub(crate) rows: i64,
    /// 出现过的维度（升序去重）。多于一种说明库里有混合维度，检索会失败。
    pub(crate) dims: Vec<i32>,
    pub(crate) active_generation: i64,
    /// active generation 用的模型指纹；从没写过向量时为 `None`。
    pub(crate) active_model: Option<String>,
    pub(crate) pending_generation: Option<i64>,
    pub(crate) status: String,
}

/// 建 state 行（已存在则不动）。机器人还不存在时 FK 会报错——调用方都在机器人
/// 已存在的路径上。
pub(crate) async fn ensure_state(
    conn: &mut PgConnection,
    robot_id: &str,
    collection: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO vector_collection_state (robot_id, collection)
         VALUES ($1, $2)
         ON CONFLICT (robot_id, collection) DO NOTHING",
    )
    .bind(robot_id)
    .bind(collection)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 当前 active generation；没有 state 行时先建一行（`active_generation = 0`）。
pub(crate) async fn active_generation(
    conn: &mut PgConnection,
    robot_id: &str,
    collection: &str,
) -> Result<i64> {
    ensure_state(&mut *conn, robot_id, collection).await?;
    let g: i64 = sqlx::query_scalar(
        "SELECT active_generation FROM vector_collection_state
          WHERE robot_id = $1 AND collection = $2",
    )
    .bind(robot_id)
    .bind(collection)
    .fetch_one(&mut *conn)
    .await?;
    Ok(g)
}

pub(crate) async fn state(
    pool: &PgPool,
    robot_id: &str,
    collection: &str,
) -> Result<Option<CollectionState>> {
    Ok(sqlx::query_as(
        "SELECT active_generation, active_model_id, pending_generation, status
           FROM vector_collection_state
          WHERE robot_id = $1 AND collection = $2",
    )
    .bind(robot_id)
    .bind(collection)
    .fetch_optional(pool)
    .await?)
}

/// 记录（或复用）一份模型定义，返回 `embedding_models.id`。
///
/// `fingerprint` 唯一，所以同一模型被多个机器人/集合用到时只有一行。
pub(crate) async fn ensure_model(conn: &mut PgConnection, model: &ModelRef) -> Result<i16> {
    Ok(sqlx::query_scalar(
        "INSERT INTO embedding_models (provider, model, dimension, normalized, fingerprint)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (fingerprint) DO UPDATE SET dimension = EXCLUDED.dimension
         RETURNING id",
    )
    .bind(&model.provider)
    .bind(&model.model)
    .bind(model.dimension)
    .bind(model.normalized)
    .bind(&model.fingerprint)
    .fetch_one(&mut *conn)
    .await?)
}

/// 给 active generation 记上模型（已记录过就保留原样）。
pub(crate) async fn ensure_generation(
    conn: &mut PgConnection,
    robot_id: &str,
    collection: &str,
    generation: i64,
    model_id: i16,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO embedding_generations
             (robot_id, collection, generation, model_id, status, started_at)
         VALUES ($1, $2, $3, $4, 'running', now())
         ON CONFLICT (robot_id, collection, generation) DO NOTHING",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .bind(model_id)
    .execute(&mut *conn)
    .await?;
    // 只在这一代还没有模型记录时补记：增量写入可能带着新模型（用户改了设置但还没
    // 重建），那一代的事实仍然是用**建立这一代时的模型**算的，不能跟着改。
    sqlx::query(
        "UPDATE vector_collection_state
            SET active_model_id = $3, updated_at = now()
          WHERE robot_id = $1 AND collection = $2
            AND active_generation = $4 AND active_model_id IS NULL",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(model_id)
    .bind(generation)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 写入一批 `(entity_id, 向量)` 到**指定** generation，返回与入参同序的
/// `vector_items.id`。
///
/// **必须在调用方的载荷事务里**（实体与向量同事务，B-22）。`(collection,
/// entity_id, generation)` 有唯一约束，同一实体重复写是覆盖（重建续跑依赖它）。
pub(crate) async fn put_many(
    conn: &mut PgConnection,
    robot_id: &str,
    collection: &str,
    generation: i64,
    model: &ModelRef,
    items: &[(String, Vec<f32>)],
) -> Result<Vec<i64>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    // 维度守卫：换了维度但还没重建时，新向量和旧向量在一个 generation 里会让索引
    // 直接不可用（BruteForce 拒绝混合维度）。宁可在写入点说清楚，也不要留下
    // "检索整个挂掉、但不知道是哪次写入"的状态。换模型但维度相同的情况仍沿用 A
    // 的行为（同一个 generation 里混模型，设置页会提示重建）。
    let existing: Option<i32> = sqlx::query_scalar(
        "SELECT octet_length(vec) / 4
           FROM vector_items
          WHERE robot_id = $1 AND collection = $2 AND generation = $3
          LIMIT 1",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(d) = existing {
        let new_dim = items[0].1.len() as i32;
        if d != new_dim {
            return Err(crate::result::Error::WithMessage(format!(
                "Embedding dimension changed from {d} to {new_dim}; \
                 rebuild the vector index before writing new vectors."
            )));
        }
    }
    let model_id = ensure_model(&mut *conn, model).await?;
    ensure_generation(&mut *conn, robot_id, collection, generation, model_id).await?;

    let mut ids = Vec::with_capacity(items.len());
    for (entity_id, vec) in items {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO vector_items (robot_id, collection, entity_id, generation, vec)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (collection, entity_id, generation)
             DO UPDATE SET vec = EXCLUDED.vec
             RETURNING id",
        )
        .bind(robot_id)
        .bind(collection)
        .bind(entity_id)
        .bind(generation)
        .bind(encode(vec))
        .fetch_one(&mut *conn)
        .await?;
        ids.push(id);
    }
    Ok(ids)
}

/// 写入当前 **active** generation（增量写：新增/编辑知识库数据）。
///
/// 返回 `(generation, vector ids)`；`generation` 给内存索引的写后同步用不到，但
/// 调用方偶尔想知道这次写的是哪一代（诊断）。
pub(crate) async fn put_many_active(
    conn: &mut PgConnection,
    robot_id: &str,
    collection: &str,
    model: &ModelRef,
    items: &[(String, Vec<f32>)],
) -> Result<Vec<i64>> {
    let generation = active_generation(&mut *conn, robot_id, collection).await?;
    put_many(&mut *conn, robot_id, collection, generation, model, items).await
}

/// 删掉这些实体的全部向量行（所有 generation），返回被删的 `vector_items.id`。
///
/// 实体行被删时必须调它：`vector_items.entity_id` 没有外键，不显式删就会留下
/// 孤儿向量 —— 那些行的维度可能还是旧配置的。
pub(crate) async fn delete_entities(
    conn: &mut PgConnection,
    robot_id: &str,
    collection: &str,
    entity_ids: &[String],
) -> Result<Vec<i64>> {
    if entity_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = sqlx::query_scalar(
        "DELETE FROM vector_items
          WHERE robot_id = $1 AND collection = $2 AND entity_id = ANY($3)
      RETURNING id",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(entity_ids)
    .fetch_all(&mut *conn)
    .await?;
    Ok(ids)
}

/// 某个实体在任意 generation 里有没有向量行（诊断/测试用）。
#[cfg(test)]
pub(crate) async fn entities_with_vector(
    pool: &PgPool,
    robot_id: &str,
    collection: &str,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT entity_id FROM vector_items
          WHERE robot_id = $1 AND collection = $2 ORDER BY entity_id",
    )
    .bind(robot_id)
    .bind(collection)
    .fetch_all(pool)
    .await?)
}

/// 载入 active generation 的全部向量行：`(vector_items.id, vec)`。
pub(crate) async fn load_active(
    pool: &PgPool,
    robot_id: &str,
    collection: &str,
) -> Result<Vec<(i64, Vec<f32>)>> {
    let generation: i64 = sqlx::query_scalar(
        "SELECT COALESCE(
                    (SELECT active_generation FROM vector_collection_state
                      WHERE robot_id = $1 AND collection = $2), 0)",
    )
    .bind(robot_id)
    .bind(collection)
    .fetch_one(pool)
    .await?;
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT id, vec FROM vector_items
          WHERE robot_id = $1 AND collection = $2 AND generation = $3
          ORDER BY id",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, blob) in rows {
        out.push((id, decode(&blob)?));
    }
    Ok(out)
}

/// active generation 的向量行数（重建进度的 before / after）。
pub(crate) async fn count_active(pool: &PgPool, robot_id: &str, collection: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*)
           FROM vector_items v
           JOIN vector_collection_state s
             ON s.robot_id = v.robot_id AND s.collection = v.collection
          WHERE v.robot_id = $1 AND v.collection = $2
            AND v.generation = s.active_generation",
    )
    .bind(robot_id)
    .bind(collection)
    .fetch_one(pool)
    .await?)
}

/// 某个 generation 的向量行数（重建进度用）。
pub(crate) async fn count_generation(
    pool: &PgPool,
    robot_id: &str,
    collection: &str,
    generation: i64,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM vector_items
          WHERE robot_id = $1 AND collection = $2 AND generation = $3",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .fetch_one(pool)
    .await?)
}

/// 开始一次重建，返回要写入的 generation（B-23 / D47）。
///
/// 幂等：同一集合已经在重建中时**复用** `pending_generation`，所以失败后重新点
/// 重建会接着往同一代写，已经写过的实体由各自的 `pending_*` 查询跳过。
pub(crate) async fn begin_rebuild(
    pool: &PgPool,
    robot_id: &str,
    collection: &str,
    model: &ModelRef,
) -> Result<i64> {
    // 模型行与 state 更新放在同一个事务里，避免留下"有 generation、没 model"。
    let mut tx = pool.begin().await?;
    ensure_state(&mut tx, robot_id, collection).await?;
    let model_id = ensure_model(&mut tx, model).await?;
    let (active, pending): (i64, Option<i64>) = sqlx::query_as(
        "SELECT active_generation, pending_generation
           FROM vector_collection_state
          WHERE robot_id = $1 AND collection = $2
            FOR UPDATE",
    )
    .bind(robot_id)
    .bind(collection)
    .fetch_one(&mut *tx)
    .await?;
    // 只要 pending 比 active 新，就说明上一轮没走完（running 或 failed），本次续跑
    // 沿用同一代：已经写过的实体由 `pending_*` 查询跳过。
    let resume = matches!(pending, Some(g) if g > active);
    let generation = if resume {
        pending.expect("resume 分支里 pending 必然存在")
    } else {
        active + 1
    };
    if resume {
        // 上次没跑完、这次换了模型：那一代已经写进去的向量是用旧模型算的，混进来
        // 就是噪声。丢掉重算，generation 号不变（待办查询也不会再跳过它们）。
        let existing: Option<i16> = sqlx::query_scalar(
            "SELECT model_id FROM embedding_generations
              WHERE robot_id = $1 AND collection = $2 AND generation = $3",
        )
        .bind(robot_id)
        .bind(collection)
        .bind(generation)
        .fetch_optional(&mut *tx)
        .await?;
        if existing != Some(model_id) {
            sqlx::query(
                "DELETE FROM vector_items
                  WHERE robot_id = $1 AND collection = $2 AND generation = $3",
            )
            .bind(robot_id)
            .bind(collection)
            .bind(generation)
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query(
        "UPDATE vector_collection_state
            SET pending_generation = $3, status = 'rebuilding', updated_at = now()
          WHERE robot_id = $1 AND collection = $2",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO embedding_generations
             (robot_id, collection, generation, model_id, status, started_at)
         VALUES ($1, $2, $3, $4, 'running', now())
         ON CONFLICT (robot_id, collection, generation) DO UPDATE
             SET model_id = EXCLUDED.model_id,
                 status = 'running',
                 started_at = COALESCE(embedding_generations.started_at, now()),
                 finished_at = NULL",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .bind(model_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(generation)
}

/// 重建成功：翻转 active、清掉旧 generation 的向量行、标记 done。
///
/// 返回翻转后的 active generation。
pub(crate) async fn finish_rebuild(
    pool: &PgPool,
    robot_id: &str,
    collection: &str,
    generation: i64,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let model_id: i16 = sqlx::query_scalar(
        "SELECT model_id FROM embedding_generations
          WHERE robot_id = $1 AND collection = $2 AND generation = $3",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE embedding_generations
            SET status = 'done', finished_at = now()
          WHERE robot_id = $1 AND collection = $2 AND generation = $3",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE vector_collection_state
            SET active_generation = $3, active_model_id = $4,
                pending_generation = NULL, status = 'idle', updated_at = now()
          WHERE robot_id = $1 AND collection = $2",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .bind(model_id)
    .execute(&mut *tx)
    .await?;
    // 旧 generation 的行在翻转之后就没有读者了；重建期间它们还是检索的事实来源。
    sqlx::query(
        "DELETE FROM vector_items
          WHERE robot_id = $1 AND collection = $2 AND generation <> $3",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 重建失败：状态标 failed，**保留** pending generation 供续跑。
pub(crate) async fn fail_rebuild(
    pool: &PgPool,
    robot_id: &str,
    collection: &str,
    generation: i64,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE embedding_generations
            SET status = 'failed', finished_at = now()
          WHERE robot_id = $1 AND collection = $2 AND generation = $3",
    )
    .bind(robot_id)
    .bind(collection)
    .bind(generation)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE vector_collection_state
            SET status = 'failed', updated_at = now()
          WHERE robot_id = $1 AND collection = $2",
    )
    .bind(robot_id)
    .bind(collection)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 一处集合的元数据报告（B-29）。不读 `vec` 列。
pub(crate) async fn collection_stats(
    pool: &PgPool,
    robot_id: &str,
    collection: &str,
) -> Result<CollectionStats> {
    let rows: Vec<(Option<i32>, i64)> = sqlx::query_as(
        "SELECT m.dimension, count(*)::bigint
           FROM vector_items v
           LEFT JOIN embedding_generations g
             ON g.robot_id = v.robot_id
            AND g.collection = v.collection
            AND g.generation = v.generation
           LEFT JOIN embedding_models m ON m.id = g.model_id
          WHERE v.robot_id = $1 AND v.collection = $2
          GROUP BY m.dimension",
    )
    .bind(robot_id)
    .bind(collection)
    .fetch_all(pool)
    .await?;

    let mut total = 0i64;
    let mut dims: Vec<i32> = Vec::new();
    for (dim, n) in rows {
        total += n;
        if let Some(d) = dim
            && !dims.contains(&d)
        {
            dims.push(d);
        }
    }
    dims.sort_unstable();

    let state = state(pool, robot_id, collection).await?;
    let (active_generation, active_model, pending_generation, status) = match &state {
        Some(s) => (
            s.active_generation,
            active_model_fingerprint(pool, s.active_model_id).await?,
            s.pending_generation,
            s.status.clone(),
        ),
        None => (0, None, None, String::from("idle")),
    };
    Ok(CollectionStats {
        collection: String::from(collection),
        rows: total,
        dims,
        active_generation,
        active_model,
        pending_generation,
        status,
    })
}

async fn active_model_fingerprint(pool: &PgPool, id: Option<i16>) -> Result<Option<String>> {
    let Some(id) = id else {
        return Ok(None);
    };
    let fp: Option<String> =
        sqlx::query_scalar("SELECT fingerprint FROM embedding_models WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(fp)
}

/// 读一行 `vector_items` 的实体 id（测试/诊断用）。
#[cfg(test)]
pub(crate) async fn vector_ids(
    pool: &PgPool,
    robot_id: &str,
    collection: &str,
) -> Result<Vec<(i64, String)>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, entity_id FROM vector_items
          WHERE robot_id = $1 AND collection = $2 ORDER BY id",
    )
    .bind(robot_id)
    .bind(collection)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
