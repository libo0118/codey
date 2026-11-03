// Adapted from fraternity-z/CodeyX (AGPL-3.0-only).
use super::{AppState, argument};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Weak, atomic::Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

pub(super) struct SessionCleanupWatcher {
    shutdown: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

pub(super) async fn start_cleanup_watcher(state: &Arc<AppState>) {
    start_cleanup_watcher_at(
        state,
        crate::codex_config::codex_home().to_path_buf(),
        CLEANUP_INTERVAL,
    )
    .await;
}

async fn start_cleanup_watcher_at(state: &Arc<AppState>, home: PathBuf, interval: Duration) {
    let mut watcher = state.session_cleanup_watcher.lock().await;
    if state.is_shutting_down()
        || watcher
            .as_ref()
            .is_some_and(|watcher| !watcher.task.is_finished())
    {
        return;
    }
    let (shutdown, shutdown_rx) = oneshot::channel();
    *watcher = Some(SessionCleanupWatcher {
        shutdown,
        task: tokio::spawn(watch_cleanup(
            Arc::downgrade(state),
            home,
            shutdown_rx,
            interval,
        )),
    });
}

pub(super) async fn stop_cleanup_watcher(state: &AppState) {
    let mut watcher = state.session_cleanup_watcher.lock().await;
    if let Some(SessionCleanupWatcher { shutdown, task }) = watcher.take() {
        let _ = shutdown.send(());
        if task.await.is_err() {
            crate::error_log::record_failure(
                "cleanup_failed",
                "stop_session_cleanup_watcher",
                "会话自动清理任务异常退出".to_string(),
                serde_json::json!({}),
            );
        }
    }
}

async fn watch_cleanup(
    state: Weak<AppState>,
    home: PathBuf,
    mut shutdown: oneshot::Receiver<()>,
    check_interval: Duration,
) {
    let mut interval = tokio::time::interval(check_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            _ = interval.tick() => {}
        }
        let Some(state) = state.upgrade() else { break };
        let _operation = tokio::select! {
            biased;
            _ = &mut shutdown => break,
            operation = state.runtime_operation.lock() => operation,
        };
        if state.is_shutting_down() {
            break;
        }
        let home = home.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::session_management::automatic_cleanup(
                &home,
                chrono::Utc::now().timestamp_millis(),
            )
        })
        .await;
        let failed = match result {
            Ok(Ok(Some(result))) => !result.errors.is_empty(),
            Ok(Ok(None)) => false,
            _ => true,
        };
        if failed {
            crate::error_log::record_failure_async(
                "cleanup_failed",
                "automatic_session_cleanup",
                "会话自动清理未完成，请在会话管理中检查设置和待清理数据".to_string(),
                serde_json::json!({}),
            )
            .await;
        }
    }
}

