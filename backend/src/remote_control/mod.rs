mod actions;
mod auth;
mod create;
mod desktop;
mod files;
mod images;
mod projection;
mod protocol;
mod server;
mod store;

use crate::commands::AppState;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, RwLock},
};
use tokio::{net::TcpListener, sync::Mutex as AsyncMutex};

type SharedAuth = Arc<Mutex<auth::Auth>>;

async fn bind_listener(address: SocketAddr) -> std::io::Result<TcpListener> {
    #[cfg(not(windows))]
    {
        TcpListener::bind(address).await
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows::Win32::Networking::WinSock::{
            SO_EXCLUSIVEADDRUSE, SOCKET, SOCKET_ERROR, SOL_SOCKET, WSAGetLastError, setsockopt,
        };

        let socket = tokio::net::TcpSocket::new_v4()?;
        let exclusive = 1_i32.to_ne_bytes();
        // Windows needs exclusivity before bind to reject overlapping interface listeners.
        // SAFETY: the socket owns the live handle and the option buffer outlives this call.
        let result = unsafe {
            setsockopt(
                SOCKET(socket.as_raw_socket() as usize),
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                Some(&exclusive),
            )
        };
        if result == SOCKET_ERROR {
            return Err(std::io::Error::from_raw_os_error(
                unsafe { WSAGetLastError() }.0,
            ));
        }
        socket.bind(address)?;
        socket.listen(128)
    }
}

#[derive(Default)]
pub(crate) struct RemoteControl {
    operation: AsyncMutex<()>,
    auth: AsyncMutex<Option<SharedAuth>>,
    running: AsyncMutex<Option<Running>>,
}

struct Running {
    address: SocketAddr,
    urls: Vec<String>,
    core: Arc<server::Core>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.core.shutdown.send_replace(true);
        self.core
            .files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.task.abort();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    username: String,
    #[serde(default)]
    password: String,
    port: u16,
    #[serde(default)]
    public_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configure {
    settings: Settings,
    revision: String,
    #[serde(default)]
    confirmed: bool,
}

impl RemoteControl {
    async fn auth(&self, state: &AppState) -> Result<SharedAuth, String> {
        let mut slot = self.auth.lock().await;
        if let Some(auth) = slot.as_ref() {
            return Ok(Arc::clone(auth));
        }
        let path = state.store.path().with_file_name("remote-control.json");
        let auth = tokio::task::spawn_blocking(move || auth::Auth::load(path))
            .await
            .map_err(|_| "读取远程账号任务失败")??;
        let auth = Arc::new(Mutex::new(auth));
        *slot = Some(Arc::clone(&auth));
        Ok(auth)
    }

    pub(crate) async fn status(&self, state: &AppState) -> Result<Value, String> {
        let auth = self.auth(state).await?;
        let mut value = {
            let auth = auth.lock().unwrap_or_else(|error| error.into_inner());
            auth.ensure_current()?;
            auth.status()
        };
        let running = self.running.lock().await;
        if let Some(running) = running
            .as_ref()
            .filter(|running| !running.task.is_finished())
        {
            value["running"] = json!(true);
            value["address"] = json!(running.address.to_string());
            value["urls"] = json!(running.urls);
        } else {
            value["running"] = json!(false);
            value["urls"] = json!([]);
        }
        Ok(value)
    }

