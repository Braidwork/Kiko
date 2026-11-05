// use std::net::SocketAddr;
use std::sync::LazyLock;
use std::vec::Vec;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use colored::Colorize;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;

use super::asset::ASSETS_MAP;
use super::auth as auth_api;
use crate::ai::crud as ai;
use crate::auth;
use crate::external::http::crud as http;
use crate::flow::mainflow::crud as mainflow;
use crate::flow::rt::facade as rt;
use crate::flow::subflow::crud as subflow;
use crate::intent::crud as intent;
use crate::kb::crud as kb;
use crate::man::settings;
use crate::repo;
use crate::result::Error;
use crate::robot::crud as robot;
use crate::variable::crud as variable;
use crate::web::AppState;

//https://stackoverflow.com/questions/27840394/how-can-a-rust-program-access-metadata-from-its-cargo-package
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
static VERSION_NUM: LazyLock<u64> = LazyLock::new(|| convert_version(VERSION));

const ASSETS: &[(&[u8], &str)] = &include!("asset.txt");

pub(crate) static IS_EN: LazyLock<bool> = LazyLock::new(|| {
    let language = get_lang();
    // println!("Your OS language is: {}", language);
    language[0..2].eq("en")
});

// https://doc.rust-lang.org/reference/conditional-compilation.html
#[cfg(windows)]
fn get_lang() -> String {
    let mut v = [0u16; windows::Win32::System::SystemServices::LOCALE_NAME_MAX_LENGTH as usize];
    unsafe {
        let l = windows::Win32::Globalization::GetUserDefaultLocaleName(&mut v) as usize;
        String::from_utf16(&v[0..l]).unwrap()
        // windows::Win32::Globalization::GetUserDefaultLangID().to_string()
    }
}

#[cfg(not(windows))]
fn get_lang() -> String {
    std::env::var("LANG").unwrap_or(String::from("en_US"))
}

// fn invalid_ip_msg(addr: &String) -> String {
//     format!("Invalid listening addr {}, please reset the configuration parameters by adding the startup parameter: {}",
//     addr.bright_red(), "-rs".bright_yellow())
// }