pub(super) async fn invoke(
    state: &Arc<AppState>,
    command: &str,
    args: &Value,
) -> Result<Value, String> {
    // 与重启串行，避免启动维护和实时删除同时操作同一批文件。
    let _operation = state.runtime_operation.lock().await;
    if state.is_shutting_down() || state.restart_in_progress.load(Ordering::Acquire) {
        return Err("Codey 正在退出或重启，请稍后重新操作".into());
    }
    enum Request {
        List,
        Delete(crate::session_management::DeleteRequest),
        SavePolicy(crate::session_management::SavePolicyRequest),
    }
    let request = match command {
        "list_codex_sessions" => Request::List,
        "delete_codex_sessions" => {
            Request::Delete(argument(args, "request").map_err(|_| "会话清理参数无效")?)
        }
        "save_codex_session_cleanup_policy" => {
            Request::SavePolicy(argument(args, "request").map_err(|_| "自动清理策略参数无效")?)
        }
        _ => return Err("未知会话管理命令".into()),
    };
    tokio::task::spawn_blocking(move || {
        let home = crate::codex_config::codex_home();
        let result = match request {
            Request::Delete(request) => {
                serde_json::to_value(crate::session_management::remove(home, request)?)
            }
            Request::List => serde_json::to_value(crate::session_management::list(home)?),
            Request::SavePolicy(request) => {
                serde_json::to_value(crate::session_management::save_policy(
                    home,
                    request,
                    chrono::Utc::now().timestamp_millis(),
                )?)
            }
        };
        result.map_err(anyhow::Error::from)
    })
    .await
    .map_err(|_| "会话管理任务异常退出，请刷新确认清理结果".to_string())?
    .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(directory: &std::path::Path) -> Arc<AppState> {
        Arc::new(AppState {
            store: crate::config::ConfigStore::new(directory.join("config.json")),
            config: tokio::sync::RwLock::new(crate::config::CodeyConfig::default()),
            config_load_error: None,
            ..AppState::default()
        })
    }

    #[tokio::test]
    async fn cleanup_watcher_reloads_policy_on_periodic_ticks() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        let state = fixture(home);
        let folder = home.join("archived_sessions");
        std::fs::create_dir(&folder).unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        let rollout = folder.join(format!("rollout-test-{id}.jsonl"));
        std::fs::write(
            &rollout,
            serde_json::json!({"type":"session_meta","payload":{"id":id}}).to_string(),
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&rollout)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now() - Duration::from_secs(40 * 86400)),
            )
            .unwrap();
        start_cleanup_watcher_at(&state, home.to_path_buf(), Duration::from_millis(10)).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(rollout.exists());
        std::fs::write(
            home.join(".codey-session-retention-v1.json"),
            r#"{"enabled":true,"retentionDays":30}"#,
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !home.join(".codey-session-cleanup-v1.json").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        stop_cleanup_watcher(&state).await;
        assert!(home.join(".codey-session-cleanup-v1.json").exists());
        assert!(rollout.exists(), "运行中的自动清理只安排任务");
        crate::session_management::finish_pending(home).unwrap();
        let inventory =
            serde_json::to_value(crate::session_management::list(home).unwrap()).unwrap();
        assert!(inventory["sessions"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_watcher_cancels_while_waiting_for_runtime_and_does_not_duplicate() {
        let directory = tempfile::tempdir().unwrap();
        let state = fixture(directory.path());
        let missing = directory.path().join("missing");
        let operation = state.runtime_operation.lock().await;
        start_cleanup_watcher_at(&state, missing.clone(), Duration::from_millis(5)).await;
        let first = state
            .session_cleanup_watcher
            .lock()
            .await
            .as_ref()
            .unwrap()
            .task
            .id();
        start_cleanup_watcher_at(&state, missing.clone(), Duration::from_millis(5)).await;
        assert_eq!(
            state
                .session_cleanup_watcher
                .lock()
                .await
                .as_ref()
                .unwrap()
                .task
                .id(),
            first
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        tokio::time::timeout(Duration::from_secs(1), stop_cleanup_watcher(&state))
            .await
            .unwrap();
        assert!(state.session_cleanup_watcher.lock().await.is_none());
        assert!(!missing.exists());
        drop(operation);

        start_cleanup_watcher_at(&state, missing.clone(), Duration::from_millis(5)).await;
        let restarted = state
            .session_cleanup_watcher
            .lock()
            .await
            .as_ref()
            .unwrap()
            .task
            .id();
        assert_ne!(first, restarted);
        super::super::runtime::begin_shutdown(&state).await;
        assert!(state.session_cleanup_watcher.lock().await.is_none());
        start_cleanup_watcher_at(&state, missing.clone(), Duration::from_millis(5)).await;
        assert!(state.session_cleanup_watcher.lock().await.is_none());
        assert!(!missing.exists());
    }

    #[tokio::test]
    async fn cleanup_policy_command_rejects_malformed_requests_before_io() {
        let directory = tempfile::tempdir().unwrap();
        let state = fixture(directory.path());
        for request in [
            serde_json::json!(null),
            serde_json::json!({"policy":{"enabled":true,"retentionDays":-1},"revision":"test","confirmed":true}),
            serde_json::json!({"policy":{"enabled":true,"retentionDays":1.5},"revision":"test","confirmed":true}),
            serde_json::json!({"policy":{"enabled":true,"retentionDays":30},"revision":"test"}),
        ] {
            assert!(
                invoke(
                    &state,
                    "save_codex_session_cleanup_policy",
                    &serde_json::json!({"request":request})
                )
                .await
                .is_err()
            );
        }
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
