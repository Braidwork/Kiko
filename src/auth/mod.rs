//! 本地管理员认证（B-33 / B-34 / B-35 / D24 / D27 / D42）。
//!
//! 设计边界（v1，已与用户确认）：
//! - 同源管理后台用 `HttpOnly; SameSite=Lax` cookie；中间件同时接受
//!   `Authorization: Bearer`，方便 CLI/测试与未来的 OIDC callback。
//! - 只有本地 admin 一种身份，**没有角色表**；权限判定集中在 [`is_admin`]，不按
//!   username 硬编码。将来接 SSO/RBAC 时只改这一处，`Principal` 的形状不变。
//! - 会话签发统一走 [`issue_session`]；认证中间件只校验会话有效性，不关心用户是
//!   本地密码登录还是将来的 OIDC 登录。
//! - `must_change_password` 只约束本地密码账号：改密前只放行 `/auth/*` 与白名单。
//! - `/flow/answer`、`/flow/answer/multipart` 在 v1 保持公开（知道 robotId 即可调用），
//!   这是已知边界，不是遗漏；机器人级 token 留到客户有明确需求时再做。
//!
//! 这里不写 SQL：用户与会话都走 `repo::user` / `repo::session`（AGENTS 硬约束 1）。

use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPool;

use crate::repo;
use crate::repo::user::User;
use crate::result::{Error, Result};
use crate::web::AppState;

/// 会话 cookie 名。
pub(crate) const SESSION_COOKIE: &str = "kiko_session";
/// 随机引导口令文件；改密成功后删除（B-07）。
pub(crate) const INITIAL_PASSWORD_FILE: &str = "data/initial_admin_password.txt";

/// 认证白名单（可扩展列表）。新增公开路由必须是**有意识的决定**：
/// 路由分类测试会逐条检查，未归类的新路由默认受保护。
pub(crate) const PUBLIC_PATHS: &[&str] = &[
    "/auth/login",
    "/auth/logout",
    "/healthz",
    "/version.json",
    "/check-new-version.json",
    // v1 运行时接口保持公开（§6.5 的 2026-10-31 确认）。
    "/flow/answer",
    "/flow/answer/multipart",
];

/// v1 唯一的权限判定。将来有角色表时，把 `status` 换成角色查询即可。
///
/// 禁止改成按 username 判断：SSO 用户的 username 可能映射成 `oidc:<sub>`。
pub(crate) fn is_admin(user: &User) -> bool {
    user.status == "active"
}

/// 已认证的请求主体。会话 id 保留下来供改密时“保留当前会话”使用。
#[derive(Clone, Debug)]
pub(crate) struct Principal {
    pub(crate) user_id: String,
    pub(crate) username: String,
    pub(crate) display_name: Option<String>,
    pub(crate) is_admin: bool,
    pub(crate) must_change_password: bool,
    pub(crate) session_id: String,
}

impl Principal {
    fn from_user(user: &User, session_id: String) -> Self {
        Self {
            user_id: user.id.clone(),
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            is_admin: is_admin(user),
            must_change_password: user.must_change_password,
            session_id,
        }
    }
}

/// `Principal` 提取器：认证中间件已经完成校验并放进 extensions。
///
/// 这里**不再查库**：中间件保证受保护路由一定有 Principal；直接挂到 handler 上
/// 却没有经过中间件时，返回 401 而不是悄悄放行。
impl<S> FromRequestParts<S> for Principal
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts.extensions.get::<Principal>().cloned().ok_or_else(|| {
            auth_error(
                StatusCode::UNAUTHORIZED,
                "Authentication required; please sign in.",
            )
        })
    }
}

/// 认证中间件。白名单与 `OPTIONS` 预检直接放行；其余一律要求有效会话。
pub(crate) async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    // CORS 预检不能要求凭据，否则浏览器永远拿不到响应头。
    if req.method() == axum::http::Method::OPTIONS {
        return next.run(req).await;
    }
    let path = req.uri().path().to_string();
    if is_public(&path) {
        return next.run(req).await;
    }

    match authenticate(&state.pool, req.headers()).await {
        Ok(Some(principal)) => {
            // 强制改密窗口：只允许认证相关接口（以及上面的白名单）。
            if principal.must_change_password && !is_auth_path(&path) {
                return auth_error(
                    StatusCode::FORBIDDEN,
                    "Password change required before accessing this resource.",
                );
            }
            req.extensions_mut().insert(principal);
            next.run(req).await
        }
        Ok(None) => auth_error(
            StatusCode::UNAUTHORIZED,
            "Authentication required; please sign in.",
        ),
        Err(e) => {
            log::error!("Session lookup failed: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Authentication is temporarily unavailable.",
            )
        }
    }
}

