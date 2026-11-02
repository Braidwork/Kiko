//! 认证 HTTP 接口（B-34）：登录、登出、当前用户、改密。
//!
//! 所有响应保持 `{status, data, err}` 信封（D9）。失败时叠加标准 HTTP 状态码：
//! 401 未认证 / 403 已认证但无权（或必须先改密）/ 400 请求内容不合法。

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use crate::auth::{self, Principal};
use crate::repo;
use crate::repo::user::User;
use crate::result::Result;
use crate::web::AppState;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChangePasswordRequest {
    old_password: String,
    new_password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserView {
    id: String,
    username: String,
    display_name: Option<String>,
    is_admin: bool,
    must_change_password: bool,
}

impl UserView {
    fn from_user(user: &User, must_change_password: bool) -> Self {
        Self {
            id: user.id.clone(),
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            is_admin: auth::is_admin(user),
            must_change_password,
        }
    }

    fn from_principal(p: &Principal) -> Self {
        Self {
            id: p.user_id.clone(),
            username: p.username.clone(),
            display_name: p.display_name.clone(),
            is_admin: p.is_admin,
            must_change_password: p.must_change_password,
        }
    }
}

pub(crate) async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    let username = req.username.trim();
    if username.is_empty() || req.password.is_empty() {
        return auth::auth_error(StatusCode::UNAUTHORIZED, "Invalid username or password.");
    }

    let user = match repo::user::find_by_username(&state.pool, username).await {
        Ok(u) => u,
        Err(e) => {
            log::error!("Login lookup failed: {e}");
            return auth::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Login is temporarily unavailable.",
            );
        }
    };
    let Some(user) = user else {
        // 不存在也跑一次等价成本的校验，避免按响应时间枚举账号。
        auth::verify_against_dummy(&req.password);
        return auth::auth_error(StatusCode::UNAUTHORIZED, "Invalid username or password.");
    };
    if !auth::verify_password(&user.password_hash, &req.password) {
        return auth::auth_error(StatusCode::UNAUTHORIZED, "Invalid username or password.");
    }
    if user.status != "active" {
        return auth::auth_error(StatusCode::FORBIDDEN, "This account is disabled.");
    }

    let ip = auth::client_ip(&headers);
    let ua = auth::user_agent(&headers);
    let (token, _session_id) =
        match auth::issue_session(&state.pool, &user.id, ip.as_deref(), ua.as_deref()).await {
            Ok(v) => v,
            Err(e) => {
                log::error!("Issuing session failed: {e}");
                return auth::auth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not create a session.",
                );
            }
        };
    if let Err(e) = repo::user::touch_login(&state.pool, &user.id).await {
        log::warn!("Updating last_login_at failed: {e}");
    }
    if let Err(e) = repo::session::delete_expired(&state.pool).await {
        log::warn!("Cleaning expired sessions failed: {e}");
    }

    let view = UserView::from_user(&user, user.must_change_password);
    json_with_cookie(
        StatusCode::OK,
        serde_json::to_value(view).unwrap_or(serde_json::Value::Null),
        Some(auth::session_cookie(&token, state.cookie_secure)),
    )
}

/// 登出是幂等的：没有有效会话也返回成功，并总是清掉浏览器 cookie。
pub(crate) async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::request_token(&headers) {
        let hash = auth::token_hash(&token);
        if let Err(e) = repo::session::revoke_by_hash(&state.pool, &hash).await {
            log::warn!("Revoking session on logout failed: {e}");
        }
    }
    json_with_cookie(
        StatusCode::OK,
        serde_json::Value::Bool(true),
        Some(auth::clear_session_cookie(state.cookie_secure)),
    )
}

pub(crate) async fn me(principal: Principal) -> Response {
    json_with_cookie(
        StatusCode::OK,
        serde_json::to_value(UserView::from_principal(&principal))
            .unwrap_or(serde_json::Value::Null),
        None,
    )
}

pub(crate) async fn change_password(
    State(state): State<AppState>,
    principal: Principal,
    Json(req): Json<ChangePasswordRequest>,
) -> Response {
    let user = match repo::user::find_by_id(&state.pool, &principal.user_id).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return auth::auth_error(StatusCode::UNAUTHORIZED, "Session user no longer exists.");
        }
        Err(e) => {
            log::error!("Reading user for password change failed: {e}");
            return auth::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not change the password.",
            );
        }
    };

    if !auth::verify_password(&user.password_hash, &req.old_password) {
        return auth::auth_error(StatusCode::BAD_REQUEST, "Current password is incorrect.");
    }
    if let Err(e) = auth::validate_password_strength(&user.username, &req.new_password) {
        return auth::auth_error(StatusCode::BAD_REQUEST, &e.message());
    }
    if req.new_password == req.old_password {
        return auth::auth_error(
            StatusCode::BAD_REQUEST,
            "New password must be different from the current password.",
        );
    }

    let hash = match auth::hash_password(&req.new_password) {
        Ok(h) => h,
        Err(e) => {
            log::error!("Hashing new password failed: {e}");
            return auth::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not change the password.",
            );
        }
    };
    let r: Result<()> = async {
        repo::user::set_password(&state.pool, &user.id, &hash, false).await?;
        // 改密后吊销其他会话；当前会话保留，避免刚改完就被踢下线。
        repo::session::revoke_others(&state.pool, &user.id, &principal.session_id).await?;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        log::error!("Changing password failed: {e}");
        return auth::auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not change the password.",
        );
    }

    // 引导口令文件里的旧口令已经失效；删掉它（失败只记日志）。
    auth::remove_initial_password_file(&state.initial_password_file);

    let view = UserView::from_user(&user, false);
    json_with_cookie(
        StatusCode::OK,
        serde_json::to_value(view).unwrap_or(serde_json::Value::Null),
        None,
    )
}

fn json_with_cookie(
    status: StatusCode,
    data: serde_json::Value,
    cookie: Option<String>,
) -> Response {
    let body = serde_json::json!({
        "status": status.as_u16(),
        "data": data,
        "err": serde_json::Value::Null,
    });
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(header::SET_COOKIE, cookie);
    }
    builder
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|e| {
            log::error!("Building auth response failed: {e}");
            auth::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not build the response.",
            )
        })
}
