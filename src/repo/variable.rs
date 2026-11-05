//! `variables` 仓储：变量跨机器人共享，表没有 `robot_id`。
//!
//! 业务主键是内部 code `var_unified_name`（归一化后的变量名），显示名放
//! `var_name`。流程表达式、会话缓存和这里的每一次查询都用 code；改名视为
//! “删除 + 新建”。
//!
//! 与 A 的语义对齐点：
//! - `write` 是整条覆盖 → 这里用 upsert；
//! - `get_all` 按内部 code 的字节序返回 → `ORDER BY var_unified_name COLLATE "C"`；
//! - 删除不存在的变量不报错（redb `remove` 也不报错）。

use sqlx::postgres::PgPool;

use crate::result::{Error, Result};
use crate::variable::dto::{
    Variable, VariableObtainValueExpressionType, VariableType, VariableValueCacheType,
    VariableValueSource,
};
use crate::variable::unify_var_name;

#[derive(sqlx::FromRow)]
struct VariableRow {
    var_unified_name: String,
    var_name: String,
    var_type: String,
    var_value_source: String,
    var_associate_data: Option<String>,
    obtain_value_expression_type: Option<String>,
    obtain_value_expression: Option<String>,
    connect_timeout_ms: Option<i64>,
    read_timeout_ms: Option<i64>,
    val_cache_type: i16,
    version: i64,
}

