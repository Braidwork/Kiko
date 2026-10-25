use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct MainFlowDetail {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    /// 乐观锁版本（B-17）：列表返回，保存时原样带回。
    #[serde(default)]
    pub(crate) version: Option<i64>,
}
