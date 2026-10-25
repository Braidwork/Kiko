use std::collections::HashMap;

use axum::{
    Json,
    extract::{Multipart, Query},
    response::IntoResponse,
};

use super::doc;
use super::dto::{DocData, QuestionAnswerPair};
use crate::result::{Error, Result};
use crate::robot::dto::RobotQuery;
use crate::web::server::to_res;

pub(crate) async fn list_doc(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    let r = super::doc::list(&q.robot_id).await;
    to_res(r)
}

pub(crate) async fn upload_doc(
    Query(q): Query<RobotQuery>,
    multipart: Multipart,
) -> impl IntoResponse {
    let r = save_doc(&q.robot_id, multipart).await;
    to_res(r)
}

async fn save_doc(robot_id: &str, mut multipart: Multipart) -> Result<String> {
    // 只取第一个文件字段（与 A 的行为一致：多文件上传只看第一个）。
    let Some(field) = multipart.next_field().await? else {
        return Err(Error::WithMessage(String::from("File not found.")));
    };
    let Some(file_name) = field.file_name() else {
        return Err(Error::WithMessage(String::from("File name is missing.")));
    };
    let file_name = file_name.to_string();
    let Some(content_type) = field.content_type() else {
        return Err(Error::WithMessage(String::from("Content type is missing.")));
    };
    let content_type = content_type.to_string();
    let data = field.bytes().await?;

    log::info!(
        "Length of `{file_name}`: `{content_type}` is {} bytes",
        data.len()
    );

    // 正文解析、分块、推理、落库、原文件保存都在 `kb::doc::save` 里；这里只
    // 负责把 multipart 拆开。返回值仍是抽取出的正文（与 A 的接口一致）。
    let text = doc::save(robot_id, &file_name, &content_type, &data).await?;
    log::info!("Extract text: {text}");
    Ok(text)
}

pub(crate) async fn update_doc(
    Query(q): Query<RobotQuery>,
    Json(doc): Json<DocData>,
) -> impl IntoResponse {
    let r = super::doc::update(&q.robot_id, &doc.id, &doc.doc_content, doc.version).await;
    to_res(r)
}

#[axum::debug_handler]
pub(crate) async fn delete_doc(
    Query(q): Query<RobotQuery>,
    Json(doc): Json<DocData>,
) -> impl IntoResponse {
    let r = super::doc::delete(&q.robot_id, &doc.id).await;
    to_res(r)
}

pub(crate) async fn list_qa(Query(q): Query<RobotQuery>) -> impl IntoResponse {
    let r = super::qa::list(&q.robot_id).await;
    to_res(r)
}

pub(crate) async fn save_qa(
    Query(q): Query<RobotQuery>,
    Json(d): Json<QuestionAnswerPair>,
) -> impl IntoResponse {
    let r = super::qa::save(&q.robot_id, d).await;
    // let r = sqlite_trans!(super::qa::add, &q.robot_id, d).await;
    to_res(r)
}

pub(crate) async fn delete_qa(
    Query(q): Query<RobotQuery>,
    Json(d): Json<QuestionAnswerPair>,
) -> impl IntoResponse {
    let r = super::qa::delete(&q.robot_id, d).await;
    to_res(r)
}

pub(crate) async fn qa_dryrun(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    let r = q.get("robotId");
    let t = q.get("text");
    if r.is_none() || t.is_none() {
        let res = Err(Error::WithMessage(String::from(
            "robotId or text was missing.",
        )));
        return to_res(res);
    }
    let r = super::qa::retrieve_answer(r.unwrap(), t.unwrap()).await;
    to_res(r)
}
