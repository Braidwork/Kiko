//! `global_settings` / `robot_settings` 仓储（§5.2 / B-12 / B-18）。
//!
//! 两份设置都以 JSONB 整条存取：同一 key 直接 upsert，最后写入胜出。设置是低频写、
//! 高频读，进程内缓存由 `man::settings` 维护，这里只负责「读出来 / 写进去」。
//!
//! 敏感字段在 JSON 里只是引用串；真正的密文由 [`super::secret`] 在**同一事务**里
//! 写入，保证不会出现「设置指向一个还不存在的 secret」。

use sqlx::postgres::PgPool;
use sqlx::types::Json;

use crate::man::settings::{GlobalSettings, Settings};
use crate::repo::secret::SecretChange;
use crate::result::Result;

pub(crate) async fn get_robot(pool: &PgPool, robot_id: &str) -> Result<Option<Settings>> {
    let data: Option<Json<Settings>> =
        sqlx::query_scalar("SELECT data FROM robot_settings WHERE robot_id = $1")
            .bind(robot_id)
            .fetch_optional(pool)
            .await?;
    Ok(data.map(|d| d.0))
}

pub(crate) async fn save_robot(
    pool: &PgPool,
    robot_id: &str,
    data: &Settings,
    secrets: &[SecretChange],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO robot_settings (robot_id, data, updated_at, version)
         VALUES ($1, $2, now(), 0)
         ON CONFLICT (robot_id) DO UPDATE
            SET data = EXCLUDED.data,
                updated_at = now(),
                version = robot_settings.version + 1",
    )
    .bind(robot_id)
    .bind(Json(data))
    .execute(&mut *tx)
    .await?;
    super::secret::apply_in_tx(&mut tx, robot_id, secrets).await?;
    tx.commit().await?;
    Ok(())
}

/// 只更新设置 JSON 里的 `sentenceEmbeddingProvider.indexedEmbedding`（B-17 / §16.4）。
///
/// 向量写入/重建之后要打这个指纹。原来的实现是"读整条设置 → 改一个字段 → 写回"，
/// 与并发的设置页保存互相覆盖。这里用 `jsonb_set` 做单字段更新，**不碰**任何其他
/// 字段，也不重新加密敏感引用（`indexedEmbedding` 不是敏感字段）。
pub(crate) async fn stamp_robot_indexed_embedding(
    pool: &PgPool,
    robot_id: &str,
    fingerprint: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE robot_settings
            SET data = jsonb_set(
                    data,
                    '{sentenceEmbeddingProvider,indexedEmbedding}',
                    to_jsonb($2::text)),
                updated_at = now(),
                version = robot_settings.version + 1
          WHERE robot_id = $1",
    )
    .bind(robot_id)
    .bind(fingerprint)
    .execute(pool)
    .await?;
    Ok(())
}

/// 给新机器人插一份默认设置；**已存在就不动**（幂等，不会覆盖用户设置/密钥）。
pub(crate) async fn init_robot(pool: &PgPool, robot_id: &str, data: &Settings) -> Result<()> {
    sqlx::query(
        "INSERT INTO robot_settings (robot_id, data, updated_at, version)
         VALUES ($1, $2, now(), 0)
         ON CONFLICT (robot_id) DO NOTHING",
    )
    .bind(robot_id)
    .bind(Json(data))
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn get_global(pool: &PgPool) -> Result<Option<GlobalSettings>> {
    let data: Option<Json<GlobalSettings>> =
        sqlx::query_scalar("SELECT data FROM global_settings WHERE id = 1")
            .fetch_optional(pool)
            .await?;
    Ok(data.map(|d| d.0))
}

pub(crate) async fn save_global(
    pool: &PgPool,
    data: &GlobalSettings,
    secrets: &[SecretChange],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO global_settings (id, data, updated_at, version)
         VALUES (1, $1, now(), 0)
         ON CONFLICT (id) DO UPDATE
            SET data = EXCLUDED.data,
                updated_at = now(),
                version = global_settings.version + 1",
    )
    .bind(Json(data))
    .execute(&mut *tx)
    .await?;
    super::secret::apply_in_tx(&mut tx, "global", secrets).await?;
    tx.commit().await?;
    Ok(())
}
