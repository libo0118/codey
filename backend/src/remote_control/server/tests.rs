use super::super::auth::{Auth, Credentials};
use super::*;
use std::sync::Mutex;

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
