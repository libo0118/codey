use std::{
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, RwLock, Weak},
    time::Duration,
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::{
    Request, Response, StatusCode, body::Incoming, server::conn::http1, service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    sync::{Mutex as AsyncMutex, Semaphore},
    task::JoinSet,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        protocol::{Role, WebSocketConfig},
    },
};

use super::desktop::Desktop;
use crate::commands::AppState;

mod downloads;
mod stream;

type Body = UnsyncBoxBody<Bytes, std::io::Error>;
type Reply = Response<Body>;
const MAX_BODY: usize = 1024 * 1024;
// Four MiB of attachments expanded as base64, plus escaped message text and metadata.
const MAX_MESSAGE_BODY: usize = 7 * 1024 * 1024;
const STATE_COALESCE_WINDOW: Duration = Duration::from_millis(100);

fn schedule_state(pending: &mut Option<tokio::time::Instant>, urgent: bool) {
    let deadline = tokio::time::Instant::now()
        + if urgent {
            Duration::ZERO
        } else {
            STATE_COALESCE_WINDOW
        };
    *pending = Some(pending.map_or(deadline, |previous| previous.min(deadline)));
}
const COOKIE: &str = "codey_remote";
const HTML: &str = "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"UTF-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1,viewport-fit=cover,interactive-widget=resizes-content\"><meta name=\"color-scheme\" content=\"light dark\"><title>Codex · Codey 远程控制</title><link rel=\"stylesheet\" href=\"/remote.css\"></head><body><div id=\"root\"></div><script src=\"/remote.js\" defer></script></body></html>";

pub(super) struct Core {
    pub auth: super::SharedAuth,
    pub state: Weak<AppState>,
    pub public_url: RwLock<Option<String>>,
    pub address: SocketAddr,
    pub actions: AsyncMutex<super::actions::Ledger>,
    pub shutdown: tokio::sync::watch::Sender<bool>,
    pub streams: Arc<Semaphore>,
    pub logins: Arc<Semaphore>,
    pub files: Arc<Mutex<super::files::Registry>>,
    pub file_transfers: Arc<Semaphore>,
}

pub(super) fn validate_public_url(value: &str) -> Result<Option<String>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let url =
        reqwest::Url::parse(value.trim()).map_err(|_| "外部访问地址须为有效 HTTP(S) origin")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("外部访问地址须为不含路径、参数和凭据的 HTTP(S) origin".into());
    }
    Ok(Some(url.origin().ascii_serialization()))
}

fn allowed_origin(core: &Core, origin: &str) -> bool {
    if core
        .public_url
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        == Some(origin)
    {
        return true;
    }
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    if url.scheme() != "http"
        || url.port_or_known_default() != Some(core.address.port())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.origin().ascii_serialization() != origin
    {
        return false;
    }
    let host = url.host_str().unwrap_or_default();
    if host == "localhost" {
        return true;
    }
    match host.trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
                || IpAddr::V4(ip) == core.address.ip()
        }
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || IpAddr::V6(ip) == core.address.ip()
        }
        _ => false,
    }
}

fn request_origin(core: &Core, request: &Request<Incoming>) -> Result<bool, &'static str> {
    let host = request
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .ok_or("请求缺少主机地址")?;
    let http_origin = format!("http://{host}");
    let https_origin = format!("https://{host}");
    let secure = core
        .public_url
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        == Some(&https_origin);
    if !secure && !allowed_origin(core, &http_origin) {
        return Err("不允许的访问主机");
    }
    if let Some(origin) = request.headers().get("origin") {
        let origin = origin.to_str().map_err(|_| "请求来源无效")?;
        if origin != if secure { &https_origin } else { &http_origin } {
            return Err("不允许跨站访问远程控制");
        }
    } else if request.method() != hyper::Method::GET {
        return Err("操作请求缺少来源");
    }
    if request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        return Err("不允许跨站访问远程控制");
    }
    Ok(secure)
}

