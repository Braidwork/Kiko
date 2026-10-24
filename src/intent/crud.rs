use std::collections::HashMap;
use std::vec::Vec;

use axum::Json;
use axum::extract::Query;
use axum::response::IntoResponse;

use super::detector;
use super::dto::{IntentDetail, IntentFormData, IntentPhraseData};
use crate::db;
use crate::db_executor;
use crate::result::{Error, Result};
use crate::web::server::to_res;

// pub(crate) const TABLE: redb::TableDefinition<&str, &[u8]> =
//     redb::TableDefinition::new(INTENT_LIST_KEY);
pub(crate) const TABLE_SUFFIX: &str = "intents";

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
    let regexes: Vec<&str> = vec![];
    let mut intent_detail = IntentDetail::new(if is_en { "Positive" } else { "肯定" });
    let mut v = keywords.into_iter().map(String::from).collect();
    intent_detail.keywords.append(&mut v);
    let mut v = regexes.into_iter().map(String::from).collect();
    intent_detail.regexes.append(&mut v);
    // let intent_detail = IntentDetail {
    //     intent_idx: 0,
    //     intent_id: intent.id.clone(),
    //     intent_name: intent.name.clone(),
    //     keywords: keywords.into_iter().map(|s| String::from(s)).collect(),
    //     regexes: regexes.into_iter().map(|s| String::from(s)).collect(),
    //     phrases: vec![],
    // };

    // let table_name = format!("{}intents", robot_id);
    // let table: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new(&table_name);
    // let mut table = write_txn.open_table(TABLE)?;
    // db::write(table, intent.id.as_str(), &intent_detail)?;
    db_executor!(
        db::write,
        robot_id,
        TABLE_SUFFIX,
        intent_detail.intent_id.as_str(),
        &intent_detail
    )?;

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
    let regexes: Vec<&str> = vec![];
    let mut intent_detail = IntentDetail::new(if is_en { "Negative" } else { "否定" });
    let mut v = keywords.into_iter().map(String::from).collect();
    intent_detail.keywords.append(&mut v);
    let mut v = regexes.into_iter().map(String::from).collect();
    intent_detail.regexes.append(&mut v);
    // let intent_detail = IntentDetail {
    //     intent_idx: 1,
    //     intent_id: intent.id.clone(),
    //     intent_name: intent.name.clone(),
    //     keywords: keywords.into_iter().map(|s| String::from(s)).collect(),
    //     regexes: regexes.into_iter().map(|s| String::from(s)).collect(),
    //     phrases: vec![],
    // };

    // db::write(table, intent.id.as_str(), &intent_detail)?;
    db_executor!(
        db::write,
        robot_id,
        TABLE_SUFFIX,
        intent_detail.intent_id.as_str(),
        &intent_detail
    )?;

    // db::write(table, INTENT_LIST_KEY, &intents)
    Ok(())
}

