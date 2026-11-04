use super::*;

#[tokio::test]
async fn subagent_count_save_preserves_omitted_fields_and_rejects_invalid_values_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let config = CodeyConfig {
        subagent_max_threads: 8,
        ..CodeyConfig::default()
    }
    .normalize();
    let state = Arc::new(AppState {
        config: RwLock::new(config.clone()),
        store: ConfigStore::new(temp.path().join("config.json")),
        ..AppState::default()
    });
    state.store.save(&config).unwrap();
    let mut legacy = serde_json::to_value(&config).unwrap();
    legacy.as_object_mut().unwrap().remove("subagentMaxThreads");
    save_codey_config_locked(
        &state,
        codey_config_save_input(&json!({"config":legacy})).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(state.store.load().unwrap().subagent_max_threads, 8);
    let current = state.config.read().await.clone();
    let original = std::fs::read(state.store.path()).unwrap();
    for invalid in [
        json!(0),
        json!(65),
        json!(-1),
        json!(1.5),
        json!("4"),
        json!(true),
        Value::Null,
    ] {
        let mut payload = serde_json::to_value(&current).unwrap();
        payload["subagentMaxThreads"] = invalid;
        if let Ok(input) = codey_config_save_input(&json!({"config":payload})) {
            assert!(save_codey_config_locked(&state, input).await.is_err());
        }
        assert_eq!(*state.config.read().await, current);
        assert_eq!(std::fs::read(state.store.path()).unwrap(), original);
    }
    let mut payload = serde_json::to_value(&current).unwrap();
    payload["subagentMaxThreads"] = json!(1);
    save_codey_config_locked(
        &state,
        codey_config_save_input(&json!({"config":payload})).unwrap(),
    )
    .await
    .unwrap();
    let saved = state.store.load().unwrap();
    assert_eq!(saved.subagent_max_threads, 1);
    assert_eq!(saved.settings_revision, current.settings_revision + 1);
    let mut runtime_config = current.clone();
    runtime_config.subagent_optimization = true;
    let mut desired = runtime_config.clone();
    desired.subagent_max_threads = 1;
    assert!(config_requires_restart(
        &runtime_config,
        &RuntimeModelConfig::from_config(&runtime_config),
        &RuntimeSubagentConfig::from_config(&desired),
        &desired
    ));
}

fn custom() -> SubagentRoleConfig {
    serde_json::from_value(
        json!({"name":"检查", "description":"用途", "developerInstructions":"返回证据"}),
    )
    .unwrap()
}

#[tokio::test]
async fn partial_role_saves_keep_customs_and_explicit_delete_is_transient_and_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = CodeyConfig::default();
    config
        .subagent_roles
        .insert("custom_agent_1".into(), custom());
    let config = config.normalize();
    let state = Arc::new(AppState {
        config: RwLock::new(config.clone()),
        store: ConfigStore::new(temp.path().join("config.json")),
        ..AppState::default()
    });
    state.store.save(&config).unwrap();
    let mut payload = serde_json::to_value(&config).unwrap();
    payload["subagentRoles"] = json!({"codey_worker": config.subagent_roles["codey_worker"]});
    save_codey_config_locked(
        &state,
        codey_config_save_input(&json!({"config":payload})).unwrap(),
    )
    .await
    .unwrap();
    assert!(
        state
            .config
            .read()
            .await
            .subagent_roles
            .contains_key("custom_agent_1")
    );
    for _ in 0..2 {
        let mut payload = serde_json::to_value(state.config.read().await.clone()).unwrap();
        payload["subagentRoles"]
            .as_object_mut()
            .unwrap()
            .remove("custom_agent_1");
        payload["deletedSubagentRoles"] = json!(["custom_agent_1"]);
        save_codey_config_locked(
            &state,
            codey_config_save_input(&json!({"config":payload})).unwrap(),
        )
        .await
        .unwrap();
        let saved = state.store.load().unwrap();
        assert!(!saved.subagent_roles.contains_key("custom_agent_1"));
        assert!(saved.subagent_roles.contains_key("default"));
        assert!(
            serde_json::to_value(saved)
                .unwrap()
                .get("deletedSubagentRoles")
                .is_none()
        );
        assert!(
            !std::fs::read_to_string(state.store.path())
                .unwrap()
                .contains("deletedSubagentRoles")
        );
    }
}

#[tokio::test]
async fn invalid_raw_role_and_delete_requests_leave_disk_and_memory_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let config = CodeyConfig::default().normalize();
    let state = Arc::new(AppState {
        config: RwLock::new(config.clone()),
        store: ConfigStore::new(temp.path().join("config.json")),
        ..AppState::default()
    });
    state.store.save(&config).unwrap();
    let original = std::fs::read(state.store.path()).unwrap();
    for invalid in [
        json!({"subagentRoles":{"../outside":custom()}}),
        json!({"subagentRoles":{"custom_agent_1":{"name":""}}}),
        json!({"deletedSubagentRoles":["default"]}),
        json!({"deletedSubagentRoles":["codey_worker"]}),
        json!({"deletedSubagentRoles":["../outside"]}),
        json!({"deletedSubagentRoles":"custom_agent_1"}),
    ] {
        let mut payload = serde_json::to_value(&config).unwrap();
        payload
            .as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        if let Ok(input) = codey_config_save_input(&json!({"config":payload})) {
            assert!(save_codey_config_locked(&state, input).await.is_err());
        }
        assert_eq!(*state.config.read().await, config);
        assert_eq!(std::fs::read(state.store.path()).unwrap(), original);
    }
}
