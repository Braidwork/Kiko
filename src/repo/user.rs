//! `users` 仓储（B-07 / B-34 / §5.8）。
//!
//! v1 只有本地管理员一种身份，但这一层不写死“只有 admin”：`status` 与
//! `must_change_password` 都如实返回，权限判定集中在 `auth` 层（D27 / D42）。
//! `password_hash` 是 argon2id PHC 串，仓储层不解释它，只负责存取。

use sqlx::postgres::PgPool;

use crate::result::{Error, Result};

#[derive(Clone, Debug)]
pub(crate) struct User {
    pub(crate) id: String,
    pub(crate) username: String,
    pub(crate) display_name: Option<String>,
    pub(crate) password_hash: String,
    pub(crate) status: String,
    pub(crate) must_change_password: bool,
}

pub(crate) type UserRow = (String, String, Option<String>, String, String, bool);

pub(crate) fn from_row(row: UserRow) -> User {
    User {
        id: row.0,
        username: row.1,
        display_name: row.2,
        password_hash: row.3,
        status: row.4,
        must_change_password: row.5,
    }
}

/// 用户数。启动引导（B-07）用它判断是否要创建管理员。
pub(crate) async fn count(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await?)
}

pub(crate) async fn find_by_username(pool: &PgPool, username: &str) -> Result<Option<User>> {
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT id, username, display_name, password_hash, status, must_change_password
           FROM users WHERE username = $1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(from_row))
}

pub(crate) async fn find_by_id(pool: &PgPool, id: &str) -> Result<Option<User>> {
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT id, username, display_name, password_hash, status, must_change_password
           FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(from_row))
}

/// 插入引导管理员。`must_change_password` 由调用方决定（B-07 固定 true）。
pub(crate) async fn insert(
    pool: &PgPool,
    id: &str,
    username: &str,
    password_hash: &str,
    must_change_password: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, status, must_change_password)
         VALUES ($1, $2, $3, 'active', $4)",
    )
    .bind(id)
    .bind(username)
    .bind(password_hash)
    .bind(must_change_password)
    .execute(pool)
    .await?;
    Ok(())
}

/// 改密：同时清掉 `must_change_password`（B-34）。
pub(crate) async fn set_password(
    pool: &PgPool,
    user_id: &str,
    password_hash: &str,
    must_change_password: bool,
) -> Result<()> {
    let r = sqlx::query(
        "UPDATE users
            SET password_hash = $2,
                must_change_password = $3,
                updated_at = now()
          WHERE id = $1",
    )
    .bind(user_id)
    .bind(password_hash)
    .bind(must_change_password)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(Error::WithMessage(String::from(
            "User does not exist any more.",
        )));
    }
    Ok(())
}

/// 登录成功更新时间；只影响 `last_login_at`。
pub(crate) async fn touch_login(pool: &PgPool, user_id: &str) -> Result<()> {
    sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}
