use serde::ser::{Serialize, SerializeStruct};
use std::convert::From;

pub(crate) type Result<D> = core::result::Result<D, Error>;

#[derive(Debug)]
pub(crate) enum Error {
    /// PostgreSQL 错误（B-08）。唯一约束 / 外键冲突单独识别成一句话，
    /// 业务侧和前端看到的就不再是 `error returned from database: …` 这种原文。
    Pg(Box<sqlx::Error>),
    /// schema 迁移失败（启动路径，D33 fail-fast）。
    Migrate(Box<sqlx::migrate::MigrateError>),
    Serde(Box<serde_json::Error>),
    TimeFormat(Box<time::error::Format>),
    WithMessage(String),
    NetworkConnectTimeout(Box<reqwest::Error>),
    NetworkReadTimeout(Box<reqwest::Error>),
    InvalidJsonStructure(Box<serde_json::Error>),
}

impl Error {
    /// 给用户看的一句话说明。
    ///
    /// 抽出来是因为它有两个消费者：HTTP 响应里的 `err.message`（[`Serialize`]
    /// 实现），以及需要把失败原因拼进**别的**响应体的地方（模型文件校验会把
    /// "哪个文件不对"直接回给前端弹给用户）。两处必须是同一句话，否则用户看到的
    /// 提示和日志/接口里的原因就开始各说各话。
    pub(crate) fn message(&self) -> String {
        match &self {
            Self::Pg(e) => match e.as_database_error() {
                Some(db) if db.is_unique_violation() => {
                    format!("Duplicate value: {}", db.message())
                }
                Some(db) if db.is_foreign_key_violation() => {
                    format!(
                        "Referenced row is missing or still in use: {}",
                        db.message()
                    )
                }
                _ => format!("Database error: {e}"),
            },
            Self::Migrate(e) => format!("Migration error: {e}"),
            Self::Serde(e) => format!("{e:?}"),
            Self::TimeFormat(e) => format!("{e:?}"),
            Self::WithMessage(s) => String::from(s),
            Self::NetworkConnectTimeout(e) => format!("Network connect timeout: {e:?}"),
            Self::NetworkReadTimeout(e) => format!("Network read timeout: {e:?}"),
            Self::InvalidJsonStructure(e) => format!("Invalid JSON structure: {e:?}"),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let message = self.message();
        let mut s = serializer.serialize_struct("Error", 1)?;
        s.serialize_field("message", &message)?;
        s.end()
    }
}

impl From<std::time::SystemTimeError> for Error {
    fn from(err: std::time::SystemTimeError) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}
impl From<regex::Error> for Error {
    fn from(err: regex::Error) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::Pg(Box::new(err))
    }
}

impl From<sqlx::migrate::MigrateError> for Error {
    fn from(err: sqlx::migrate::MigrateError) -> Self {
        Error::Migrate(Box::new(err))
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Serde(Box::new(err))
    }
}

impl From<lettre::address::AddressError> for Error {
    fn from(err: lettre::address::AddressError) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

impl From<lettre::transport::smtp::Error> for Error {
    fn from(err: lettre::transport::smtp::Error) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

impl From<lettre::error::Error> for Error {
    fn from(err: lettre::error::Error) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

// impl From<oasysdb::prelude::Error> for Error {
//     fn from(err: oasysdb::prelude::Error) -> Self {
//         Error::ErrorWithMessage(format!("{err:?}"))
//     }
// }

impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

impl From<reqwest::header::InvalidHeaderValue> for Error {
    fn from(err: reqwest::header::InvalidHeaderValue) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

// impl From<hf_hub::api::tokio::ApiError> for Error {
//     fn from(err: hf_hub::api::tokio::ApiError) -> Self {
//         Error::ErrorWithMessage(format!("{err:?}"))
//     }
// }

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

impl From<std::env::VarError> for Error {
    fn from(err: std::env::VarError) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

impl From<candle::Error> for Error {
    fn from(err: candle::Error) -> Self {
        Error::WithMessage(format!("{err:?}"))
    }
}

impl<T> From<tokio::sync::mpsc::error::TrySendError<T>> for Error {
    fn from(err: tokio::sync::mpsc::error::TrySendError<T>) -> Self {
        Error::WithMessage(format!("Sent failed, err: {err:?}"))
    }
}

impl<T> From<tokio::sync::mpsc::error::SendError<T>> for Error {
    fn from(err: tokio::sync::mpsc::error::SendError<T>) -> Self {
        Error::WithMessage(format!("Sent failed, err: {err:?}"))
    }
}

impl<T> From<std::sync::PoisonError<T>> for Error {
    fn from(err: std::sync::PoisonError<T>) -> Self {
        Error::WithMessage(format!("Poison error: {err:?}"))
    }
}

impl From<std::num::ParseFloatError> for Error {
    fn from(err: std::num::ParseFloatError) -> Self {
        Error::WithMessage(format!("Parse float error: {err:?}"))
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(err: tokio::task::JoinError) -> Self {
        Error::WithMessage(format!("Thread join error: {err:?}"))
    }
}

impl From<axum::extract::multipart::MultipartError> for Error {
    fn from(err: axum::extract::multipart::MultipartError) -> Self {
        Error::WithMessage(format!("Multipart error: {err:?}"))
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(err: zip::result::ZipError) -> Self {
        Error::WithMessage(format!("ZipError failed: {err:?}"))
    }
}

impl From<quick_xml::encoding::EncodingError> for Error {
    fn from(err: quick_xml::encoding::EncodingError) -> Self {
        Error::WithMessage(format!("quick_xml EncodingError failed: {err:?}"))
    }
}

// turso 已随 S4 一起移除：写锁争用（`Busy` / `BusySnapshot`）与 `retry_on_busy!`
// 的重试语义都不再需要 —— PostgreSQL 的并发由事务与行锁处理，失败直接返回。

// impl From<cxx::Exception> for Error {
//     fn from(err: cxx::Exception) -> Self {
//         Error::ErrorWithMessage(format!("USearch occorred an error {:?}", err))
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;

    /// PG 错误映射（B-08）仍然按类别给可读的一句话：唯一约束 / 外键冲突不能把
    /// `error returned from database: …` 这种原文丢给用户。
    #[test]
    fn error_message_is_human_readable() {
        let e = Error::WithMessage(String::from("boom"));
        assert_eq!(e.message(), "boom");
        assert_eq!(format!("{e}"), "boom");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["message"], "boom");
    }
}
