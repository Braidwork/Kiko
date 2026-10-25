//! `subflows` 仓储（§5.3 / §15.4 S3.6 / B-15）。
//!
//! A 把「一个主流程下的子流程列表」整条 JSON 存在 redb 的一个 key 里，保存/删除都
//! 按**数组下标**。B 拆成一行一个子流程：
//! - 顺序用 `position`（新增 = `MAX(position)+1`，删除留空洞）；
//! - 保存画布 / 删除都按子流程 id（D10），不再有读整条 → 改数组 → 写回；
//! - 同一主流程上的并发追加先锁父主流程行，避免撞出相同 position。
//!
//! `canvas` 在 API 里是 JSON **字符串**（前端 `JSON.stringify(graph.toJSON())`），
//! 在库里是 `JSONB`：写时解析、读时再序列化。空画布按 §5.3 存成 `{}`，读回时映射成
//! 空串 —— 前端用 `if (canvas)` 判断是否 `graph.fromJSON`，空对象不能原样回给它。

use serde_json::{Value, json};
use sqlx::postgres::PgPool;
use sqlx::{Postgres, Transaction};

use crate::flow::subflow::dto::SubFlowDetail;
use crate::result::{Error, Result};

#[derive(sqlx::FromRow)]
struct SubFlowRow {
    id: String,
    name: String,
    canvas: Value,
    version: i64,
}

/// 一个主流程下的全部子流程，按 `position`。
pub(crate) async fn list(
    pool: &PgPool,
    robot_id: &str,
    main_flow_id: &str,
) -> Result<Vec<SubFlowDetail>> {
    let rows: Vec<SubFlowRow> = sqlx::query_as(
        r#"SELECT id, name, canvas, version FROM subflows
            WHERE robot_id = $1 AND main_flow_id = $2
            ORDER BY position, id COLLATE "C""#,
    )
    .bind(robot_id)
    .bind(main_flow_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(SubFlowDetail {
                id: r.id,
                name: r.name,
                canvas: canvas_to_string(r.canvas)?,
                version: Some(r.version),
            })
        })
        .collect()
}

/// 在调用方的事务里插入一条子流程，返回新 id。
///
/// 供 [`super::mainflow::create`]（主流程 + 默认子流程同一事务）和 [`create`] 共用。
pub(crate) async fn insert_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    robot_id: &str,
    main_flow_id: &str,
    name: &str,
) -> Result<String> {
    let id = scru128::new_string();
    sqlx::query(
        "INSERT INTO subflows (id, robot_id, main_flow_id, name, canvas, position)
         VALUES ($1, $2, $3, $4, '{}'::jsonb,
                 (SELECT COALESCE(MAX(position), -1) + 1
                    FROM subflows WHERE main_flow_id = $3))",
    )
    .bind(&id)
    .bind(robot_id)
    .bind(main_flow_id)
    .bind(name)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// 追加一条子流程，返回追加后的完整列表（前端用返回值刷新本地列表）。
/// 主流程不存在或不属于该机器人时返回业务错误。
pub(crate) async fn create(
    pool: &PgPool,
    robot_id: &str,
    main_flow_id: &str,
    name: &str,
) -> Result<Vec<SubFlowDetail>> {
    let mut tx = pool.begin().await?;
    let found: Option<String> =
        sqlx::query_scalar("SELECT id FROM mainflows WHERE id = $1 AND robot_id = $2 FOR UPDATE")
            .bind(main_flow_id)
            .bind(robot_id)
            .fetch_optional(&mut *tx)
            .await?;
    if found.is_none() {
        return Err(Error::WithMessage(String::from(
            "Can NOT find the main flow",
        )));
    }
    insert_in_tx(&mut tx, robot_id, main_flow_id, name).await?;
    tx.commit().await?;
    list(pool, robot_id, main_flow_id).await
}

/// 保存一条子流程的画布。
///
/// `expected_version` **必填**（B-17 强制模式）：版本不符返回冲突错误；返回是否
/// 真的更新了一行（不存在/不属于该主流程时为 false）。
pub(crate) async fn save_canvas(
    pool: &PgPool,
    robot_id: &str,
    main_flow_id: &str,
    subflow_id: &str,
    canvas: &str,
    expected_version: i64,
) -> Result<bool> {
    let canvas = canvas_to_db(canvas)?;
    let r = sqlx::query(
        "UPDATE subflows
            SET canvas = $4, updated_at = now(), version = version + 1
          WHERE id = $1 AND robot_id = $2 AND main_flow_id = $3 AND version = $5",
    )
    .bind(subflow_id)
    .bind(robot_id)
    .bind(main_flow_id)
    .bind(canvas)
    .bind(expected_version)
    .execute(pool)
    .await?;
    if r.rows_affected() > 0 {
        return Ok(true);
    }
    let exists: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM subflows WHERE id = $1 AND robot_id = $2 AND main_flow_id = $3",
    )
    .bind(subflow_id)
    .bind(robot_id)
    .bind(main_flow_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_some() {
        Err(crate::repo::version_conflict("Sub-flow"))
    } else {
        Ok(false)
    }
}

/// 按 id 删除一条子流程。返回是否删掉了行。
pub(crate) async fn delete(
    pool: &PgPool,
    robot_id: &str,
    main_flow_id: &str,
    subflow_id: &str,
) -> Result<bool> {
    let r =
        sqlx::query("DELETE FROM subflows WHERE id = $1 AND robot_id = $2 AND main_flow_id = $3")
            .bind(subflow_id)
            .bind(robot_id)
            .bind(main_flow_id)
            .execute(pool)
            .await?;
    Ok(r.rows_affected() > 0)
}

/// API 的 canvas 字符串 → 数据库的 JSONB。
///
/// 空串是「还没画过」；库里存 `{}`（§5.3 要求空流程也是合法 JSON）。
fn canvas_to_db(canvas: &str) -> Result<Value> {
    if canvas.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(canvas)
        .map_err(|e| Error::WithMessage(format!("Invalid flow canvas: {e}")))?;
    if !v.is_object() {
        return Err(Error::WithMessage(String::from(
            "Flow canvas must be a JSON object",
        )));
    }
    Ok(v)
}

/// 数据库的 JSONB → API 的 canvas 字符串。空对象映射回空串（见模块注释）。
fn canvas_to_string(canvas: Value) -> Result<String> {
    match &canvas {
        Value::Object(map) if map.is_empty() => Ok(String::new()),
        _ => Ok(serde_json::to_string(&canvas)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_canvas_maps_to_an_empty_json_object_and_back() {
        assert_eq!(canvas_to_db("").unwrap(), json!({}));
        assert_eq!(canvas_to_db("  ").unwrap(), json!({}));
        assert_eq!(canvas_to_string(json!({})).unwrap(), "");
    }

    #[test]
    fn a_real_canvas_round_trips_as_a_string() {
        let s = r#"{"cells":[]}"#;
        let v = canvas_to_db(s).unwrap();
        assert_eq!(canvas_to_string(v).unwrap(), r#"{"cells":[]}"#);
    }

    #[test]
    fn invalid_or_non_object_canvas_is_rejected() {
        assert!(canvas_to_db("not json").is_err());
        assert!(canvas_to_db("[]").is_err());
        assert!(canvas_to_db("null").is_err());
    }
}