fn session_token(request: &Request<Incoming>) -> String {
    request
        .headers()
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.to_string())
        .unwrap_or_default()
}

fn authorized(core: &Core, token: &str) -> bool {
    core.auth
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .authorized(token)
}

fn response(status: StatusCode, content_type: &str, body: impl Into<Bytes>) -> Reply {
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(
            Full::new(body.into())
                .map_err(|never| -> std::io::Error { match never {} })
                .boxed_unsync(),
        )
        .expect("static response headers")
}

fn json_response(status: StatusCode, body: Value) -> Reply {
    response(status, "application/json; charset=utf-8", body.to_string())
}

fn failure(status: StatusCode, message: &str) -> Reply {
    json_response(status, json!({"status":"failed","message":message}))
}

fn cookie(token: &str, remember: bool, secure: bool) -> String {
    format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict{}{}",
        if token.is_empty() {
            "; Max-Age=0".to_owned()
        } else if remember {
            format!("; Max-Age={}", super::auth::REMEMBER_SECONDS)
        } else {
            String::new()
        },
        if secure { "; Secure" } else { "" }
    )
}

async fn login(core: Arc<Core>, ip: IpAddr, args: Value, secure: bool) -> Reply {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Credentials {
        username: String,
        password: String,
        #[serde(default)]
        remember: bool,
    }
    let Ok(credentials) = serde_json::from_value::<Credentials>(args) else {
        return failure(StatusCode::BAD_REQUEST, "登录字段无效");
    };
    if credentials.username.len() > 512 || credentials.password.len() > 1024 {
        return failure(StatusCode::UNAUTHORIZED, "账号或密码错误");
    }
    let Ok(permit) = Arc::clone(&core.logins).try_acquire_owned() else {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "登录尝试过于频繁，请稍后重试",
        );
    };
    let remember = credentials.remember;
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let (account, revision) = {
            let mut auth = core.auth.lock().unwrap_or_else(|error| error.into_inner());
            if !auth.attempt(ip) {
                return Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    "登录尝试过于频繁，请稍后重试",
                ));
            }
            (auth.credentials(), auth.revision().to_owned())
        };
        if !account
            .is_some_and(|account| account.verify(&credentials.username, &credentials.password))
        {
            return Err((StatusCode::UNAUTHORIZED, "账号或密码错误"));
        }
        core.auth
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .issue(&revision, remember)
            .map_err(|_| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "登录状态未保存，请稍后重试或在本机检查账号设置",
                )
            })
    })
    .await;
    match result {
        Ok(Ok(login)) => {
            let mut reply = json_response(StatusCode::OK, login.session);
            reply.headers_mut().insert(
                "set-cookie",
                cookie(&login.token, remember, secure)
                    .parse()
                    .expect("generated cookie"),
            );
            reply
        }
        Ok(Err((status, message))) => failure(status, message),
        Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE, "登录服务暂不可用"),
    }
}

