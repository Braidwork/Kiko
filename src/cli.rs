//! 命令行入口（B-07）。
//!
//! 目前只处理 `kiko hash-password`：从 stdin 读口令、输出 argon2id PHC 串，供
//! `[bootstrap] admin_password_hash` 使用。口令**不从 argv 读**，避免出现在 shell
//! 历史与进程列表里。其余参数（`-ip` / `-port` / `-rs`）仍由 `start_app` 处理。

use std::io::Write;

use crate::auth;
use crate::result::Result;

/// 若参数命中 CLI 子命令，返回退出码；否则返回 `None`，继续走服务启动。
pub fn run() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("hash-password") => Some(hash_password_command()),
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
         kiko hash-password\n\n\
         --migrate-only runs pending migrations and exits (no listener).\n\
         --check-schema compares the database migration version with this binary.\n\
         hash-password reads the password from stdin and prints an argon2id hash\n\
         for [bootstrap] admin_password_hash in data/kiko.toml.",
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
        return Err(crate::result::Error::WithMessage(String::from(
            "Password must not be empty.",
        )));
    }
    auth::hash_password(password)
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
