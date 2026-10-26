use axum::Json;
use axum::extract::Query;
use axum::http::{StatusCode, header::HeaderMap};
use axum::response::{IntoResponse, Response};

use super::dto::{SubFlowDetail, SubFlowFormData};
use crate::flow::demo;
use crate::repo;
use crate::result::Result;
use crate::web::server::{self, to_res};

pub(crate) async fn list(headers: HeaderMap, Query(q): Query<SubFlowFormData>) -> Response {
    let is_en = server::is_en(&headers);
    if let Some(t) = demo::get_demo(is_en, &q.main_flow_id) {
        return (StatusCode::OK, t).into_response();
    }
    let r: Result<Vec<SubFlowDetail>> =
        repo::subflow::list(repo::pool(), &q.robot_id, &q.main_flow_id).await;
    to_res(r).into_response()
}

pub(crate) async fn simple_list(Query(q): Query<SubFlowFormData>) -> Response {
    let r: Result<Vec<SubFlowDetail>> = async {
        let mut flows = repo::subflow::list(repo::pool(), &q.robot_id, &q.main_flow_id).await?;
        for f in flows.iter_mut() {
            f.canvas.clear();
        }
        Ok(flows)
    }
    .await;
    to_res(r).into_response()
}

pub(crate) async fn new_subflow(
    robot_id: &str,
    mainflow_id: &str,
    subflow_name: &str,
) -> Result<Vec<SubFlowDetail>> {
    // 追加是单行 INSERT + 父主流程行锁，不再读整条列表再写回（§16.4）。
    repo::subflow::create(repo::pool(), robot_id, mainflow_id, subflow_name).await
}

pub(crate) async fn new(Query(form): Query<SubFlowFormData>) -> impl IntoResponse {
    if let Err(e) = demo::ensure_editable(&form.main_flow_id) {
        return to_res::<Vec<SubFlowDetail>>(Err(e));
    }
    to_res(new_subflow(&form.robot_id, &form.main_flow_id, &form.data).await)
}

/// 保存画布。`data.id` 是子流程 id（D10，不再使用数组下标）；返回保存后的完整列表。
pub(crate) async fn save(
    Query(q): Query<SubFlowFormData>,
    Json(data): Json<SubFlowDetail>,
) -> impl IntoResponse {
    if let Err(e) = demo::ensure_editable(&q.main_flow_id) {
        return to_res::<Vec<SubFlowDetail>>(Err(e));
    }
    let Some(version) = data.version else {
        return to_res::<Vec<SubFlowDetail>>(Err(crate::repo::version_required("Sub-flow")));
    };
    let r: Result<Vec<SubFlowDetail>> = async {
        repo::subflow::save_canvas(
            repo::pool(),
            &q.robot_id,
            &q.main_flow_id,
            &data.id,
            &data.canvas,
            version,
        )
        .await?;
        repo::subflow::list(repo::pool(), &q.robot_id, &q.main_flow_id).await
    }
    .await;
    to_res(r)
}

/// 删除子流程。`data` 是子流程 id（D10），不再是数组下标。
pub(crate) async fn delete(Query(q): Query<SubFlowFormData>) -> impl IntoResponse {
    if let Err(e) = demo::ensure_editable(&q.main_flow_id) {
        return to_res::<()>(Err(e));
    }
    let r: Result<()> = async {
        repo::subflow::delete(repo::pool(), &q.robot_id, &q.main_flow_id, &q.data).await?;
        Ok(())
    }
    .await;
    to_res(r)
}

pub(crate) async fn release(
    headers: HeaderMap,
    Query(q): Query<SubFlowFormData>,
) -> impl IntoResponse {
    let is_en = server::is_en(&headers);
    let r = crate::flow::rt::convertor::convert_flow(is_en, &q.robot_id, &q.main_flow_id).await;
    to_res(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn demo_form() -> SubFlowFormData {
        SubFlowFormData {
            robot_id: String::from("r1"),
            main_flow_id: String::from("demo-collect"),
            data: String::from("s1"),
        }
    }

    async fn body_string(r: Response) -> String {
        let b = to_bytes(r.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    /// demo 是平台共用的只读模板：保存画布、新建、删除都必须返回业务错误，
    /// 不能像普通主流程那样静默成功（§18）。
    #[tokio::test]
    async fn demo_subflow_is_read_only() {
        let data = SubFlowDetail {
            id: String::from("s1"),
            name: String::from("x"),
            canvas: String::from("{}"),
            version: None,
        };
        let body = body_string(save(Query(demo_form()), Json(data)).await.into_response()).await;
        assert!(body.contains("read-only"), "{body}");

        let body = body_string(delete(Query(demo_form())).await.into_response()).await;
        assert!(body.contains("read-only"), "{body}");

        let body = body_string(new(Query(demo_form())).await.into_response()).await;
        assert!(body.contains("read-only"), "{body}");
    }
}