pub(crate) async fn list(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    // let r: Result<Option<Vec<Intent>>> = db::query(TABLE, INTENT_LIST_KEY);
    if let Some(robot_id) = q.get("robotId") {
        let r: Result<Vec<IntentDetail>> = async {
            db_executor!(db::get_all, robot_id, TABLE_SUFFIX,)
        }
        .await;
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

async fn add_intent(robot_id: &str, intent_name: &str) -> Result<()> {
    // let d: Option<Vec<Intent>> = db::query(TABLE, INTENT_LIST_KEY)?;
    let intent_detail = IntentDetail::new(intent_name);
    // db::write(TABLE, intent.id.as_str(), &intent_detail)?;
    // Ok(())
    db_executor!(
        db::write,
        robot_id,
        TABLE_SUFFIX,
        intent_detail.intent_id.as_str(),
        &intent_detail
    )
}

pub(crate) async fn remove(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<()> = async {
        super::phrase::remove_by_intent_id(&params.robot_id, params.id.as_str()).await?;
        db_executor!(
            db::remove,
            &params.robot_id,
            TABLE_SUFFIX,
            params.id.as_str()
        )
    }
    .await;
    to_res(r)
}

pub(in crate::intent) async fn get_detail_by_id(
    robot_id: &str,
    intent_id: &str,
) -> Result<Option<IntentDetail>> {
    db_executor!(db::query, robot_id, TABLE_SUFFIX, intent_id)
}

pub(crate) async fn detail(Query(params): Query<IntentFormData>) -> impl IntoResponse {
    // let mut od: Option<IntentDetail> = None;
    // let r = db::process_data(dbg!(params.id.as_str()), |d: &mut IntentDetail| {
    //     od = Some(d);
    //     Ok(())
    // }).map(|_| od);
    // to_res(r)
    // let r: Result<Option<IntentDetail>> = db::query(TABLE, params.id.as_str());
    let r = get_detail_by_id(&params.robot_id, params.id.as_str()).await;
    to_res(r)
}

pub(crate) async fn add_keyword(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let key = params.id.as_str();
    let r: Result<()> = async {
        let op: Option<IntentDetail> =
            db_executor!(db::query, &params.robot_id, TABLE_SUFFIX, key)?;
        if let Some(mut d) = op {
            d.keywords.push(String::from(params.data.as_str()));
            db_executor!(db::write, &params.robot_id, TABLE_SUFFIX, key, &d)?;
        }
        Ok(())
    }
    .await;
    to_res(r)
}

pub(crate) async fn remove_keyword(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<()> = async {
        let idx = params.data.parse::<usize>().map_err(|e| {
            log::error!("{e:?}");
            Error::WithMessage(String::from("Invalid parameter"))
        })?;
        let key = params.id.as_str();
        let op: Option<IntentDetail> =
            db_executor!(db::query, &params.robot_id, TABLE_SUFFIX, key)?;
        if let Some(mut d) = op {
            d.keywords.remove(idx);
            db_executor!(db::write, &params.robot_id, TABLE_SUFFIX, key, &d)?;
        }
        Ok(())
    }
    .await;
    to_res(r)
}

pub(crate) async fn add_regex(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let key = params.id.as_str();
    let r: Result<()> = async {
        let op: Option<IntentDetail> =
            db_executor!(db::query, &params.robot_id, TABLE_SUFFIX, key)?;
        if let Some(mut d) = op {
            let _ = regex::Regex::new(params.data.as_str())?;
            d.regexes.push(String::from(params.data.as_str()));
            db_executor!(db::write, &params.robot_id, TABLE_SUFFIX, key, &d)?;
        }
        Ok(())
    }
    .await;
    to_res(r)
}

pub(crate) async fn remove_regex(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r: Result<()> = async {
        let idx = params.data.parse::<usize>().map_err(|e| {
            log::error!("{e:?}");
            Error::WithMessage(String::from("Invalid parameter"))
        })?;
        let key = params.id.as_str();
        let op: Option<IntentDetail> =
            db_executor!(db::query, &params.robot_id, TABLE_SUFFIX, key)?;
        if let Some(mut d) = op {
            d.regexes.remove(idx);
            db_executor!(db::write, &params.robot_id, TABLE_SUFFIX, key, &d)?;
        }
        Ok(())
    }
    .await;
    to_res(r)
}

#[axum::debug_handler]
pub(crate) async fn add_phrase(
    // Query(query): Query<IntentFormData>,
    Json(params): Json<IntentFormData>,
) -> impl IntoResponse {
    let r: Result<()> = async {
        let intent_id = params.id.as_str();
        let phrase = &params.data;
        let d: Option<IntentDetail> =
            db_executor!(db::query, &params.robot_id, TABLE_SUFFIX, intent_id)?;
        let mut d = match d {
            Some(d) => d,
            None => {
                return Err(Error::WithMessage(String::from(
                    "Can NOT find intention detail",
                )));
            }
        };
        let vec_row_id =
            super::phrase::add(&params.robot_id, None, intent_id, &d.intent_name, phrase)
                .await
                .map_err(|e| {
                    log::error!("{:#?}", &e);
                    Error::WithMessage(String::from("Invalid idx parameter."))
                })?;
        d.phrases.push(IntentPhraseData {
            id: vec_row_id,
            phrase: String::from(params.data.as_str()),
        });
        // log::info!("intent detail {} {}", intent_id, serde_json::to_string(&d).unwrap());
        db_executor!(db::write, &params.robot_id, TABLE_SUFFIX, intent_id, &d)
    }
    .await;
    to_res(r)
}

/*
pub(crate) async fn add_phrase(
    Query(query): Query<IntentFormData>,
    Json(params): Json<IntentFormData>,
) -> impl IntoResponse {
    let key = params.id.as_str();
    let r: Result<Option<IntentDetail>> =
        db_executor!(db::query, &params.robot_id, TABLE_SUFFIX, key);
    if r.is_err() {
        return to_res(r.map(|_| ()));
    }
    let r = r.unwrap();
    if r.is_none() {
        return to_res(Err(Error::ErrorWithMessage(String::from(
            "Can NOT find intention detail",
        ))));
    }
    let mut d = r.unwrap();
    let r =
        detector::save_intent_embedding(&params.robot_id, key, &d.intent_name, &params.data).await;
    if r.is_err() {
        return to_res(r.map(|_| ()));
    }
    // let r:Result<i64> = Ok(0i64);
    d.phrases.push(IntentPhraseData {
        id: r.unwrap(),
        phrase: String::from(params.data.as_str()),
    });
    let r = query
        .data
        .parse::<usize>()
        .map_err(|e| {
            log::error!("{:#?}", &e);
            Error::ErrorWithMessage(String::from("Invalid idx parameter."))
        })
        .and_then(|idx| {
            change_num(&params.robot_id, key, &mut d, |i: &mut Vec<Intent>| {
                i[idx].phrase_num = i[idx].phrase_num + 1
            })
        });
    to_res(r)
}
*/

pub(crate) async fn remove_phrase(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    let r = params.data.parse::<usize>();
    let phrase_idx = match r {
        Ok(n) => n,
        Err(e) => {
            log::error!("{e:?}");
            return to_res(Err(Error::WithMessage(String::from("Invalid parameter"))));
        }
    };
    let key = params.id.as_str();
    let r: Result<()> = async {
        let op: Option<IntentDetail> =
            db_executor!(db::query, &params.robot_id, TABLE_SUFFIX, key)?;
        if let Some(mut d) = op {
            let phrase = d.phrases.remove(phrase_idx);
            super::phrase::remove(&params.robot_id, phrase.id).await?;
            db_executor!(db::write, &params.robot_id, TABLE_SUFFIX, key, &d)?;
        }
        Ok(())
    }
    .await;
    to_res(r)
    // let r = params
    //     .data
    //     .parse::<usize>()
    //     .map_err(|e| {
    //         log::error!("{:?}", e);
    //         Error::ErrorWithMessage(String::from("Invalid parameter"))
    //     })
    //     .and_then(|idx| {
    //         let key = params.id.as_str();
    //         let result: Result<Option<IntentDetail>> =
    //             db_executor!(db::query, &params.robot_id, TABLE_SUFFIX, key);
    //         result.and_then(|mut op| {
    //             if op.is_some() {
    //                 let mut d = op.as_mut().unwrap();
    //                 let phrase = d.phrases.remove(idx);
    //                 crate::db::embedding::remove(&params.robot_id, key, phrase.id).await.and_then(|_| {
    //                     let idx = d.intent_idx;
    //                     change_num(&params.robot_id, key, &mut d, |i: &mut Vec<Intent>| {
    //                         i[idx].phrase_num = i[idx].phrase_num - 1
    //                     })
    //                 })
    //             } else {
    //                 Ok(())
    //             }
    //         })
    //     });
    // to_res(r)
}

pub(crate) async fn detect(Json(params): Json<IntentFormData>) -> impl IntoResponse {
    to_res(detector::detect(&params.robot_id, &params.data).await)
}

pub(crate) async fn regenerate_embeddings(
    Query(params): Query<IntentFormData>,
) -> impl IntoResponse {
    let key = params.id.as_str();
    let r: Result<()> = async {
        let op: Option<IntentDetail> =
            db_executor!(db::query, &params.robot_id, TABLE_SUFFIX, key)?;
        let d = match op {
            Some(d) => d,
            None => {
                return Err(Error::WithMessage(String::from(
                    "Can NOT find intention detail",
                )));
            }
        };
        // let array: Vec<&str> = d.phrases.iter().map(|v| v.phrase.as_ref()).collect();
        super::phrase::batch_add(&params.robot_id, &params.data, &d.intent_name, &d.phrases).await
    }
    .await;
    to_res(r)
}
