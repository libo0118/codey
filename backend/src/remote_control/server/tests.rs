use super::super::auth::{Auth, Credentials};
use super::*;

#[tokio::test]
async fn thread_activity_queries_only_filtered_inventory_and_never_adds_runtime_rows() {
    let inventory = json!([
        {"id":"root-a","title":"A","cwd":"/project","updatedAt":10},
        {"id":"root-b","title":"B","cwd":"/project","updatedAt":9},
        {"id":"unknown","title":"C","cwd":"/project","updatedAt":8}
    ]);
    let mut expected = inventory.clone();
    expected[0]["running"] = json!(true);
    expected[1]["running"] = json!(false);
    expected[2]["running"] = Value::Null;
    let rows = threads_with_activity(inventory, |ids| async move {
        assert_eq!(ids, ["root-a", "root-b", "unknown"]);
        json!({"running":{"root-a":true,"root-b":false,"hidden-subagent":true,"archived":true}})
    })
    .await;
    assert_eq!(rows, expected);
}

#[tokio::test]
async fn thread_activity_unavailability_is_unknown_and_empty_inventory_does_not_query() {
    let rows = threads_with_activity(json!([{"id":"root-a"}]), |_| async { Value::Null }).await;
    assert_eq!(rows, json!([{"id":"root-a","running":null}]));
    let rows = threads_with_activity(json!([]), |_| async {
        panic!("an empty inventory must not probe the desktop")
    })
    .await;
    assert_eq!(rows, json!([]));
    let inventory = json!(
        (0..200)
            .map(|id| json!({"id":id.to_string()}))
            .collect::<Vec<_>>()
    );
    let rows = threads_with_activity(inventory, |ids| async move {
        assert_eq!(ids.len(), 200);
        assert_eq!(ids.first().unwrap(), "0");
        assert_eq!(ids.last().unwrap(), "199");
        json!({"running":{"0":"true","199":false}})
    })
    .await;
    assert_eq!(rows[0]["running"], Value::Null);
    assert_eq!(rows[199]["running"], false);
}

#[tokio::test]
async fn existing_desktop_owners_are_followed_without_resuming_and_only_unowned_threads_open() {
    let owner = follow_or_open(async { Ok("existing owner") }, async {
        panic!("an existing owner must not be resumed or checked against the model gate")
    })
    .await
    .unwrap();
    assert_eq!(owner, "existing owner");
    for error in [
        "登录会话已撤销",
        "Codey 正在退出",
        "Codex 桌面连接已断开",
        "桌面快照会话不匹配",
    ] {
        let result = follow_or_open::<()>(async { Err(error.into()) }, async {
            panic!("unknown follow failures must not resume the thread")
        })
        .await;
        assert_eq!(result.unwrap_err(), error);
    }
    let owner = follow_or_open(
        async { Err(super::super::desktop::OWNER_UNAVAILABLE.into()) },
        async { Ok("opened owner") },
    )
    .await
    .unwrap();
    assert_eq!(owner, "opened owner");
    assert_eq!(
        follow_or_open::<()>(
            async { Err(super::super::desktop::OWNER_UNAVAILABLE.into()) },
            async { Err("模型请求校验尚未就绪".into()) },
        )
        .await
        .unwrap_err(),
        "模型请求校验尚未就绪"
    );
}

#[tokio::test]
async fn remote_model_gate_requires_true_and_hides_transport_errors() {
    for result in [
        json!({"result":{"type":"boolean","value":true}}),
        json!({"result":{"type":"boolean","value":false}}),
        json!({"result":{"type":"string","value":"true"}}),
        json!({"exceptionDetails":{"text":"PRIVATE_TRANSPORT_DETAILS"}}),
        Value::Null,
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let ready = result["result"]["value"] == true;
        let fixture = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let request: Value =
                serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(request["method"], "Runtime.evaluate");
            assert_eq!(
                request["params"]["expression"],
                "window.__codeyModelWhitelistPatch?.requestGateReady?.() === true"
            );
            assert_eq!(request["params"]["awaitPromise"], false);
            if !result.is_null() {
                socket
                    .send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
            }
        });
        let result = require_model_request_gate(&url).await;
        assert_eq!(result.is_ok(), ready);
        if !ready {
            assert_eq!(
                result.unwrap_err(),
                "模型请求校验尚未就绪，远程操作未发送，请稍后重试"
            );
        }
        fixture.await.unwrap();
    }
}

#[tokio::test]
async fn remote_model_gate_times_out_without_retrying_or_sending_a_mutation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (observed, received) = tokio::sync::oneshot::channel();
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let request: Value =
            serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(request["method"], "Runtime.evaluate");
        observed.send(()).unwrap();
        let _socket = socket;
        std::future::pending::<()>().await;
    });
    let started = tokio::time::Instant::now();
    assert_eq!(
        require_model_request_gate(&url).await.unwrap_err(),
        "模型请求校验尚未就绪，远程操作未发送，请稍后重试"
    );
    received.await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    fixture.abort();
    let _ = fixture.await;
}

