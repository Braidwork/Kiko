use std::vec::Vec;

use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
pub(crate) enum Protocol {
    HTTP,
    HTTPS,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) enum Method {
    GET,
    POST,
}

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) enum PostContentType {
    UrlEncoded,
    Raw,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) enum ValueSource {
    Val,
    Var,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct HttpReqParam {
    pub(crate) name: String,
    pub(crate) value: String,
    #[serde(rename = "valueSource")]
    pub(crate) value_source: ValueSource,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct HttpReqInfo {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) protocol: Protocol,
    pub(crate) method: Method,
    pub(crate) address: String,
    // #[serde(rename = "timeoutMilliseconds")]
    // pub(crate) timeout_milliseconds: u64,
    #[serde(rename = "postContentType")]
    pub(crate) post_content_type: PostContentType,
    pub(crate) headers: Vec<HttpReqParam>,
    #[serde(rename = "queryParams")]
    pub(crate) query_params: Vec<HttpReqParam>,
    #[serde(rename = "formData")]
    pub(crate) form_data: Vec<HttpReqParam>,
    #[serde(rename = "requestBody")]
    pub(crate) request_body: String,
    /// The `Content-Type` to send with `request_body`. Empty means "derive it
    /// from `post_content_type`", which is what every record written before
    /// this field existed did.
    #[serde(rename = "contentType", default)]
    pub(crate) content_type: String,
    #[serde(rename = "userAgent")]
    pub(crate) user_agent: String,
    // #[serde(rename = "asyncReq")]
    // pub(crate) async_req: bool,
}

pub(crate) enum ResponseData {
    Str(String),
    Bin(Vec<u8>),
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record as it was written before `contentType` existed and while the
    /// raw variant was still called `JSON`.
    const LEGACY_RECORD: &str = r#"{
        "id": "abc",
        "name": "legacy",
        "description": "",
        "protocol": "HTTPS",
        "method": "POST",
        "address": "api.example.com/v1",
        "postContentType": "Raw",
        "headers": [],
        "queryParams": [],
        "formData": [],
        "requestBody": "{}",
        "userAgent": "ua"
    }"#;

    #[test]
    fn legacy_record_still_reads() {
        let info: HttpReqInfo = serde_json::from_str(LEGACY_RECORD).unwrap();
        assert!(matches!(info.post_content_type, PostContentType::Raw));
        // Missing `contentType` falls back to the empty string, which the
        // request builder sends as application/json.
        assert_eq!(info.content_type, "");
    }

    #[test]
    fn record_is_written_with_the_new_names() {
        let mut info: HttpReqInfo = serde_json::from_str(LEGACY_RECORD).unwrap();
        info.content_type = String::from("text/plain");
        let s = serde_json::to_string(&info).unwrap();
        assert!(s.contains(r#""postContentType":"Raw""#), "{s}");
        assert!(s.contains(r#""contentType":"text/plain""#), "{s}");
    }
}
