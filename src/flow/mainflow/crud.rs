use std::collections::HashMap;
use std::sync::OnceLock;

use axum::extract::Query;
use axum::{Json, response::IntoResponse};

use super::dto::MainFlowDetail;
use crate::db;
use crate::db_executor;
use crate::flow::subflow::crud as subflow;
use crate::result::{Error, Result};
use crate::web::server::to_res;

// const TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("mainflows");
pub(crate) const TABLE_SUFFIX: &str = "mainflows";

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
    // let table_name = format!("{}-mainflows", robot_id);
    // let main_flow_table: TableDefinition<&str, &[u8]> = TableDefinition::new(&table_name);
    // db::init_table(main_flow_table)?;
    db_executor!(db::init_table, robot_id, TABLE_SUFFIX,)?;
    // let table_name = format!("{}-subflows", robot_id);
    // let sub_flow_table: TableDefinition<&str, &[u8]> = TableDefinition::new(&table_name);
    // db::init_table(sub_flow_table)?;
    db_executor!(
        db::init_table,
        robot_id,
        crate::flow::subflow::crud::TABLE_SUFFIX,
    )?;
    create_main_flow(robot_id, &DEFAULT_NAMES.get().unwrap().0).await
}

pub(crate) async fn list(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    // to_res::<Vec<MainFlowDetail>>(db::get_all(TABLE))
    if let Some(robot_id) = q.get("robotId") {
        let r: Result<Vec<MainFlowDetail>> = async {
            db_executor!(db::get_all, robot_id, TABLE_SUFFIX,)
        }
        .await;
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
    // 这里原有一把 `Mutex<bool>` 保护 count 的读-改-写，现在去掉了：写由 redb 的单写
    // 线程串行化，而 id 的唯一性本来就来自 scru128 后缀 —— count 前缀只是给人看的序号，
    // 并发下偶尔重复不影响任何查找（所有查询都按完整 id 走）。
    let count = db_executor!(db::count, robot_id, TABLE_SUFFIX,)?;
    let mut buffer = itoa::Buffer::new();
    let count = buffer.format(count + 1);
    let id = format!("{}{}", count, scru128::new_string());
    let main_flow = MainFlowDetail {
        id,
        name: String::from(name),
        enabled: true,
    };
    // db::write(TABLE, main_flow.id.as_str(), &main_flow)?;
    db_executor!(
        db::write,
        robot_id,
        TABLE_SUFFIX,
        main_flow.id.as_str(),
        &main_flow
    )?;
    subflow::new_subflow(robot_id, &main_flow.id, &DEFAULT_NAMES.get().unwrap().1).await?;
    Ok(main_flow)
}

pub(crate) async fn save(
    Query(q): Query<HashMap<String, String>>,
    Json(data): Json<MainFlowDetail>,
) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        let main_flow = MainFlowDetail {
            id: data.id.clone(),
            name: data.name.clone(),
            enabled: data.enabled,
        };
        let r: Result<()> = async {
            db_executor!(
                db::write,
                robot_id,
                TABLE_SUFFIX,
                &data.id,
                &main_flow
            )
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
        let main_flow_id = data.id.as_str();
        // 运行时节点表归属该机器人，先解析 store 清掉它，再删主流程本身。
        let r: Result<()> = async {
            let store = db::store(db::StoreKey::Robot(robot_id.as_str())).await?;
            crate::flow::rt::crud::remove_runtime_nodes(store, main_flow_id).await?;
            db_executor!(db::remove, robot_id, TABLE_SUFFIX, main_flow_id)
        }
        .await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}
