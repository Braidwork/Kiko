use std::collections::HashMap;
use std::vec::Vec;

use axum::Json;
use axum::extract::Query;
use axum::response::IntoResponse;

use super::detector;
use super::dto::{IntentDetail, IntentFormData};
use crate::repo;
use crate::result::{Error, Result};
use crate::web::server::to_res;

pub(crate) async fn init(robot_id: &str, is_en: bool) -> Result<()> {
    // Positive
    let keywords = if is_en {
        vec![
            "sure",
            "ok",
            "okay",
            "no problem",
            "affim",
            "certainly",
            "of course",
            "definitely",
            "correct",
            "pleasant",
            "yes",
        ]
    } else {
        vec![
            "嗯", "恩", "可以", "是", "是的", "好", "好的", "对", "对的", "ok", "OK", "Ok", "知道",
            "明白", "行", "愿意", "方便", "正确",
        ]
    };
    let keywords: Vec<String> = keywords.into_iter().map(String::from).collect();
    // 意图与关键词都在 PG（S3.3）；新机器人的两条种子意图按顺序拿 position 0 / 1。
    repo::intent::create(
        repo::pool(),
        robot_id,
        if is_en { "Positive" } else { "肯定" },
        &keywords,
    )
    .await?;

    // Negative
    let keywords = if is_en {
        vec![
            "no",
            "not",
            "Reject",
            "can't",
            "can not",
            "cannot",
            "deny",
            "forbid",
            "forbidden",
            "stop",
            "gross",
            "impossible",
            "never",
            "rarely",
            "hardly",
            "none",
            "nothing",
            "incorrect",
            "awful",
            "unpleasant",
            "sick",
            "disappointed",
        ]
    } else {
        vec![
            "no",
            "不",
            "不是",
            "不要",
            "不用",
            "没用",
            "不好",
            "没",
            "没有",
            "不清楚",
            "不知道",
            "不明白",
            "不可以",
            "不行",
            "不愿意",
            "不考虑",
            "不方便",
            "不正确",
        ]
    };
    let keywords: Vec<String> = keywords.into_iter().map(String::from).collect();
    repo::intent::create(
        repo::pool(),
        robot_id,
        if is_en { "Negative" } else { "否定" },
        &keywords,
    )
    .await?;

    Ok(())
}

