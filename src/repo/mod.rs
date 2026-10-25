//! 仓储层：**SQL 只出现在这里**（AGENTS.md 硬约束 1）。
//!
//! 业务模块不持有 `sqlx::Pool`：要落库就调 `repo::xxx`，池由 [`pool`] 取。
//! 每个仓储方法显式接收 `&PgPool` 是有意的 —— 集成测试因此可以给每个用例一个
//! 独立 schema 的池（`tmp_<uuid>`），而不必去改全局单例（§7 测试约定）。
//!
//! 迁移状态：redb 与 turso 都已移除（S4 完成），关系型数据、知识库载荷与向量全部
//! 落在 PG；进程内的向量索引只是缓存（D4/D35）。**新表、新字段一律只落在 PG**
//! （硬约束 3）。

pub(crate) mod app_meta;
pub(crate) mod context;
pub(crate) mod doc;
pub(crate) mod external_http;
pub(crate) mod intent;
pub(crate) mod mainflow;
pub(crate) mod qa;
pub(crate) mod robot;
pub(crate) mod runtime_node;
pub(crate) mod secret;
pub(crate) mod settings;
pub(crate) mod subflow;
pub(crate) mod variable;
pub(crate) mod vector;

#[cfg(test)]
mod tests;

use std::sync::OnceLock;
use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::result::{Error, Result};

/// 固定 schema（D37）：不用 `public`，客户库里多半已经有一堆业务表。
pub(crate) const SCHEMA: &str = "kiko";

/// 乐观锁冲突（B-17）：调用方带上了 `version`，但行已经被别人改过。
///
/// 文案要能让前端直接提示"刷新后重试"，测试也按这句断言。用 `WithMessage` 而不是
/// 新错误变体，是为了不改变 `{status, data, err}` 的形状（D9）。
pub(crate) fn version_conflict(entity: &str) -> Error {
    Error::WithMessage(format!(
        "{entity} has been modified by someone else; reload it and try again"
    ))
}

/// 更新请求没带 `version`（B-17 强制模式）。
///
/// 新项目没有旧客户端包袱，所以**更新类接口一律要求**客户端把它读到的版本带回来；
/// 缺了就直接报错，而不是悄悄退化成"最后写入胜出"。
pub(crate) fn version_required(entity: &str) -> Error {
    Error::WithMessage(format!(
        "{entity} update is missing its version; reload it and try again"
    ))
}

static POOL: OnceLock<PgPool> = OnceLock::new();

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/pg");

/// 进程内唯一的池。调用前必须 [`init`] 过。
pub(crate) fn pool() -> &'static PgPool {
    POOL.get()
        .expect("repo::init() 必须先于任何仓储调用（start_app 的第一步）")
}

/// 集成测试用：把全局池指向某个 `tmp_<uuid>` schema，供需要 `repo::pool()` 的
/// 业务粘合层（settings 缓存/加密）做端到端测试。只能成功一次。
#[cfg(test)]
pub(crate) fn install_test_pool(pool: PgPool) -> bool {
    POOL.set(pool).is_ok()
}

/// 连池 + 建 schema + 跑 migration。任何一步失败都直接返回错误，由启动路径 fail-fast（D33）。
pub(crate) async fn init(database_url: &str, auto_migrate: bool) -> Result<()> {
    if POOL.get().is_some() {
        return Ok(());
    }

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(10))
        // 每条连接都固定 search_path：migration 文件里不写 schema 前缀，
        // 靠它落到 kiko（生产）或 tmp_<uuid>（集成测试）。
        .after_connect(|conn, _| {
            Box::pin(async move {
                // schema 名是编译期常量，不存在注入面；sqlx 0.9 要求显式断言。
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "SET search_path TO {SCHEMA}, public"
                )))
                .execute(conn)
                .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await?;

    // `_sqlx_migrations` 也跟着 search_path 走，所以必须先建 schema 再跑 migration，
    // 否则迁移记录本身会落进 public。
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE SCHEMA IF NOT EXISTS {SCHEMA}"
    )))
    .execute(&pool)
    .await?;

    if auto_migrate {
        MIGRATOR.run(&pool).await?;
    } else {
        ensure_migrated(&pool).await?;
    }

    POOL.set(pool)
        .map_err(|_| Error::WithMessage(String::from("repo 已经初始化过")))?;
    Ok(())
}

