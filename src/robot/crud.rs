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
        version: None,
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
    // 编辑已有机器人：机器人行、设置、流程、意图、知识库都在 PG。
    // B-17 强制乐观锁：编辑必须带上读到的版本，缺了直接拒绝。
    let Some(version) = d.version else {
        return to_res(Err(repo::version_required("Robot")));
    };
    let r = async {
        if !repo::robot::update(repo::pool(), &d, version).await? {
            return Err(Error::WithMessage(format!(
                "Robot {} does not exist any more.",
                d.robot_id
            )));
        }
        Ok(())
    }
    .await;
    to_res(r)
}

async fn new(d: &RobotData, is_en: bool) -> Result<()> {
    persist(d).await?;
    // 机器人意图
    settings::init(&d.robot_id).await?;
    // 知识库（documents / doc_chunks / qa / qa_questions / vector_items）全部由
    // migration 建表、按 robot_id 外键挂在机器人下，新机器人**不需要建表**。
    // 意图
    intent::init(&d.robot_id, is_en).await?;
    // 变量（已在 PG）
    variable::init(&d.robot_id, is_en).await?;
    // 主流程
    mainflow::init(&d.robot_id).await?;
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

/// 删机器人：全部数据都在 PG，`robots` 的 FK 级联负责业务行；剩下两处要显式清。
///
/// 1. `secrets.scope` 没有外键，不跟着级联 —— 必须显式删；
/// 2. `data/uploads/<robot_id>/` 是本地文件（D30 / §5.6，A 里这段被注释掉了）。
///
/// 顺序：先清文件、再删机器人行、最后清 secrets。反过来失败会留下"机器人还在、
/// 文件没了"；按这个顺序失败，残留的是可重试的孤儿目录或孤儿 secret。每一步都
/// 幂等（目录不存在 / 行已不存在都当成功），所以重跑一次 purge 就能补齐。
/// 删完还要把这个机器人的内存向量索引作废（`vector::invalidate_robot`）。
async fn purge(robot_id: &str) -> Result<()> {
    crate::kb::doc::remove_uploads(robot_id).await?;
    repo::robot::delete(repo::pool(), robot_id).await?;
    repo::secret::delete_scope(repo::pool(), robot_id).await?;
    for collection in repo::vector::COLLECTIONS {
        crate::vector::clear(robot_id, collection);
    }
    Ok(())
}