pub(super) async fn serve(listener: TcpListener, core: Arc<Core>) {
    let permits = Arc::new(Semaphore::new(32));
    let mut connections = JoinSet::new();
    let mut shutdown = core.shutdown.subscribe();
    let mut expiry = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = expiry.tick() => {
                core.files.lock().unwrap_or_else(|e|e.into_inner()).prune(|token|authorized(&core,token));
            }
            accepted = listener.accept() => {
                let Ok((stream,peer)) = accepted else { tokio::time::sleep(Duration::from_millis(100)).await; continue; };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else { continue; };
                let core = Arc::clone(&core);
                connections.spawn(async move {
                    let _permit = permit;
                    let service = service_fn(move |request| handle(Arc::clone(&core),request,peer));
                    let _ = http1::Builder::new().timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(10))
                        .max_buf_size(16 * 1024).serve_connection(TokioIo::new(stream),service).with_upgrades().await;
                });
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    core.files.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

// A boxed service boundary also breaks the recursive type from the local
// command dispatcher that starts this server and the panel commands it serves.
fn handle(
    core: Arc<Core>,
    request: Request<Incoming>,
    peer: SocketAddr,
) -> futures_util::future::BoxFuture<'static, Result<Reply, Infallible>> {
    Box::pin(async move {
        let mut reply = route(core, request, peer).await;
        let headers = reply.headers_mut();
        for (name, value) in [
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
            ("x-content-type-options", "nosniff"),
            ("x-frame-options", "DENY"),
            (
                "content-security-policy",
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
        ] {
            headers.insert(
                hyper::header::HeaderName::from_static(name),
                hyper::header::HeaderValue::from_static(value),
            );
        }
        Ok(reply)
    })
}

async fn read_json(request: Request<Incoming>, max_body: usize) -> Result<Value, &'static str> {
    if request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v.split(';').next() != Some("application/json"))
    {
        return Err("请求须使用 JSON");
    }
    let mut body = request.into_body();
    let read = async {
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| "读取远程请求失败")?;
            if let Some(data) = frame.data_ref() {
                if bytes.len() + data.len() > max_body {
                    return Err("远程请求过大");
                }
                bytes.extend_from_slice(data);
            }
        }
        serde_json::from_slice(&bytes).map_err(|_| "远程请求 JSON 无效")
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .map_err(|_| "读取远程请求超时")?
}

