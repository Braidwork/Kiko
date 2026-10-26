use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub(crate) struct RobotQuery {
    #[serde(rename = "robotId")]
    pub(crate) robot_id: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) enum RobotType {
    InboundCallBot,
    OutboundCallBot,
    TextBot,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct RobotData {
    #[serde(rename = "robotId")]
    pub(crate) robot_id: String,
    #[serde(rename = "robotName")]
    pub(crate) robot_name: String,
    #[serde(rename = "robotType")]
    pub(crate) robot_type: RobotType,
    /// 乐观锁版本（B-17）：读接口返回，编辑时原样带回。
    #[serde(default)]
    pub(crate) version: Option<i64>,
}
