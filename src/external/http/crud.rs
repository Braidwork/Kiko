use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query};
use axum::response::IntoResponse;

use super::dto::HttpReqInfo;
use crate::repo;
use crate::result::{Error, Result};
use crate::web::server::to_res;

pub(crate) async fn list(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        let r: Result<Vec<HttpReqInfo>> = repo::external_http::list(repo::pool(), robot_id).await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

pub(crate) async fn get_detail(robot_id: &str, id: &str) -> Result<Option<HttpReqInfo>> {
    repo::external_http::get(repo::pool(), robot_id, id).await
}

pub(crate) async fn detail(
    Query(q): Query<HashMap<String, String>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        let r: Result<Option<HttpReqInfo>> = get_detail(robot_id, id.as_str()).await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

pub(crate) async fn save(
    Query(q): Query<HashMap<String, String>>,
    Json(mut params): Json<HttpReqInfo>,
) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        if params.id.is_empty() || params.id.eq("new") {
            params.id = scru128::new_string();
        }
        let r = repo::external_http::upsert(repo::pool(), robot_id, &params).await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

pub(crate) async fn remove(
    Query(q): Query<HashMap<String, String>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        let r = repo::external_http::delete(repo::pool(), robot_id, id.as_str()).await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}
