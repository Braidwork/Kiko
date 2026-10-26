//! `contexts` 仓储（§5.3 / §15.4 S3.4）。
//!
//! 会话是**高频写、最后写入胜出**的数据（§6.2）：每轮对话至少一次整条 `save`，
//! 不做乐观锁，也不做读-改-写；同 `(robot_id, session_id)` 直接 upsert。
//!
//! 两处拆分沿用 §5.3 / D29：
//! - `data JSONB` 存会话其余字段（vars、nodes、chat_history…），序列化格式与 A 的
//!   redb JSON blob 相同，只是落点换成 PG；
//! - `node_state BYTEA` 单独存 `Context.node` 的 rkyv 字节 —— 不塞进 JSON。
//!
//! `expires_at` 在写入时算好（`now() + ttl`），过期清理就是一条
//! `DELETE … WHERE expires_at < now()`，不再遍历机器人、也不再读每个机器人的设置。

use serde_json::Value;
use sqlx::postgres::PgPool;

use crate::result::Result;

/// 一条已持久化的会话：JSON 载荷 + 可选的 rkyv 节点状态。
pub(crate) struct ContextRow {
    pub(crate) data: Value,
    pub(crate) node_state: Option<Vec<u8>>,
}

/// 读一条会话。`(robot_id, session_id)` 是主键；机器人不在时自然读不到。
pub(crate) async fn get(
    pool: &PgPool,
    robot_id: &str,
    session_id: &str,
) -> Result<Option<ContextRow>> {
    let row: Option<(Value, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT data, node_state FROM contexts WHERE robot_id = $1 AND session_id = $2",
    )
    .bind(robot_id)
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(data, node_state)| ContextRow { data, node_state }))
}

/// 整条覆盖（最后写入胜出）。`ttl_secs` 由调用方按机器人设置算好；
/// 0 表示下一次清理就过期（设置缺失时与旧行为一致）。
pub(crate) async fn upsert(
    pool: &PgPool,
    robot_id: &str,
    session_id: &str,
    data: &Value,
    node_state: Option<&[u8]>,
    ttl_secs: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO contexts (robot_id, session_id, data, node_state, updated_at, expires_at)
         VALUES ($1, $2, $3, $4, now(), now() + ($5::bigint * interval '1 second'))
         ON CONFLICT (robot_id, session_id) DO UPDATE
            SET data       = EXCLUDED.data,
                node_state = EXCLUDED.node_state,
                updated_at = now(),
                expires_at = EXCLUDED.expires_at",
    )
    .bind(robot_id)
    .bind(session_id)
    .bind(data)
    .bind(node_state)
    .bind(ttl_secs)
    .execute(pool)
    .await?;
    Ok(())
}

/// 删除所有已过期的会话，返回删除行数。一分钟一次的后台任务直接调它。
pub(crate) async fn delete_expired(pool: &PgPool) -> Result<u64> {
    let r = sqlx::query("DELETE FROM contexts WHERE expires_at IS NOT NULL AND expires_at < now()")
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}
