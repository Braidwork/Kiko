//! `variables` 仓储：第一个从 redb 搬到 PG 的业务模块（§15.4 S3.1）。
//!
//! 表没有代理 ID：`(robot_id, var_name)` 就是业务主键，流程表达式按名字引用变量，
//! 改名 = 删除 + 新建（§5.3）。
//!
//! 与 A 的语义对齐点：
//! - `write` 是整条覆盖 → 这里用 upsert；
//! - `get_all` 按 redb key 的字节序返回 → 这里 `ORDER BY var_name COLLATE "C"`；
//! - 删除不存在的变量不报错（redb `remove` 也不报错）。

use sqlx::postgres::PgPool;

use crate::result::{Error, Result};
use crate::variable::dto::{
    Variable, VariableObtainValueExpressionType, VariableType, VariableValueSource,
};

#[derive(sqlx::FromRow)]
struct VariableRow {
    var_name: String,
    var_type: String,
    var_value_source: String,
    var_constant_value: Option<String>,
    var_associate_data: Option<String>,
    obtain_value_expression_type: Option<String>,
    obtain_value_expression: Option<String>,
    connect_timeout_ms: Option<i64>,
    read_timeout_ms: Option<i64>,
    cache_enabled: Option<bool>,
    version: i64,
}

/// 一个机器人的全部变量。顺序与 A 的 redb key 序一致（字节序，`COLLATE "C"`）。
pub(crate) async fn list(pool: &PgPool, robot_id: &str) -> Result<Vec<Variable>> {
    let rows: Vec<VariableRow> = sqlx::query_as(
        r#"SELECT var_name, var_type, var_value_source, var_constant_value, var_associate_data,
                  obtain_value_expression_type, obtain_value_expression,
                  connect_timeout_ms, read_timeout_ms, cache_enabled, version
             FROM variables
            WHERE robot_id = $1
            ORDER BY var_name COLLATE "C""#,
    )
    .bind(robot_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(Variable::try_from).collect()
}

pub(crate) async fn get(pool: &PgPool, robot_id: &str, var_name: &str) -> Result<Option<Variable>> {
    let row: Option<VariableRow> = sqlx::query_as(
        r#"SELECT var_name, var_type, var_value_source, var_constant_value, var_associate_data,
                  obtain_value_expression_type, obtain_value_expression,
                  connect_timeout_ms, read_timeout_ms, cache_enabled, version
             FROM variables
            WHERE robot_id = $1 AND var_name = $2"#,
    )
    .bind(robot_id)
    .bind(var_name)
    .fetch_optional(pool)
    .await?;
    row.map(Variable::try_from).transpose()
}

/// 插入或整条覆盖（对应 A 的 `db::write`）。
pub(crate) async fn upsert(pool: &PgPool, robot_id: &str, v: &Variable) -> Result<()> {
    let connect_timeout_ms = i64::try_from(v.connect_timeout_milliseconds)
        .map_err(|_| Error::WithMessage(String::from("connectTimeoutMilliseconds 超出范围")))?;
    let read_timeout_ms = i64::try_from(v.read_timeout_milliseconds)
        .map_err(|_| Error::WithMessage(String::from("readTimeoutMilliseconds 超出范围")))?;

    sqlx::query(
        "INSERT INTO variables (
             robot_id, var_name, var_type, var_value_source,
             var_constant_value, var_associate_data,
             obtain_value_expression_type, obtain_value_expression,
             connect_timeout_ms, read_timeout_ms, cache_enabled)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         ON CONFLICT (robot_id, var_name) DO UPDATE
            SET var_type                     = EXCLUDED.var_type,
                var_value_source             = EXCLUDED.var_value_source,
                var_constant_value           = EXCLUDED.var_constant_value,
                var_associate_data           = EXCLUDED.var_associate_data,
                obtain_value_expression_type = EXCLUDED.obtain_value_expression_type,
                obtain_value_expression      = EXCLUDED.obtain_value_expression,
                connect_timeout_ms           = EXCLUDED.connect_timeout_ms,
                read_timeout_ms              = EXCLUDED.read_timeout_ms,
                cache_enabled                = EXCLUDED.cache_enabled,
                updated_at                   = now(),
                version                      = variables.version + 1",
    )
    .bind(robot_id)
    .bind(&v.var_name)
    .bind(var_type_to_db(&v.var_type))
    .bind(value_source_to_db(&v.var_val_source))
    .bind(&v.var_constant_value)
    .bind(&v.var_associate_data)
    .bind(expression_type_to_db(&v.obtain_value_expression_type))
    .bind(&v.obtain_value_expression)
    .bind(connect_timeout_ms)
    .bind(read_timeout_ms)
    .bind(v.cache_enabled)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn delete(pool: &PgPool, robot_id: &str, var_name: &str) -> Result<()> {
    sqlx::query("DELETE FROM variables WHERE robot_id = $1 AND var_name = $2")
        .bind(robot_id)
        .bind(var_name)
        .execute(pool)
        .await?;
    Ok(())
}

/// 更新一个已存在的变量（B-17 强制乐观锁）。返回是否更新了一行；版本不符返回
/// [`crate::repo::version_conflict`]，不存在返回 `false`。
pub(crate) async fn update(
    pool: &PgPool,
    robot_id: &str,
    v: &Variable,
    expected_version: i64,
) -> Result<bool> {
    let connect_timeout_ms = i64::try_from(v.connect_timeout_milliseconds)
        .map_err(|_| Error::WithMessage(String::from("connectTimeoutMilliseconds 超出范围")))?;
    let read_timeout_ms = i64::try_from(v.read_timeout_milliseconds)
        .map_err(|_| Error::WithMessage(String::from("readTimeoutMilliseconds 超出范围")))?;

    let r = sqlx::query(
        "UPDATE variables
            SET var_type                     = $3,
                var_value_source             = $4,
                var_constant_value           = $5,
                var_associate_data           = $6,
                obtain_value_expression_type = $7,
                obtain_value_expression      = $8,
                connect_timeout_ms           = $9,
                read_timeout_ms              = $10,
                cache_enabled                = $11,
                updated_at                   = now(),
                version                      = version + 1
          WHERE robot_id = $1 AND var_name = $2 AND version = $12",
    )
    .bind(robot_id)
    .bind(&v.var_name)
    .bind(var_type_to_db(&v.var_type))
    .bind(value_source_to_db(&v.var_val_source))
    .bind(&v.var_constant_value)
    .bind(&v.var_associate_data)
    .bind(expression_type_to_db(&v.obtain_value_expression_type))
    .bind(&v.obtain_value_expression)
    .bind(connect_timeout_ms)
    .bind(read_timeout_ms)
    .bind(v.cache_enabled)
    .bind(expected_version)
    .execute(pool)
    .await?;
    if r.rows_affected() > 0 {
        return Ok(true);
    }
    let exists: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM variables WHERE robot_id = $1 AND var_name = $2")
            .bind(robot_id)
            .bind(&v.var_name)
            .fetch_optional(pool)
            .await?;
    if exists.is_some() {
        Err(crate::repo::version_conflict("Variable"))
    } else {
        Ok(false)
    }
}

impl TryFrom<VariableRow> for Variable {
    type Error = Error;

    fn try_from(r: VariableRow) -> Result<Self> {
        Ok(Variable {
            var_name: r.var_name,
            var_type: var_type_from_db(&r.var_type)?,
            var_val_source: value_source_from_db(&r.var_value_source)?,
            var_constant_value: r.var_constant_value.unwrap_or_default(),
            var_associate_data: r.var_associate_data.unwrap_or_default(),
            obtain_value_expression_type: expression_type_from_db(
                r.obtain_value_expression_type.as_deref().unwrap_or("None"),
            )?,
            obtain_value_expression: r.obtain_value_expression.unwrap_or_default(),
            connect_timeout_milliseconds: u64::try_from(r.connect_timeout_ms.unwrap_or(0))
                .map_err(|_| {
                    Error::WithMessage(String::from("数据库里的 connect_timeout_ms 是负数"))
                })?,
            read_timeout_milliseconds: u64::try_from(r.read_timeout_ms.unwrap_or(0)).map_err(
                |_| Error::WithMessage(String::from("数据库里的 read_timeout_ms 是负数")),
            )?,
            cache_enabled: r.cache_enabled.unwrap_or(true),
            version: Some(r.version),
        })
    }
}

// 枚举 ↔ TEXT：存变体名，DBA 在库里看得到人话，列类型仍是 TEXT（§5.2）。
fn var_type_to_db(t: &VariableType) -> &'static str {
    match t {
        VariableType::Str => "Str",
        VariableType::Num => "Num",
    }
}