pub(crate) async fn list(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        let r: Result<Vec<IntentDetail>> = repo::intent::list(repo::pool(), robot_id).await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

pub(crate) async fn add(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r = add_intent(&params.robot_id, params.data.as_str()).await;
    to_res(r)
}

async fn add_intent(robot_id: &str, intent_name: &str) -> Result<String> {
    repo::intent::create(repo::pool(), robot_id, intent_name, &[]).await
}

pub(crate) async fn remove(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<()> = async {
        // 短语向量在 turso，先清（幂等），再删 PG 行；PG 级联掉关键词/正则/短语。
        // 两步之间失败只会留下可由重建补齐的孤儿向量，不会留下「意图还在但向量没了」。
        super::phrase::remove_by_intent_id(&params.robot_id, params.id.as_str()).await?;
        repo::intent::delete(repo::pool(), &params.robot_id, params.id.as_str()).await?;
        Ok(())
    }
    .await;
    to_res(r)
}

pub(in crate::intent) async fn get_detail_by_id(
    robot_id: &str,
    intent_id: &str,
) -> Result<Option<IntentDetail>> {
    repo::intent::get(repo::pool(), robot_id, intent_id).await
}

pub(crate) async fn detail(Query(params): Query<IntentFormData>) -> impl IntoResponse {
    let r = get_detail_by_id(&params.robot_id, params.id.as_str()).await;
    to_res(r)
}

/// 追加关键词。返回新关键词的 id，前端据此删除（D10：不再用数组下标）。
pub(crate) async fn add_keyword(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r = repo::intent::add_keyword(
        repo::pool(),
        &params.robot_id,
        params.id.as_str(),
        params.data.as_str(),
    )
    .await;
    to_res(r)
}

/// `data` 是关键词的 id（不再是下标）。
pub(crate) async fn remove_keyword(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<()> = async {
        repo::intent::remove_keyword(
            repo::pool(),
            &params.robot_id,
            params.id.as_str(),
            params.data.as_str(),
        )
        .await?;
        Ok(())
    }
    .await;
    to_res(r)
}

pub(crate) async fn add_regex(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<Option<String>> = async {
        // 语法错误在写库之前就返回（与 A 一致）。
        let _ = regex::Regex::new(params.data.as_str())?;
        repo::intent::add_regex(
            repo::pool(),
            &params.robot_id,
            params.id.as_str(),
            params.data.as_str(),
        )
        .await
    }
    .await;
    to_res(r)
}

/// `data` 是正则的 id（不再是下标）。
pub(crate) async fn remove_regex(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<()> = async {
        repo::intent::remove_regex(
            repo::pool(),
            &params.robot_id,
            params.id.as_str(),
            params.data.as_str(),
        )
        .await?;
        Ok(())
    }
    .await;
    to_res(r)
}

/// 加一条短语：先算向量写 turso，再落 PG 载荷。返回新短语的 id。
#[axum::debug_handler]
pub(crate) async fn add_phrase(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<Option<String>> = async {
        let intent_id = params.id.as_str();
        let phrase = params.data.as_str();
        let Some(meta) = repo::intent::get_meta(repo::pool(), &params.robot_id, intent_id).await?
        else {
            return Err(Error::WithMessage(String::from(
                "Can NOT find intention detail",
            )));
        };
        let vec_row_id = super::phrase::add(&params.robot_id, intent_id, &meta.name, phrase)
            .await
            .map_err(|e| {
                log::error!("{:#?}", e);
                Error::WithMessage(String::from("Invalid idx parameter."))
            })?;

        match repo::intent::add_phrase(
            repo::pool(),
            &params.robot_id,
            intent_id,
            phrase,
            vec_row_id,
        )
        .await
        {
            Ok(Some(id)) => Ok(Some(id)),
            Ok(None) => {
                // 意图在算向量期间被删掉了：回滚刚写的向量行，避免留下孤儿。
                if let Err(e) = super::phrase::remove(&params.robot_id, vec_row_id).await {
                    log::warn!("Removing orphan phrase vector failed: {e:?}");
                }
                Err(Error::WithMessage(String::from(
                    "Can NOT find intention detail",
                )))
            }
            Err(e) => {
                if let Err(e2) = super::phrase::remove(&params.robot_id, vec_row_id).await {
                    log::warn!("Removing orphan phrase vector failed: {e2:?}");
                }
                Err(e)
            }
        }
    }
    .await;
    to_res(r)
}

/// 按 id 删除短语：先删向量行，再删 PG 行。
///
/// 顺序是有意的：turso 删失败就整体失败、PG 行还在，重试即可；PG 删失败最多留下
/// 一条指向已不存在向量的载荷，下一次「重建」会把它连向量一起补回来。
pub(crate) async fn remove_phrase(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<()> = async {
        let Some(p) = repo::intent::get_phrase(
            repo::pool(),
            &params.robot_id,
            params.id.as_str(),
            params.data.as_str(),
        )
        .await?
        else {
            return Ok(());
        };
        if let Some(vec_row_id) = p.vec_row_id {
            super::phrase::remove(&params.robot_id, vec_row_id).await?;
        }
        repo::intent::delete_phrase(
            repo::pool(),
            &params.robot_id,
            params.id.as_str(),
            params.data.as_str(),
        )
        .await?;
        Ok(())
    }
    .await;
    to_res(r)
}

pub(crate) async fn detect(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    to_res(detector::detect(&params.robot_id, &params.data).await)
}

/// 给一个意图的**全部**短语重算向量（模型/维度变了之后用）。
///
/// 按 `vec_row_id` 原地更新，不删表：这些短语的载荷现在在 PG，删 turso 表后
/// 旧实现用 `UPDATE … WHERE id = ?` 会一行都更新不到（见 §18）。
pub(crate) async fn regenerate_embeddings(
    Query(params): Query<IntentFormData>,
) -> impl IntoResponse {
    let intent_id = params.id.as_str();
    let r: Result<()> = async {
        let Some(meta) = repo::intent::get_meta(repo::pool(), &params.robot_id, intent_id).await?
        else {
            return Err(Error::WithMessage(String::from(
                "Can NOT find intention detail",
            )));
        };
        let phrases: Vec<(i64, String)> =
            repo::intent::list_phrase_vectors(repo::pool(), &params.robot_id, intent_id)
                .await?
                .into_iter()
                .filter_map(|p| p.vec_row_id.map(|id| (id, p.phrase)))
                .collect();
        super::phrase::update_vectors(&params.robot_id, intent_id, &meta.name, &phrases).await
    }
    .await;
    to_res(r)
}
