pub(crate) mod asset;
pub(crate) mod auth;
pub mod server;

/// 路由层共享状态：只放认证需要的 PG 池与 cookie 属性。
///
/// 业务 handler 仍走 `repo::pool()` 的单例；这里显式带池是为了让路由/认证测试
/// 能给每个用例一个独立 `tmp_<uuid>` schema，不必去改全局单例。
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) pool: sqlx::postgres::PgPool,
    pub(crate) cookie_secure: bool,
    /// CORS 白名单（B-40）；空 = 只允许同源。
    pub(crate) cors_origins: Vec<String>,
    /// 随机引导口令文件；改密成功后删除（B-07）。
    pub(crate) initial_password_file: std::path::PathBuf,
}

// pub use crate::flow::rt::context::clean_expired_session;
// pub use crate::flow::rt::convertor::t1;
