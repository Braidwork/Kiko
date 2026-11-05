//! 命令行入口（B-07 / 运维恢复）。
//!
//! 目前处理：
//!
//! - `kiko hash-password`：从 stdin 读口令、输出 argon2id PHC 串；
//! - `kiko reset-password [username]`：从 stdin 读两次新口令，更新
//!   `users.password_hash` 并吊销该用户全部会话；
//!
//! 口令一律从 stdin 读，**不从 argv 读**，避免出现在 shell 历史与进程列表里。
//! 两个命令都不启动服务；`reset-password` 只连库，不初始化主密钥、不 bootstrap。

use std::io::{self, Write};

use crate::auth;
use crate::result::{Error, Result};

/// 若参数命中 CLI 子命令，返回退出码；否则返回 `None`，继续走服务启动。
pub fn run() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("hash-password") => Some(hash_password_command()),
        Some("reset-password") => {
            let username = args.next();
            if matches!(username.as_deref(), Some("-h" | "--help")) {
                print_help();
                return Some(0);
            }
            if args.next().is_some() {
                eprintln!("Usage: kiko reset-password [username]");
                return Some(2);
            }
            Some(reset_password_command(username.as_deref()))
        }
        Some("-h" | "--help") => {
            print_help();
            Some(0)
        }
        _ => None,
    }
}

fn print_help() {
    println!(
        "Kiko {}\n\n\
         Usage:\n  \
         kiko [ -ip <addr> ] [ -port <port> ] [ -rs ]\n  \
         kiko --migrate-only\n  \
         kiko --check-schema\n  \
         kiko hash-password\n  \
         kiko reset-password [username]\n\n\
         --migrate-only runs pending migrations and exits (no listener).\n\
         --check-schema compares the database migration version with this binary.\n\
         hash-password reads the password from stdin and prints an argon2id hash\n\
         for [bootstrap] admin_password_hash in data/kiko.toml.\n\
         reset-password reads the new password twice from stdin, updates the\n\
         user's password, and revokes all of that user's sessions. The username\n\
         defaults to [bootstrap] admin_username (usually `admin`).",
        env!("CARGO_PKG_VERSION")
    );
}

fn hash_password_command() -> i32 {
    eprint!("Password: ");
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_line(&mut input) {
        eprintln!("Failed to read the password: {e}");
        return 1;
    }
    // 只去掉行尾换行；前后空格可能是口令的一部分。
    let password = input.trim_end_matches(['\r', '\n']);
    match hash_stdin_password(password) {
        Ok(hash) => {
            println!("{hash}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// 供 CLI 与单测共用：空口令拒绝，其余交给 argon2id。
pub(crate) fn hash_stdin_password(password: &str) -> Result<String> {
    if password.is_empty() {
        return Err(Error::WithMessage(String::from(
            "Password must not be empty.",
        )));
    }
    auth::hash_password(password)
}

fn reset_password_command(username_arg: Option<&str>) -> i32 {
    let config = match crate::config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let username = username_arg
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&config.bootstrap_admin_username)
        .to_string();

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to start the async runtime: {e}");
            return 1;
        }
    };

    match runtime.block_on(reset_password_async(&config, &username)) {
        Ok(()) => {
            println!("Password reset for user '{username}'. All sessions were revoked.");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

async fn reset_password_async(config: &crate::config::Config, username: &str) -> Result<()> {
    // 只连库，且不跑 migration（auto_migrate = false）：运维恢复不应该在服务运行时
    // 偷偷改 schema。库落后时让用户先执行 `kiko --migrate-only`。
    // 也不初始化 master.key、不跑 bootstrap（用户已存在，bootstrap 本来也会跳过）。
    crate::repo::init(&config.database_url, false).await?;
    let pool = crate::repo::pool();
    let result = reset_password_with_pool(pool, username).await;
    // CLI 进程马上退出；显式关池，避免 current-thread runtime 先于 sqlx 连接销毁。
    pool.close().await;
    result
}

async fn reset_password_with_pool(pool: &sqlx::postgres::PgPool, username: &str) -> Result<()> {
    let user = crate::repo::user::find_by_username(pool, username)
        .await?
        .ok_or_else(|| {
            Error::WithMessage(format!(
                "User '{username}' does not exist. Pass the correct username: kiko reset-password <username>"
            ))
        })?;
    if user.status != "active" {
        eprintln!(
            "Warning: user '{}' has status '{}'; login stays blocked until it is 'active'.",
            user.username, user.status
        );
    }

    let password = read_new_password(&user.username)?;
    auth::reset_password(pool, &user, &password).await
}

/// 读两次新口令并确认。交互式运行时口令会回显；脚本化时可以用两行 stdin：
/// `printf 'new-password\nnew-password\n' | kiko reset-password admin`。
fn read_new_password(username: &str) -> Result<String> {
    eprint!("New password for '{username}': ");
    io::stderr().flush()?;
    let mut first = String::new();
    if io::stdin().read_line(&mut first)? == 0 {
        return Err(Error::WithMessage(String::from(
            "No password was provided on stdin.",
        )));
    }
    let first = first.trim_end_matches(['\r', '\n']);

    eprint!("Confirm new password: ");
    io::stderr().flush()?;
    let mut second = String::new();
    if io::stdin().read_line(&mut second)? == 0 {
        return Err(Error::WithMessage(String::from(
            "No password confirmation was provided on stdin.",
        )));
    }
    let second = second.trim_end_matches(['\r', '\n']);

    if first != second {
        return Err(Error::WithMessage(String::from("Passwords do not match.")));
    }
    if first.is_empty() {
        return Err(Error::WithMessage(String::from(
            "Password must not be empty.",
        )));
    }
    Ok(first.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_hash_is_argon2id_and_verifiable() {
        let hash = hash_stdin_password("a long enough passphrase").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(auth::verify_password(&hash, "a long enough passphrase"));
        assert!(!auth::verify_password(&hash, "another password"));
    }

    #[test]
    fn empty_password_is_rejected() {
        assert!(hash_stdin_password("").is_err());
    }
}