pub async fn start_app() {
    // 启动顺序（D33 fail-fast）：配置 → PostgreSQL（连池 + 迁移）→ 其余存储 → 起服务。
    // 任何一步失败都以非 0 退出：不猜 DSN、不进降级模式、不做 setup 向导。
    let config = match crate::config::load() {
        Ok(c) => c,
        Err(e) => {
            log::error!("{e}");
            std::process::exit(1);
        }
    };
    log::info!("Using config file: {}", config.source.display());

    // B-04/B-05：离线迁移与 schema 预检。执行完直接退出，不起监听、不初始化主密钥
    // 与管理员；沿用 `-ip` / `-port` 一样的手写 argv 解析。
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--migrate-only") {
        migrate_only(&config).await;
        return;
    }
    if args.iter().any(|a| a == "--check-schema") {
        check_schema(&config).await;
        return;
    }

    if let Err(e) = crate::security::init(config.master_key.as_deref()) {
        log::error!("Failed to initialize master key: {e}");
        std::process::exit(1);
    }
    if let Err(e) = crate::repo::init(&config.database_url, config.auto_migrate).await {
        log::error!(
            "Failed to initialize PostgreSQL: {e}\n\
             Check the [database] url in {} or the {} environment variable.",
            config.source.display(),
            crate::config::DATABASE_URL_ENV
        );
        std::process::exit(1);
    }

    // B-07：users 为空时创建本地管理员。必须在起监听之前完成，失败即退出。
    // 优先级：KIKO_BOOTSTRAP_ADMIN_PASSWORD > admin_password_hash > 随机文件。
    let initial_password_file = std::path::PathBuf::from(auth::INITIAL_PASSWORD_FILE);
    if let Err(e) = auth::bootstrap_admin(
        repo::pool(),
        &config.bootstrap_admin_username,
        config.bootstrap_admin_password.as_deref(),
        config.bootstrap_admin_password_hash.as_deref(),
        &initial_password_file,
    )
    .await
    {
        log::error!("Failed to bootstrap the local administrator: {e}");
        std::process::exit(1);
    }

    let app_state = AppState {
        pool: repo::pool().clone(),
        cookie_secure: config.cookie_secure,
        cors_origins: config.cors_origins.clone(),
        initial_password_file: initial_password_file.clone(),
    };

    // 知识库与向量的存储全部在 PG（S4）；进程内索引按 `(robot, collection)` 懒加载，
    // 启动时不需要初始化任何嵌入式数据库。
    log::info!(
        "Vector index budget: {} MB; reconciliation every {} s (0 = disabled)",
        crate::config::vector_index_budget_bytes() / (1024 * 1024),
        crate::config::vector_reconcile_secs()
    );
    // 后台对账（B-28）：已载入的索引按行数与 PG 比对，漂移就作废缓存并标 dirty；
    // `[vector] reconcile_secs = 0`（或环境变量 `KIKO_VECTOR_RECONCILE_SECS=0`）关闭。
    // 必须在 repo::init 之后。
    crate::vector::reconcile::spawn();

    let settings = {
        let is_en = *IS_EN;
        mainflow::init_default_names(is_en).expect("Initialize default flow names failed");
        let mut s = settings::init_global()
            .await
            .expect("Initialize global settings failed");
        // 首次启动（没有任何机器人）才建默认机器人；不要每次启动都建。
        let robots = repo::robot::list(repo::pool())
            .await
            .expect("List robots failed");
        if robots.is_empty() {
            robot::init(is_en)
                .await
                .expect("Initialize default robot failed");
        }
        // 升级兼容：机器人行可能早于设置迁移就存在（A/redb 时代的旧库），
        // 没有 robot_settings 的补一份默认值，避免运行期读不到设置。
        for r in robots.iter() {
            let exists = repo::settings::get_robot(repo::pool(), &r.robot_id)
                .await
                .expect("Read robot settings failed")
                .is_some();
            if !exists {
                settings::init(&r.robot_id)
                    .await
                    .expect("Initialize robot settings failed");
            }
        }
        for argument in std::env::args() {
            if argument.eq("-rs") {
                s = settings::GlobalSettings::default();
                settings::save_global_settings(&s)
                    .await
                    .expect("Reset settings failed");
                break;
            }
        }
        s
    };

    let mut listening_ip = String::with_capacity(32);
    let mut port: u16 = 0;
    let mut set_listening_ip = false;
    let mut set_listening_port = false;
    for argument in std::env::args() {
        if set_listening_ip {
            listening_ip.push_str(&argument);
            set_listening_ip = false;
            continue;
        }
        if argument.eq("-ip") {
            set_listening_ip = true;
            continue;
        }
        if set_listening_port {
            port = argument.parse::<u16>().unwrap();
            set_listening_port = false;
            continue;
        }
        if argument.eq("-port") {
            set_listening_port = true;
            continue;
        }
    }
    if listening_ip.is_empty() {
        listening_ip.push_str(&settings.ip);
    }
    if port == 0 {
        port = settings.port;
    }

    let (sender, recv) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(crate::flow::rt::context::clean_expired_session(recv));

    let r: Router = gen_router(app_state);
    let app = into_app(r);
    // let socket_addr: SocketAddr = addr.parse().expect(&invalid_ip_msg(&addr));
    let mut bind_res;
    // let mut port = settings.port;
    loop {
        let addr = format!("{}:{}", &listening_ip, port);
        bind_res = tokio::net::TcpListener::bind(&addr).await;
        if bind_res.is_ok() {
            break;
        }
        if !settings.select_random_port_when_conflict {
            log::error!("The listening port is occupied and the program fails to start.");
            log::info!("Tip: You can check the random port in the settings to avoid this problem.");
            std::process::exit(-1);
        }
        port += 1;
        if port == settings.port {
            log::error!("The listening port is occupied and the program fails to start.");
            log::info!("Tip: You can check the random port in the settings to avoid this problem.");
            std::process::exit(-1);
        }
        if port == 65535 {
            port = 1025;
        }
    }
    let listener = bind_res.unwrap();

    #[cfg(target_os = "windows")]
    colored::control::set_virtual_terminal(true).unwrap();

    log::info!(
        "-->  {} {}{}:{}",
        if *IS_EN {
            "The server is running, please open a browser and visit"
        } else {
            "服务已启动，请用浏览器访问"
        },
        "http://".bright_green(),
        if listening_ip.eq("0.0.0.0") {
            "<Your machine IP>".bright_green()
        } else {
            listening_ip.bright_green()
        },
        port.to_string().blue()
    );
    log::info!(
        "Tip: {} {} {} {} {}",
        if *IS_EN {
            "You can use"
        } else {
            "你可以使用"
        },
        "-ip".yellow(),
        if *IS_EN { "and" } else { "和" },
        "-port".yellow(),
        if *IS_EN {
            "to customize the listening IP and port"
        } else {
            "来自定义监听的IP和端口"
        },
    );
    log::info!("---------------------------------------------");
    log::info!("Current version: {VERSION}");
    log::info!("Visiting https://braidwork.github.io/kiko/ for the latest releases");

    log::info!("-->  To close the server, hit {}", "Ctrl+C".bright_red());

    // let addr = format!("{}:{}", settings.ip, settings.port);
    // let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    // let addr = SocketAddr::from((settings.ip, settings.port));
    let serve = axum::serve(listener, app);
    // log::info!("{:?}", serve.local_addr().unwrap());
    serve
        .with_graceful_shutdown(shutdown_signal(sender))
        .await
        .unwrap();
}

/// `--migrate-only`：跑完 migration 就退出。成功 0，失败 1；不绑定端口。
async fn migrate_only(config: &crate::config::Config) {
    log::info!("Running migrations only (no listener will be started)");
    if let Err(e) = repo::init(&config.database_url, true).await {
        log::error!("Migration failed: {e}");
        std::process::exit(1);
    }
    match repo::schema_version(repo::pool()).await {
        Ok(version) => {
            println!("database_schema_version={version}");
            println!("program_schema_version={}", repo::program_schema_version());
            std::process::exit(0);
        }
        Err(e) => {
            log::error!("Reading schema version after migration failed: {e}");
            std::process::exit(1);
        }
    }
}