    async fn configure(&self, state: &AppState, request: Configure) -> Result<Value, String> {
        if !request.confirmed {
            return Err("请确认远程账号设置变更".into());
        }
        if request.settings.port == 0 {
            return Err("端口须为1–65535".into());
        }
        let public_url =
            server::validate_public_url(&request.settings.public_url)?.unwrap_or_default();
        let _operation = self.operation.lock().await;
        let auth = self.auth(state).await?;
        let running = self
            .running
            .lock()
            .await
            .as_ref()
            .is_some_and(|running| !running.task.is_finished());
        {
            let current = auth.lock().unwrap_or_else(|error| error.into_inner());
            if current.revision() != request.revision {
                return Err("远程设置已更新，请刷新后重试".into());
            }
            if running
                && (current.port() != request.settings.port || current.public_url() != public_url)
            {
                return Err("请先停止远程服务再修改监听设置".into());
            }
        }
        tokio::task::spawn_blocking(move || {
            let credential = if request.settings.password.is_empty() {
                None
            } else {
                Some(auth::Credentials::new(
                    &request.settings.username,
                    &request.settings.password,
                )?)
            };
            auth.lock()
                .unwrap_or_else(|error| error.into_inner())
                .configure(
                    &request.revision,
                    &request.settings.username,
                    credential,
                    request.settings.port,
                    public_url,
                )
        })
        .await
        .map_err(|_| "保存远程账号任务失败")??;
        self.status(state).await
    }

    async fn start(&self, state: &Arc<AppState>) -> Result<Value, String> {
        let _operation = self.operation.lock().await;
        if state.is_shutting_down() {
            return Err("Codey 正在退出".into());
        }
        let mut slot = self.running.lock().await;
        if slot
            .as_ref()
            .is_some_and(|running| !running.task.is_finished())
        {
            return Err("远程服务已启动".into());
        }
        let auth = self.auth(state).await?;
        let (port, public_url) = {
            let auth = auth.lock().unwrap_or_else(|error| error.into_inner());
            if !auth.configured() {
                return Err("请先设置管理员账号和密码".into());
            }
            (auth.port(), server::validate_public_url(auth.public_url())?)
        };
        let listener = bind_listener(SocketAddr::from(([0, 0, 0, 0], port)))
            .await
            .map_err(|_| "远程服务监听失败，请检查端口占用和防火墙设置")?;
        let address = listener.local_addr().map_err(|_| "读取远程监听地址失败")?;
        let saved_auth = Arc::clone(&auth);
        tokio::task::spawn_blocking(move || {
            saved_auth
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .set_enabled(true)
        })
        .await
        .map_err(|_| "保存远程启动设置失败")??;
        let mut urls = vec![format!("http://127.0.0.1:{}", address.port())];
        // UDP connect selects a LAN route without sending a packet.
        if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
            && socket.connect("192.0.2.1:80").is_ok()
            && let Ok(local) = socket.local_addr()
            && !local.ip().is_loopback()
            && !local.ip().is_unspecified()
        {
            urls.insert(
                0,
                format!("http://{}", SocketAddr::new(local.ip(), address.port())),
            );
        }
        if let Some(public) = &public_url {
            urls.insert(0, public.clone());
        }
        let core = Arc::new(server::Core {
            auth,
            state: Arc::downgrade(state),
            public_url: RwLock::new(public_url),
            address,
            actions: AsyncMutex::new(actions::Ledger::default()),
            shutdown: tokio::sync::watch::channel(false).0,
            streams: Arc::new(tokio::sync::Semaphore::new(8)),
            logins: Arc::new(tokio::sync::Semaphore::new(2)),
            files: Arc::new(Mutex::new(files::Registry::default())),
            file_transfers: Arc::new(tokio::sync::Semaphore::new(2)),
        });
        let task = tokio::spawn(server::serve(listener, Arc::clone(&core)));
        *slot = Some(Running {
            address,
            urls,
            core,
            task,
        });
        drop(slot);
        self.status(state).await
    }

    async fn stop(&self, state: &AppState) -> Result<Value, String> {
        let _operation = self.operation.lock().await;
        let auth = self.auth(state).await?;
        tokio::task::spawn_blocking(move || {
            auth.lock()
                .unwrap_or_else(|error| error.into_inner())
                .set_enabled(false)
        })
        .await
        .map_err(|_| "保存远程停止设置失败")??;
        self.close_listener().await;
        self.status(state).await
    }

