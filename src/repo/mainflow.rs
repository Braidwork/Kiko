//! `mainflows` 仓储（§5.3 / §15.4 S3.6 / B-15）。
//!
//! A 的一行主流程只有 id/name/enabled，id 是 `count + scru128`；B 按 D48 去掉
//! `count` 前缀（每次创建不必再全表 `COUNT`，而且前缀是字典序，10 个以上时顺序
//! 本来就不对）。顺序改用 `position` 显式维护（§5.1）。
//!
//! 新建主流程时同时插入默认子流程，**同一个事务**完成；否则一次失败会留下一个
//! 没有子流程的主流程，前端画布会打不开。

use sqlx::postgres::PgPool;

use crate::flow::mainflow::dto::MainFlowDetail;
use crate::result::Result;

/// 一个机器人的全部主流程，按 `position`。
pub(crate) async fn list(pool: &PgPool, robot_id: &str) -> Result<Vec<MainFlowDetail>> {
    let rows: Vec<(String, String, bool, i64)> = sqlx::query_as(
        r#"SELECT id, name, enabled, version FROM mainflows
            WHERE robot_id = $1
            ORDER BY position, id COLLATE "C""#,
    )
    .bind(robot_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, enabled, version)| MainFlowDetail {
            id,
            name,
            enabled,
            version: Some(version),
        })
        .collect())
}

/// 新建主流程 + 默认子流程（同一事务）。返回新主流程。
pub(crate) async fn create(
    pool: &PgPool,
    robot_id: &str,
    name: &str,
    default_subflow_name: &str,
) -> Result<MainFlowDetail> {
    let mut tx = pool.begin().await?;

    // 机器人不存在时这里返回 None，紧接着的 INSERT 会以外键冲突失败（错误文案由
    // `Error::message` 统一翻译）。存在时这行锁把同一机器人上的并发创建串行化，
    // 保证 `position` 不重复。
    let _ = sqlx::query_scalar::<_, String>("SELECT id FROM robots WHERE id = $1 FOR UPDATE")
        .bind(robot_id)
        .fetch_optional(&mut *tx)
        .await?;

    // D48：直接用 scru128，不再拼 count 前缀。
    let id = scru128::new_string();
    sqlx::query(
        "INSERT INTO mainflows (id, robot_id, name, enabled, position)
         VALUES ($1, $2, $3, TRUE,
                 (SELECT COALESCE(MAX(position), -1) + 1
                    FROM mainflows WHERE robot_id = $2))",
    )
    .bind(&id)
    .bind(robot_id)
    .bind(name)
    .execute(&mut *tx)
    .await?;

    super::subflow::insert_in_tx(&mut tx, robot_id, &id, default_subflow_name).await?;
    tx.commit().await?;

    Ok(MainFlowDetail {
        id,
        name: String::from(name),
        enabled: true,
        version: Some(0),
    })
}

/// 保存（新建或更新）一条主流程。返回是否真的写入/更新了一行。
///
/// 走 upsert 是因为 A 的 `write` 也是「没有就建」；但 `id` 属于另一个机器人时
/// 不更新（`WHERE mainflows.robot_id = EXCLUDED.robot_id`），避免拿别人的 id 改数据。
///
/// `expected_version = Some(v)` 时做乐观锁（B-17）：版本不符返回
/// [`crate::repo::version_conflict`]，不会覆盖别人的编辑。
pub(crate) async fn upsert(
    pool: &PgPool,
    robot_id: &str,
    d: &MainFlowDetail,
    expected_version: Option<i64>,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let _ = sqlx::query_scalar::<_, String>("SELECT id FROM robots WHERE id = $1 FOR UPDATE")
        .bind(robot_id)
        .fetch_optional(&mut *tx)
        .await?;

    let r = sqlx::query(
        "INSERT INTO mainflows (id, robot_id, name, enabled, position)
         VALUES ($1, $2, $3, $4,
                 (SELECT COALESCE(MAX(position), -1) + 1
                    FROM mainflows WHERE robot_id = $2))
         ON CONFLICT (id) DO UPDATE
            SET name       = EXCLUDED.name,
                enabled    = EXCLUDED.enabled,
                updated_at = now(),
                version    = mainflows.version + 1
          WHERE mainflows.robot_id = EXCLUDED.robot_id
            AND ($5::BIGINT IS NULL OR mainflows.version = $5)",
    )
    .bind(&d.id)
    .bind(robot_id)
    .bind(&d.name)
    .bind(d.enabled)
    .bind(expected_version)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if r.rows_affected() > 0 {
        return Ok(true);
    }
    if expected_version.is_none() {
        return Ok(false);
    }
    let exists: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM mainflows WHERE id = $1 AND robot_id = $2")
            .bind(&d.id)
            .bind(robot_id)
            .fetch_optional(pool)
            .await?;
    if exists.is_some() {
        Err(crate::repo::version_conflict("Main flow"))
    } else {
        Ok(false)
    }
}

/// 删除主流程。运行时节点没有指向 `mainflows` 的外键（demo 流程没有主流程行，
/// 见 `migrations/pg/0003_runtime_nodes_by_robot.sql`），所以要在同一事务里显式清；
/// 子流程由 FK 级联。
pub(crate) async fn delete(pool: &PgPool, robot_id: &str, id: &str) -> Result<bool> {
    let mut tx = pool.begin().await?;
    super::runtime_node::delete_in_tx(&mut tx, robot_id, id).await?;
    let r = sqlx::query("DELETE FROM mainflows WHERE id = $1 AND robot_id = $2")
        .bind(id)
        .bind(robot_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(r.rows_affected() > 0)
}
