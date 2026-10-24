//! `external_http_apis` / `external_http_params` 仓储（§5.5 / §15.4 S3.2）。
//!
//! A 把这个聚合整条 JSON 塞进 redb，键是 API 的 id；B 拆成「主表 + 参数子表」，
//! 参数用 `kind`（header / query / form）+ `position` 保序。读写都要保持
//! `HttpReqInfo` 的原有形状：三组参数各自有序、`valueSource` 原样往返。
//!
//! 保存语义与 A 的整条覆盖一致：同一个 id 再存一次 = 主表更新 + 参数整组替换。
//! 参数 id 是内部行号，不出现在 API 契约里。

use std::collections::HashMap;

use sqlx::postgres::PgPool;

use crate::external::http::dto::{
    HttpReqInfo, HttpReqParam, Method, PostContentType, Protocol, ValueSource,
};
use crate::result::{Error, Result};

const KIND_HEADER: &str = "header";
const KIND_QUERY: &str = "query";
const KIND_FORM: &str = "form";

#[derive(sqlx::FromRow)]
struct ApiRow {
    id: String,
    name: String,
    description: Option<String>,
    protocol: String,
    method: String,
    address: String,
    post_content_type: String,
    request_body: Option<String>,
    content_type: Option<String>,
    user_agent: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ParamRow {
    api_id: String,
    kind: String,
    name: String,
    value: String,
    value_source: String,
}

const API_COLUMNS: &str = "id, name, description, protocol, method, address, \
                           post_content_type, request_body, content_type, user_agent";

/// 一个机器人的全部 API。顺序沿用 A 的 redb key 序（id 字节序，§5.1）。
pub(crate) async fn list(pool: &PgPool, robot_id: &str) -> Result<Vec<HttpReqInfo>> {
    let apis: Vec<ApiRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        r#"SELECT {API_COLUMNS} FROM external_http_apis
            WHERE robot_id = $1
            ORDER BY id COLLATE "C""#
    )))
    .bind(robot_id)
    .fetch_all(pool)
    .await?;
    if apis.is_empty() {
        return Ok(Vec::new());
    }

    // 一次把参数取全再分组，避免每个 API 一次查询（列表页一次要显示几十条）。
    let params: Vec<ParamRow> = sqlx::query_as(
        r#"SELECT p.api_id, p.kind, p.name, p.value, p.value_source
             FROM external_http_params p
             JOIN external_http_apis a ON a.id = p.api_id
            WHERE a.robot_id = $1
            ORDER BY p.api_id COLLATE "C", p.position"#,
    )
    .bind(robot_id)
    .fetch_all(pool)
    .await?;

    let mut grouped: HashMap<String, Vec<ParamRow>> = HashMap::new();
    for p in params {
        grouped.entry(p.api_id.clone()).or_default().push(p);
    }

    apis.into_iter()
        .map(|api| {
            let params = grouped.remove(&api.id).unwrap_or_default();
            build_info(api, params)
        })
        .collect()
}

pub(crate) async fn get(pool: &PgPool, robot_id: &str, id: &str) -> Result<Option<HttpReqInfo>> {
    let api: Option<ApiRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        r#"SELECT {API_COLUMNS} FROM external_http_apis WHERE id = $1 AND robot_id = $2"#
    )))
    .bind(id)
    .bind(robot_id)
    .fetch_optional(pool)
    .await?;
    let Some(api) = api else {
        return Ok(None);
    };

    let params: Vec<ParamRow> = sqlx::query_as(
        r#"SELECT api_id, kind, name, value, value_source
             FROM external_http_params
            WHERE api_id = $1
            ORDER BY position"#,
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    Ok(Some(build_info(api, params)?))
}

/// 插入或整条覆盖。参数在同一个事务里整组替换，避免「主表更新了、参数还是旧的」。
pub(crate) async fn upsert(pool: &PgPool, robot_id: &str, info: &HttpReqInfo) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO external_http_apis (
             id, robot_id, name, description, protocol, method, address,
             post_content_type, request_body, content_type, user_agent)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         ON CONFLICT (id) DO UPDATE
            SET name              = EXCLUDED.name,
                description       = EXCLUDED.description,
                protocol          = EXCLUDED.protocol,
                method            = EXCLUDED.method,
                address           = EXCLUDED.address,
                post_content_type = EXCLUDED.post_content_type,
                request_body      = EXCLUDED.request_body,
                content_type      = EXCLUDED.content_type,
                user_agent        = EXCLUDED.user_agent,
                updated_at        = now(),
                version           = external_http_apis.version + 1",
    )
    .bind(&info.id)
    .bind(robot_id)
    .bind(&info.name)
    .bind(&info.description)
    .bind(protocol_to_db(&info.protocol))
    .bind(method_to_db(&info.method))
    .bind(&info.address)
    .bind(post_content_type_to_db(&info.post_content_type))
    .bind(&info.request_body)
    .bind(&info.content_type)
    .bind(&info.user_agent)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM external_http_params WHERE api_id = $1")
        .bind(&info.id)
        .execute(&mut *tx)
        .await?;

    for (kind, list) in [
        (KIND_HEADER, &info.headers),
        (KIND_QUERY, &info.query_params),
        (KIND_FORM, &info.form_data),
    ] {
        for (position, p) in list.iter().enumerate() {
            sqlx::query(
                "INSERT INTO external_http_params
                     (id, api_id, kind, position, name, value, value_source)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(scru128::new_string())
            .bind(&info.id)
            .bind(kind)
            .bind(position as i32)
            .bind(&p.name)
            .bind(&p.value)
            .bind(value_source_to_db(&p.value_source))
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(())
}

/// 删除一个 API。参数由 FK 级联；不存在的 id 不报错（与 redb `remove` 一致）。
pub(crate) async fn delete(pool: &PgPool, robot_id: &str, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM external_http_apis WHERE id = $1 AND robot_id = $2")
        .bind(id)
        .bind(robot_id)
        .execute(pool)
        .await?;
    Ok(())
}

fn build_info(api: ApiRow, params: Vec<ParamRow>) -> Result<HttpReqInfo> {
    let mut headers = Vec::new();
    let mut query_params = Vec::new();
    let mut form_data = Vec::new();

    for p in params {
        let param = HttpReqParam {
            name: p.name,
            value: p.value,
            value_source: value_source_from_db(&p.value_source)?,
        };
        match p.kind.as_str() {
            KIND_HEADER => headers.push(param),
            KIND_QUERY => query_params.push(param),
            KIND_FORM => form_data.push(param),
            other => {
                return Err(Error::WithMessage(format!(
                    "Unknown external_http_params kind in database: {other}"
                )));
            }
        }
    }

    Ok(HttpReqInfo {
        id: api.id,
        name: api.name,
        description: api.description.unwrap_or_default(),
        protocol: protocol_from_db(&api.protocol)?,
        method: method_from_db(&api.method)?,
        address: api.address,
        post_content_type: post_content_type_from_db(&api.post_content_type)?,
        headers,
        query_params,
        form_data,
        request_body: api.request_body.unwrap_or_default(),
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

fn value_source_to_db(v: &ValueSource) -> &'static str {
    match v {
        ValueSource::Val => "Val",
        ValueSource::Var => "Var",
    }
}

fn value_source_from_db(s: &str) -> Result<ValueSource> {
    match s {
        "Val" => Ok(ValueSource::Val),
        "Var" => Ok(ValueSource::Var),
        other => Err(Error::WithMessage(format!(
            "Unknown external HTTP value_source in database: {other}"
        ))),
    }
}