async fn route(core: Arc<Core>, mut request: Request<Incoming>, peer: SocketAddr) -> Reply {
    let secure = match request_origin(&core, &request) {
        Ok(secure) => secure,
        Err(error) => return failure(StatusCode::FORBIDDEN, error),
    };
    let token = session_token(&request);
    let path = request.uri().path().to_string();
    let get = request.method() == hyper::Method::GET;
    if get {
        match path.as_str() {
            "/" => return response(StatusCode::OK, "text/html; charset=utf-8", HTML),
            "/remote.js" => {
                return response(
                    StatusCode::OK,
                    "text/javascript; charset=utf-8",
                    include_str!("../../../dist-overlay/codey-remote.js"),
                );
            }
            "/remote.css" => {
                return response(
                    StatusCode::OK,
                    "text/css; charset=utf-8",
                    include_str!("../../../dist-overlay/codey-remote.css"),
                );
            }
            _ => {}
        }
    }
    if get && path == "/remote/session" {
        return json_response(
            StatusCode::OK,
            core.auth
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .session(&token),
        );
    }
    let login_request = path == "/remote/login" && request.method() == hyper::Method::POST;
    if !login_request && !authorized(&core, &token) {
        return failure(StatusCode::UNAUTHORIZED, "登录已失效，请重新登录");
    }
    if get && let Some((thread, id)) = super::files::download_route(&path) {
        if request.headers().contains_key("range") {
            return failure(StatusCode::BAD_REQUEST, "暂不支持分段下载，请请求完整文件");
        }
        return downloads::download(core, token, thread.to_string(), id.to_string()).await;
    }
    if get && let Some(id) = path.strip_prefix("/remote/events/") {
        if !request.headers().contains_key("origin") {
            return failure(StatusCode::FORBIDDEN, "实时连接缺少来源");
        }
        if super::desktop::validate_thread(id).is_err() {
            return failure(StatusCode::BAD_REQUEST, "会话标识无效");
        }
        return event_stream(core, token, id.to_string(), &mut request);
    }
    if request.method() != hyper::Method::POST {
        return failure(StatusCode::METHOD_NOT_ALLOWED, "请求方法不支持");
    }
    if !login_request
        && !super::auth::csrf_matches(
            &token,
            request
                .headers()
                .get("x-csrf-token")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default(),
        )
    {
        return failure(StatusCode::FORBIDDEN, "请求校验失败，请刷新页面");
    }
    let max_body = if login_request {
        16 * 1024
    } else if matches!(path.as_str(), "/remote/action" | "/remote/create") {
        MAX_MESSAGE_BODY
    } else {
        MAX_BODY
    };
    let args = match read_json(request, max_body).await {
        Ok(args) if args.is_object() => args,
        Ok(_) => return failure(StatusCode::BAD_REQUEST, "远程请求须为 JSON 对象"),
        Err(error) => return failure(StatusCode::BAD_REQUEST, error),
    };
    if login_request {
        return login(core, peer.ip(), args, secure).await;
    }
    // A body upload may outlive revocation.
    if !authorized(&core, &token) {
        return failure(StatusCode::UNAUTHORIZED, "登录已失效，请重新登录");
    }
    if path == "/remote/logout" {
        let auth = Arc::clone(&core.auth);
        let result = tokio::task::spawn_blocking(move || {
            auth.lock()
                .unwrap_or_else(|e| e.into_inner())
                .logout(&token)
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            return failure(StatusCode::SERVICE_UNAVAILABLE, "注销未完成，请稍后重试");
        }
        let mut reply = json_response(StatusCode::OK, json!({"authenticated":false}));
        reply.headers_mut().insert(
            "set-cookie",
            cookie("", false, secure).parse().expect("generated cookie"),
        );
        return reply;
    }
    let result = match path.as_str() {
        "/remote/threads" => {
            let search = args["search"].as_str().unwrap_or_default().to_string();
            let archived = args["archived"].as_bool().unwrap_or(false);
            remote_threads(&core, search, archived).await
        }
        "/remote/projects" => {
            tokio::task::spawn_blocking(
                || super::store::projects(crate::codex_config::codex_home()),
            )
            .await
            .map_err(|_| "读取项目任务失败".to_string())
            .and_then(std::convert::identity)
        }
        "/remote/models" => {
            let Some(state) = core.state.upgrade() else {
                return failure(StatusCode::SERVICE_UNAVAILABLE, "Codey 正在退出");
            };
            Ok(state
                .bridge_request("/codex-model-catalog".into(), json!({}))
                .await)
        }
        "/remote/defaults" => {
            let Some(state) = core.state.upgrade() else {
                return failure(StatusCode::SERVICE_UNAVAILABLE, "Codey 正在退出");
            };
            super::create::defaults(&state, &args).await
        }
        "/remote/action" | "/remote/create" => action_once(&core, &token, &path, &args).await,
        "/remote/files/resolve" => downloads::resolve(&core, &token, args).await,
        _ if path.starts_with("/api/") => {
            let command = path.strip_prefix("/api/").unwrap_or_default();
            if !panel_command(command) || !panel_arguments_allowed(command, &args) {
                return failure(StatusCode::FORBIDDEN, "此操作仅可在电脑端使用");
            }
            let Some(state) = core.state.upgrade() else {
                return failure(StatusCode::SERVICE_UNAVAILABLE, "Codey 正在退出");
            };
            let result = if command == "load_codey_config" {
                // Snapshot only; the desktop loader may import or synchronize routes.
                let config = state.config.read().await;
                json!({"config":{"defaultModel":config.default_model,"fastContextTools":config.fast_context_tools,"profiles":config.profiles.iter().map(|profile|
                        json!({"id":profile.id,"name":profile.name,"enabled":profile.enabled})).collect::<Vec<_>>()}})
            } else {
                crate::commands::invoke_api(&state, command, args).await
            };
            Ok(panel_summary(command, result))
        }
        _ => return failure(StatusCode::NOT_FOUND, "远程接口不存在"),
    };
    match result {
        Ok(value) => json_response(StatusCode::OK, value),
        Err(error) => failure(StatusCode::BAD_REQUEST, &error),
    }
}

async fn remote_threads(core: &Core, search: String, archived: bool) -> Result<Value, String> {
    let threads = tokio::task::spawn_blocking(move || {
        super::store::threads(crate::codex_config::codex_home(), &search, archived)
    })
    .await
    .map_err(|_| "读取会话任务失败".to_string())??;
    Ok(threads_with_activity(threads, |ids| read_thread_activity(core, ids)).await)
}

async fn read_thread_activity(core: &Core, thread_ids: Vec<String>) -> Value {
    let Some(state) = core.state.upgrade() else {
        return Value::Null;
    };
    let Some(runtime) = state.runtime.lock().await.clone() else {
        return Value::Null;
    };
    let websocket = runtime.renderer_websocket_url().await;
    match crate::cdp::remote_control_request(
        &websocket,
        &json!({"action":"activity","threadIds":thread_ids}),
    )
    .await
    {
        Ok(value) => {
            if let Some(reason @ ("timeout" | "disconnected" | "request_failed")) =
                value["reason"].as_str()
            {
                eprintln!("[Codey] remote_thread_activity_unavailable: {reason}");
            }
            value
        }
        Err(_) => {
            eprintln!("[Codey] remote_thread_activity_unavailable: desktop_request_failed");
            Value::Null
        }
    }
}

async fn threads_with_activity<F: std::future::Future<Output = Value>>(
    mut threads: Value,
    read: impl FnOnce(Vec<String>) -> F,
) -> Value {
    let rows = threads
        .as_array_mut()
        .expect("thread inventory is an array");
    let ids = rows
        .iter()
        .take(200)
        .map(|row| {
            row["id"]
                .as_str()
                .expect("inventory thread has an id")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let activity = if ids.is_empty() {
        Value::Null
    } else {
        read(ids).await
    };
    // Merge only into filtered inventory rows; runtime results cannot revive hidden threads.
    for row in rows {
        let id = row["id"].as_str().expect("inventory thread has an id");
        row["running"] = activity["running"][id]
            .as_bool()
            .map_or(Value::Null, Value::Bool);
    }
    threads
}

fn event_stream(
    core: Arc<Core>,
    token: String,
    thread: String,
    request: &mut Request<Incoming>,
) -> Reply {
    let version = match super::projection::StreamVersion::from_query(request.uri().query()) {
        Ok(version) => version,
        Err(error) => return failure(StatusCode::BAD_REQUEST, error),
    };
    let headers = request.headers();
    let key = headers
        .get("sec-websocket-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    use base64::Engine;
    if headers
        .get("upgrade")
        .is_none_or(|v| !v.as_bytes().eq_ignore_ascii_case(b"websocket"))
        || headers
            .get("connection")
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| {
                !v.split(',')
                    .any(|s| s.trim().eq_ignore_ascii_case("upgrade"))
            })
        || headers
            .get("sec-websocket-version")
            .is_none_or(|v| v != "13")
        || !base64::engine::general_purpose::STANDARD
            .decode(key)
            .is_ok_and(|v| v.len() == 16)
    {
        return failure(StatusCode::BAD_REQUEST, "需要有效的 WebSocket 连接");
    }
    let Ok(permit) = Arc::clone(&core.streams).try_acquire_owned() else {
        return failure(StatusCode::TOO_MANY_REQUESTS, "实时连接已达上限");
    };
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
    let upgrade = hyper::upgrade::on(request);
    tokio::spawn(async move {
        let _permit = permit;
        let Ok(Ok(upgraded)) = tokio::time::timeout(Duration::from_secs(10), upgrade).await else {
            return;
        };
        let socket = WebSocketStream::from_raw_socket(
            TokioIo::new(upgraded),
            Role::Server,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(1024))
                    .max_frame_size(Some(1024)),
            ),
        )
        .await;
        stream::session(
            socket,
            &core,
            &token,
            version,
            follow_desktop(&core, &token, &thread),
        )
        .await;
    });
    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-accept", accept)
        .body(
            Full::new(Bytes::new())
                .map_err(|never| -> std::io::Error { match never {} })
                .boxed_unsync(),
        )
        .unwrap()
}

async fn action_once(
    core: &Arc<Core>,
    token: &str,
    path: &str,
    args: &Value,
) -> Result<Value, String> {
    let id = args["requestId"].as_str().ok_or("操作缺少请求标识")?;
    super::desktop::validate_thread(id)?;
    let digest: [u8; 32] = Sha256::digest(format!("{path}\n{args}").as_bytes()).into();
    authorize_action(core, token)?;
    let thread = if path == "/remote/create" {
        "/create"
    } else {
        args["threadId"].as_str().unwrap_or_default()
    };
    let operation = {
        let core = Arc::clone(core);
        let token = token.to_string();
        let create = path == "/remote/create";
        let args = args.clone();
        async move {
            // A queued action may outlive device authorization or service shutdown.
            authorize_action(&core, &token)?;
            if !create {
                return perform_action(&core, &token, &args).await;
            }
            let state = core.state.upgrade().ok_or("Codey 正在退出")?;
            if args.get("model").is_some() || args.get("effort").is_some() {
                let catalog = state
                    .bridge_request("/codex-model-catalog".into(), json!({}))
                    .await;
                validate_model_settings(&catalog, &json!({}), &args)?;
            }
            authorize_action(&core, &token)?;
            super::create::create(&state, &args, || authorize_action(&core, &token)).await
        }
    };
    let result = core
        .actions
        .lock()
        .await
        .start(id, digest, thread, operation)?;
    let value = result.finish().await;
    authorize_action(core, token)?;
    Ok(value)
}

fn authorize_action(core: &Core, token: &str) -> Result<(), String> {
    if *core.shutdown.borrow() || !authorized(core, token) {
        return Err("登录会话已撤销".into());
    }
    Ok(())
}

async fn follow_desktop(core: &Core, token: &str, thread: &str) -> Result<Desktop, String> {
    authorize_action(core, token)?;
    let desktop = follow_or_open(Desktop::follow(thread), async {
        authorize_action(core, token)?;
        let state = core.state.upgrade().ok_or("Codey 正在退出")?;
        super::create::open(&state, thread).await?;
        Desktop::follow(thread).await
    })
    .await?;
    authorize_action(core, token)?;
    Ok(desktop)
}

async fn follow_or_open<T>(
    following: impl std::future::Future<Output = Result<T, String>>,
    open: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    match following.await {
        Err(error) if error == super::desktop::OWNER_UNAVAILABLE => open.await,
        result => result,
    }
}

async fn perform_action(core: &Core, token: &str, args: &Value) -> Result<Value, String> {
    let thread = args["threadId"].as_str().ok_or("缺少会话标识")?;
    super::desktop::validate_thread(thread)?;
    let action = args["action"].as_str().ok_or("缺少会话操作")?;
    if action == "open" {
        follow_desktop(core, token, thread).await?;
        return Ok(json!({"status":"ok"}));
    }
    let mut desktop = Desktop::follow(thread).await?;
    if action == "settings" && (args.get("model").is_some() || args.get("effort").is_some()) {
        let state = core.state.upgrade().ok_or("Codey 正在退出")?;
        let catalog = state
            .bridge_request("/codex-model-catalog".into(), json!({}))
            .await;
        validate_model_settings(&catalog, &desktop.state, args)?;
    }
    authorize_action(core, token)?;
    desktop
        .action(
            action,
            args,
            async {
                let state = core.state.upgrade().ok_or("Codey 正在退出")?;
                let runtime = state
                    .runtime
                    .lock()
                    .await
                    .clone()
                    .ok_or("Codex 桌面尚未启动")?;
                let websocket = runtime.renderer_websocket_url().await;
                require_model_request_gate(&websocket).await?;
                authorize_action(core, token)
            },
            async {
                authorize_action(core, token)?;
                let state = core.state.upgrade().ok_or("Codey 正在退出")?;
                super::create::prepare_files(&state, args, || authorize_action(core, token)).await
            },
        )
        .await
}

pub(super) async fn require_model_request_gate(websocket_url: &str) -> Result<(), String> {
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        codey_runtime_core::bridge::evaluate_script(
            websocket_url,
            "window.__codeyModelWhitelistPatch?.requestGateReady?.() === true",
        ),
    )
    .await;
    if matches!(response, Ok(Ok(ref value)) if value["result"]["result"]["value"] == true) {
        Ok(())
    } else {
        Err("模型请求校验尚未就绪，远程操作未发送，请稍后重试".into())
    }
}

