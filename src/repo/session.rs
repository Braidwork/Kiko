//! `sessions` 仓储（B-34 / §5.8）。
//!
//! 只存 token 的 SHA-256（`token_hash BYTEA UNIQUE`），不存原文；原文只出现在
//! 登录响应与浏览器 HttpOnly cookie 里。有效期/续期/吊销都在 SQL 里算，避免把
//! `TIMESTAMPTZ` 映射进 Rust 类型（sqlx 没开 time/chrono 特性）。
//!
//! TTL（已与用户确认）：
//! - 滑动 12h：每次请求最多把 `expires_at` 推后 12h；
//! - 绝对 7d：`created_at + 7d` 是硬上限，`find_valid` 也会检查；
//! - `last_seen_at` 最多每 60s 写一次，避免每个请求都写一行。

use sqlx::postgres::PgPool;

use crate::repo::user::User;
use crate::result::Result;

/// 滑动有效期（秒）。
pub(crate) const SLIDING_TTL_SECS: i64 = 12 * 60 * 60;
/// 绝对有效期（秒）。
pub(crate) const ABSOLUTE_TTL_SECS: i64 = 7 * 24 * 60 * 60;
/// `last_seen_at` 的写入节流（秒）。
pub(crate) const TOUCH_INTERVAL_SECS: i64 = 60;

#[derive(Clone, Debug)]
pub(crate) struct SessionUser {
    pub(crate) session_id: String,
    pub(crate) user: User,
}

/// 新建会话，返回 session id。`expires_at` 由 SQL 计算（created_at = now）。
pub(crate) async fn create(
    pool: &PgPool,
    user_id: &str,
    token_hash: &[u8],
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> Result<String> {
    let id = scru128::new_string();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash, expires_at, last_seen_at, ip, user_agent)
         VALUES ($1, $2, $3, now() + ($4::bigint * interval '1 second'), now(), $5, $6)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(token_hash)
    .bind(SLIDING_TTL_SECS)
    .bind(ip)
    .bind(user_agent)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 查有效会话并带上用户行。过期、已吊销、用户被停用都返回 `None`。
pub(crate) async fn find_valid(pool: &PgPool, token_hash: &[u8]) -> Result<Option<SessionUser>> {
    let row: Option<(String, String, String, Option<String>, String, String, bool)> =
        sqlx::query_as(
            "SELECT s.id,
                u.id, u.username, u.display_name, u.password_hash,
                u.status, u.must_change_password
           FROM sessions s
           JOIN users u ON u.id = s.user_id
          WHERE s.token_hash = $1
            AND s.revoked_at IS NULL
            AND s.expires_at > now()
            AND s.created_at + ($2::bigint * interval '1 second') > now()
            AND u.status = 'active'",
        )
        .bind(token_hash)
        .bind(ABSOLUTE_TTL_SECS)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(
        |(session_id, id, username, display_name, password_hash, status, must_change_password)| {
            SessionUser {
                session_id,
                user: User {
                    id,
                    username,
                    display_name,
                    password_hash,
                    status,
                    must_change_password,
                },
            }
        },
    ))
}

/// 滑动续期；60s 内重复请求不写库。只对仍然有效的会话生效。
pub(crate) async fn touch(pool: &PgPool, session_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE sessions
            SET last_seen_at = now(),
                expires_at = LEAST(
                    now() + ($2::bigint * interval '1 second'),
                    created_at + ($3::bigint * interval '1 second')
                )
          WHERE id = $1
            AND revoked_at IS NULL
            AND expires_at > now()
            AND (last_seen_at IS NULL
                 OR last_seen_at < now() - ($4::bigint * interval '1 second'))",
    )
    .bind(session_id)
    .bind(SLIDING_TTL_SECS)
    .bind(ABSOLUTE_TTL_SECS)
    .bind(TOUCH_INTERVAL_SECS)
    .execute(pool)
    .await?;
    Ok(())
}

/// 按原始 token 的哈希吊销（logout 用；不需要先认证）。
pub(crate) async fn revoke_by_hash(pool: &PgPool, token_hash: &[u8]) -> Result<()> {
    sqlx::query(
        "UPDATE sessions SET revoked_at = now()
          WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(token_hash)
    .execute(pool)
    .await?;
    Ok(())
}

/// 测试用：把会话强制过期。SQL 仍然只在 repo 层（AGENTS 硬约束 1）。
#[cfg(test)]
pub(crate) async fn expire_for_test(pool: &PgPool, session_id: &str) -> Result<()> {
    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 改密后吊销该用户其他会话，保留当前会话（B-34）。
pub(crate) async fn revoke_others(
    pool: &PgPool,
    user_id: &str,
    keep_session_id: &str,
) -> Result<u64> {
    let r = sqlx::query(
        "UPDATE sessions SET revoked_at = now()
          WHERE user_id = $1 AND id <> $2 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(keep_session_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// 运维重置密码后吊销该用户的全部会话（没有“保留当前会话”的概念）。
pub(crate) async fn revoke_all_for_user(pool: &PgPool, user_id: &str) -> Result<u64> {
    let r = sqlx::query(
        "UPDATE sessions SET revoked_at = now()
          WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// 清理已经过期/已吊销的会话行（登录时顺手做，避免表无限长）。
pub(crate) async fn delete_expired(pool: &PgPool) -> Result<u64> {
    let r = sqlx::query(
        "DELETE FROM sessions
          WHERE expires_at <= now()
             OR (revoked_at IS NOT NULL AND revoked_at < now() - interval '30 days')",
    )
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::test_support::TestDb;

    #[tokio::test]
    async fn session_round_trip_and_revocation() {
        let Some(db) = TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let user_id = scru128::new_string();
        crate::repo::user::insert(
            &db.pool,
            &user_id,
            "session-test",
            "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$Zm9v",
            false,
        )
        .await
        .unwrap();

        let hash = vec![7u8; 32];
        let session_id = create(&db.pool, &user_id, &hash, Some("127.0.0.1"), Some("test"))
            .await
            .unwrap();
        let found = find_valid(&db.pool, &hash).await.unwrap().expect("valid");
        assert_eq!(found.session_id, session_id);
        assert_eq!(found.user.id, user_id);
        assert!(touch(&db.pool, &session_id).await.is_ok());

        revoke_by_hash(&db.pool, &hash).await.unwrap();
        assert!(find_valid(&db.pool, &hash).await.unwrap().is_none());

        let other = create(&db.pool, &user_id, &[8u8; 32], None, None)
            .await
            .unwrap();
        assert_eq!(
            revoke_others(&db.pool, &user_id, &other).await.unwrap(),
            0,
            "只吊销其他未吊销的会话"
        );
        db.cleanup().await;
    }
}
