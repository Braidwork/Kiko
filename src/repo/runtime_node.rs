//! `runtime_nodes` 仓储（§5.3 / §15.4 S3.5）。
//!
//! 运行时节点是 **release 时从 canvas 重新生成的派生数据**（D29：rkyv 字节）。
//! 主键 `(robot_id, main_flow_id, node_id)`：
//! - `robot_id` 外键挂在机器人上，删机器人自动级联；
//! - `main_flow_id` 只是命名空间：真实主流程 id，或 demo 模板 id（`demo-*`）。
//!   demo 没有 `mainflows` 行，所以这里**不**外键到主流程；删真实主流程时由
//!   [`super::mainflow::delete`] 在同一事务里显式清。
//!
//! PG 返回的 `BYTEA` 是未对齐切片，反序列化前必须拷进 `AlignedVec` —— 这个拷贝在
//! `flow/rt/node.rs::deser_node` 里，**不是**冗余（§5.3）。

use sqlx::postgres::PgPool;
use sqlx::{Postgres, Transaction};

use crate::result::Result;

/// 读一条运行时节点，返回原始 rkyv 字节。
pub(crate) async fn get(
    pool: &PgPool,
    robot_id: &str,
    main_flow_id: &str,
    node_id: &str,
) -> Result<Option<Vec<u8>>> {
    Ok(sqlx::query_scalar(
        "SELECT data FROM runtime_nodes
          WHERE robot_id = $1 AND main_flow_id = $2 AND node_id = $3",
    )
    .bind(robot_id)
    .bind(main_flow_id)
    .bind(node_id)
    .fetch_optional(pool)
    .await?)
}

/// 整批写入/覆盖一个主流程（或 demo）的运行时节点。
///
/// 一个事务，失败则整批回滚：一次 release 要么全部生效，要么保留上一版。
pub(crate) async fn upsert_many(
    pool: &PgPool,
    robot_id: &str,
    main_flow_id: &str,
    nodes: &[(String, Vec<u8>)],
) -> Result<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (node_id, data) in nodes {
        sqlx::query(
            "INSERT INTO runtime_nodes (robot_id, main_flow_id, node_id, data, updated_at)
             VALUES ($1, $2, $3, $4, now())
             ON CONFLICT (robot_id, main_flow_id, node_id) DO UPDATE
                SET data = EXCLUDED.data,
                    updated_at = now()",
        )
        .bind(robot_id)
        .bind(main_flow_id)
        .bind(node_id)
        .bind(data)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 在调用方的事务里删掉一个主流程的全部节点（删主流程时用）。
pub(crate) async fn delete_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    robot_id: &str,
    main_flow_id: &str,
) -> Result<u64> {
    let r = sqlx::query("DELETE FROM runtime_nodes WHERE robot_id = $1 AND main_flow_id = $2")
        .bind(robot_id)
        .bind(main_flow_id)
        .execute(&mut **tx)
        .await?;
    Ok(r.rows_affected())
}
