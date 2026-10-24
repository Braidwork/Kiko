use axum::extract::Query;
use axum::{Json, response::IntoResponse};

use super::dto::{RobotData, RobotQuery, RobotType};
use crate::flow::mainflow::crud as mainflow;
use crate::intent::crud as intent;
use crate::man::settings;
use crate::repo;
use crate::result::{Error, Result};
use crate::variable::crud as variable;
use crate::web::server;
use crate::web::server::to_res;

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
    // 编辑已有机器人：机器人行与设置/流程/意图等都在 PG；KB 向量仍在 turso。
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
/// 顺序是有意的：先把 turso 里按 `robot_id` 命名的表清掉，再删 PG 里的 `robots`
/// 行（`variables` / `intents` / `contexts` / `external_http_apis` / `mainflows` /
/// `subflows` / `runtime_nodes` / `robot_settings` 由 FK 级联），最后清 `secrets`
/// （它没有外键，不会跟着级联）。反过来先删 robots 会留下「机器人还在、子数据
/// 已删」的半截状态；按这个顺序失败，残留的是孤儿表/孤儿 secret，重跑一次 purge
/// 就能补齐。
async fn purge(robot_id: &str) -> Result<()> {
    // turso 三件套：这些表在各自文件里都只属于这一个机器人，整表删掉。
    // 三个 helper 都带 `IF EXISTS` —— 表是懒创建的，没建过就得跳过而不是报错，
    // 否则第一步失败会让整个 purge 什么都不做。
    crate::intent::phrase::remove_tables(robot_id).await?;
    crate::kb::qa::remove_tables(robot_id).await?;
    crate::kb::doc::remove_tables(robot_id).await?;
    // secrets.scope 没有外键，必须显式清。放在删机器人**之后**：如果删机器人失败，
    // 至少不会先把还活着的机器人的密钥清掉；反过来失败只留下可重试的孤儿 secret。
    repo::robot::delete(repo::pool(), robot_id).await?;
    repo::secret::delete_scope(repo::pool(), robot_id).await?;
    Ok(())
}