fn var_type_from_db(s: &str) -> Result<VariableType> {
    match s {
        "Str" => Ok(VariableType::Str),
        "Num" => Ok(VariableType::Num),
        other => Err(Error::WithMessage(format!(
            "Unknown variable var_type in database: {other}"
        ))),
    }
}

fn value_source_to_db(s: &VariableValueSource) -> &'static str {
    match s {
        VariableValueSource::Import => "Import",
        VariableValueSource::Collect => "Collect",
        VariableValueSource::UserInput => "UserInput",
        VariableValueSource::Constant => "Constant",
        VariableValueSource::ExternalHttp => "ExternalHttp",
    }
}

fn value_source_from_db(s: &str) -> Result<VariableValueSource> {
    match s {
        "Import" => Ok(VariableValueSource::Import),
        "Collect" => Ok(VariableValueSource::Collect),
        "UserInput" => Ok(VariableValueSource::UserInput),
        "Constant" => Ok(VariableValueSource::Constant),
        "ExternalHttp" => Ok(VariableValueSource::ExternalHttp),
        other => Err(Error::WithMessage(format!(
            "Unknown variable var_value_source in database: {other}"
        ))),
    }
}

fn expression_type_to_db(t: &VariableObtainValueExpressionType) -> &'static str {
    match t {
        VariableObtainValueExpressionType::None => "None",
        VariableObtainValueExpressionType::JsonPointer => "JsonPointer",
        VariableObtainValueExpressionType::HtmlScrape => "HtmlScrape",
        VariableObtainValueExpressionType::JavaScript => "JavaScript",
    }
}

fn expression_type_from_db(s: &str) -> Result<VariableObtainValueExpressionType> {
    match s {
        "None" => Ok(VariableObtainValueExpressionType::None),
        "JsonPointer" => Ok(VariableObtainValueExpressionType::JsonPointer),
        "HtmlScrape" => Ok(VariableObtainValueExpressionType::HtmlScrape),
        "JavaScript" => Ok(VariableObtainValueExpressionType::JavaScript),
        other => Err(Error::WithMessage(format!(
            "Unknown obtain_value_expression_type in database: {other}"
        ))),
    }
}
