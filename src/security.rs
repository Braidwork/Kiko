//! 主密钥与敏感字段加解密（B-06 / D20 / D36）。
//!
//! 只做一件事：AES-256-GCM 加解密 `secrets.value`。密钥来源优先级：
//!
//! 1. 环境变量 `KIKO_MASTER_KEY`（base64 的 32 字节）；
//! 2. 配置文件 `[security] master_key`（同上）；
//! 3. `data/master.key`（首次启动生成，Unix 下 0600）。
//!
//! 放文件而不是数据库，是因为它必须在连库之前就可用；根目录的 `kiko.toml` 是部署方
//! 管理的文件，程序不擅自改写它，所以自动生成落在 `data/master.key`。交付文档必须
//! 写清边界：主密钥与备份同机存放挡不住拿到整机的人，只能防数据库备份泄露。
//!
//! 密文格式：`v1:` + base64(nonce(12) || ciphertext+tag)。前缀留给以后换算法。

use std::path::Path;
use std::sync::OnceLock;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};

use crate::result::{Error, Result};

/// 环境变量覆盖（D20）。
pub(crate) const MASTER_KEY_ENV: &str = "KIKO_MASTER_KEY";
/// 自动生成的主密钥文件。
pub(crate) const MASTER_KEY_FILE: &str = "data/master.key";

const CIPHER_PREFIX: &str = "v1:";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

static MASTER_KEY: OnceLock<[u8; KEY_LEN]> = OnceLock::new();

/// 启动时初始化。`configured` 是配置文件里的 `[security] master_key`。
pub(crate) fn init(configured: Option<&str>) -> Result<()> {
    let from_env = std::env::var(MASTER_KEY_ENV).ok();
    let source = from_env
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| configured.map(str::trim).filter(|s| !s.is_empty()));

    let key = match source {
        Some(encoded) => decode_key(encoded)?,
        None => load_or_generate_key_file()?,
    };
    set_key(key)
}

/// 测试/工具用：直接给一把密钥。
#[cfg(test)]
pub(crate) fn init_with_key(key: [u8; KEY_LEN]) {
    let _ = MASTER_KEY.set(key);
}

pub(crate) fn encrypt(plaintext: &str) -> Result<String> {
    let key = MASTER_KEY
        .get()
        .ok_or_else(|| Error::WithMessage(String::from("Master key is not initialized")))?;
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| Error::WithMessage(format!("Invalid master key: {e}")))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes)
        .map_err(|e| Error::WithMessage(format!("Generating nonce failed: {e}")))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| Error::WithMessage(format!("Encrypting secret failed: {e}")))?;

    let mut payload = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&ciphertext);
    Ok(format!("{CIPHER_PREFIX}{}", B64.encode(payload)))
}

pub(crate) fn decrypt(encoded: &str) -> Result<String> {
    let encoded = encoded.trim();
    let Some(body) = encoded.strip_prefix(CIPHER_PREFIX) else {
        return Err(Error::WithMessage(String::from(
            "Unsupported secret ciphertext version",
        )));
    };
    let key = MASTER_KEY
        .get()
        .ok_or_else(|| Error::WithMessage(String::from("Master key is not initialized")))?;
    let payload = B64
        .decode(body)
        .map_err(|e| Error::WithMessage(format!("Decoding secret failed: {e}")))?;
    if payload.len() <= NONCE_LEN {
        return Err(Error::WithMessage(String::from(
            "Secret ciphertext is too short",
        )));
    }
    let (nonce, ciphertext) = payload.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| Error::WithMessage(format!("Invalid master key: {e}")))?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| Error::WithMessage(String::from("Decrypting secret failed")))?;
    String::from_utf8(plaintext)
        .map_err(|e| Error::WithMessage(format!("Secret is not valid UTF-8: {e}")))
}

fn set_key(key: [u8; KEY_LEN]) -> Result<()> {
    MASTER_KEY
        .set(key)
        .map_err(|_| Error::WithMessage(String::from("Master key was initialized twice")))?;
    Ok(())
}

fn decode_key(encoded: &str) -> Result<[u8; KEY_LEN]> {
    let bytes = B64
        .decode(encoded.trim())
        .map_err(|e| Error::WithMessage(format!("Master key is not valid base64: {e}")))?;
    let len = bytes.len();
    bytes.try_into().map_err(|_| {
        Error::WithMessage(format!(
            "Master key must decode to {KEY_LEN} bytes, got {len}"
        ))
    })
}

fn load_or_generate_key_file() -> Result<[u8; KEY_LEN]> {
    let path = Path::new(MASTER_KEY_FILE);
    if path.exists() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::WithMessage(format!("Reading {} failed: {e}", path.display())))?;
        return decode_key(&text);
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::WithMessage(format!("Creating {} failed: {e}", dir.display())))?;
    }
    let mut key = [0u8; KEY_LEN];
    getrandom::getrandom(&mut key)
        .map_err(|e| Error::WithMessage(format!("Generating master key failed: {e}")))?;
    std::fs::write(path, format!("{}\n", B64.encode(key)))
        .map_err(|e| Error::WithMessage(format!("Writing {} failed: {e}", path.display())))?;
    restrict_permissions(path)?;
    log::info!(
        "Generated a new master key at {} (override with {} if needed)",
        path.display(),
        MASTER_KEY_ENV
    );
    Ok(key)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| Error::WithMessage(format!("Reading {} failed: {e}", path.display())))?
        .permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)
        .map_err(|e| Error::WithMessage(format!("Chmod {} failed: {e}", path.display())))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    // Windows 下依赖用户的目录 ACL；这里不做额外处理。
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_test_key() {
        init_with_key([7u8; KEY_LEN]);
    }

    #[test]
    fn ciphertext_round_trips() {
        init_test_key();
        let cipher = encrypt("sk-secret-值").unwrap();
        assert!(cipher.starts_with(CIPHER_PREFIX));
        assert!(!cipher.contains("sk-secret"), "密文里不能出现明文");
        assert_eq!(decrypt(&cipher).unwrap(), "sk-secret-值");
    }

    #[test]
    fn the_same_plaintext_gets_a_fresh_nonce() {
        init_test_key();
        let a = encrypt("same").unwrap();
        let b = encrypt("same").unwrap();
        assert_ne!(a, b, "随机 nonce 必须让密文不同");
        assert_eq!(decrypt(&a).unwrap(), decrypt(&b).unwrap());
    }

    #[test]
    fn malformed_ciphertext_is_an_error_not_a_panic() {
        init_test_key();
        assert!(decrypt("v1:not-base64").is_err());
        assert!(decrypt("v2:abc").is_err());
        assert!(decrypt("").is_err());
    }

    #[test]
    fn key_must_be_32_bytes() {
        assert!(decode_key(&B64.encode([0u8; 31])).is_err());
        assert!(decode_key(&B64.encode([0u8; 32])).is_ok());
        assert!(decode_key("not base64").is_err());
    }
}
