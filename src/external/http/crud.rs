use axum::Json;
use axum::extract::Path;
use axum::response::IntoResponse;

use super::dto::HttpReqInfo;
use crate::repo;
use crate::result::{Error, Result};
use crate::web::server::to_res;

pub(crate) async fn list() -> impl IntoResponse {
    let r: Result<Vec<HttpReqInfo>> = repo::external_http::list(repo::pool()).await;
    to_res(r)
}

pub(crate) async fn get_detail(id: &str) -> Result<Option<HttpReqInfo>> {
    repo::external_http::get(repo::pool(), id).await
}

pub(crate) async fn detail(Path(id): Path<String>) -> impl IntoResponse {
    let r: Result<Option<HttpReqInfo>> = get_detail(id.as_str()).await;
    to_res(r)
}

pub(crate) async fn save(Json(mut params): Json<HttpReqInfo>) -> impl IntoResponse {
    if params.id.is_empty() || params.id.eq("new") {
        params.id = scru128::new_string();
        return to_res(repo::external_http::upsert(repo::pool(), &params).await);
    }
    // 编辑已有 API：B-17 强制乐观锁。
    let Some(version) = params.version else {
        return to_res::<()>(Err(repo::version_required("External HTTP API")));
    };
    let r = async {
        if !repo::external_http::update(repo::pool(), &params, version).await? {
            return Err(Error::WithMessage(format!(
                "External HTTP API {} does not exist any more.",
                params.id
            )));
        }
        Ok(())
    }
    .await;
    to_res(r)
}

pub(crate) async fn remove(Path(id): Path<String>) -> impl IntoResponse {
    let r = repo::external_http::delete(repo::pool(), id.as_str()).await;
    to_res(r)
}
