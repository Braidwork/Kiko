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
        // 载荷与短语向量在同一个 PG 事务里删（repo::intent::delete 负责），
        // 内存索引里这个机器人的短语缓存整份作废即可：删意图不常见，重载比
        // "先查出所有 vector id 再逐个删"更不容易漏。
        repo::intent::delete(repo::pool(), &params.robot_id, params.id.as_str()).await?;
        crate::vector::invalidate(&params.robot_id, repo::vector::PHRASE);
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

/// 加一条短语：先算向量（事务外），再把载荷与向量写给仓储（同事务）。返回新短语 id。
#[axum::debug_handler]
pub(crate) async fn add_phrase(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<Option<String>> = async {
        let intent_id = params.id.as_str();
        let phrase = params.data.as_str();
        let (vec, model) = super::phrase::embed(&params.robot_id, phrase)
            .await
            .map_err(|e| {
                log::error!("{:#?}", e);
                Error::WithMessage(String::from("Invalid idx parameter."))
            })?;
        let Some((id, vector_id)) = repo::intent::add_phrase(
            repo::pool(),
            &params.robot_id,
            intent_id,
            phrase,
            vec.clone(),
            &model,
        )
        .await?
        else {
            return Err(Error::WithMessage(String::from(
                "Can NOT find intention detail",
            )));
        };
        crate::vector::upsert(
            &params.robot_id,
            repo::vector::PHRASE,
            &[(vector_id as u64, vec)],
        )
        .await;
        if let Err(e) = crate::man::settings::stamp_embedding_index(&params.robot_id).await {
            log::warn!("Stamping embedding index failed: {e:?}");
        }
        Ok(Some(id))
    }
    .await;
    to_res(r)
}

/// 按 id 删除短语：载荷与向量在同一个事务里删（repo::intent::delete_phrase）。
///
/// 返回被删的 `vector_items.id` 用来同步内存索引，避免整份短语缓存作废。
pub(crate) async fn remove_phrase(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<()> = async {
        let ids = repo::intent::delete_phrase(
            repo::pool(),
            &params.robot_id,
            params.id.as_str(),
            params.data.as_str(),
        )
        .await?;
        let ids: Vec<u64> = ids.into_iter().map(|i| i as u64).collect();
        crate::vector::delete(&params.robot_id, repo::vector::PHRASE, &ids).await;
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
/// 短语 id 不变，所以 `put_phrase_vectors` 是覆盖同一批 `vector_items` 行
/// （`ON CONFLICT (collection, entity_id, generation) DO UPDATE`），内存索引可以
/// 原地更新。
pub(crate) async fn regenerate_embeddings(
    Query(params): Query<IntentFormData>,
) -> impl IntoResponse {
    let intent_id = params.id.as_str();
    let r: Result<()> = async {
        let phrases = repo::intent::list_phrases(repo::pool(), &params.robot_id, intent_id).await?;
        if phrases.is_empty() {
            return Ok(());
        }
        let mut items: Vec<(String, Vec<f32>)> = Vec::with_capacity(phrases.len());
        let mut model = None;
        for p in phrases.iter() {
            let (vec, m) = super::phrase::embed(&params.robot_id, &p.phrase).await?;
            model = Some(m);
            items.push((p.id.clone(), vec));
        }
        let model = model.expect("phrases is not empty");
        let ids = repo::intent::put_phrase_vectors(repo::pool(), &params.robot_id, &items, &model)
            .await?;
        let rows: Vec<(u64, Vec<f32>)> = ids
            .into_iter()
            .zip(items)
            .map(|(id, (_, vec))| (id as u64, vec))
            .collect();
        crate::vector::upsert(&params.robot_id, repo::vector::PHRASE, &rows).await;
        if let Err(e) = crate::man::settings::stamp_embedding_index(&params.robot_id).await {
            log::warn!("Stamping embedding index failed: {e:?}");
        }
        Ok(())
    }
    .await;
    to_res(r)
}
