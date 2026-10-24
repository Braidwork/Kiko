//! `robots` 仓储：机器人注册表。
//!
//! 这是 S2 的第一块 —— 机器人是其它所有按机器人隔离的数据的外键父表，所以它必须
//! 先落 PG，其它模块（变量、意图、流程…）才可能带着 FK 搬过来。

use sqlx::postgres::PgPool;

use crate::result::{Error, Result};
use crate::robot::dto::{RobotData, RobotType};

/// 清单顺序：A 的 redb 按 key 的字节序迭代，用 `COLLATE "C"` 保持同样的顺序，
/// 不跟库默认的本地化排序走（§5.1）。
pub(crate) async fn list(pool: &PgPool) -> Result<Vec<RobotData>> {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as(r#"SELECT id, robot_name, robot_type FROM robots ORDER BY id COLLATE "C""#)
            .fetch_all(pool)
            .await?;
    rows.into_iter()
        .map(|(robot_id, robot_name, robot_type)| {
            Ok(RobotData {
                robot_id,
                robot_name,
                robot_type: robot_type_from_db(&robot_type)?,
            })
        })
        .collect()
}

pub(crate) async fn get(pool: &PgPool, robot_id: &str) -> Result<Option<RobotData>> {
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, robot_name, robot_type FROM robots WHERE id = $1")
            .bind(robot_id)
            .fetch_optional(pool)
            .await?;
    row.map(|(robot_id, robot_name, robot_type)| {
        Ok(RobotData {
            robot_id,
            robot_name,
            robot_type: robot_type_from_db(&robot_type)?,
        })
    })
    .transpose()
}

/// 插入或覆盖。A 的 `persist` 是整条覆盖（redb 写入即替换），这里保持同样语义。
pub(crate) async fn upsert(pool: &PgPool, d: &RobotData) -> Result<()> {
    sqlx::query(
        "INSERT INTO robots (id, robot_name, robot_type) VALUES ($1, $2, $3)
         ON CONFLICT (id) DO UPDATE
            SET robot_name = EXCLUDED.robot_name,
                robot_type = EXCLUDED.robot_type,
                updated_at = now(),
                version    = robots.version + 1",
    )
    .bind(&d.robot_id)
    .bind(&d.robot_name)
    .bind(robot_type_to_db(&d.robot_type))
    .execute(pool)
    .await?;
    Ok(())
}

/// 删除机器人。级联由 FK 负责（`variables` / `robot_settings` 等）。
/// 返回是否真的删掉了一行 —— purge 要幂等，调用方据此判断要不要继续。
pub(crate) async fn delete(pool: &PgPool, robot_id: &str) -> Result<bool> {
    let r = sqlx::query("DELETE FROM robots WHERE id = $1")
        .bind(robot_id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// 目前只有集成测试用它做断言；purge 统计以后会用到。
#[allow(dead_code)]
pub(crate) async fn count(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM robots")
        .fetch_one(pool)
        .await?)
}

/// 数据库里存的是枚举的变体名（`TextBot` 等）。用显式映射而不是 serde JSON，
/// 这样 DBA 在库里看得到人话，列类型也还是 TEXT（§5.2）。
fn robot_type_to_db(t: &RobotType) -> &'static str {
    match t {
        RobotType::InboundCallBot => "InboundCallBot",
        RobotType::OutboundCallBot => "OutboundCallBot",
        RobotType::TextBot => "TextBot",
    }
}

fn robot_type_from_db(s: &str) -> Result<RobotType> {
    match s {
        "InboundCallBot" => Ok(RobotType::InboundCallBot),
        "OutboundCallBot" => Ok(RobotType::OutboundCallBot),
        "TextBot" => Ok(RobotType::TextBot),
        other => Err(Error::WithMessage(format!(
            "Unknown robot_type in database: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robot_type_round_trips() {
        for t in [
            RobotType::InboundCallBot,
            RobotType::OutboundCallBot,
            RobotType::TextBot,
        ] {
            let s = robot_type_to_db(&t);
            assert!(robot_type_from_db(s).is_ok(), "{s} 应该能读回来");
        }
        assert!(robot_type_from_db("bogus").is_err());
    }
}