    async fn close_listener(&self) {
        if let Some(mut running) = self.running.lock().await.take() {
            running.core.shutdown.send_replace(true);
            running
                .core
                .files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            running.task.abort();
            let _ = (&mut running.task).await;
        }
    }

    // Ordinary application shutdown preserves the explicitly enabled setting.
    pub(crate) async fn shutdown(&self) {
        let _operation = self.operation.lock().await;
        self.close_listener().await;
    }

    pub(crate) async fn restore_startup(&self, state: &Arc<AppState>) -> Result<(), String> {
        let auth = self.auth(state).await?;
        let enabled = auth
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .enabled();
        if enabled {
            self.start(state).await?;
        }
        Ok(())
    }
}

pub(crate) async fn invoke(
    state: &Arc<AppState>,
    command: &str,
    args: &Value,
) -> Result<Value, String> {
    match command {
        "remote_control_status" => state.remote_control.status(state).await,
        "configure_remote_control" => {
            let request = serde_json::from_value(args.clone()).map_err(|_| "远程设置字段无效")?;
            state.remote_control.configure(state, request).await
        }
        "start_remote_control" => state.remote_control.start(state).await,
        "stop_remote_control" => state.remote_control.stop(state).await,
        "revoke_remote_session" => {
            let id = args["id"].as_str().ok_or("缺少登录会话标识")?.to_string();
            if id != "*" && uuid::Uuid::parse_str(&id).is_err() {
                return Err("登录会话标识无效".into());
            }
            let auth = state.remote_control.auth(state).await?;
            tokio::task::spawn_blocking(move || {
                auth.lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .revoke(&id)
            })
            .await
            .map_err(|_| "撤销登录任务失败")??;
            state.remote_control.status(state).await
        }
        _ => Err("未知远程管理操作".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: std::path::PathBuf) -> Arc<AppState> {
        let mut state = AppState::default();
        state.store = crate::config::ConfigStore::new(path);
        state.config = tokio::sync::RwLock::new(crate::config::CodeyConfig::default());
        Arc::new(state)
    }

    #[tokio::test]
    async fn service_is_disabled_and_unconfigured_by_default() {
        let temp = tempfile::tempdir().unwrap();
        let state = fixture(temp.path().join("config.json"));
        let status = invoke(&state, "remote_control_status", &json!({}))
            .await
            .unwrap();
        assert_eq!(status["port"], 18787);
        assert_eq!(status["running"], false);
        assert_eq!(status["accountConfigured"], false);
        assert!(
            invoke(&state, "start_remote_control", &json!({}))
                .await
                .is_err()
        );
        assert!(!temp.path().join("remote-control.json").exists());
    }

    #[tokio::test]
    async fn setup_requires_confirmation_password_and_valid_listener() {
        let temp = tempfile::tempdir().unwrap();
        let state = fixture(temp.path().join("config.json"));
        for settings in [
            json!({"username":"admin","password":"short","port":18787}),
            json!({"username":"admin","password":"test-password-1234","port":0}),
            json!({"username":"admin","password":"test-password-1234","port":18787,"publicUrl":"https://host.invalid/path"}),
        ] {
            assert!(
                invoke(
                    &state,
                    "configure_remote_control",
                    &json!({"settings":settings,"revision":"","confirmed":true})
                )
                .await
                .is_err()
            );
        }
        assert!(invoke(&state,"configure_remote_control",&json!({"settings":{"username":"admin","password":"test-password-1234","port":18787},"revision":""})).await.is_err());
        assert!(!temp.path().join("remote-control.json").exists());
    }

    #[tokio::test]
    async fn occupied_custom_port_does_not_enable_or_replace_the_listener() {
        for host in [[127, 0, 0, 1], [0, 0, 0, 0]] {
            let temp = tempfile::tempdir().unwrap();
            let state = fixture(temp.path().join("config.json"));
            let control = &state.remote_control;
            let occupied = TcpListener::bind(SocketAddr::from((host, 0)))
                .await
                .unwrap();
            let port = occupied.local_addr().unwrap().port();
            invoke(
                &state,
                "configure_remote_control",
                &json!({
                    "settings":{"username":"admin","password":"test-password-1234","port":port},
                    "revision":"","confirmed":true,
                }),
            )
            .await
            .unwrap();
            let auth = control.auth(&state).await.unwrap();
            for enabled in [false, true] {
                auth.lock().unwrap().set_enabled(enabled).unwrap();
                let saved = std::fs::read(temp.path().join("remote-control.json")).unwrap();
                assert!(
                    invoke(&state, "start_remote_control", &json!({}))
                        .await
                        .is_err()
                );
                assert_eq!(control.status(&state).await.unwrap()["running"], false);
                assert!(control.running.lock().await.is_none());
                assert_eq!(auth.lock().unwrap().enabled(), enabled);
                assert_eq!(
                    std::fs::read(temp.path().join("remote-control.json")).unwrap(),
                    saved
                );
                assert_eq!(occupied.local_addr().unwrap().port(), port);
            }
            drop(occupied);
            control.restore_startup(&state).await.unwrap();
            let started = control.status(&state).await.unwrap();
            assert_eq!(started["running"], true);
            assert_eq!(started["address"], format!("0.0.0.0:{port}"));
            let core = Arc::clone(&control.running.lock().await.as_ref().unwrap().core);
            assert!(
                invoke(&state, "start_remote_control", &json!({}))
                    .await
                    .is_err()
            );
            assert!(Arc::ptr_eq(
                &core,
                &control.running.lock().await.as_ref().unwrap().core,
            ));
            assert_eq!(control.status(&state).await.unwrap(), started);
            assert!(auth.lock().unwrap().enabled());
            invoke(&state, "stop_remote_control", &json!({}))
                .await
                .unwrap();
            assert!(!auth.lock().unwrap().enabled());
            let released = TcpListener::bind(SocketAddr::from((host, port)))
                .await
                .unwrap();
            drop(released);
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_listener_rejects_reuse_address_takeover() {
        let listener = bind_listener(SocketAddr::from(([0, 0, 0, 0], 0)))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        for host in [[127, 0, 0, 1], [0, 0, 0, 0]] {
            let takeover = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )
            .unwrap();
            takeover.set_reuse_address(true).unwrap();
            assert!(
                takeover
                    .bind(&SocketAddr::from((host, port)).into())
                    .is_err()
            );
        }
        drop(listener);
        bind_listener(SocketAddr::from(([0, 0, 0, 0], port)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ordinary_shutdown_preserves_startup_preference_but_explicit_stop_disables_it() {
        let temp = tempfile::tempdir().unwrap();
        let state = fixture(temp.path().join("config.json"));
        let available = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = available.local_addr().unwrap().port();
        drop(available);
        invoke(
            &state,
            "configure_remote_control",
            &json!({
                "settings":{"username":"admin","password":"test-password-1234","port":port},
                "revision":"","confirmed":true,
            }),
        )
        .await
        .unwrap();
        assert!(
            invoke(&state, "start_remote_control", &json!({}))
                .await
                .unwrap()["running"]
                .as_bool()
                .unwrap()
        );
        state.remote_control.shutdown().await;
        assert!(
            auth::Auth::load(temp.path().join("remote-control.json"))
                .unwrap()
                .enabled()
        );
        let released = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
            .await
            .unwrap();
        drop(released);
        state.remote_control.restore_startup(&state).await.unwrap();
        assert_eq!(
            state.remote_control.status(&state).await.unwrap()["running"],
            true
        );
        invoke(&state, "stop_remote_control", &json!({}))
            .await
            .unwrap();
        assert!(
            !auth::Auth::load(temp.path().join("remote-control.json"))
                .unwrap()
                .enabled()
        );
        state.remote_control.restore_startup(&state).await.unwrap();
        assert_eq!(
            state.remote_control.status(&state).await.unwrap()["running"],
            false
        );
    }
}
