//! `app_meta` 仓储（§5.2）：启动时写入的少量元数据。
//!
//! 目前只放 `db_init_time` / `app_version`；`schema_version` / `compat_version`
//! 的正式校验留给 B-04。

use sqlx::postgres::PgPool;

use crate::result::Result;

pub(crate) async fn put(pool: &PgPool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO app_meta (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// 只写一次（已存在就不动）。
pub(crate) async fn put_if_absent(pool: &PgPool, key: &str, value: &str) -> Result<()> {
    sqlx::query("INSERT INTO app_meta (key, value) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING")
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) async fn get(pool: &PgPool, key: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT value FROM app_meta WHERE key = $1")
            .bind(key)
            .fetch_optional(pool)
            .await?,
    )
}