/// 全部变量（跨机器人）。顺序按内部 code 的字节序（`COLLATE "C"`）。
pub(crate) async fn list(pool: &PgPool) -> Result<Vec<Variable>> {
    let rows: Vec<VariableRow> = sqlx::query_as(
        r#"SELECT var_unified_name, var_name, var_type, var_value_source, var_associate_data,
                  obtain_value_expression_type, obtain_value_expression,
                  connect_timeout_ms, read_timeout_ms, val_cache_type, version
             FROM variables
            ORDER BY var_unified_name COLLATE "C""#,
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(Variable::try_from).collect()
}

/// 按显示名或内部 code 取变量（都先归一化成 code 再查）。
pub(crate) async fn get(pool: &PgPool, var_name: &str) -> Result<Option<Variable>> {
    let unified = unify_var_name(var_name);
    let row: Option<VariableRow> = sqlx::query_as(
        r#"SELECT var_unified_name, var_name, var_type, var_value_source, var_associate_data,
                  obtain_value_expression_type, obtain_value_expression,
                  connect_timeout_ms, read_timeout_ms, val_cache_type, version
             FROM variables
            WHERE var_unified_name = $1"#,
    )
    .bind(unified)
    .fetch_optional(pool)
    .await?;
    row.map(Variable::try_from).transpose()
}

/// 插入或整条覆盖（对应 A 的 `db::write`）。
pub(crate) async fn upsert(pool: &PgPool, v: &Variable) -> Result<()> {
    let connect_timeout_ms = i64::try_from(v.connect_timeout_milliseconds)
        .map_err(|_| Error::WithMessage(String::from("connectTimeoutMilliseconds 超出范围")))?;
    let read_timeout_ms = i64::try_from(v.read_timeout_milliseconds)
        .map_err(|_| Error::WithMessage(String::from("readTimeoutMilliseconds 超出范围")))?;
    let unified = if v.var_unified_name.is_empty() {
        unify_var_name(&v.var_name)
    } else {
        v.var_unified_name.clone()
    };

    sqlx::query(
        "INSERT INTO variables (
             var_unified_name, var_name, var_type, var_value_source,
             var_associate_data,
             obtain_value_expression_type, obtain_value_expression,
             connect_timeout_ms, read_timeout_ms, val_cache_type)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (var_unified_name) DO UPDATE
            SET var_name                     = EXCLUDED.var_name,
                var_type                     = EXCLUDED.var_type,
                var_value_source             = EXCLUDED.var_value_source,
                var_associate_data           = EXCLUDED.var_associate_data,
                obtain_value_expression_type = EXCLUDED.obtain_value_expression_type,
                obtain_value_expression      = EXCLUDED.obtain_value_expression,
                connect_timeout_ms           = EXCLUDED.connect_timeout_ms,
                read_timeout_ms              = EXCLUDED.read_timeout_ms,
                val_cache_type               = EXCLUDED.val_cache_type,
                updated_at                   = now(),
                version                      = variables.version + 1",
    )
    .bind(unified)
    .bind(&v.var_name)
    .bind(var_type_to_db(&v.var_type))
    .bind(value_source_to_db(&v.var_val_source))
    .bind(&v.var_associate_data)
    .bind(expression_type_to_db(&v.obtain_value_expression_type))
    .bind(&v.obtain_value_expression)
    .bind(connect_timeout_ms)
    .bind(read_timeout_ms)
    .bind(cache_type_to_db(v.val_cache_type))
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn delete(pool: &PgPool, var_name: &str) -> Result<()> {
    sqlx::query("DELETE FROM variables WHERE var_unified_name = $1")
        .bind(unify_var_name(var_name))
        .execute(pool)
        .await?;
    Ok(())
}

/// 更新一个已存在的变量（B-17 强制乐观锁）。返回是否更新了一行；版本不符返回
/// [`crate::repo::version_conflict`]，不存在返回 `false`。
pub(crate) async fn update(pool: &PgPool, v: &Variable, expected_version: i64) -> Result<bool> {
    let connect_timeout_ms = i64::try_from(v.connect_timeout_milliseconds)
        .map_err(|_| Error::WithMessage(String::from("connectTimeoutMilliseconds 超出范围")))?;
    let read_timeout_ms = i64::try_from(v.read_timeout_milliseconds)
        .map_err(|_| Error::WithMessage(String::from("readTimeoutMilliseconds 超出范围")))?;
    let unified = if v.var_unified_name.is_empty() {
        unify_var_name(&v.var_name)
    } else {
        v.var_unified_name.clone()
    };

    let r = sqlx::query(
        "UPDATE variables
            SET var_name                     = $2,
                var_type                     = $3,
                var_value_source             = $4,
                var_associate_data           = $5,
                obtain_value_expression_type = $6,
                obtain_value_expression      = $7,
                connect_timeout_ms           = $8,
                read_timeout_ms              = $9,
                val_cache_type               = $10,
                updated_at                   = now(),
                version                      = version + 1
          WHERE var_unified_name = $1 AND version = $11",
    )
    .bind(&unified)
    .bind(&v.var_name)
    .bind(var_type_to_db(&v.var_type))
    .bind(value_source_to_db(&v.var_val_source))
    .bind(&v.var_associate_data)
    .bind(expression_type_to_db(&v.obtain_value_expression_type))
    .bind(&v.obtain_value_expression)
    .bind(connect_timeout_ms)
    .bind(read_timeout_ms)
    .bind(cache_type_to_db(v.val_cache_type))
    .bind(expected_version)
    .execute(pool)
    .await?;
    if r.rows_affected() > 0 {
        return Ok(true);
    }
    let exists: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM variables WHERE var_unified_name = $1")
            .bind(&unified)
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
            var_unified_name: r.var_unified_name,
            var_name: r.var_name,
            var_type: var_type_from_db(&r.var_type)?,
            var_val_source: value_source_from_db(&r.var_value_source)?,
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
            val_cache_type: cache_type_from_db(r.val_cache_type)?,
            version: Some(r.version),
        })
    }
}

fn cache_type_to_db(t: VariableValueCacheType) -> i16 {
    match t {
        VariableValueCacheType::NoCache => 0,
        VariableValueCacheType::Request => 1,
        VariableValueCacheType::Session => 2,
    }
}

fn cache_type_from_db(n: i16) -> Result<VariableValueCacheType> {
    match n {
        0 => Ok(VariableValueCacheType::NoCache),
        1 => Ok(VariableValueCacheType::Request),
        2 => Ok(VariableValueCacheType::Session),
        other => Err(Error::WithMessage(format!(
            "Unknown variable val_cache_type in database: {other}"
        ))),
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
        VariableValueSource::ExternalHttp => "ExternalHttp",
    }
}

fn value_source_from_db(s: &str) -> Result<VariableValueSource> {
    match s {
        "Import" => Ok(VariableValueSource::Import),
        "Collect" => Ok(VariableValueSource::Collect),
        "UserInput" => Ok(VariableValueSource::UserInput),
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