/// 安全响应头（B-40）。只加头，不碰 body，因此不会破坏 `/flow/answer` 的 NDJSON。
pub(crate) async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::HeaderName::from_static("x-frame-options"),
        header::HeaderValue::from_static("DENY"),
    );
    headers.insert(
        header::HeaderName::from_static("referrer-policy"),
        header::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        header::HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    res
}

fn is_auth_path(path: &str) -> bool {
    path.starts_with("/auth/")
}

/// 白名单判断。路径比较是精确的：`/auth/login/` 之类的变体不会被误放行。
pub(crate) fn is_public(path: &str) -> bool {
    PUBLIC_PATHS.contains(&path)
}

/// 从请求头取 token：Bearer 优先，其次 cookie。logout 也用它。
pub(crate) fn request_token(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get(header::AUTHORIZATION)
        && let Ok(text) = value.to_str()
        && let Some((scheme, token)) = text.split_once(' ')
        && scheme.eq_ignore_ascii_case("bearer")
        && !token.trim().is_empty()
    {
        return Some(token.trim().to_string());
    }
    cookie_token(headers)
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    for value in headers.get_all(header::COOKIE) {
        let Ok(text) = value.to_str() else { continue };
        for part in text.split(';') {
            let Some((name, value)) = part.trim().split_once('=') else {
                continue;
            };
            if name.trim() == SESSION_COOKIE {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// 校验会话并构造 Principal。令牌原文只在这里短暂存在。
pub(crate) async fn authenticate(pool: &PgPool, headers: &HeaderMap) -> Result<Option<Principal>> {
    let Some(token) = request_token(headers) else {
        return Ok(None);
    };
    // token 是 32 字节 base64url；长度异常就不必查库。
    if !(20..=256).contains(&token.len()) {
        return Ok(None);
    }
    let hash = token_hash(&token);
    let Some(session) = repo::session::find_valid(pool, &hash).await? else {
        return Ok(None);
    };
    let principal = Principal::from_user(&session.user, session.session_id);
    // 滑动续期失败不应让本次请求失败；下一轮再补。
    if let Err(e) = repo::session::touch(pool, &principal.session_id).await {
        log::warn!("Touching session failed: {e}");
    }
    Ok(Some(principal))
}

/// 会话签发集中入口（B-33 的 SSO 预留缝）。本地登录与未来 OIDC callback 共用。
pub(crate) async fn issue_session(
    pool: &PgPool,
    user_id: &str,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> Result<(String, String)> {
    let token = new_session_token()?;
    let session_id =
        repo::session::create(pool, user_id, &token_hash(&token), ip, user_agent).await?;
    Ok((token, session_id))
}

/// 32 字节随机 token，base64url（无 padding），可直接放进 cookie。
fn new_session_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::WithMessage(format!("Generating session token failed: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// 只存 token 的 SHA-256。
pub(crate) fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

pub(crate) fn session_cookie(token: &str, secure: bool) -> String {
    let mut cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        repo::session::ABSOLUTE_TTL_SECS
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

pub(crate) fn clear_session_cookie(secure: bool) -> String {
    let mut cookie = String::from(
        "kiko_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0; \
         Expires=Thu, 01 Jan 1970 00:00:00 GMT",
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// argon2id 哈希（RustCrypto 默认参数：m=19456 KiB, t=2, p=1）。
///
/// argon2 0.6 的 `hash_password` 自己生成随机 salt（`getrandom` 默认特性），
/// 不再需要手工构造 `SaltString`。
pub(crate) fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| Error::WithMessage(format!("Hashing password failed: {e}")))
}

pub(crate) fn verify_password(hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// 用户不存在时也跑一次同样成本的校验，避免按响应时间枚举账号。
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_password("kiko-dummy-password-for-timing").unwrap_or_else(|_| {
        // 理论上不会失败；真失败时给一个不可能匹配的串，登录仍会拒绝。
        String::from("$argon2id$invalid")
    })
});

pub(crate) fn verify_against_dummy(password: &str) {
    let _ = verify_password(&DUMMY_HASH, password);
}

/// 口令强度（D24）：长度 + 常见弱口令黑名单 + 不能等于用户名。
///
/// 哈希路径没法检查强度（只有 PHC 串），因此这里只用于明文路径。
pub(crate) fn validate_password_strength(username: &str, password: &str) -> Result<()> {
    const WEAK: &[&str] = &[
        "password",
        "password123",
        "passw0rd",
        "123456789012",
        "1234567890123",
        "12345678901234",
        "qwertyuiop12",
        "administrator",
        "changeme",
        "letmein",
        "kiko",
        "admin",
        "admin123",
        "kiko123",
    ];
    if password.chars().count() < 12 {
        return Err(Error::WithMessage(String::from(
            "Password is too short; use at least 12 characters.",
        )));
    }
    let lower = password.to_lowercase();
    if WEAK.iter().any(|w| lower == *w) || lower == username.to_lowercase() {
        return Err(Error::WithMessage(String::from(
            "Password is a well-known weak password; choose another one.",
        )));
    }
    Ok(())
}

/// 首次启动引导管理员（B-07）。`users` 非空时什么也不做。
///
/// 优先级：环境变量明文 > `admin_password_hash` > 随机生成并写文件。
pub(crate) async fn bootstrap_admin(
    pool: &PgPool,
    username: &str,
    env_password: Option<&str>,
    configured_hash: Option<&str>,
    password_file: &Path,
) -> Result<bool> {
    if repo::user::count(pool).await? > 0 {
        return Ok(false);
    }

    let username = username.trim();
    if username.is_empty() {
        return Err(Error::WithMessage(String::from(
            "Bootstrap administrator username is empty.",
        )));
    }

    let hash = if let Some(password) = env_password {
        validate_password_strength(username, password)?;
        hash_password(password)?
    } else if let Some(hash) = configured_hash {
        let parsed = PasswordHash::new(hash).map_err(|e| {
            Error::WithMessage(format!(
                "[bootstrap] admin_password_hash is not a valid PHC hash: {e}"
            ))
        })?;
        if parsed.algorithm.as_str() != "argon2id" {
            return Err(Error::WithMessage(String::from(
                "[bootstrap] admin_password_hash must be an argon2id hash; run `kiko hash-password`.",
            )));
        }
        hash.to_string()
    } else {
        let password = generate_password();
        let hash = hash_password(&password)?;
        write_initial_password_file(password_file, &password)?;
        log::info!(
            "Generated a random bootstrap administrator password at {} \
             (it will be removed after the first password change).",
            password_file.display()
        );
        hash
    };

    let id = scru128::new_string();
    repo::user::insert(pool, &id, username, &hash, true).await?;
    log::info!(
        "Created bootstrap administrator '{username}' (password change required on first login)."
    );
    Ok(true)
}

/// 运维恢复：重置一个已存在用户的密码，并吊销其全部会话（CLI `kiko reset-password`）。
///
/// 与 [`bootstrap_admin`] 的区别：这里不检查 `users` 是否为空，也不改 `status`；
/// 明文口令先过强度校验，再哈希。`must_change_password` 置 false，调用方（运维）
/// 已经选定了一个合规的新口令。
pub(crate) async fn reset_password(pool: &PgPool, user: &User, new_password: &str) -> Result<()> {
    validate_password_strength(&user.username, new_password)?;
    let hash = hash_password(new_password)?;
    repo::user::set_password(pool, &user.id, &hash, false).await?;
    repo::session::revoke_all_for_user(pool, &user.id).await?;
    Ok(())
}

/// 32 字符随机口令，去掉容易混淆的 `0/O/1/l/I`。
fn generate_password() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
    let mut bytes = [0u8; 32];
    // getrandom 失败概率极低；退化成固定串会违反 D24，所以先重试一次再放弃。
    if getrandom::fill(&mut bytes).is_err() {
        let mut retry = [0u8; 32];
        getrandom::fill(&mut retry).expect("OS random source is unavailable");
        bytes = retry;
    }
    let mut password = String::with_capacity(bytes.len());
    for b in bytes {
        password.push(CHARSET[b as usize % CHARSET.len()] as char);
    }
    password
}

fn write_initial_password_file(path: &Path, password: &str) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::WithMessage(format!("Creating {} failed: {e}", dir.display())))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| Error::WithMessage(format!("Writing {} failed: {e}", path.display())))?;
    file.write_all(format!("{password}\n").as_bytes())
        .map_err(|e| Error::WithMessage(format!("Writing {} failed: {e}", path.display())))?;
    restrict_permissions(path)?;
    Ok(())
}

/// 改密成功后删除初始化文件。删不掉只记日志：口令已经换了，文件内容也失效了。
pub(crate) fn remove_initial_password_file(path: &Path) {
    if !path.exists() {
        return;
    }
    match std::fs::remove_file(path) {
        Ok(()) => log::info!("Removed bootstrap password file {}", path.display()),
        Err(e) => log::warn!(
            "Could not remove bootstrap password file {}: {e}",
            path.display()
        ),
    }
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
    // Windows 依赖用户目录 ACL；与 security.rs 的 master.key 处理保持一致。
    Ok(())
}

/// 取客户端 IP：反代场景优先 `X-Forwarded-For`，否则 `X-Real-IP`。
/// 只作为会话元数据，不用于授权判断。
pub(crate) fn client_ip(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get("x-forwarded-for")
        && let Ok(text) = value.to_str()
        && let Some(first) = text.split(',').next()
        && !first.trim().is_empty()
    {
        return Some(first.trim().to_string());
    }
    headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(crate) fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(512).collect())
}

/// 认证失败的 JSON 信封：HTTP 状态码与信封 `status` 一致，形状保持
/// `{status, data, err:{message}}`（D9）。
pub(crate) fn auth_error(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({
        "status": status.as_u16(),
        "data": serde_json::Value::Null,
        "err": { "message": message },
    });
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| status.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hashing_round_trips_and_rejects_wrong_input() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password(&hash, "correct horse battery staple"));
        assert!(!verify_password(&hash, "wrong"));
        assert!(!verify_password("not-a-hash", "anything"));
    }

    #[test]
    fn weak_passwords_are_rejected() {
        assert!(validate_password_strength("admin", "short").is_err());
        assert!(validate_password_strength("admin", "password123").is_err());
        assert!(validate_password_strength("admin", "admin").is_err());
        assert!(validate_password_strength("admin", "a-perfectly-fine-passphrase").is_ok());
    }

    #[test]
    fn generated_password_is_long_and_has_no_ambiguous_characters() {
        let p = generate_password();
        assert_eq!(p.chars().count(), 32);
        assert!(!p.contains(['0', 'O', '1', 'l', 'I']));
        assert!(validate_password_strength("admin", &p).is_ok());
    }

    #[test]
    fn tokens_are_unique_and_hashes_are_stable() {
        let a = new_session_token().unwrap();
        let b = new_session_token().unwrap();
        assert_ne!(a, b);
        assert_eq!(token_hash(&a), token_hash(&a));
        assert_ne!(token_hash(&a), token_hash(&b));
        assert_eq!(token_hash(&a).len(), 32);
    }

    #[test]
    fn cookie_attributes_are_httponly_samesite_lax_and_optional_secure() {
        let c = session_cookie("abc", false);
        assert!(c.starts_with("kiko_session=abc;"));
        assert!(c.contains("HttpOnly"));
        assert!(c.contains("SameSite=Lax"));
        assert!(!c.contains("Secure"));
        assert!(session_cookie("abc", true).contains("Secure"));
        assert!(clear_session_cookie(false).contains("Max-Age=0"));
    }

    #[test]
    fn public_paths_are_exact_and_runtime_endpoints_stay_public() {
        assert!(is_public("/auth/login"));
        assert!(is_public("/healthz"));
        assert!(is_public("/flow/answer"));
        assert!(is_public("/flow/answer/multipart"));
        assert!(!is_public("/flow/answer/"));
        assert!(!is_public("/robot"));
        assert!(!is_public("/auth/me"));
    }

    // ================= B-07：引导管理员三条路径 =================

    use crate::repo::test_support::TestDb;

    fn temp_password_file() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "kiko-initial-{}.txt",
            scru128::new_string().to_lowercase()
        ))
    }

    #[tokio::test]
    async fn bootstrap_uses_the_environment_password_first() {
        let Some(db) = TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let file = temp_password_file();
        let created = bootstrap_admin(
            &db.pool,
            "admin",
            Some("a long enough password"),
            Some("$argon2id$should-be-ignored"),
            &file,
        )
        .await
        .unwrap();
        assert!(created);
        let user = crate::repo::user::find_by_username(&db.pool, "admin")
            .await
            .unwrap()
            .unwrap();
        assert!(user.must_change_password);
        assert!(verify_password(
            &user.password_hash,
            "a long enough password"
        ));
        assert!(!file.exists(), "env 路径不应写随机口令文件");

        // 已有用户时不再创建。
        assert!(
            !bootstrap_admin(
                &db.pool,
                "admin",
                Some("another long password"),
                None,
                &file
            )
            .await
            .unwrap()
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn bootstrap_accepts_a_configured_argon2id_hash() {
        let Some(db) = TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let hash = hash_password("a configured passphrase").unwrap();
        let file = temp_password_file();

        // 非 argon2id（这里用 argon2i 生成一个合法 PHC 串）必须被拒绝。
        // 必须在 users 为空时验证：引导函数对非空表直接返回 false。
        let bad = Argon2::new(
            argon2::Algorithm::Argon2i,
            argon2::Version::V0x13,
            argon2::Params::default(),
        )
        .hash_password(b"x")
        .unwrap()
        .to_string();
        let err = bootstrap_admin(&db.pool, "admin2", None, Some(&bad), &file)
            .await
            .unwrap_err();
        assert!(err.message().contains("argon2id"), "{err}");

        assert!(
            bootstrap_admin(&db.pool, "admin", None, Some(&hash), &file)
                .await
                .unwrap()
        );
        let user = crate::repo::user::find_by_username(&db.pool, "admin")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.password_hash, hash);
        assert!(verify_password(
            &user.password_hash,
            "a configured passphrase"
        ));

        db.cleanup().await;
    }

    #[tokio::test]
    async fn bootstrap_generates_a_random_password_file_when_nothing_is_configured() {
        let Some(db) = TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let file = temp_password_file();
        assert!(
            bootstrap_admin(&db.pool, "admin", None, None, &file)
                .await
                .unwrap()
        );
        let password = std::fs::read_to_string(&file).unwrap();
        let password = password.trim_end();
        assert_eq!(password.chars().count(), 32);
        let user = crate::repo::user::find_by_username(&db.pool, "admin")
            .await
            .unwrap()
            .unwrap();
        assert!(verify_password(&user.password_hash, password));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_file(&file);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn reset_password_updates_hash_and_revokes_sessions() {
        let Some(db) = TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let user_id = scru128::new_string();
        let old_hash = hash_password("old-password-12345").unwrap();
        repo::user::insert(&db.pool, &user_id, "admin", &old_hash, false)
            .await
            .unwrap();
        let user = repo::user::find_by_username(&db.pool, "admin")
            .await
            .unwrap()
            .unwrap();

        let (token, _session_id) = issue_session(&db.pool, &user.id, None, None).await.unwrap();
        let token_hash = token_hash(&token);
        assert!(
            repo::session::find_valid(&db.pool, &token_hash)
                .await
                .unwrap()
                .is_some()
        );

        // 弱口令必须被拒绝，且旧密码仍然有效。
        let err = reset_password(&db.pool, &user, "short").await.unwrap_err();
        assert!(err.message().contains("too short"), "{err}");
        assert!(verify_password(&old_hash, "old-password-12345"));

        reset_password(&db.pool, &user, "a new long password")
            .await
            .unwrap();
        let user = repo::user::find_by_username(&db.pool, "admin")
            .await
            .unwrap()
            .unwrap();
        assert!(!user.must_change_password);
        assert!(verify_password(&user.password_hash, "a new long password"));
        // 重置后旧会话必须立即失效。
        assert!(
            repo::session::find_valid(&db.pool, &token_hash)
                .await
                .unwrap()
                .is_none()
        );

        db.cleanup().await;
    }
}