fn validate_model_settings(catalog: &Value, desktop: &Value, args: &Value) -> Result<(), String> {
    if catalog["status"] != "ok" || catalog["clear_models"] == true {
        return Err("模型目录暂不可用，请刷新模型列表".into());
    }
    let model = args.get("model").unwrap_or_else(|| {
        desktop["latestThreadSettings"]
            .get("model")
            .unwrap_or(&desktop["latestModel"])
    });
    if !catalog["models"]
        .as_array()
        .is_some_and(|models| models.contains(model))
    {
        return Err("该模型已不可用，请刷新模型列表后重新选择".into());
    }
    if let Some(effort) = args.get("effort") {
        let supported = catalog["model_metadata"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["model"] == *model)
            .and_then(|entry| entry["supported_reasoning_efforts"].as_array())
            .is_some_and(|values| values.contains(effort));
        if !supported {
            return Err("该模型不支持此思考程度，请刷新模型列表".into());
        }
    }
    Ok(())
}

// Remote summaries are allowlists, not a blacklist of known credential keys.
fn panel_summary(command: &str, value: Value) -> Value {
    if value["status"] == "failed" {
        return json!({"status":"failed","message":"读取远程摘要失败，请在本机检查状态"});
    }
    match command {
        "runtime_status" => json!({"running":value["running"].as_bool().unwrap_or(false),
            "fastContextToolsActive":value["fastContextToolsActive"].as_bool(),
            "clientPlatform":value["clientPlatform"].as_str().unwrap_or(""),
            "codexAppVersion":value["codexAppVersion"].as_str().unwrap_or("")}),
        "load_codey_config" => {
            json!({"config":{"defaultModel":value["config"]["defaultModel"].as_str().unwrap_or(""),
            "fastContextTools":value["config"]["fastContextTools"].as_bool(),
            "profiles":value["config"]["profiles"].as_array().into_iter().flatten().map(|profile|
                json!({"id":profile["id"].as_str().unwrap_or(""),"name":profile["name"].as_str().unwrap_or(""),
                    "enabled":profile["enabled"].as_bool().unwrap_or(true)})).collect::<Vec<_>>()}})
        }
        "builtin_tools" => {
            let mut summary = json!({"installed":value["installed"].as_bool().unwrap_or(false),"version":value["version"].as_str()});
            if value.get("mcp").is_some() {
                summary["mcp"] =
                    json!({"enabled":value["mcp"]["enabled"].as_bool().unwrap_or(false)});
            } else {
                summary["enabled"] = json!(value["enabled"].as_bool().unwrap_or(false));
            }
            summary
        }
        "list_codey_plugins" => {
            json!({"plugins":value["plugins"].as_array().into_iter().flatten().map(|plugin|
            json!({"id":plugin["id"].as_str().unwrap_or(""),"name":plugin["name"].as_str().unwrap_or(""),
                "enabled":plugin["enabled"].as_bool().unwrap_or(false)})).collect::<Vec<_>>() })
        }
        _ => json!({"status":"failed","message":"此操作仅可在本机使用"}),
    }
}

fn panel_command(command: &str) -> bool {
    matches!(
        command,
        "runtime_status" | "load_codey_config" | "builtin_tools" | "list_codey_plugins"
    )
}

fn panel_arguments_allowed(command: &str, args: &Value) -> bool {
    let Some(fields) = args.as_object() else {
        return false;
    };
    match command {
        "builtin_tools" => {
            args["action"] == "status"
                && args.get("tool").is_none_or(|tool| tool == "ponytail")
                && fields
                    .keys()
                    .all(|key| matches!(key.as_str(), "action" | "tool"))
        }
        _ => fields.is_empty(),
    }
}

#[cfg(test)]
mod tests;