/// `auto_migrate = false`（D45）：只校验版本，不跑 DDL。
async fn ensure_migrated(pool: &PgPool) -> Result<()> {
    let expected = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    let latest: Option<i64> = sqlx::query_scalar(
        "SELECT version FROM _sqlx_migrations WHERE success = TRUE ORDER BY version DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db) if db.code().as_deref() == Some("42P01") => {
            // undefined_table：库还没迁移过
            Error::WithMessage(String::from(
                "数据库尚未迁移（找不到 _sqlx_migrations），请先执行 kiko --migrate-only",
            ))
        }
        e => Error::from(e),
    })?;

    match latest {
        Some(v) if v >= expected => Ok(()),
        Some(v) => Err(Error::WithMessage(format!(
            "数据库 schema 版本是 {v}，低于程序要求的 {expected}；请先执行 kiko --migrate-only"
        ))),
        None => Err(Error::WithMessage(String::from(
            "数据库有 _sqlx_migrations 但没有任何成功的迁移记录，请检查迁移是否失败",
        ))),
    }
}

#[cfg(test)]
mod test_support {
    //! 集成测试的公共脚手架：每个用例一个 `tmp_<uuid>` schema，结束时 drop。

    use sqlx::postgres::{PgPool, PgPoolOptions};

    use super::MIGRATOR;

    /// 测试库 DSN。没配就跳过（CI 上没有 PG 时不至于红一片）。
    pub(crate) fn test_dsn() -> Option<String> {
        std::env::var("KIKO_TEST_DATABASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
    }

    pub(crate) struct TestDb {
        pub(crate) pool: PgPool,
        pub(crate) schema: String,
        admin_url: String,
    }

    impl TestDb {
        pub(crate) async fn new() -> Option<Self> {
            let url = test_dsn()?;
            // secrets 的加解密需要主密钥；测试用固定密钥，避免碰 data/master.key。
            crate::security::init_with_key([42u8; 32]);
            let schema = format!("tmp_{}", scru128::new_string().to_lowercase());
            let admin = PgPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .expect("连接测试库失败");
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
                .execute(&admin)
                .await
                .expect("建测试 schema 失败");

            let pool = PgPoolOptions::new()
                .max_connections(2)
                .after_connect({
                    let schema = schema.clone();
                    move |conn, _| {
                        let sql = format!("SET search_path TO {schema}, public");
                        Box::pin(async move {
                            sqlx::query(sqlx::AssertSqlSafe(sql)).execute(conn).await?;
                            Ok(())
                        })
                    }
                })
                .connect(&url)
                .await
                .expect("连接测试 schema 失败");

            MIGRATOR
                .run(&pool)
                .await
                .expect("在测试 schema 上跑迁移失败");
            admin.close().await;

            Some(Self {
                pool,
                schema,
                admin_url: url,
            })
        }

        /// 显式清理：正常结束的用例都会走到这里。用例 panic 时 schema 会残留
        /// （名字唯一，不影响后续用例；手动 `DROP SCHEMA` 即可）。
        pub(crate) async fn cleanup(self) {
            let TestDb {
                pool,
                schema,
                admin_url,
            } = self;
            pool.close().await;
            let admin = PgPoolOptions::new()
                .max_connections(1)
                .connect(&admin_url)
                .await
                .expect("连接测试库失败");
            let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE"
            )))
            .execute(&admin)
            .await;
            admin.close().await;
        }
    }
}
