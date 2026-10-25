use std::collections::HashMap;
use std::sync::OnceLock;

use axum::extract::Query;
use axum::{Json, response::IntoResponse};

use super::dto::MainFlowDetail;
use crate::flow::demo;
use crate::repo;
use crate::result::{Error, Result};
use crate::web::server::to_res;

static DEFAULT_NAMES: OnceLock<(String, String)> = OnceLock::new();

pub(crate) fn init_default_names(is_en: bool) -> Result<()> {
    let (name, subflow_name) = if is_en {
        ("The first main flow", "First sub-flow")
    } else {
        ("第一个主流程", "第一个子流程")
    };
    DEFAULT_NAMES
        .set((String::from(name), String::from(subflow_name)))
        .map_err(|_| Error::WithMessage(String::from("Dup")))
}

pub(crate) async fn init(robot_id: &str) -> Result<MainFlowDetail> {
    create_main_flow(robot_id, &DEFAULT_NAMES.get().unwrap().0).await
}

pub(crate) async fn list(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        let r: Result<Vec<MainFlowDetail>> = repo::mainflow::list(repo::pool(), robot_id).await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robot_id is missing.",
        ))))
    }
}

pub(crate) async fn new(
    Query(q): Query<HashMap<String, String>>,
    Json(data): Json<MainFlowDetail>,
) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        to_res::<MainFlowDetail>(create_main_flow(robot_id, &data.name).await)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

async fn create_main_flow(robot_id: &str, name: &str) -> Result<MainFlowDetail> {
    // D48：id 由仓储直接生成 scru128，不再 `count + 1` 拼前缀；新建主流程与默认
    // 子流程在同一个事务里（见 `repo::mainflow::create`）。
    repo::mainflow::create(
        repo::pool(),
        robot_id,
        name,
        &DEFAULT_NAMES.get().unwrap().1,
    )
    .await
}

pub(crate) async fn save(
    Query(q): Query<HashMap<String, String>>,
    Json(data): Json<MainFlowDetail>,
) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        if let Err(e) = demo::ensure_editable(&data.id) {
            return to_res::<()>(Err(e));
        }
        let Some(version) = data.version else {
            return to_res::<()>(Err(crate::repo::version_required("Main flow")));
        };
        let r: Result<()> = async {
            if !repo::mainflow::update(repo::pool(), robot_id, &data, version).await? {
                return Err(Error::WithMessage(format!(
                    "Main flow {} does not exist any more.",
                    data.id
                )));
            }
            Ok(())
        }
        .await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

pub(crate) async fn delete(
    Query(q): Query<HashMap<String, String>>,
    Json(data): Json<MainFlowDetail>,
) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        if let Err(e) = demo::ensure_editable(&data.id) {
            return to_res::<()>(Err(e));
        }
        let r: Result<()> = async {
            // 运行时节点在同一事务里显式清（没有 mainflows 外键，见 §5.3）；
            // 子流程由 FK 级联。
            repo::mainflow::delete(repo::pool(), robot_id, &data.id).await?;
            Ok(())
        }
        .await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn demo_query() -> Query<HashMap<String, String>> {
        Query(HashMap::from([(
            String::from("robotId"),
            String::from("r1"),
        )]))
    }

    fn demo_flow(id: &str) -> MainFlowDetail {
        MainFlowDetail {
            id: String::from(id),
            name: String::from("demo"),
            enabled: true,
            version: None,
        }
    }

    async fn body_string(r: axum::response::Response) -> String {
        let b = to_bytes(r.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    /// demo 的固定 id 不能通过主流程保存/删除接口修改。
    #[tokio::test]
    async fn demo_main_flow_is_read_only() {
        for id in [
            crate::flow::demo::DEMO_REPAY_ID,
            crate::flow::demo::DEMO_COLLECT_ID,
            crate::flow::demo::DEMO_NOTIFY_ID,
        ] {
            let body = body_string(
                save(demo_query(), Json(demo_flow(id)))
                    .await
                    .into_response(),
            )
            .await;
            assert!(body.contains("read-only"), "{id}: {body}");

            let body = body_string(
                delete(demo_query(), Json(demo_flow(id)))
                    .await
                    .into_response(),
            )
            .await;
            assert!(body.contains("read-only"), "{id}: {body}");
        }
    }
}
