// Adapted from fraternity-z/CodeyX (AGPL-3.0-only).
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{AppState, finish_codey_config_save, persist_codey_config_locked, string_argument};
use crate::backup::{self, webdav};

static OPERATION: Mutex<()> = Mutex::const_new(());

fn require_confirmation(args: &Value) -> Result<(), String> {
    if args.get("confirmed").and_then(Value::as_bool) != Some(true) {
        return Err("请先确认备份包含线路密钥，恢复操作会替换当前配置".into());
    }
    Ok(())
}

pub(super) async fn invoke(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let _operation = OPERATION
        .try_lock()
        .map_err(|_| "备份操作正在进行，请稍后重试")?;
    let action = string_argument(args, "action")?;
    if matches!(action.as_str(), "export" | "restore" | "webdav_upload") {
        require_confirmation(args)?;
    }
    match action.as_str() {
        "export" => Ok(json!({
            "content": backup::export(state.config.read().await.clone())?,
            "containsSecrets": true,
        })),
        "inspect" => {
            let backup = backup::parse(&string_argument(args, "content")?)?;
            let previous = state.config.read().await.clone();
            let summary = backup.summary(previous.settings_revision);
            backup.prepare(&previous)?;
            Ok(summary)
        }
        "restore" => {
            let backup = backup::parse(&string_argument(args, "content")?)?;
            let revision = args
                .get("settingsRevision")
                .and_then(Value::as_u64)
                .ok_or("缺少配置版本")?;
            let (saved, recovery_path) = restore_config(state, backup, revision).await?;
            let public_config = super::redacted_config(&saved.config);
            let config_apply = saved.config_apply.clone();
            // 配置已持久化，之后的运行时查询失败不能被误报为“尚未恢复”。
            let mut result = finish_codey_config_save(state, saved)
                .await
                .unwrap_or_else(|_| {
                    json!({
                        "status": "ok", "config": public_config, "configApply": config_apply,
                        "restartRequired": true,
                        "applyWarning": "配置已恢复，但运行时状态查询失败，请刷新或重启后检查",
                    })
                });
            result["restored"] = json!(true);
            result["recoveryPath"] = json!(recovery_path.to_string_lossy());
            Ok(result)
        }
        "webdav_get" | "webdav_save" => {
            let path = state.store.path().with_file_name("backup-webdav.json");
            let settings = if action == "webdav_save" {
                Some(
                    serde_json::from_value::<webdav::Settings>(
                        args.get("settings").cloned().ok_or("缺少 WebDAV 设置")?,
                    )
                    .map_err(|_| "WebDAV 设置字段无效")?,
                )
            } else {
                None
            };
            tokio::task::spawn_blocking(move || match settings {
                Some(settings) => webdav::save(&path, settings),
                None => webdav::load(&path).map(|settings| settings.public()),
            })
            .await
            .map_err(|_| "读取或保存 WebDAV 设置任务失败")?
        }
        "webdav_upload" | "webdav_download" => {
            let path = state.store.path().with_file_name("backup-webdav.json");
            let settings = tokio::task::spawn_blocking(move || webdav::load(&path))
                .await
                .map_err(|_| "读取 WebDAV 设置任务失败")??;
            let content = if action == "webdav_upload" {
                Some(backup::export(state.config.read().await.clone())?)
            } else {
                None
            };
            let content = webdav::transfer(&settings, content).await?;
            Ok(json!({"status": "ok", "content": content}))
        }
        _ => Err("未知备份操作".into()),
    }
}

