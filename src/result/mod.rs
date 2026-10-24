use serde::ser::{Serialize, SerializeStruct};
use std::convert::From;

pub(crate) type Result<D> = core::result::Result<D, Error>;

#[derive(Debug)]
pub(crate) enum Error {
    Db(Box<redb::Error>),
    /// turso 的写锁争用（`Busy` / `BusySnapshot`）。
    ///
    /// 从 `turso::Error` 里单独拆出来，是因为它可恢复 —— 调用方可以退避后重试。
    /// 把它和别的 turso 错误一起压进 `WithMessage` 就失去了这个能力。
    DbBusy(String),
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
            Self::Db(e) => format!("{e:?}"),
            Self::DbBusy(s) => format!("Database is busy: {s}"),
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

impl From<redb::Error> for Error {
    fn from(err: redb::Error) -> Self {
        Error::Db(Box::new(err))
    }
}

impl From<redb::TransactionError> for Error {
    fn from(err: redb::TransactionError) -> Self {
        Error::Db(Box::new(err.into()))
    }
}

impl From<redb::DatabaseError> for Error {
    fn from(err: redb::DatabaseError) -> Self {
        Error::Db(Box::new(err.into()))
    }
}

impl From<redb::StorageError> for Error {
    fn from(err: redb::StorageError) -> Self {
        Error::Db(Box::new(err.into()))
    }
}

impl From<redb::TableError> for Error {
    fn from(err: redb::TableError) -> Self {
        Error::Db(Box::new(err.into()))
    }
}

impl From<redb::CommitError> for Error {
    fn from(err: redb::CommitError) -> Self {
        Error::Db(Box::new(err.into()))
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

impl From<turso::Error> for Error {
    fn from(err: turso::Error) -> Self {
        // 写锁争用是可恢复的，单独成一个变体，好让调用方能退避重试。
        // 其余 turso 错误仍按原样字符串化，保持既有行为。
        match err {
            turso::Error::Busy(s) | turso::Error::BusySnapshot(s) => Error::DbBusy(s),
            e => Error::WithMessage(format!("turso Error failed: {e:?}")),
        }
    }
}

/// 给写操作套一层 `Busy` 退避重试。
///
/// `busy_timeout`（见 `kb/qa.rs` 的 `conn()`）已经把绝大多数写锁争用变成"排队
/// 等待"，这里是安全网 —— 覆盖超时之后仍然漏出来的 `Busy`：跨进程竞争、别的
/// 写者拿着锁做长事务（比如把模型推理放在事务里）等等。
///
/// 传进来的表达式必须满足两个条件，否则重试会损坏数据：
///
/// 1. **每次求值都从头开始** —— 传一个 `async { ... }` 块，不要把状态留在块外。
/// 2. **可重入** —— 第一次尝试失败后不能留下半提交的状态，也不能就地改写入参后
///    在下一次尝试里读到它。就地改写入参的函数（编辑前的 `qa::save` 就是）必须
///    先自己把状态收敛好再套这个宏，
///
/// 参考：`qa::save` 用"每轮在副本上改、commit 成功才落回入参"来满足第 2 条；
/// `doc::save` 用"把 INSERT 收进事务"来满足它。
#[macro_export]
macro_rules! retry_on_busy {
    ($e: expr) => {{
        const MAX_RETRIES: u32 = 3;
        let mut retries: u32 = 0;
        loop {
            match $e.await {
                Err($crate::result::Error::DbBusy(m)) if retries < MAX_RETRIES => {
                    // 100ms / 200ms / 400ms。重试次数很少，加抖动没有意义。
                    let backoff = std::time::Duration::from_millis(100u64 << retries);
                    retries += 1;
                    log::warn!(
                        "Database is busy ({m}), retrying in {backoff:?} (retry {retries}/{MAX_RETRIES})"
                    );
                    tokio::time::sleep(backoff).await;
                }
                r => break r,
            }
        }
    }};
}

// impl From<cxx::Exception> for Error {
//     fn from(err: cxx::Exception) -> Self {
//         Error::ErrorWithMessage(format!("USearch occorred an error {:?}", err))
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn retry_on_busy_retries_transient_busy() {
        let n = AtomicU32::new(0);
        let r: Result<u32> = retry_on_busy!(async {
            let i = n.fetch_add(1, Ordering::SeqCst);
            if i < 2 {
                Err(Error::DbBusy(String::from("busy")))
            } else {
                Ok(i)
            }
        });
        // 第三次尝试才成功，且每次尝试都从头开始求值。
        assert!(matches!(r, Ok(2)), "unexpected result: {r:?}");
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_on_busy_gives_up_after_max_retries() {
        let n = AtomicU32::new(0);
        let r: Result<u32> = retry_on_busy!(async {
            n.fetch_add(1, Ordering::SeqCst);
            Err(Error::DbBusy(String::from("busy")))
        });
        assert!(matches!(r, Err(Error::DbBusy(_))), "unexpected result: {r:?}");
        // 首次 + MAX_RETRIES 次重试；超时后不再无限重试，把 Busy 交回调用方。
        assert_eq!(n.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn retry_on_busy_leaves_other_errors_alone() {
        let n = AtomicU32::new(0);
        let r: Result<u32> = retry_on_busy!(async {
            n.fetch_add(1, Ordering::SeqCst);
            Err(Error::WithMessage(String::from("boom")))
        });
        assert!(
            matches!(r, Err(Error::WithMessage(_))),
            "unexpected result: {r:?}"
        );
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }
}
