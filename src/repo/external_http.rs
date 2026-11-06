//! `external_http_apis` 仓储（§5.5 / §15.4 S3.2）。
//!
//! 外部 HTTP API 跨机器人共享，表里没有 `robot_id`。参数全部内联在主表：
//!
//! - `headers` / `query_string` / `urlencoded_payload` 是 JSONB 数组，元素形状
//!   与 [`HttpReqParam`] 一致：`{"name","value","valueSource"}`；
//! - `payload` 是原始请求体 TEXT，仅 `post_content_type = "Raw"` 时使用。
//!
//! 保存语义仍是整条覆盖；一行就是一个完整的 [`HttpReqInfo`]，不再需要主表 +
//! 参数子表的事务。

use sqlx::postgres::PgPool;
use sqlx::types::Json;

use crate::external::http::dto::{HttpReqInfo, HttpReqParam, Method, PostContentType, Protocol};
use crate::result::{Error, Result};

#[derive(sqlx::FromRow)]
struct ApiRow {
    id: String,
    name: String,
    description: Option<String>,
    protocol: String,
    method: String,
    address: String,
    post_content_type: String,
    headers: Json<Vec<HttpReqParam>>,
    query_string: Json<Vec<HttpReqParam>>,
    urlencoded_payload: Json<Vec<HttpReqParam>>,
    payload: String,
    content_type: Option<String>,
    user_agent: Option<String>,
    version: i64,
}

const API_COLUMNS: &str = "id, name, description, protocol, method, address, \
                           post_content_type, headers, query_string, urlencoded_payload, \
                           payload, content_type, user_agent, version";

/// 全部 API（跨机器人）。顺序沿用 id 字节序（§5.1）。
pub(crate) async fn list(pool: &PgPool) -> Result<Vec<HttpReqInfo>> {
    let apis: Vec<ApiRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        r#"SELECT {API_COLUMNS} FROM external_http_apis ORDER BY id COLLATE "C""#
    )))
    .fetch_all(pool)
    .await?;
    apis.into_iter().map(build_info).collect()
}

pub(crate) async fn get(pool: &PgPool, id: &str) -> Result<Option<HttpReqInfo>> {
    let api: Option<ApiRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        r#"SELECT {API_COLUMNS} FROM external_http_apis WHERE id = $1"#
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    api.map(build_info).transpose()
}

/// 插入或整条覆盖。一行就是一个完整的 API，不需要事务。
pub(crate) async fn upsert(pool: &PgPool, info: &HttpReqInfo) -> Result<()> {
    sqlx::query(
        "INSERT INTO external_http_apis (
             id, name, description, protocol, method, address,
             post_content_type, headers, query_string, urlencoded_payload,
             payload, content_type, user_agent)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         ON CONFLICT (id) DO UPDATE
            SET name               = EXCLUDED.name,
                description        = EXCLUDED.description,
                protocol           = EXCLUDED.protocol,
                method             = EXCLUDED.method,
                address            = EXCLUDED.address,
                post_content_type  = EXCLUDED.post_content_type,
                headers            = EXCLUDED.headers,
                query_string       = EXCLUDED.query_string,
                urlencoded_payload = EXCLUDED.urlencoded_payload,
                payload            = EXCLUDED.payload,
                content_type       = EXCLUDED.content_type,
                user_agent         = EXCLUDED.user_agent,
                updated_at         = now(),
                version            = external_http_apis.version + 1",
    )
    .bind(&info.id)
    .bind(&info.name)
    .bind(&info.description)
    .bind(protocol_to_db(&info.protocol))
    .bind(method_to_db(&info.method))
    .bind(&info.address)
    .bind(post_content_type_to_db(&info.post_content_type))
    .bind(Json(&info.headers))
    .bind(Json(&info.query_string))
    .bind(Json(&info.urlencoded_payload))
    .bind(&info.payload)
    .bind(&info.content_type)
    .bind(&info.user_agent)
    .execute(pool)
    .await?;
    Ok(())
}