struct Server {
    core: Arc<Core>,
    base: String,
    task: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.core.shutdown.send_replace(true);
        self.task.abort();
    }
}

async fn fixture() -> Server {
    let directory = tempfile::tempdir().unwrap();
    let mut auth = Auth::load(directory.path().join("remote.json")).unwrap();
    auth.configure(
        "",
        "admin",
        Some(Credentials::new("admin", "test-password-1234").unwrap()),
        18787,
        String::new(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let core = Arc::new(Core {
        auth: Arc::new(Mutex::new(auth)),
        state: Weak::new(),
        public_url: RwLock::new(None),
        address,
        actions: AsyncMutex::new(super::super::actions::Ledger::default()),
        shutdown: tokio::sync::watch::channel(false).0,
        streams: Arc::new(Semaphore::new(8)),
        logins: Arc::new(Semaphore::new(2)),
        files: Arc::new(Mutex::new(super::super::files::Registry::with_protection(
            None,
            vec![directory.path().join("synthetic-codex-home")],
        ))),
        file_transfers: Arc::new(Semaphore::new(2)),
    });
    let task = tokio::spawn(serve(listener, Arc::clone(&core)));
    Server {
        core,
        base: format!("http://{address}"),
        task,
        _directory: directory,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn login_session(server: &Server, remember: bool) -> (String, Value) {
    let reply = client()
        .post(format!("{}/remote/login", server.base))
        .header("origin", &server.base)
        .json(&json!({"username":"admin","password":"test-password-1234","remember":remember}))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    let cookie = reply.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    assert_eq!(cookie.contains("Max-Age=2592000"), remember);
    assert!(!cookie.contains("Secure"));
    let session: Value = reply.json().await.unwrap();
    assert_eq!(session["authenticated"], true);
    assert_eq!(session["username"], "admin");
    assert!(session["expiresAt"].as_str().unwrap().contains('T'));
    assert!(session.get("token").is_none());
    assert!(session.get("password").is_none());
    (cookie.split(';').next().unwrap().into(), session)
}

#[tokio::test]
async fn cookie_authentication_requires_login_and_logout_requires_csrf() {
    let server = fixture().await;
    let response = client()
        .get(format!("{}/remote/session", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"authenticated":false})
    );
    let response = client()
        .post(format!("{}/remote/threads", server.base))
        .header("origin", &server.base)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let (cookie, session) = login_session(&server, true).await;
    let response = client()
        .post(format!("{}/remote/logout", server.base))
        .header("origin", &server.base)
        .header("cookie", &cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = client()
        .post(format!("{}/remote/logout", server.base))
        .header("origin", &server.base)
        .header("cookie", &cookie)
        .header("x-csrf-token", session["csrfToken"].as_str().unwrap())
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let response = client()
        .get(format!("{}/remote/session", server.base))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.json::<Value>().await.unwrap()["authenticated"],
        false
    );
}

#[tokio::test]
async fn cross_site_host_and_csrf_fail_before_dispatch() {
    let server = fixture().await;
    for (host, origin) in [
        (None, Some("https://evil.example")),
        (Some("evil.example"), None),
    ] {
        let mut request = client().post(format!("{}/remote/login", server.base));
        if let Some(host) = host {
            request = request.header("host", host);
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        assert_eq!(
            request
                .json(&json!({"username":"admin","password":"test-password-1234"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let (cookie, _) = login_session(&server, false).await;
    let response = client()
        .post(format!("{}/remote/threads", server.base))
        .header("origin", &server.base)
        .header("cookie", cookie)
        .header("x-csrf-token", "wrong")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(validate_public_url("https://example.com/path").is_err());
    assert!(validate_public_url("https://user:password@example.com").is_err());
    assert_eq!(
        validate_public_url("http://example.com").unwrap(),
        Some("http://example.com".into())
    );
}

#[tokio::test]
async fn wrong_logins_are_generic_rate_limited_and_cannot_bypass_host_with_forwarded_headers() {
    let server = fixture().await;
    for username in ["admin", "unknown"] {
        let response = client()
            .post(format!("{}/remote/login", server.base))
            .header("origin", &server.base)
            .json(&json!({"username":username,"password":"wrong"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.json::<Value>().await.unwrap()["message"],
            "账号或密码错误"
        );
    }
    for _ in 0..8 {
        assert!(
            server
                .core
                .auth
                .lock()
                .unwrap()
                .attempt("127.0.0.1".parse().unwrap())
        );
    }
    let response = client()
        .post(format!("{}/remote/login", server.base))
        .header("origin", &server.base)
        .json(&json!({"username":"admin","password":"test-password-1234"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let response = client()
        .get(format!("{}/", server.base))
        .header("host", "evil.example")
        .header("forwarded", "host=127.0.0.1;proto=https")
        .header("x-forwarded-host", "localhost")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn only_an_explicit_trusted_https_origin_sets_secure_cookies() {
    let server = fixture().await;
    *server.core.public_url.write().unwrap() = Some("https://admin.example".into());
    let response = client()
        .post(format!("{}/remote/login", server.base))
        .header("host", "admin.example")
        .header("origin", "https://admin.example")
        .json(&json!({"username":"admin","password":"test-password-1234","remember":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("; Secure")
    );
    let response = client()
        .post(format!("{}/remote/login", server.base))
        .header("origin", &server.base)
        .header("x-forwarded-proto", "https")
        .json(&json!({"username":"admin","password":"test-password-1234"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("; Secure")
    );
}

#[tokio::test]
async fn websocket_requires_cookie_origin_and_valid_upgrade_and_revocation_is_immediate() {
    let server = fixture().await;
    let id = uuid::Uuid::new_v4();
    let response = client()
        .get(format!("{}/remote/events/{id}", server.base))
        .header("origin", &server.base)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let (cookie, session) = login_session(&server, false).await;
    let response = client()
        .get(format!("{}/remote/events/{id}", server.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = client()
        .get(format!("{}/remote/events/{id}", server.base))
        .header("origin", &server.base)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let token = cookie.split_once('=').unwrap().1;
    assert!(authorize_action(&server.core, token).is_ok());
    server.core.auth.lock().unwrap().revoke("*").unwrap();
    assert!(authorize_action(&server.core, token).is_err());
    let response = client()
        .post(format!("{}/remote/threads", server.base))
        .header("origin", &server.base)
        .header("cookie", cookie)
        .header("x-csrf-token", session["csrfToken"].as_str().unwrap())
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn query_bearer_tokens_and_api_path_aliases_are_not_authorization() {
    let server = fixture().await;
    let (cookie, session) = login_session(&server, true).await;
    let token = cookie.split_once('=').unwrap().1;
    let response = client()
        .get(format!("{}/remote/session?token={token}", server.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.json::<Value>().await.unwrap()["authenticated"],
        false
    );
    let response = client()
        .post(format!("{}/api//api/runtime_status", server.base))
        .header("origin", &server.base)
        .header("cookie", cookie)
        .header("x-csrf-token", session["csrfToken"].as_str().unwrap())
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[test]
fn desktop_only_operations_and_missing_subactions_are_not_remote_capabilities() {
    for command in [
        "config_backup",
        "delete_codex_sessions",
        "save_codex_session_cleanup_policy",
        "configure_remote_control",
        "start_remote_control",
        "stop_remote_control",
        "revoke_remote_session",
        "install_codey_plugin",
        "import_official_account_credential",
        "restart_codey",
        "install_downloaded_update",
        "anything_else",
    ] {
        assert!(!panel_command(command), "{command}");
    }
    assert!(!panel_arguments_allowed("builtin_tools", &json!({})));
    assert!(!panel_command("codex_extensions"));
    assert!(panel_arguments_allowed(
        "builtin_tools",
        &json!({"action":"status","tool":"ponytail"})
    ));
    assert!(!panel_arguments_allowed(
        "builtin_tools",
        &json!({"action":"set_enabled","enabled":true})
    ));
    assert!(!panel_arguments_allowed(
        "builtin_tools",
        &json!({"action":"status","tool":"unknown"})
    ));
    assert!(!panel_arguments_allowed(
        "builtin_tools",
        &json!({"action":"status","enabled":true})
    ));
    assert!(!panel_arguments_allowed(
        "runtime_status",
        &json!({"refreshInjectionStatus":true})
    ));
}

#[test]
fn panel_summaries_drop_unknown_credentials_and_only_expose_view_fields() {
    let config = panel_summary(
        "load_codey_config",
        json!({"config":{"defaultModel":"model","profiles":[{"id":"r","name":"route","enabled":true,
        "apiKey":"key-secret","modelRequestHeaders":{"Authorization":"header-secret"},"upstreamProxy":"http://user:proxy-secret@host"}],
        "codexLaunchProxy":"proxy-secret","webhook":{"channels":[{"botToken":"bot-secret"}]},"unknownFutureCredential":"future-secret"},
        "path":"secret-path","auth":"auth-secret"}),
    );
    assert_eq!(
        config,
        json!({"config":{"defaultModel":"model","fastContextTools":null,"profiles":[{"id":"r","name":"route","enabled":true}]}})
    );
    let runtime = panel_summary(
        "runtime_status",
        json!({"running":true,"clientPlatform":"windows","codexAppVersion":"1.2.3",
        "routerToken":"token-secret","startupError":"secret-error","auth":{"token":"another-secret"}}),
    );
    assert_eq!(
        runtime,
        json!({"running":true,"fastContextToolsActive":null,"clientPlatform":"windows","codexAppVersion":"1.2.3"})
    );
    let plugins = panel_summary(
        "list_codey_plugins",
        json!({"plugins":[{"id":"p","name":"plugin","enabled":true,"config":{"password":"plugin-secret"}}]}),
    );
    assert_eq!(
        plugins,
        json!({"plugins":[{"id":"p","name":"plugin","enabled":true}]})
    );
}

#[test]
fn fastctx_summary_preserves_only_known_booleans_and_never_invents_disabled_state() {
    for value in [
        json!(true),
        json!(false),
        json!(null),
        json!("private-secret"),
    ] {
        let runtime = panel_summary("runtime_status", json!({"fastContextToolsActive":value}));
        let config = panel_summary(
            "load_codey_config",
            json!({"config":{"fastContextTools":value}}),
        );
        assert_eq!(runtime["fastContextToolsActive"], json!(value.as_bool()));
        assert_eq!(config["config"]["fastContextTools"], json!(value.as_bool()));
        assert!(!runtime.to_string().contains("private-secret"));
        assert!(!config.to_string().contains("private-secret"));
    }
}

#[tokio::test(start_paused = true)]
async fn state_coalescing_does_not_delay_urgent_changes() {
    let mut pending = None;
    schedule_state(&mut pending, false);
    let initial = pending;
    tokio::time::advance(Duration::from_millis(5)).await;
    schedule_state(&mut pending, false);
    assert_eq!(pending, initial);
    schedule_state(&mut pending, true);
    assert_eq!(pending, Some(tokio::time::Instant::now()));
}

async fn native_read(stream: &mut tokio::io::DuplexStream) -> Value {
    use tokio::io::AsyncReadExt;
    let size = stream.read_u32_le().await.unwrap() as usize;
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn native_write(stream: &mut tokio::io::DuplexStream, value: Value) {
    use tokio::io::AsyncWriteExt;
    let bytes = value.to_string().into_bytes();
    stream.write_u32_le(bytes.len() as u32).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
}

async fn synthetic_stream(
    server: &Server,
    cookie: &str,
    state: Value,
    version: super::super::projection::StreamVersion,
) -> (
    WebSocketStream<tokio::io::DuplexStream>,
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<()>,
) {
    let thread = state["id"].as_str().unwrap().to_string();
    // A single state cannot fit in the WebSocket's 1 KiB transport. Tests
    // deliberately leave it unread while sending real framed Native updates.
    let (socket, client) = tokio::io::duplex(1024);
    let (native, mut desktop) = tokio::io::duplex(2 * 1024 * 1024);
    let socket = WebSocketStream::from_raw_socket(
        socket,
        Role::Server,
        Some(
            WebSocketConfig::default()
                .max_message_size(Some(1024))
                .max_frame_size(Some(1024)),
        ),
    )
    .await;
    let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let core = server.core.clone();
    let token = cookie.split_once('=').unwrap().1.to_string();
    let task = tokio::spawn(async move {
        stream::session(
            socket,
            &core,
            &token,
            version,
            Desktop::follow_transport(native, &thread),
        )
        .await;
    });
    let initialize = native_read(&mut desktop).await;
    assert_eq!(initialize["method"], "initialize");
    native_write(&mut desktop, json!({"type":"response","requestId":initialize["requestId"],"resultType":"success","result":{"clientId":"synthetic-mobile"}})).await;
    let following = native_read(&mut desktop).await;
    assert_eq!(following["method"], "thread-stream-following-changed");
    assert_eq!(following["params"]["conversationId"], state["id"]);
    native_write(&mut desktop, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"synthetic-owner","params":{"hostId":"local","conversationId":state["id"],"change":{"type":"snapshot","revision":1,"conversationState":state}}})).await;
    (client, desktop, task)
}

async fn socket_value(socket: &mut WebSocketStream<tokio::io::DuplexStream>) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

fn stream_state(server: &Server) -> Value {
    let root = server
        ._directory
        .path()
        .canonicalize()
        .unwrap()
        .join("workspace");
    std::fs::create_dir_all(root.join("outputs")).unwrap();
    std::fs::write(
        root.join("outputs/result.bin"),
        b"synthetic public artifact",
    )
    .unwrap();
    json!({"id":uuid::Uuid::new_v4().to_string(),"cwd":root,"turns":[{"turnId":"turn","status":"inProgress","items":(0..80).map(|id|json!({"id":format!("item-{id}"),"type":"agentMessage","text":if id == 0 { "[old file](outputs/result.bin)".into() } else { "x".repeat(4096) }})).collect::<Vec<_>>()}]})
}

async fn wait_for_reference(server: &Server, token: &str, thread: &str, present: bool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let current = server
                .core
                .files
                .lock()
                .unwrap()
                .selection(token, thread, "outputs/result.bin")
                .is_ok();
            if current == present {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_slow_socket_keeps_reading_native_revokes_hidden_refs_and_delivers_the_final_delta() {
    use super::super::projection::{MessageWindow, Projection, PublicState, StreamVersion};
    let server = fixture().await;
    let (cookie, _) = login_session(&server, false).await;
    let token = cookie.split_once('=').unwrap().1;
    let mut state = stream_state(&server);
    let thread = state["id"].as_str().unwrap().to_string();
    let (mut socket, mut native, task) =
        synthetic_stream(&server, &cookie, state.clone(), StreamVersion::V2).await;
    wait_for_reference(&server, token, &thread, true).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    for byte in 0..3 {
        socket
            .send(Message::Ping(Bytes::from(vec![byte])))
            .await
            .unwrap();
    }
    let first_update = json!([
        {"op":"replace","path":"/turns/0/items/0/text","value":"old download removed"},
        {"op":"replace","path":"/turns/0/items/79/text","value":"not delivered intermediate"},
        {"op":"add","path":"/title","value":"not delivered intermediate"}
    ]);
    native_write(&mut native, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"synthetic-owner","params":{"hostId":"local","conversationId":thread,"change":{"type":"patches","baseRevision":1,"revision":2,"patches":first_update}}})).await;
    // This must complete before the phone starts consuming its first frame.
    wait_for_reference(&server, token, &thread, false).await;
    super::super::protocol::apply_patches(&mut state, &first_update).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let final_update = json!([
        {"op":"replace","path":"/turns/0/items/79/text","value":"final public record"},
        {"op":"replace","path":"/turns/0/status","value":"completed"},
        {"op":"replace","path":"/title","value":"final title"}
    ]);
    native_write(&mut native, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"synthetic-owner","params":{"hostId":"local","conversationId":thread,"change":{"type":"patches","baseRevision":2,"revision":3,"patches":final_update}}})).await;
    super::super::protocol::apply_patches(&mut state, &final_update).unwrap();
    // A real Native discovery response is an ordering barrier after the final
    // update, not an assumption based on elapsed time or a mirrored stream loop.
    native_write(
        &mut native,
        json!({"type":"client-discovery-request","requestId":"final-read-barrier"}),
    )
    .await;
    let discovery = tokio::time::timeout(Duration::from_secs(1), native_read(&mut native))
        .await
        .unwrap();
    assert_eq!(discovery["type"], "client-discovery-response");
    assert_eq!(discovery["requestId"], "final-read-barrier");
    let first = socket_value(&mut socket).await;
    assert_eq!(first["type"], "state");
    assert_eq!(first["revision"], 1);
    assert_eq!(first["state"]["turns"][0]["status"], "inProgress");
    assert_eq!(
        first["state"]["turns"][0]["messages"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
    assert!(!first.to_string().contains("outputs/result.bin"));
    let delta = socket_value(&mut socket).await;
    assert_eq!(delta["type"], "delta");
    assert_eq!(delta["baseRevision"], 1);
    assert_eq!(delta["revision"], 2);
    assert!(!delta.to_string().contains("not delivered intermediate"));
    let mut client = first["state"].clone();
    super::super::protocol::apply_patches(&mut client, &delta["patches"]).unwrap();
    let PublicState::V2(expected) = Projection::default().public_state(
        &state,
        StreamVersion::V2,
        &mut MessageWindow::default(),
    ) else {
        panic!("v2");
    };
    assert_eq!(client, expected);
    assert_eq!(
        client["turns"][0]["messages"][19]["text"],
        "final public record"
    );
    native_write(&mut native, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"synthetic-owner","params":{"hostId":"local","conversationId":thread,"change":{"type":"patches","baseRevision":3,"revision":4,"patches":[]}}})).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(150), async {
            loop {
                match socket.next().await {
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    event => return event,
                }
            }
        })
        .await
        .is_err()
    );
    socket
        .send(Message::Text(r#"{"type":"history"}"#.into()))
        .await
        .unwrap();
    let history = socket_value(&mut socket).await;
    assert_eq!(history["baseRevision"], 2);
    assert_eq!(history["revision"], 3);
    super::super::protocol::apply_patches(&mut client, &history["patches"]).unwrap();
    assert_eq!(client["turns"][0]["messages"].as_array().unwrap().len(), 70);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), native_read(&mut native))
            .await
            .is_err(),
        "local history must not call Native actions"
    );
    socket
        .send(Message::Text(
            r#"{"type":"history","action":"send"}"#.into(),
        ))
        .await
        .unwrap();
    assert_eq!(socket_value(&mut socket).await["type"], "disconnected");
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    wait_for_reference(&server, token, &thread, false).await;
}

#[tokio::test]
async fn uncertain_file_provenance_revokes_downloads_without_disconnect_of_valid_chat() {
    use super::super::projection::StreamVersion;
    for revision in [json!(1), json!("unknown-native-revision")] {
        let server = fixture().await;
        let (cookie, session) = login_session(&server, false).await;
        let token = cookie.split_once('=').unwrap().1;
        let mut state = stream_state(&server);
        state["turns"][0]["items"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        let thread = state["id"].as_str().unwrap().to_string();
        let (mut socket, mut native, task) =
            synthetic_stream(&server, &cookie, state.clone(), StreamVersion::V2).await;
        assert_eq!(socket_value(&mut socket).await["type"], "state");
        state["title"] = json!("known revision");
        native_write(&mut native, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"synthetic-owner","params":{"hostId":"local","conversationId":thread,"change":{"type":"snapshot","revision":10,"conversationState":state}}})).await;
        assert_ne!(socket_value(&mut socket).await["type"], "disconnected");
        let file = resolve_download(&server, &cookie, &session, &thread).await;
        let url = file["file"]["downloadUrl"].as_str().unwrap();
        state["title"] = json!("chat survives uncertain file authority");
        native_write(&mut native, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"synthetic-owner","params":{"hostId":"local","conversationId":thread,"change":{"type":"snapshot","revision":revision,"conversationState":state}}})).await;
        let frame = socket_value(&mut socket).await;
        assert_ne!(frame["type"], "disconnected");
        assert!(frame.to_string().contains("chat survives"));
        wait_for_reference(&server, token, &thread, false).await;
        assert_eq!(
            client()
                .get(format!("{}{url}", server.base))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        native_write(&mut native, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"synthetic-owner","params":{"hostId":"local","conversationId":thread,"change":{"type":"patches","baseRevision":revision,"revision":11,"patches":[{"op":"replace","path":"/title","value":"chat still live"}]}}})).await;
        let frame = socket_value(&mut socket).await;
        assert_ne!(frame["type"], "disconnected");
        assert!(frame.to_string().contains("chat still live"));
        socket.send(Message::Close(None)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn revocation_and_native_errors_cancel_a_blocked_write_and_drop_its_binding() {
    use super::super::projection::StreamVersion;
    for reason in ["revoke", "owner-disconnected", "stale-native-revision"] {
        let server = fixture().await;
        let (cookie, _) = login_session(&server, false).await;
        let token = cookie.split_once('=').unwrap().1;
        let state = stream_state(&server);
        let thread = state["id"].as_str().unwrap().to_string();
        let (mut socket, mut native, task) =
            synthetic_stream(&server, &cookie, state, StreamVersion::V2).await;
        wait_for_reference(&server, token, &thread, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        match reason {
            "revoke" => { server.core.auth.lock().unwrap().revoke("*").unwrap(); },
            "owner-disconnected" => native_write(&mut native, json!({"type":"broadcast","method":"client-status-changed","params":{"clientId":"synthetic-owner","status":"disconnected"}})).await,
            _ => native_write(&mut native, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"synthetic-owner","params":{"hostId":"local","conversationId":thread,"change":{"type":"patches","baseRevision":9,"revision":10,"patches":[]}}})).await,
        }
        // The network budget is at least 10 s, but authorization/Native failure
        // must cancel it without flushing the remaining stale public frame.
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        wait_for_reference(&server, token, &thread, false).await;
        let result = tokio::time::timeout(Duration::from_secs(1), socket.next())
            .await
            .unwrap();
        assert!(
            !matches!(result, Some(Ok(Message::Text(_)))),
            "stale frame completed after {reason}"
        );
    }
}

#[tokio::test]
async fn legacy_socket_keeps_full_shape_and_deduplicates_without_accepting_history_commands() {
    use super::super::{projection::StreamVersion, protocol};
    let server = fixture().await;
    let (cookie, _) = login_session(&server, false).await;
    let state = stream_state(&server);
    let thread = state["id"].clone();
    let expected = json!({"type":"state","state":protocol::view(&state)});
    let (mut socket, mut native, task) =
        synthetic_stream(&server, &cookie, state, StreamVersion::Legacy).await;
    assert_eq!(socket_value(&mut socket).await, expected);
    native_write(&mut native, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"synthetic-owner","params":{"hostId":"local","conversationId":thread,"change":{"type":"patches","baseRevision":1,"revision":2,"patches":[]}}})).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(150), socket.next())
            .await
            .is_err()
    );
    socket
        .send(Message::Text(r#"{"type":"history"}"#.into()))
        .await
        .unwrap();
    assert_eq!(socket_value(&mut socket).await["type"], "disconnected");
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}

fn bind_download_fixture(
    server: &Server,
    cookie: &str,
    bytes: &[u8],
) -> (
    String,
    super::super::files::BindingGuard,
    std::path::PathBuf,
    Value,
) {
    let root = server
        ._directory
        .path()
        .canonicalize()
        .unwrap()
        .join("workspace");
    std::fs::create_dir_all(root.join("outputs")).unwrap();
    let path = root.join("outputs/result.bin");
    std::fs::write(&path, bytes).unwrap();
    let thread = uuid::Uuid::new_v4().to_string();
    let state = json!({"id":thread,"cwd":root,"turns":[{"turnId":"turn","items":[{"id":"answer","type":"agentMessage","text":"[生成文件](outputs/result.bin)"}]}]});
    let token = cookie.split_once('=').unwrap().1;
    let binding = super::super::files::BindingGuard::new(
        server.core.files.clone(),
        token,
        &thread,
        &state,
        &super::super::desktop::NativeProvenance {
            owner: "synthetic-owner".into(),
            revision: Some(1),
        },
    )
    .unwrap();
    (thread, binding, path, state)
}

async fn resolve_download(server: &Server, cookie: &str, session: &Value, thread: &str) -> Value {
    let reply = client()
        .post(format!("{}/remote/files/resolve", server.base))
        .header("origin", &server.base)
        .header("cookie", cookie)
        .header("x-csrf-token", session["csrfToken"].as_str().unwrap())
        .json(&json!({"threadId":thread,"reference":"outputs/result.bin"}))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    reply.json().await.unwrap()
}

#[tokio::test]
async fn stale_native_window_cannot_resolve_or_get_an_observed_removed_file() {
    use super::super::{desktop::NativeProvenance, files::BindingGuard};
    let server = fixture().await;
    let (cookie, session) = login_session(&server, false).await;
    let (thread, newer, _, mut state) = bind_download_fixture(&server, &cookie, b"synthetic");
    let token = cookie.split_once('=').unwrap().1;
    let provenance = NativeProvenance {
        owner: "synthetic-owner".into(),
        revision: Some(1),
    };
    let stale_state = state.clone();
    let stale = BindingGuard::new(
        server.core.files.clone(),
        token,
        &thread,
        &state,
        &provenance,
    )
    .unwrap();
    let file = resolve_download(&server, &cookie, &session, &thread).await;
    let url = file["file"]["downloadUrl"].as_str().unwrap();
    state["turns"][0]["items"] = json!([]);
    newer
        .update(
            &state,
            &NativeProvenance {
                revision: Some(2),
                ..provenance.clone()
            },
        )
        .unwrap();
    stale.update(&stale_state, &provenance).unwrap();
    drop(newer);
    assert_eq!(
        client()
            .get(format!("{}{url}", server.base))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client()
            .post(format!("{}/remote/files/resolve", server.base))
            .header("origin", &server.base)
            .header("cookie", &cookie)
            .header("x-csrf-token", session["csrfToken"].as_str().unwrap())
            .json(&json!({"threadId":thread,"reference":"outputs/result.bin"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn file_routes_require_cookie_origin_csrf_and_an_existing_bound_projection() {
    let server = fixture().await;
    let thread = uuid::Uuid::new_v4().to_string();
    let route = format!("/api/sessions/{thread}/files/{}", "a".repeat(64));
    assert_eq!(
        client()
            .get(format!("{}{route}", server.base))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client()
            .post(format!("{}/remote/files/resolve", server.base))
            .header("origin", &server.base)
            .json(&json!({"threadId":thread,"reference":"x"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let (cookie, session) = login_session(&server, false).await;
    for (origin, csrf) in [
        (
            "https://evil.invalid",
            session["csrfToken"].as_str().unwrap(),
        ),
        (server.base.as_str(), "wrong"),
    ] {
        assert_eq!(
            client()
                .post(format!("{}/remote/files/resolve", server.base))
                .header("origin", origin)
                .header("cookie", &cookie)
                .header("x-csrf-token", csrf)
                .json(&json!({"threadId":thread,"reference":"x"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let reply = client()
        .post(format!("{}/remote/files/resolve", server.base))
        .header("origin", &server.base)
        .header("cookie", &cookie)
        .header("x-csrf-token", session["csrfToken"].as_str().unwrap())
        .json(&json!({"threadId":thread,"reference":"x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::BAD_REQUEST);
    assert!(
        reply.json::<Value>().await.unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("先打开")
    );
    assert_eq!(
        client()
            .get(format!("{}{route}", server.base))
            .header("origin", "https://evil.invalid")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client()
            .get(format!("{}{route}", server.base))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let reply = client()
        .post(format!("{}/remote/files/resolve", server.base))
        .header("origin", &server.base)
        .header("cookie", &cookie)
        .header("x-csrf-token", session["csrfToken"].as_str().unwrap())
        .json(&json!({"threadId":thread,"reference":"x","cwd":"C:/","owner":"fake"}))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::BAD_REQUEST);
    // The fixture has no AppState/desktop owner; metadata must not launch either.
    assert!(server.core.state.upgrade().is_none());
}

#[tokio::test]
async fn real_download_bytes_headers_sha_cross_thread_changes_and_revocation_are_checked() {
    let server = fixture().await;
    let (cookie, session) = login_session(&server, false).await;
    let bytes = b"synthetic artifact\0actual bytes";
    let (thread, binding, path, mut state) = bind_download_fixture(&server, &cookie, bytes);
    let resolved = resolve_download(&server, &cookie, &session, &thread).await;
    let file = &resolved["file"];
    assert_eq!(file["size"], bytes.len());
    assert_eq!(file["name"], "result.bin");
    assert!(!resolved.to_string().contains("workspace"));
    let url = file["downloadUrl"].as_str().unwrap();
    let reply = client()
        .get(format!("{}{url}", server.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(reply.headers()["content-length"], bytes.len().to_string());
    assert_eq!(reply.headers()["cache-control"], "no-store");
    assert_eq!(reply.headers()["x-content-type-options"], "nosniff");
    assert!(
        reply.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(
        reply.headers()["etag"],
        format!("\"sha256-{}\"", crate::fs_util::sha256_hex(bytes))
    );
    assert_eq!(reply.bytes().await.unwrap().as_ref(), bytes);
    let other = url.replace(&thread, &uuid::Uuid::new_v4().to_string());
    assert_eq!(
        client()
            .get(format!("{}{other}", server.base))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let (another, _) = login_session(&server, false).await;
    assert_eq!(
        client()
            .get(format!("{}{url}", server.base))
            .header("cookie", another)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client()
            .get(format!("{}{url}", server.base))
            .header("cookie", &cookie)
            .header("range", "bytes=0-1")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    std::fs::write(&path, b"changed source").unwrap();
    assert_eq!(
        client()
            .get(format!("{}{url}", server.base))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    state["turns"][0]["items"][0]["text"] = json!("[replacement](outputs/other.bin)");
    binding
        .update(
            &state,
            &super::super::desktop::NativeProvenance {
                owner: "synthetic-owner".into(),
                revision: Some(2),
            },
        )
        .unwrap();
    assert_eq!(
        client()
            .get(format!("{}{url}", server.base))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    server.core.auth.lock().unwrap().revoke("*").unwrap();
    assert_eq!(
        client()
            .get(format!("{}{url}", server.base))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(std::fs::read(path).unwrap(), b"changed source");
}

#[tokio::test]
async fn dropped_or_revoked_download_bodies_stop_producers_and_release_transfer_slots() {
    let server = fixture().await;
    let (cookie, session) = login_session(&server, false).await;
    let bytes = vec![b'x'; 1024 * 1024];
    let (thread, _binding, _, _) = bind_download_fixture(&server, &cookie, &bytes);
    let resolved = resolve_download(&server, &cookie, &session, &thread).await;
    let id = resolved["file"]["id"].as_str().unwrap().to_string();
    let token = cookie.split_once('=').unwrap().1.to_string();
    for _ in 0..3 {
        resolve_download(&server, &cookie, &session, &thread).await;
    }
    assert_eq!(server.core.file_transfers.available_permits(), 2);
    let first = downloads::download(
        server.core.clone(),
        token.clone(),
        thread.clone(),
        id.clone(),
    )
    .await;
    let second = downloads::download(
        server.core.clone(),
        token.clone(),
        thread.clone(),
        id.clone(),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let busy = downloads::download(
        server.core.clone(),
        token.clone(),
        thread.clone(),
        id.clone(),
    )
    .await;
    assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);
    drop((first, second, busy));
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.core.file_transfers.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let reply = downloads::download(
        server.core.clone(),
        token.clone(),
        thread.clone(),
        id.clone(),
    )
    .await;
    assert_eq!(reply.status(), StatusCode::OK);
    drop(reply);
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.core.file_transfers.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let reply = downloads::download(server.core.clone(), token, thread, id).await;
    assert_eq!(reply.status(), StatusCode::OK);
    let mut body = reply.into_body();
    assert!(body.frame().await.unwrap().is_ok());
    server.core.auth.lock().unwrap().revoke("*").unwrap();
    assert!(body.frame().await.unwrap().is_err());
    drop(body);
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.core.file_transfers.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
