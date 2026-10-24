use std::vec::Vec;

use axum::extract::Query;
use axum::{Json, response::IntoResponse};

use super::dto::{RobotData, RobotQuery, RobotType};
use crate::db_executor;
use crate::flow::mainflow::crud as mainflow;
use crate::intent::crud as intent;
use crate::man::settings;
use crate::repo;
use crate::result::{Error, Result};
use crate::variable::crud as variable;
use crate::web::server;
use crate::{db, web::server::to_res};

fn get_robot_id() -> String {
    let mut id = String::with_capacity(32);
    id.push('r');
    let gen_id = scru128::new_string();
    id.push_str(&gen_id);
    id
}

pub(crate) async fn init(is_en: bool) -> Result<()> {
    let name = if is_en {
        "My first robot"
    } else {
        "我的第一个机器人"
    };
    let d = RobotData {
        robot_id: get_robot_id(),
        robot_name: String::from(name),
        robot_type: RobotType::TextBot,
    };
    new(&d, is_en).await
}

pub(crate) async fn save(
    headers: axum::http::HeaderMap,
    Json(mut d): Json<RobotData>,
) -> impl IntoResponse {
    if d.robot_id.is_empty() {
        let is_en = server::is_en(&headers);
        d.robot_id = get_robot_id();
        if let Err(e) = new(&d, is_en).await {
            return to_res(Err(Error::WithMessage(format!(
                "Failed to create robot, error detail was: {e:?}"
            ))));
        }
        return to_res(Ok(()));
    }
    // 编辑已有机器人：机器人行在 PG，其余子实体仍在 redb/turso。
    to_res(persist(&d).await)
}

async fn new(d: &RobotData, is_en: bool) -> Result<()> {
    persist(d).await?;
    // 机器人意图
    settings::init(&d.robot_id).await?;
    // crate::intent::phrase::init_tables(&d.robot_id).await?;
    crate::kb::qa::init_tables(&d.robot_id).await?;
    crate::kb::doc::init_tables(&d.robot_id).await?;
    // 意图
    intent::init(&d.robot_id, is_en).await?;
    // 变量（已在 PG）
    variable::init(&d.robot_id, is_en).await?;
    // 主流程
    mainflow::init(&d.robot_id).await?;
    // 外部 HTTP 接口：表在 PG，按 robot_id 外键挂在机器人下，不需要建表。
    // 流程上下文：表在 PG，不需要建表（也不再是每机器人一张）。
    Ok(())
}

async fn persist(d: &RobotData) -> Result<()> {
    repo::robot::upsert(repo::pool(), d).await
}

pub(crate) async fn list() -> impl IntoResponse {
    let r: Result<Vec<RobotData>> = repo::robot::list(repo::pool()).await;
    to_res(r)
}

pub(crate) async fn detail(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    let r = repo::robot::get(repo::pool(), &q.robot_id).await;
    to_res(r)
}

pub(crate) async fn delete(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    to_res(purge(&q.robot_id).await)
}

/// 删机器人 = 跨库的两段式清理（§15.3 硬规矩 2）：**不做双写**，每一步都幂等、
/// 可重试，允许中间态残留。
///
/// 顺序是有意的：先把 turso / redb 里按 `robot_id` 命名的表清掉，最后才删 PG 里的
/// `robots` 行（`variables` / `intents` / `contexts` / `external_http_apis` /
/// `robot_settings` 由 FK 级联）。反过来的话，中途失败会留下「机器人还在、子数据
/// 已删」的半截状态；按这个顺序失败，残留的是孤儿表，重跑一次 purge 就能补齐。
async fn purge(robot_id: &str) -> Result<()> {
    // turso 三件套：这些表在各自文件里都只属于这一个机器人，整表删掉。
    // 三个 helper 都带 `IF EXISTS` —— 表是懒创建的，没建过就得跳过而不是报错，
    // 否则第一步失败会让整个 purge 什么都不做。
    crate::intent::phrase::remove_tables(robot_id).await?;
    crate::kb::qa::remove_tables(robot_id).await?;
    crate::kb::doc::remove_tables(robot_id).await?;
    // 每机器人的设置表（原来只是全局 settings 表里以 robot_id 为键的一行）
    db_executor!(db::delete_table, robot_id, settings::TABLE_SUFFIX,)?;
    db_executor!(
        db::delete_table,
        robot_id,
        crate::flow::subflow::crud::TABLE_SUFFIX,
    )?;
    let r: Vec<crate::flow::mainflow::dto::MainFlowDetail> = db_executor!(
        db::get_all,
        robot_id,
        crate::flow::mainflow::crud::TABLE_SUFFIX,
    )?;
    // 运行时节点表是按 main_flow_id 命名、归属该机器人的，删主流程之前先清掉。
    let store = db::store(db::StoreKey::Robot(robot_id)).await?;
    for v in r.iter() {
        crate::flow::rt::crud::remove_runtime_nodes(store, &v.id).await?;
    }
    db_executor!(
        db::delete_table,
        robot_id,
        crate::flow::mainflow::crud::TABLE_SUFFIX,
    )?;
    // 最后才是 PG：机器人行没了，按 robot_id 外键挂在它下面的数据由级联清掉。
    repo::robot::delete(repo::pool(), robot_id).await?;
    Ok(())
}
