use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
pub(crate) struct IntentFormData {
    #[serde(rename = "robotId")]
    pub(crate) robot_id: String,
    pub(crate) id: String,
    pub(crate) data: String,
}

/// 意图详情里的一条关键词。
///
/// A 直接回 `Vec<String>`，删除只能传数组下标；D10 要求按 ID 操作，所以这里
/// 带上子项自己的 id（scru128），前端删除时回传 `id`（§7）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntentKeywordData {
    pub(crate) id: String,
    pub(crate) keyword: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntentRegexData {
    pub(crate) id: String,
    pub(crate) regex: String,
}

/// 一条相似短语。`id` 是 `intent_phrases.id`（scru128 字符串），不再是
/// turso 的 `i64` 行号（D11）；向量行号留在仓储内部（见 `repo/intent.rs`）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntentPhraseData {
    pub(crate) id: String,
    pub(crate) phrase: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntentDetail {
    pub(crate) intent_id: String,
    pub(crate) intent_name: String,
    pub(crate) keywords: Vec<IntentKeywordData>,
    pub(crate) regexes: Vec<IntentRegexData>,
    pub(crate) phrases: Vec<IntentPhraseData>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §7：意图详情改 camelCase、keyword/regex 变对象数组、不再有 `phraseVecRowId`。
    #[test]
    fn intent_detail_serializes_camel_case_with_child_ids() {
        let d = IntentDetail {
            intent_id: String::from("i1"),
            intent_name: String::from("hello"),
            keywords: vec![IntentKeywordData {
                id: String::from("k1"),
                keyword: String::from("hi"),
            }],
            regexes: vec![IntentRegexData {
                id: String::from("r1"),
                regex: String::from("^h"),
            }],
            phrases: vec![IntentPhraseData {
                id: String::from("p1"),
                phrase: String::from("hello there"),
            }],
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["intentId"], "i1");
        assert_eq!(v["intentName"], "hello");
        assert_eq!(v["keywords"][0]["id"], "k1");
        assert_eq!(v["keywords"][0]["keyword"], "hi");
        assert_eq!(v["regexes"][0]["regex"], "^h");
        assert_eq!(v["phrases"][0]["id"], "p1");
        assert_eq!(v["phrases"][0]["phrase"], "hello there");
        assert!(
            v.get("phraseVecRowId").is_none(),
            "内部向量行号不能回给前端（D11）"
        );
        assert!(
            v.get("intent_id").is_none(),
            "线上 JSON 必须是 camelCase（D8）"
        );
    }
}