async fn restore_config(
    state: &Arc<AppState>,
    backup: backup::Backup,
    revision: u64,
) -> Result<(super::SavedCodeyConfig, std::path::PathBuf), String> {
    let _guard = state.config_write_lock.lock().await;
    let previous = state.config.read().await.clone();
    if revision != previous.settings_revision {
        return Err("配置已被其他操作更新，请重新预览备份后再恢复".into());
    }
    let config = backup.prepare(&previous)?;
    super::validate_official_account_config_change(&previous, &config)
        .map_err(|_| "备份配置不符合本机官方账号边界")?;
    super::ensure_local_route_config_change_allowed(&previous, &config)
        .map_err(|_| "本地路由关闭时不能恢复线路配置，请先在本机启用本地路由")?;
    let path = state.store.path().to_path_buf();
    let snapshot = previous.clone();
    let recovery_path = tokio::task::spawn_blocking(move || backup::save_recovery(&path, snapshot))
        .await
        .map_err(|_| "保存恢复前备份任务失败，原配置未更改")??;
    let mut saved = persist_codey_config_locked(
        state,
        &previous,
        config,
        super::current_fast_context_tools_status(),
    )
    .await
    .map_err(|_| {
        format!(
            "恢复未完成，内存未更新；磁盘主文件可能已轮转，请用恢复前快照核对/恢复：{}",
            recovery_path.display()
        )
    })?;
    saved.config_apply = super::config_apply::enqueue(state, saved.reconcile_subagent_config);
    Ok((saved, recovery_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CodeyConfig, ConfigStore};

    fn state(path: std::path::PathBuf) -> Arc<AppState> {
        Arc::new(AppState {
            store: ConfigStore::new(path),
            config: tokio::sync::RwLock::new(CodeyConfig::default()),
            runtime_handle: None,
            ..AppState::default()
        })
    }
    fn incoming() -> backup::Backup {
        let config = CodeyConfig {
            stream_max_retries: 3,
            ..CodeyConfig::default()
        };
        backup::parse(&backup::export(config).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn sensitive_actions_require_confirmation_before_reading_or_writing_backups() {
        let temp = tempfile::tempdir().unwrap();
        let state = state(temp.path().join("config.json"));
        for action in ["export", "restore", "webdav_upload"] {
            let error = invoke(
                &state,
                &json!({"action":action,"content":"private-content"}),
            )
            .await
            .unwrap_err();
            assert!(error.contains("请先确认"));
            assert!(!error.contains("private-content"));
        }
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn malformed_webdav_settings_do_not_echo_credentials_or_touch_storage() {
        let temp = tempfile::tempdir().unwrap();
        let state = state(temp.path().join("config.json"));
        let error = invoke(
            &state,
            &json!({
                "action":"webdav_save", "settings":{"url":42,"password":"private-content"},
            }),
        )
        .await
        .unwrap_err();
        assert!(!error.contains("private-content"));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn backup_restore_checks_revision_then_snapshots_and_commits() {
        let temp = tempfile::tempdir().unwrap();
        let state = state(temp.path().join("config.json"));
        let previous = state.config.read().await.clone();
        assert!(
            restore_config(&state, incoming(), previous.settings_revision + 1)
                .await
                .is_err()
        );
        assert!(!state.store.path().exists());
        let (saved, path) = restore_config(&state, incoming(), previous.settings_revision)
            .await
            .unwrap();
        let snapshot: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            snapshot["config"]["streamMaxRetries"],
            previous.stream_max_retries
        );
        assert_eq!(saved.config.stream_max_retries, 3);
        assert_eq!(
            saved.config.settings_revision,
            previous.settings_revision + 1
        );
        assert_eq!(state.store.load().unwrap(), *state.config.read().await);
    }

    #[tokio::test]
    async fn backup_restore_commits_complete_model_preferences_to_memory_and_disk() {
        let temp = tempfile::tempdir().unwrap();
        let state = state(temp.path().join("config.json"));
        let previous = state.config.read().await.clone();
        state.store.save(&previous).unwrap();
        let mut incoming = previous.clone();
        let profile = &mut incoming.profiles[0];
        profile.name = "Backup route".into();
        profile.short_name = "Bkp".into();
        profile.base_url = "https://example.com/v1".into();
        profile.api_key = "backup-secret".into();
        let provider = profile.provider_id().to_owned();
        incoming
            .selected_models_by_provider
            .insert(provider.clone(), vec!["backup-model".into()]);
        incoming.manual_third_party_models_by_provider.insert(
            provider.clone(),
            vec!["backup-model".into(), "manual-model".into()],
        );
        incoming
            .upstream_models_by_provider
            .insert(provider.clone(), vec!["backup-model".into()]);
        incoming.default_model = format!("{provider}/backup-model");
        incoming.model_reasoning_efforts_by_provider.insert(
            provider.clone(),
            std::collections::BTreeMap::from([(
                "backup-model".into(),
                vec![crate::config::ModelReasoningEffort {
                    level: "high".into(),
                    value: "backup-high".into(),
                }],
            )]),
        );
        incoming
            .upstream_model_reasoning_efforts_by_provider
            .insert(
                provider.clone(),
                std::collections::BTreeMap::from([(
                    "backup-model".into(),
                    vec![crate::config::ModelReasoningEffort {
                        level: "medium".into(),
                        value: "upstream-medium".into(),
                    }],
                )]),
            );
        incoming.model_context_by_provider.insert(
            provider,
            std::collections::BTreeMap::from([(
                "backup-model".into(),
                crate::config::ModelContextConfig {
                    context_window_tokens: 128_000,
                    auto_compact_token_limit: Some(96_000),
                    reserve_output_tokens: Some(16_000),
                },
            )]),
        );
        crate::config::validate_provider_profiles(&incoming.profiles).unwrap();
        let mut expected = incoming.clone().normalize();
        let content = backup::export(incoming).unwrap();
        assert_eq!(
            backup::parse(&content).unwrap().prepare(&previous).unwrap(),
            expected
        );
        assert_ne!(expected.default_model, previous.default_model);
        assert_ne!(
            expected.selected_models_by_provider,
            previous.selected_models_by_provider
        );
        assert_ne!(
            expected.manual_third_party_models_by_provider,
            previous.manual_third_party_models_by_provider
        );
        assert_ne!(
            expected.model_reasoning_efforts_by_provider,
            previous.model_reasoning_efforts_by_provider
        );
        assert_ne!(
            expected.upstream_model_reasoning_efforts_by_provider,
            previous.upstream_model_reasoning_efforts_by_provider
        );
        assert_ne!(
            expected.model_reasoning_efforts_by_provider,
            expected.upstream_model_reasoning_efforts_by_provider
        );
        assert_ne!(
            expected.model_context_by_provider,
            previous.model_context_by_provider
        );
        let (saved, _) = restore_config(
            &state,
            backup::parse(&content).unwrap(),
            previous.settings_revision,
        )
        .await
        .unwrap();
        expected.settings_revision += 1;
        assert_eq!(saved.config, expected);
        assert_eq!(*state.config.read().await, expected);
        assert_eq!(state.store.load().unwrap(), expected);
        let disk: CodeyConfig =
            serde_json::from_slice(&std::fs::read(state.store.path()).unwrap()).unwrap();
        assert_eq!(disk, expected);

        // 普通设置表单仍是部分更新，不能借恢复修复改写模型管理入口的语义。
        let mut form = expected.clone();
        form.default_model.clear();
        form.selected_models_by_provider.clear();
        form.manual_third_party_models_by_provider.clear();
        form.stream_max_retries = 2;
        let _guard = state.config_write_lock.lock().await;
        let saved = super::super::save_codey_config_locked(
            &state,
            super::super::CodeyConfigSaveInput::complete(form),
        )
        .await
        .unwrap();
        assert_eq!(saved.config.default_model, expected.default_model);
        assert_eq!(
            saved.config.selected_models_by_provider,
            expected.selected_models_by_provider
        );
        assert_eq!(
            saved.config.manual_third_party_models_by_provider,
            expected.manual_third_party_models_by_provider
        );
        assert_eq!(saved.config.stream_max_retries, 2);
    }

    #[tokio::test]
    async fn backup_restore_failure_does_not_publish_config() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, "keep").unwrap();
        for path in [
            blocker.join("config.json"),
            temp.path().join("directory.json"),
        ] {
            if path.ends_with("directory.json") {
                std::fs::create_dir(&path).unwrap();
            }
            let state = state(path);
            let previous = state.config.read().await.clone();
            assert!(
                restore_config(&state, incoming(), previous.settings_revision)
                    .await
                    .is_err()
            );
            assert_eq!(*state.config.read().await, previous);
        }
        assert_eq!(std::fs::read_to_string(blocker).unwrap(), "keep");
    }

    #[tokio::test]
    async fn failed_restore_commit_preserves_recoverable_disk_config_and_memory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::create_dir(&path).unwrap();
        let state = state(path.clone());
        let mut previous = state.config.read().await.clone().normalize();
        previous.profiles[0].base_url = "https://api.example.com/v1".into();
        previous.profiles[0].api_key = "recovery-snapshot-secret".into();
        crate::config::validate_provider_profiles(&previous.profiles).unwrap();
        *state.config.write().await = previous;
        let previous_memory = state.config.read().await.clone();
        let backup_path = path.with_file_name("config.json.bak.1");
        ConfigStore::new(&backup_path)
            .save(&previous_memory)
            .unwrap();
        let bytes = std::fs::read(&backup_path).unwrap();
        let previous_disk = state.store.load().unwrap();
        assert_eq!(previous_disk, previous_memory.clone().normalize());
        assert_ne!(previous_disk, previous_memory);
        let error = restore_config(&state, incoming(), previous_memory.settings_revision)
            .await
            .err()
            .unwrap();
        assert!(error.contains("恢复未完成，内存未更新"));
        assert!(error.contains("磁盘主文件可能已轮转"));
        assert!(error.contains("请用恢复前快照核对/恢复"));
        assert!(!error.contains("原配置未更改"));
        assert!(!error.contains("recovery-snapshot-secret"));
        assert_eq!(*state.config.read().await, previous_memory);
        assert_eq!(state.store.load().unwrap(), previous_disk);
        assert_eq!(std::fs::read(backup_path).unwrap(), bytes);
        assert!(path.is_dir());
        let snapshots = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("before-restore-")
            })
            .collect::<Vec<_>>();
        assert_eq!(snapshots.len(), 1);
        assert!(error.contains(snapshots[0].to_string_lossy().as_ref()));
        let snapshot_content = std::fs::read_to_string(&snapshots[0]).unwrap();
        let snapshot: Value = serde_json::from_str(&snapshot_content).unwrap();
        let snapshot_config: CodeyConfig =
            serde_json::from_value(snapshot["config"].clone()).unwrap();
        assert_eq!(snapshot_config, previous_memory);
        let snapshot = backup::parse(&snapshot_content)
            .unwrap()
            .prepare(&previous_memory)
            .unwrap();
        assert_eq!(snapshot, previous_disk);
    }
}
