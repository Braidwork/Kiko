//! `secrets` 仓储（§5.2 / D36 / B-06）。
//!
//! 设置 JSON 里只存引用串（`enc:chat_api_key` / `env:OPENAI_API_KEY` /
//! `file:/path/key`），这里存的是 `enc:` 对应的密文。`scope` 是 `'global'` 或
//! `robot_id`。
//!
//! **本层不接触明文**：加密/解密在 [`crate::security`]，`man::settings` 负责
//! 把设置对象拆成「引用串 + SecretChange」，再一起提交。

use sqlx::postgres::PgPool;
use sqlx::{Postgres, Transaction};

use crate::result::Result;

/// 一次保存里对某个 secret 的动作。和设置 JSON 的 upsert 放在同一事务里。
pub(crate) enum SecretChange {
    Upsert {
        name: String,
        kind: String,
        value: String,
    },
    Delete {
        name: String,
    },
}

#[derive(sqlx::FromRow)]
pub(crate) struct SecretRow {
    pub(crate) kind: String,
    pub(crate) value: String,
}

pub(crate) async fn get(pool: &PgPool, scope: &str, name: &str) -> Result<Option<SecretRow>> {
    Ok(
        sqlx::query_as("SELECT kind, value FROM secrets WHERE scope = $1 AND name = $2")
            .bind(scope)
            .bind(name)
            .fetch_optional(pool)
            .await?,
    )
}

/// 在调用方的事务里应用一批 secret 变更。
pub(crate) async fn apply_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    scope: &str,
    changes: &[SecretChange],
) -> Result<()> {
    for change in changes {
        match change {
            SecretChange::Upsert { name, kind, value } => {
                sqlx::query(
                    "INSERT INTO secrets (scope, name, kind, value, updated_at)
                     VALUES ($1, $2, $3, $4, now())
                     ON CONFLICT (scope, name) DO UPDATE
                        SET kind = EXCLUDED.kind,
                            value = EXCLUDED.value,
                            updated_at = now()",
                )
                .bind(scope)
                .bind(name)
                .bind(kind)
                .bind(value)
                .execute(&mut **tx)
                .await?;
            }
            SecretChange::Delete { name } => {
                sqlx::query("DELETE FROM secrets WHERE scope = $1 AND name = $2")
                    .bind(scope)
                    .bind(name)
                    .execute(&mut **tx)
                    .await?;
            }
        }
    }
    Ok(())
}

/// 删机器人时清掉它名下的 secret（`secrets.scope` 没有外键，不会自动级联）。
pub(crate) async fn delete_scope(pool: &PgPool, scope: &str) -> Result<u64> {
    let r = sqlx::query("DELETE FROM secrets WHERE scope = $1")
        .bind(scope)
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}