/// 更新一个已存在的 API（B-17 强制乐观锁）。
pub(crate) async fn update(
    pool: &PgPool,
    info: &HttpReqInfo,
    expected_version: i64,
) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE external_http_apis
            SET name               = $2,
                description        = $3,
                protocol           = $4,
                method             = $5,
                address            = $6,
                post_content_type  = $7,
                headers            = $8,
                query_string       = $9,
                urlencoded_payload = $10,
                payload            = $11,
                content_type       = $12,
                user_agent         = $13,
                updated_at         = now(),
                version            = version + 1
          WHERE id = $1 AND version = $14",
    )
    .bind(&info.id)
    .bind(&info.name)
    .bind(&info.description)
    .bind(protocol_to_db(&info.protocol))
    .bind(method_to_db(&info.method))
    .bind(&info.address)
    .bind(post_content_type_to_db(&info.post_content_type))
    .bind(Json(&info.headers))
    .bind(Json(&info.query_string))
    .bind(Json(&info.urlencoded_payload))
    .bind(&info.payload)
    .bind(&info.content_type)
    .bind(&info.user_agent)
    .bind(expected_version)
    .execute(pool)
    .await?;
    if r.rows_affected() > 0 {
        return Ok(true);
    }
    let exists: Option<i32> = sqlx::query_scalar("SELECT 1 FROM external_http_apis WHERE id = $1")
        .bind(&info.id)
        .fetch_optional(pool)
        .await?;
    if exists.is_some() {
        Err(crate::repo::version_conflict("External HTTP API"))
    } else {
        Ok(false)
    }
}

/// 删除一个 API。不存在的 id 不报错（与 redb `remove` 一致）。
pub(crate) async fn delete(pool: &PgPool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM external_http_apis WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

fn build_info(api: ApiRow) -> Result<HttpReqInfo> {
    Ok(HttpReqInfo {
        id: api.id,
        version: Some(api.version),
        name: api.name,
        description: api.description.unwrap_or_default(),
        protocol: protocol_from_db(&api.protocol)?,
        method: method_from_db(&api.method)?,
        address: api.address,
        post_content_type: post_content_type_from_db(&api.post_content_type)?,
        headers: api.headers.0,
        query_string: api.query_string.0,
        urlencoded_payload: api.urlencoded_payload.0,
        payload: api.payload,
        content_type: api.content_type.unwrap_or_default(),
        user_agent: api.user_agent.unwrap_or_default(),
    })
}

// 枚举 ↔ TEXT：存变体名，库里看得懂，列类型仍是 TEXT（§5.5）。
fn protocol_to_db(v: &Protocol) -> &'static str {
    match v {
        Protocol::HTTP => "HTTP",
        Protocol::HTTPS => "HTTPS",
    }
}

fn protocol_from_db(s: &str) -> Result<Protocol> {
    match s {
        "HTTP" => Ok(Protocol::HTTP),
        "HTTPS" => Ok(Protocol::HTTPS),
        other => Err(Error::WithMessage(format!(
            "Unknown external HTTP protocol in database: {other}"
        ))),
    }
}

fn method_to_db(v: &Method) -> &'static str {
    match v {
        Method::GET => "GET",
        Method::POST => "POST",
    }
}

fn method_from_db(s: &str) -> Result<Method> {
    match s {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        other => Err(Error::WithMessage(format!(
            "Unknown external HTTP method in database: {other}"
        ))),
    }
}

fn post_content_type_to_db(v: &PostContentType) -> &'static str {
    match v {
        PostContentType::UrlEncoded => "UrlEncoded",
        PostContentType::Raw => "Raw",
    }
}

fn post_content_type_from_db(s: &str) -> Result<PostContentType> {
    match s {
        "UrlEncoded" => Ok(PostContentType::UrlEncoded),
        "Raw" => Ok(PostContentType::Raw),
        other => Err(Error::WithMessage(format!(
            "Unknown external HTTP post_content_type in database: {other}"
        ))),
    }
}