/// `--check-schema`：只读比对数据库与程序内置的 migration 最大版本。
/// 只有两者相等才返回 0；落后（需要迁移）或超前（二进制太旧）都返回 1。
async fn check_schema(config: &crate::config::Config) {
    match repo::check_schema(&config.database_url).await {
        Ok((db, program)) => {
            println!("database_schema_version={db}");
            println!("program_schema_version={program}");
            if db == program {
                std::process::exit(0);
            }
            if db < program {
                eprintln!(
                    "Database schema is behind this binary; run `kiko --migrate-only` \
                     before starting the service."
                );
            } else {
                eprintln!(
                    "Database schema is newer than this binary; use a newer Kiko binary \
                     instead of starting this one."
                );
            }
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Schema check failed: {e}");
            std::process::exit(1);
        }
    }
}

fn gen_router(state: AppState) -> Router {
    let cors = cors_layer(&state.cors_origins);
    let router = Router::new()
        // ---- 认证（B-34）----
        .route("/auth/login", post(auth_api::login))
        .route("/auth/logout", post(auth_api::logout))
        .route("/auth/me", get(auth_api::me))
        .route("/auth/change-password", post(auth_api::change_password))
        .route(
            "/robot",
            get(robot::list).post(robot::save).delete(robot::delete),
        )
        .route("/robot/detail", get(robot::detail))
        .route(
            "/intent",
            get(intent::list).post(intent::add).delete(intent::remove),
        )
        .route("/intent/detect", post(intent::detect))
        .route("/intent/detail", get(intent::detail))
        .route(
            "/intent/keyword",
            post(intent::add_keyword).delete(intent::remove_keyword),
        )
        .route(
            "/intent/regex",
            post(intent::add_regex).delete(intent::remove_regex),
        )
        .route(
            "/intent/phrase",
            post(intent::add_phrase).delete(intent::remove_phrase),
        )
        .route(
            "/intent/phrase/regenerate-all",
            post(intent::regenerate_embeddings),
        )
        .route(
            "/variable",
            get(variable::list)
                .post(variable::add)
                .delete(variable::delete),
        )
        .route(
            "/mainflow",
            get(mainflow::list)
                .post(mainflow::new)
                .put(mainflow::save)
                .delete(mainflow::delete),
        )
        .route("/mainflow/release", post(subflow::release))
        .route(
            "/subflow",
            get(subflow::list)
                .post(subflow::save)
                .delete(subflow::delete),
        )
        .route("/subflow/simple", get(subflow::simple_list))
        .route("/subflow/new", post(subflow::new))
        .route("/external/http", get(http::list))
        .route(
            "/external/http/{id}",
            get(http::detail).post(http::save).delete(http::remove),
        )
        .route(
            "/management/global-settings",
            get(settings::rest_get_global_settings).post(settings::rest_save_global_settings),
        )
        .route(
            "/management/settings",
            get(settings::get).post(settings::save),
        )
        .route(
            "/management/settings/model/download",
            post(settings::download_model_files),
        )
        .route(
            "/management/settings/model/download/progress",
            get(settings::download_model_progress),
        )
        .route(
            "/management/settings/model/check/files",
            post(settings::check_model_files),
        )
        .route(
            "/management/settings/model/local/path",
            post(settings::local_model_paths),
        )
        .route(
            "/management/settings/model/load/progress",
            get(settings::model_load_progress),
        )
        .route(
            "/management/settings/model/load",
            post(settings::load_model_now),
        )
        .route(
            "/management/settings/model/check/embedding",
            get(settings::check_embedding_model),
        )
        .route(
            "/management/settings/embedding/vector-dimensions",
            get(settings::vector_dimensions),
        )
        .route(
            "/management/settings/embedding/reindex",
            post(crate::man::reindex::start),
        )
        .route(
            "/management/settings/embedding/reindex/progress",
            get(crate::man::reindex::progress),
        )
        .route(
            "/management/settings/embedding/reindex/hint",
            get(crate::man::reindex::hint),
        )
        .route(
            "/management/settings/model/openai/list",
            get(settings::list_openai_models),
        )
        .route(
            "/kb/qa",
            get(kb::list_qa).post(kb::save_qa).delete(kb::delete_qa),
        )
        .route(
            "/kb/doc",
            get(kb::list_doc)
                .post(kb::update_doc)
                .delete(kb::delete_doc),
        )
        .route("/kb/doc/upload", post(kb::upload_doc))
        .route("/kb/qa/dryrun", get(kb::qa_dryrun))
        .route("/management/settings/smtp/test", post(settings::smtp_test))
        // v1 运行时接口保持公开（B-33 白名单；见 auth::PUBLIC_PATHS）。
        .route("/flow/answer", post(rt::answer))
        .route("/flow/answer/multipart", post(rt::answer_multipart))
        .route("/ai/text/generation", post(ai::gen_text))
        .route("/healthz", get(healthz))
        .route("/version.json", get(version))
        .route("/check-new-version.json", get(check_new_version))
        // .route("/o", get(subflow::output))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(
            250 * 1024 * 1024, /* 250mb */
        ))
        // 认证层：只校验会话，不关心登录方式（D14/D27/D42）。
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        .with_state(state);
    match cors {
        Some(cors) => router.layer(cors),
        None => router,
    }
}

