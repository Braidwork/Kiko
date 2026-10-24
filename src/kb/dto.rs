use std::vec::Vec;

use serde::{Deserialize, Serialize};

// #[derive(Deserialize, Serialize)]
// pub(crate) struct QuestionAnswerData {
//     pub(super) id: Option<String>,
//     #[serde(rename = "qaData")]
//     pub(super) qa_data: QuestionAnswerPair,
// }

// Clone 是 qa::save 需要的：它要在一份副本上做就地修改、提交成功后才落回入参，
// 这样重试才是安全的（见 result::retry_on_busy 的文档）。
#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct QuestionAnswerPair {
    pub(super) id: Option<i64>,
    pub(super) question: QuestionData,
    #[serde(rename = "similarQuestions")]
    pub(super) similar_questions: Vec<QuestionData>,
    pub(crate) answer: String,
}

#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct QuestionData {
    pub(super) question: String,
    pub(super) vec_row_id: Option<u64>,
}

#[derive(Deserialize, Serialize)] // , sqlx::FromRow
pub(crate) struct DocData {
    pub(crate) id: i64,
    #[serde(rename = "fileName")]
    pub(crate) file_name: String,
    #[serde(rename = "fileSize")]
    pub(crate) file_size: i64,
    #[serde(rename = "docContent")]
    pub(crate) doc_content: String,
}
