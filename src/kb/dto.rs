use std::vec::Vec;

use serde::{Deserialize, Serialize};

/// 一条问答对。`id` 是 `qa.id`（scru128 字符串，§5.1）；`None` 表示新建。
///
/// `QuestionData` 里原来的 `vec_row_id` 已随 B-30 一起删除：向量身份统一成
/// `vector_items.id`（D7），不再泄漏内部行号（D11）。
#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct QuestionAnswerPair {
    pub(crate) id: Option<String>,
    /// 乐观锁版本（B-17）：读接口返回，编辑时原样带回；新建时为 `None`。
    #[serde(default)]
    pub(crate) version: Option<i64>,
    pub(crate) question: QuestionData,
    #[serde(rename = "similarQuestions")]
    pub(crate) similar_questions: Vec<QuestionData>,
    pub(crate) answer: String,
}

#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct QuestionData {
    pub(crate) question: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct DocData {
    /// `documents.id`（scru128）。A 用的是自增行号，换成 PG 后按 D6/§5.1 用字符串。
    pub(crate) id: String,
    /// 乐观锁版本（B-17）：读接口返回，编辑时原样带回。
    #[serde(default)]
    pub(crate) version: Option<i64>,
    #[serde(rename = "fileName")]
    pub(crate) file_name: String,
    #[serde(rename = "fileSize")]
    pub(crate) file_size: i64,
    #[serde(rename = "docContent")]
    pub(crate) doc_content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B-30：问答载荷的 JSON 里不能再出现任何内部向量行号。
    #[test]
    fn question_json_has_no_vector_row_id() {
        let d = QuestionAnswerPair {
            id: Some(String::from("qa1")),
            version: Some(0),
            question: QuestionData {
                question: String::from("q"),
            },
            similar_questions: vec![QuestionData {
                question: String::from("q2"),
            }],
            answer: String::from("a"),
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["question"]["question"], "q");
        assert_eq!(v["similarQuestions"][0]["question"], "q2");
        assert!(v["question"].get("vec_row_id").is_none());
        assert!(v["question"].get("vecRowId").is_none());
    }

    /// 文档 DTO 的 id 是字符串（D6），字段名沿用 A 的 camelCase（D8）。
    #[test]
    fn doc_json_uses_string_id_and_camel_case() {
        let d = DocData {
            id: String::from("doc1"),
            version: Some(3),
            file_name: String::from("a.docx"),
            file_size: 12,
            doc_content: String::from("hello"),
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["id"], "doc1");
        assert_eq!(v["fileName"], "a.docx");
        assert_eq!(v["fileSize"], 12);
        assert_eq!(v["docContent"], "hello");
        assert!(v.get("file_size").is_none());
    }
}