/// 组装最终应用：先补静态资源 fallback（**不加认证**），再统一加安全头。
///
/// fallback 必须在认证层之后挂，否则 `index.html`/JS/CSS 也会被 401；安全头则
/// 要覆盖包括静态资源在内的所有响应。
pub(crate) fn into_app(router: Router) -> Router {
    router
        .fallback(fallback)
        .layer(axum::middleware::from_fn(auth::security_headers))
}

/// CORS 默认同源：只有 `[server] cors_origins` 非空时才放行指定源。
///
/// `allow_credentials(true)` 与 cookie 会话配套；`AllowOrigin::list` 不接受 `*`，
/// 因此不会出现“任意站点携带 cookie”的配置。白名单为空时完全不挂 CORS 层，
/// 浏览器会按同源策略拒绝跨源读取。
fn cors_layer(origins: &[String]) -> Option<CorsLayer> {
    if origins.is_empty() {
        return None;
    }
    let values: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|origin| match origin.parse::<HeaderValue>() {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!("Ignoring invalid CORS origin {origin:?}: {e}");
                None
            }
        })
        .collect();
    if values.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(values))
            .allow_credentials(true)
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
            ]),
    )
}

// https://docs.rs/axum/0.6.18/axum/response/index.html

async fn fallback(uri: Uri) -> Response {
    let v = ASSETS_MAP.get(uri.path());
    if v.is_some() {
        let idx = v.unwrap();
        let d = ASSETS[*idx];
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, d.1.parse().unwrap());
        headers.insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
        (StatusCode::OK, headers, d.0).into_response()
    } else {
        (StatusCode::NOT_FOUND, format!("Not Found: {}", uri.path())).into_response()
    }
}

fn convert_version(ver: &str) -> u64 {
    let arr: Vec<&str> = ver.split('.').collect();
    let mut v = String::with_capacity(VERSION.len() + 4);
    v.push_str(arr[0]);
    if arr[1].len() == 1 {
        v.push_str("00");
    } else if arr[1].len() == 2 {
        v.push('0');
    }
    v.push_str(arr[1]);
    if arr[2].len() == 1 {
        v.push_str("00");
    } else if arr[2].len() == 2 {
        v.push('0');
    }
    v.push_str(arr[2]);
    // log::info!("vernum={}", &v);
    v.parse().expect("Wrong version")
}

async fn version() -> impl IntoResponse {
    let mut v = String::with_capacity(15);
    v.push('"');
    v.push_str(VERSION);
    v.push('"');
    v
}

/// B-52：运维用的健康检查。公开（白名单），不查业务数据，只确认进程和
/// `_sqlx_migrations` 可读；schema 版本对不上时运维能立刻看出升级没做完。
async fn healthz(axum::extract::State(state): axum::extract::State<AppState>) -> Response {
    match repo::schema_version(&state.pool).await {
        Ok(schema_version) => {
            let body = serde_json::json!({
                "status": StatusCode::OK.as_u16(),
                "data": {
                    "status": "ok",
                    "appVersion": VERSION,
                    "schemaVersion": schema_version,
                    "programSchemaVersion": repo::program_schema_version(),
                },
                "err": serde_json::Value::Null,
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        }
        Err(e) => {
            log::error!("Health check failed: {e}");
            auth::auth_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Database schema is unavailable.",
            )
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct VersionInfo {
    version: String,
    changelog: Vec<String>,
}

async fn check_new_version() -> impl IntoResponse {
    // D38：私有化/内网部署默认不主动出网；[server] check_new_version = true 才检查。
    if !crate::config::check_new_version_enabled() {
        return to_res(Ok(None::<VersionInfo>));
    }
    let r = reqwest::get("https://braidwork.github.io/kiko/check-new-version.json").await;
    if let Err(e) = r {
        return to_res(Err(Error::NetworkConnectTimeout(Box::new(e))));
    }
    r.unwrap().text().await.map_or_else(
        |e| to_res(Err(Error::NetworkReadTimeout(Box::new(e)))),
        |s| {
            let obj: core::result::Result<VersionInfo, _> = serde_json::from_str(&s);
            if let Err(e) = obj {
                return to_res(Err(Error::InvalidJsonStructure(Box::new(e))));
            }
            let v = obj.unwrap();
            if convert_version(&v.version) > *VERSION_NUM {
                to_res(Ok(Some(v)))
            } else {
                to_res(Ok(None))
            }
        },
    )
}

async fn shutdown_signal(sender: tokio::sync::oneshot::Sender<()>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    match sender.send(()) {
        Ok(_) => {}
        Err(_) => log::info!("中断 ctx 失败"),
    };

    // crate::intent::phrase::shutdown_db().await;
    // crate::kb::qa::shutdown_db().await;
    // crate::kb::doc::shutdown_db().await;

    let m = if *IS_EN {
        "This program has been terminated"
    } else {
        "应用已退出"
    };
    log::info!("{m}");
}

#[derive(Serialize)]
struct ResponseData<D> {
    pub(crate) status: u16,
    pub(crate) data: Option<D>,
    pub(crate) err: Option<Error>,
}

/// Serializes one result as `{status, data, err}`.
///
/// Both transports use this: the single-document response is exactly this text,
/// and the terminal frame of a streamed response carries the same text in its
/// `content`, so a client reads a result the same way either way. The status is
/// inside the document rather than on the HTTP response, which is why a failure
/// is a 200 at the HTTP level.
pub(crate) fn envelope_json<D>(r: Result<D, Error>) -> String
where
    D: serde::Serialize,
{
    let res: ResponseData<D> = match r {
        Ok(d) => ResponseData {
            status: StatusCode::OK.as_u16(),
            data: Some(d),
            err: None,
        },
        Err(e) => ResponseData {
            status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            data: None,
            err: Some(e),
        },
    };
    serde_json::to_string(&res).unwrap()
}

pub(crate) fn to_res2<D>(r: Result<D, Error>) -> axum::response::Response
where
    D: serde::Serialize + 'static + std::marker::Send,
{
    let body = axum::body::Body::from(envelope_json(r));
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

/// Streams frames as newline-delimited JSON: one JSON value per line, so a
/// client can split on `\n` and parse each line on its own.
///
/// `X-Accel-Buffering` is not decoration — nginx buffers proxied chunked
/// responses by default, which would hold every frame until the end and defeat
/// the whole point of streaming in the most common deployment.
pub(crate) fn to_ndjson(
    receiver: tokio::sync::mpsc::UnboundedReceiver<crate::flow::rt::dto::StreamingResponseData>,
) -> axum::response::Response {
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(receiver);
    let body = axum::body::Body::from_stream(stream.map(|frame| {
        Ok::<_, std::convert::Infallible>(format!("{}\n", serde_json::to_string(&frame).unwrap()))
    }));
    Response::builder()
        .status(200)
        .header(header::TRANSFER_ENCODING, "chunked")
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap()
}

// pub(crate) enum ResponseDataHolder<D> {
//     Normal(D),
//     Chunked(tokio::sync::mpsc::UnboundedReceiver<D>),
// }

// pub(crate) fn t<D>(d: ResponseDataHolder<D>) -> axum::response::Response
// where
//     D: serde::Serialize + 'static + std::marker::Send,
// {
//     let builder = Response::builder().status(200);
//     match d {
//         ResponseDataHolder::Normal(d) => {
//             let res = ResponseData {
//                 status: StatusCode::OK.as_u16(),
//                 data: Some(d),
//                 err: None,
//             };
//             let body = axum::body::Body::from(serde_json::to_string(&res).unwrap());
//             builder
//                 .header(header::CONTENT_TYPE, "application/json")
//                 .body(body)
//                 .unwrap()
//         }
//         ResponseDataHolder::Chunked(r) => {
//             let s = tokio_stream::wrappers::UnboundedReceiverStream::new(r);
//             let body = axum::body::Body::from_stream(s.map(|d| {
//                 let res = ResponseData {
//                     status: StatusCode::OK.as_u16(),
//                     data: Some(d),
//                     err: None,
//                 };
//                 let body = serde_json::to_string(&res).unwrap();
//                 Ok::<_, std::convert::Infallible>(body)
//             }));
//             builder
//                 .header(header::TRANSFER_ENCODING, "chunked")
//                 .body(body)
//                 .unwrap()
//         }
//     }
// }

pub(crate) fn to_res<D>(r: Result<D, Error>) -> impl IntoResponse
where
    D: serde::Serialize + 'static,
{
    // let now = std::time::Instant::now();
    let data = match r {
        Ok(d) => {
            let res = ResponseData {
                status: StatusCode::OK.as_u16(),
                data: Some(&d),
                err: None,
            };
            serde_json::to_string(&res).unwrap()
            // simd_json::to_string(&res).unwrap()
        }
        Err(e) => {
            let res: ResponseData<D> = ResponseData {
                status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                data: None,
                err: Some(e),
            };
            serde_json::to_string(&res).unwrap()
            // simd_json::to_string(&res).unwrap()
        }
    };
    // log::info!("serialize used time:{:?}", now.elapsed());
    let mut header_map = HeaderMap::new();
    header_map.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    (StatusCode::OK, header_map, data)
}

pub(crate) fn is_en(headers: &axum::http::HeaderMap) -> bool {
    let client_language = headers
        .get("Accept-Language")
        .map_or_else(|| "en-US", |v| v.to_str().unwrap_or("en-US"));
    if !client_language.is_empty() && client_language.starts_with("en") {
        true
    } else {
        *IS_EN
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::rt::dto::{Request, ResponseChannelWrapper, ResponseData};
    use crate::flow::subflow::dto::NextActionType;

    fn req() -> Request {
        serde_json::from_str(
            r#"{"robotId":"rb","mainFlowId":"mf","sessionId":"ss","userInputResult":"Successful","userInput":"hi"}"#,
        )
        .unwrap()
    }

    /// The transport contract every client depends on: one JSON value per line,
    /// each line newline-terminated. Without that delimiter a client has to
    /// guess where a frame ends — which is what the shipped JavaScript SDK did,
    /// by splitting on `}{`.
    #[tokio::test]
    async fn frames_are_newline_delimited_and_the_last_one_is_the_terminal_frame() {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let channel = ResponseChannelWrapper::new(sender);
        assert!(channel.push_frame(0, String::from("Hel")));
        assert!(channel.push_frame(0, String::from("lo")));
        let mut data = ResponseData::new(&req());
        data.next_action = NextActionType::Terminate;
        assert!(channel.push_terminal(envelope_json(Ok(data))));
        // Dropping the only sender ends the body, exactly as finishing the flow
        // task does.
        drop(channel);

        let res = to_ndjson(receiver);
        assert_eq!(
            res.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/x-ndjson"
        );
        assert!(
            res.headers().contains_key("x-accel-buffering"),
            "a proxy must be told not to buffer the frames"
        );
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = core::str::from_utf8(&body).unwrap();
        let Some(text) = text.strip_suffix('\n') else {
            panic!("the body must end with a newline: {text:?}");
        };
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 3, "one line per frame: {lines:?}");

        let delta: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(delta["contentSeq"], 0);
        assert_eq!(delta["content"], "Hel");

        let last: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert!(
            last["contentSeq"].is_null(),
            "a null sequence is what marks the terminal frame"
        );
        let envelope: serde_json::Value =
            serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
        assert_eq!(envelope["status"], 200);
        assert_eq!(envelope["data"]["nextAction"], "Terminate");
    }

    /// The point of the whole exercise, checked on the wire rather than through
    /// a `Body`: the frames have to leave as they are produced. Reading the body
    /// in-process would happily buffer everything and still pass.
    #[tokio::test]
    async fn frames_reach_the_socket_one_by_one() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn stream_slowly() -> axum::response::Response {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let channel = ResponseChannelWrapper::new(sender);
            tokio::spawn(async move {
                channel.push_frame(0, String::from("first"));
                tokio::time::sleep(core::time::Duration::from_millis(300)).await;
                channel.push_frame(0, String::from("second"));
                tokio::time::sleep(core::time::Duration::from_millis(300)).await;
                let mut data = ResponseData::new(&req());
                data.next_action = NextActionType::Terminate;
                channel.push_terminal(envelope_json(Ok(data)));
            });
            to_ndjson(receiver)
        }

        let app = axum::Router::new().route("/flow/answer", axum::routing::post(stream_slowly));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
        socket
            .write_all(
                b"POST /flow/answer HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\
                  Connection: close\r\n\r\n",
            )
            .await
            .unwrap();

        let expected = [
            r#"{"contentSeq":0,"content":"first"}"#,
            r#"{"contentSeq":0,"content":"second"}"#,
            r#""contentSeq":null"#,
        ];
        let mut seen: Vec<Option<std::time::Instant>> = vec![None; expected.len()];
        let mut raw: Vec<u8> = Vec::with_capacity(4096);
        loop {
            let mut chunk = [0u8; 4096];
            let n = socket.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&raw);
            for (i, want) in expected.iter().enumerate() {
                if seen[i].is_none() && text.contains(want) {
                    seen[i] = Some(std::time::Instant::now());
                }
            }
        }

        let text = String::from_utf8_lossy(&raw);
        let head = text.split("\r\n\r\n").next().unwrap().to_lowercase();
        assert!(head.starts_with("http/1.1 200 ok"), "{head}");
        assert_eq!(
            head.matches("transfer-encoding: chunked").count(),
            1,
            "the response must be chunked exactly once: {head}"
        );
        assert!(
            head.contains("content-type: application/x-ndjson"),
            "{head}"
        );

        let seen: Vec<std::time::Instant> = seen
            .into_iter()
            .map(|t| t.expect("a frame never arrived"))
            .collect();
        // The flow waits 300 ms between the first two frames, so a body that was
        // buffered until the end would show almost no spread.
        let spread = seen[2].duration_since(seen[0]);
        assert!(
            spread >= core::time::Duration::from_millis(250),
            "the frames arrived {spread:?} apart, they were held back"
        );
    }

    /// A failure is carried inside the terminal frame with the same shape a
    /// non-streaming request uses, because the HTTP status is already committed
    /// by the time the failure is known.
    #[tokio::test]
    async fn a_failure_is_reported_in_the_terminal_frame() {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let channel = ResponseChannelWrapper::new(sender);
        channel.push_terminal(envelope_json::<ResponseData>(Err(Error::WithMessage(
            String::from("boom"),
        ))));
        drop(channel);

        let res = to_ndjson(receiver);
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let frame: serde_json::Value =
            serde_json::from_str(core::str::from_utf8(&body).unwrap().trim()).unwrap();
        assert!(frame["contentSeq"].is_null());
        let envelope: serde_json::Value =
            serde_json::from_str(frame["content"].as_str().unwrap()).unwrap();
        assert_eq!(envelope["status"], 500);
        assert_eq!(envelope["err"]["message"], "boom");
        assert!(envelope["data"].is_null());
    }

    // ================= B-41：认证 / 会话 / 路由白名单 =================

    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    const ALL_ROUTE_PATHS: &[&str] = &[
        "/auth/login",
        "/auth/logout",
        "/auth/me",
        "/auth/change-password",
        "/robot",
        "/robot/detail",
        "/intent",
        "/intent/detect",
        "/intent/detail",
        "/intent/keyword",
        "/intent/regex",
        "/intent/phrase",
        "/intent/phrase/regenerate-all",
        "/variable",
        "/mainflow",
        "/mainflow/release",
        "/subflow",
        "/subflow/simple",
        "/subflow/new",
        "/external/http",
        "/external/http/x",
        "/management/global-settings",
        "/management/settings",
        "/management/settings/model/download",
        "/management/settings/model/download/progress",
        "/management/settings/model/check/files",
        "/management/settings/model/local/path",
        "/management/settings/model/load/progress",
        "/management/settings/model/load",
        "/management/settings/model/check/embedding",
        "/management/settings/embedding/vector-dimensions",
        "/management/settings/embedding/reindex",
        "/management/settings/embedding/reindex/progress",
        "/management/settings/embedding/reindex/hint",
        "/management/settings/model/openai/list",
        "/kb/qa",
        "/kb/doc",
        "/kb/doc/upload",
        "/kb/qa/dryrun",
        "/management/settings/smtp/test",
        "/flow/answer",
        "/flow/answer/multipart",
        "/ai/text/generation",
        "/healthz",
        "/version.json",
        "/check-new-version.json",
    ];

    fn test_state(db: &crate::repo::test_support::TestDb) -> AppState {
        AppState {
            pool: db.pool.clone(),
            cookie_secure: false,
            cors_origins: Vec::new(),
            initial_password_file: std::env::temp_dir()
                .join(format!("kiko-initial-{}.txt", scru128::new_string())),
        }
    }

    fn test_app(db: &crate::repo::test_support::TestDb) -> Router {
        into_app(gen_router(test_state(db)))
    }

    fn get_req(uri: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn get_req_with_cookie(uri: &str, cookie: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("GET")
            .uri(uri)
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    }

    fn post_json(uri: &str, body: serde_json::Value) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn json_body(res: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// 每条路由都必须被归类：要么在白名单里（有意识的公开），要么未认证返回 401。
    /// 新增路由时先把路径加进 `ALL_ROUTE_PATHS`，测试会替你做这个判断。
    #[tokio::test]
    async fn every_route_is_either_public_or_rejects_unauthenticated_requests() {
        let Some(db) = crate::repo::test_support::TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        assert_eq!(ALL_ROUTE_PATHS.len(), 46, "路由数量变了：请同步本测试");
        let app = test_app(&db);
        for path in ALL_ROUTE_PATHS {
            if auth::is_public(path) {
                continue;
            }
            let res = app.clone().oneshot(get_req(path)).await.unwrap();
            assert_eq!(
                res.status(),
                StatusCode::UNAUTHORIZED,
                "protected route {path} did not return 401"
            );
            let body = json_body(res).await;
            assert_eq!(body["status"], 401, "{path}: {body}");
            assert!(body["data"].is_null(), "{path}: {body}");
            assert!(
                body["err"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("Authentication required"),
                "{path}: {body}"
            );
        }
        db.cleanup().await;
    }

    /// 白名单路由不能被认证层拦下；静态资源 fallback 也不经过认证。
    #[tokio::test]
    async fn public_routes_and_static_assets_are_not_401() {
        let Some(db) = crate::repo::test_support::TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let app = test_app(&db);
        // 运行时接口：空请求体不是认证问题（handler 内部会报业务错误）。
        for path in ["/flow/answer", "/flow/answer/multipart"] {
            let res = app
                .clone()
                .oneshot(post_json(path, serde_json::json!({})))
                .await
                .unwrap();
            assert_ne!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
        // 静态资源：GET / 由 fallback 返回 index.html，不经过认证。
        let res = app.clone().oneshot(get_req("/")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().contains_key("x-content-type-options"));
        // version.json 公开。
        let res = app.clone().oneshot(get_req("/version.json")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        // B-52：健康检查公开，返回 app/schema 版本。
        let res = app.clone().oneshot(get_req("/healthz")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = json_body(res).await;
        assert_eq!(body["data"]["status"], "ok");
        assert_eq!(body["data"]["appVersion"], VERSION);
        assert!(body["data"]["schemaVersion"].as_i64().unwrap_or(0) > 0);
        assert_eq!(
            body["data"]["schemaVersion"],
            body["data"]["programSchemaVersion"]
        );
        // D38：默认不检查更新，也不出网。
        let res = app
            .clone()
            .oneshot(get_req("/check-new-version.json"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = json_body(res).await;
        assert_eq!(body["status"], 200);
        assert!(body["data"].is_null());
        db.cleanup().await;
    }

    async fn insert_user(
        db: &crate::repo::test_support::TestDb,
        must_change_password: bool,
    ) -> String {
        let id = scru128::new_string();
        let hash = auth::hash_password("a long enough password").unwrap();
        repo::user::insert(&db.pool, &id, "admin", &hash, must_change_password)
            .await
            .unwrap();
        id
    }

    fn set_cookie_token(res: &Response) -> String {
        let value = res.headers().get(header::SET_COOKIE).unwrap();
        let text = value.to_str().unwrap();
        text.split(';').next().unwrap().to_string()
    }

    #[tokio::test]
    async fn login_me_logout_round_trip_uses_httponly_cookie() {
        let Some(db) = crate::repo::test_support::TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        insert_user(&db, false).await;
        let app = test_app(&db);

        let res = app
            .clone()
            .oneshot(post_json(
                "/auth/login",
                serde_json::json!({"username":"admin","password":"a long enough password"}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        let cookie = set_cookie_token(&res);
        let body = json_body(res).await;
        assert_eq!(body["data"]["username"], "admin");
        assert_eq!(body["data"]["mustChangePassword"], false);

        let res = app
            .clone()
            .oneshot(get_req_with_cookie("/auth/me", &cookie))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = json_body(res).await;
        assert_eq!(body["data"]["username"], "admin");
        assert_eq!(body["data"]["isAdmin"], true);

        let res = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/auth/logout")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let res = app
            .clone()
            .oneshot(get_req_with_cookie("/auth/me", &cookie))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn forged_expired_and_revoked_tokens_are_rejected() {
        let Some(db) = crate::repo::test_support::TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let user_id = insert_user(&db, false).await;
        let app = test_app(&db);

        let forged = HttpRequest::builder()
            .method("GET")
            .uri("/auth/me")
            .header(header::AUTHORIZATION, "Bearer forged-token-value")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(forged).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        let (token, session_id) = auth::issue_session(&db.pool, &user_id, None, None)
            .await
            .unwrap();
        let cookie = format!("kiko_session={token}");
        assert_eq!(
            app.clone()
                .oneshot(get_req_with_cookie("/auth/me", &cookie))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        repo::session::expire_for_test(&db.pool, &session_id)
            .await
            .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(get_req_with_cookie("/auth/me", &cookie))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );

        let (token2, _) = auth::issue_session(&db.pool, &user_id, None, None)
            .await
            .unwrap();
        repo::session::revoke_by_hash(&db.pool, &auth::token_hash(&token2))
            .await
            .unwrap();
        let cookie2 = format!("kiko_session={token2}");
        assert_eq!(
            app.clone()
                .oneshot(get_req_with_cookie("/auth/me", &cookie2))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn must_change_password_blocks_business_routes_until_changed() {
        let Some(db) = crate::repo::test_support::TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let password_file =
            std::env::temp_dir().join(format!("kiko-initial-{}.txt", scru128::new_string()));
        std::fs::write(&password_file, "old password\n").unwrap();
        let state = AppState {
            pool: db.pool.clone(),
            cookie_secure: false,
            cors_origins: Vec::new(),
            initial_password_file: password_file.clone(),
        };
        let app = into_app(gen_router(state));
        insert_user(&db, true).await;

        let res = app
            .clone()
            .oneshot(post_json(
                "/auth/login",
                serde_json::json!({"username":"admin","password":"a long enough password"}),
            ))
            .await
            .unwrap();
        let cookie = set_cookie_token(&res);
        assert_eq!(res.status(), StatusCode::OK);

        let res = app
            .clone()
            .oneshot(get_req_with_cookie("/robot", &cookie))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let body = json_body(res).await;
        assert_eq!(body["status"], 403);
        assert!(
            body["err"]["message"]
                .as_str()
                .unwrap()
                .contains("Password change")
        );

        // 改密前 /auth/me 仍可用。
        assert_eq!(
            app.clone()
                .oneshot(get_req_with_cookie("/auth/me", &cookie))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let res = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/auth/change-password")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "oldPassword": "a long enough password",
                            "newPassword": "a brand new passphrase"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{:?}", json_body(res).await);
        assert!(!password_file.exists(), "改密后必须删除引导口令文件");

        let user = repo::user::find_by_username(&db.pool, "admin")
            .await
            .unwrap()
            .unwrap();
        assert!(!user.must_change_password);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn changing_the_password_revokes_other_sessions_but_keeps_the_current_one() {
        let Some(db) = crate::repo::test_support::TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let user_id = insert_user(&db, false).await;
        let app = test_app(&db);
        let (current, _) = auth::issue_session(&db.pool, &user_id, None, None)
            .await
            .unwrap();
        let (other, _) = auth::issue_session(&db.pool, &user_id, None, None)
            .await
            .unwrap();
        let current = format!("kiko_session={current}");
        let other = format!("kiko_session={other}");

        let res = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/auth/change-password")
                    .header(header::COOKIE, &current)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "oldPassword": "a long enough password",
                            "newPassword": "a brand new passphrase"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            app.clone()
                .oneshot(get_req_with_cookie("/auth/me", &current))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(get_req_with_cookie("/auth/me", &other))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        db.cleanup().await;
    }

    /// D12：有副作用的接口必须 POST；已登录的 GET 应该被路由层拒绝（405），
    /// 而不是落到 handler 里执行。
    #[tokio::test]
    async fn side_effectful_routes_are_post_only() {
        let Some(db) = crate::repo::test_support::TestDb::new().await else {
            eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
            return;
        };
        let user_id = insert_user(&db, false).await;
        let (token, _) = auth::issue_session(&db.pool, &user_id, None, None)
            .await
            .unwrap();
        let cookie = format!("kiko_session={token}");
        let app = test_app(&db);
        for path in [
            "/mainflow/release?robotId=x&mainFlowId=y&data=",
            "/intent/phrase/regenerate-all?robotId=x&id=y&data=",
        ] {
            let res = app
                .clone()
                .oneshot(get_req_with_cookie(path, &cookie))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED, "{path}");
        }
        db.cleanup().await;
    }
}
