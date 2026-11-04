use super::*;

fn custom() -> SubagentRoleConfig {
    serde_json::from_value(serde_json::json!({
        "name":"检查", "description":"只读检查", "developerInstructions":"检查并返回证据。",
        "model":"gpt-5.6-terra", "reasoningEffort":"ultra"
    }))
    .unwrap()
}

fn start(home: &Path, marker: &Path, config: &CodeyConfig) -> Result<AppliedRuntimeRouterConfig> {
    apply_isolated_runtime_router_config(
        home,
        RouterApplyOptions {
            local_router: None,
            remote_compaction_models: None,
            stream_max_retries: 5,
            use_official_catalog: false,
            model_contexts: None,
            required_catalog_models: &[],
            default_model: None,
            fastctx_command: None,
            subagent_optimization: true,
            subagent_model: &config.subagent_model,
            subagent_reasoning_effort: &config.subagent_reasoning_effort,
            subagent_roles: Some(&config.subagent_roles),
            scope_guard: false,
            marker,
            backup_root: &marker.with_file_name("backups"),
        },
    )
}

fn config() -> CodeyConfig {
    let mut config = CodeyConfig {
        subagent_optimization: true,
        ..CodeyConfig::default()
    };
    config
        .subagent_roles
        .insert("custom_agent_1".into(), custom());
    config.normalize()
}

#[test]
fn custom_runtime_files_preserve_native_config_and_read_only_does_not_grant_workspace_write() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let original = "model = 'native-model'\n[agents.native_role]\nconfig_file = 'native.toml'\n";
    fs::write(home.join("config.toml"), original).unwrap();
    let marker = temp.path().join("state/lease.json");
    let config = config();
    let applied = start(&home, &marker, &config).unwrap();
    assert!(
        applied
            .runtime_config_overrides
            .iter()
            .any(|arg| arg.starts_with("agents.custom_agent_1.config_file="))
    );
    let path = runtime_agent_path(
        &marker.with_file_name(CODEY_CONSTRAINTS_DIR),
        "custom_agent_1",
    );
    let generated = parse_document(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(generated["sandbox_mode"].as_str(), Some("read-only"));
    assert_eq!(generated["model_reasoning_effort"].as_str(), Some("ultra"));
    assert!(
        generated["developer_instructions"]
            .as_str()
            .unwrap()
            .contains("检查并返回证据")
    );
    assert_eq!(
        fs::read_to_string(home.join("config.toml")).unwrap(),
        original
    );
    assert!(restore_runtime_config_at(&home, &marker, false).unwrap());
    assert_eq!(
        fs::read_to_string(home.join("config.toml")).unwrap(),
        original
    );
}

#[test]
fn hot_delete_disables_registered_custom_and_new_role_waits_without_delaying_other_updates() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let marker = temp.path().join("state/lease.json");
    let mut config = config();
    start(&home, &marker, &config).unwrap();
    let constraints = marker.with_file_name(CODEY_CONSTRAINTS_DIR);
    let orphan = constraints
        .join(CODEY_RUNTIME_AGENTS_DIR)
        .join("native_external.toml");
    fs::write(&orphan, "external native file").unwrap();
    config.subagent_roles.remove("custom_agent_1");
    config
        .subagent_roles
        .get_mut("codey_worker")
        .unwrap()
        .reasoning_effort = "max".into();
    config
        .subagent_roles
        .insert("custom_agent_2".into(), custom());
    let error = reconcile_runtime_subagent_roles_at(&config, &marker).unwrap_err();
    assert!(error.is::<SubagentRoleRegistrationChanged>());
    let policy = crate::subagent_gate::load_runtime_subagent_policy(
        &home.join(crate::subagent_gate::STATE_DIRECTORY),
    )
    .unwrap()
    .unwrap();
    assert!(!policy.roles.contains_key("custom_agent_1"));
    assert!(!policy.roles.contains_key("custom_agent_2"));
    assert_eq!(policy.roles["codey_worker"].reasoning_effort, "max");
    let lease: RuntimeConfigLease = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    assert!(
        lease
            .registered_subagent_roles
            .contains(&"custom_agent_1".to_string())
    );
    let placeholder = parse_document(
        &fs::read_to_string(runtime_agent_path(&constraints, "custom_agent_1")).unwrap(),
    )
    .unwrap();
    assert_eq!(placeholder["sandbox_mode"].as_str(), Some("read-only"));
    assert!(!runtime_agent_path(&constraints, "custom_agent_2").exists());
    assert_eq!(fs::read_to_string(orphan).unwrap(), "external native file");
}

#[test]
fn custom_role_transaction_restores_files_policy_and_lease_when_final_commit_fails() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let marker = temp.path().join("state/lease.json");
    let mut config = config();
    start(&home, &marker, &config).unwrap();
    let role = runtime_agent_path(
        &marker.with_file_name(CODEY_CONSTRAINTS_DIR),
        "custom_agent_1",
    );
    let (policy, pending) = crate::subagent_gate::runtime_subagent_policy_paths(&home);
    let before = [
        fs::read(&role).unwrap(),
        fs::read(&policy).unwrap(),
        fs::read(&marker).unwrap(),
    ];
    config
        .subagent_roles
        .get_mut("custom_agent_1")
        .unwrap()
        .developer_instructions = "新的指令".into();
    let error = runtime_role_transaction::refresh_runtime_subagent_roles_with_commit(
        &config,
        &marker,
        None,
        |_, _, _| anyhow::bail!("injected final policy commit failure"),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("已恢复原配置"));
    assert_eq!(
        [
            fs::read(role).unwrap(),
            fs::read(policy).unwrap(),
            fs::read(marker).unwrap()
        ],
        before
    );
    assert!(!pending.exists());
}

#[test]
fn native_role_collision_is_rejected_without_replacing_native_config() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let original = "[agents.custom_agent_1]\nconfig_file = 'external.toml'\n";
    fs::write(home.join("config.toml"), original).unwrap();
    let marker = temp.path().join("state/lease.json");
    assert!(start(&home, &marker, &config()).is_err());
    assert_eq!(
        fs::read_to_string(home.join("config.toml")).unwrap(),
        original
    );
    assert!(!marker.exists());
}

#[test]
fn startup_rejects_same_name_unmanaged_runtime_file_without_replacing_it() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let marker = temp.path().join("state/lease.json");
    let path = runtime_agent_path(
        &marker.with_file_name(CODEY_CONSTRAINTS_DIR),
        "custom_agent_1",
    );
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "external user file").unwrap();
    let error = start(&home, &marker, &config()).unwrap_err();
    assert!(format!("{error:#}").contains("不是 Codey 受管文件"));
    assert_eq!(fs::read_to_string(path).unwrap(), "external user file");
    assert!(!marker.exists());
}

#[test]
fn hot_update_rejects_replaced_runtime_file_before_policy_or_lease_changes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let marker = temp.path().join("state/lease.json");
    let mut config = config();
    start(&home, &marker, &config).unwrap();
    let path = runtime_agent_path(
        &marker.with_file_name(CODEY_CONSTRAINTS_DIR),
        "custom_agent_1",
    );
    let lease = fs::read(&marker).unwrap();
    let (policy, pending) = crate::subagent_gate::runtime_subagent_policy_paths(&home);
    let policy_bytes = fs::read(&policy).unwrap();
    fs::write(&path, "external replacement").unwrap();
    config
        .subagent_roles
        .get_mut("custom_agent_1")
        .unwrap()
        .reasoning_effort = "high".into();
    let error = reconcile_runtime_subagent_roles_at(&config, &marker).unwrap_err();
    assert!(format!("{error:#}").contains("不是 Codey 受管文件"));
    assert_eq!(fs::read_to_string(path).unwrap(), "external replacement");
    assert_eq!(fs::read(marker).unwrap(), lease);
    assert_eq!(fs::read(policy).unwrap(), policy_bytes);
    assert!(!pending.exists());
}

#[test]
fn native_agent_file_names_are_checked_by_declared_name_with_bounded_fail_closed_reads() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    fs::create_dir(home.join("agents")).unwrap();
    let native = home.join("agents/not_the_role_id.toml");
    let roles = config().subagent_roles;
    fs::write(&native, "name = 'custom_agent_1'\ndescription = 'native'\n").unwrap();
    let original = fs::read(&native).unwrap();
    assert!(custom_subagents::validate_custom_subagent_role_conflicts_at(&roles, home).is_err());
    assert_eq!(fs::read(&native).unwrap(), original);
    fs::write(&native, "name = 'native_external'\n").unwrap();
    custom_subagents::validate_custom_subagent_role_conflicts_at(&roles, home).unwrap();
    for invalid in [
        "not valid toml".to_string(),
        "description = 'missing name'".to_string(),
        "x".repeat(256 * 1024 + 1),
    ] {
        fs::write(&native, invalid).unwrap();
        assert!(
            custom_subagents::validate_custom_subagent_role_conflicts_at(&roles, home).is_err()
        );
    }
}

#[test]
fn writable_custom_role_inherits_parent_sandbox_instead_of_overriding_it() {
    let temp = tempfile::tempdir().unwrap();
    let mut role = custom();
    role.access = Some(crate::config::SubagentAccess::ReadWrite);
    let plans = plan_runtime_agent_files(
        temp.path(),
        &BTreeMap::from([("custom_agent_1".into(), role)]),
        None,
        None,
    )
    .unwrap();
    let document = parse_document(str::from_utf8(&plans[0].contents).unwrap()).unwrap();
    assert!(document.get("sandbox_mode").is_none());
    assert!(document.get("approval_policy").is_none());
}

#[cfg(unix)]
#[test]
fn linked_native_agent_file_is_not_followed() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("agents")).unwrap();
    let outside = temp.path().join("external.toml");
    fs::write(&outside, "name = 'native_external'\n").unwrap();
    std::os::unix::fs::symlink(&outside, temp.path().join("agents/linked.toml")).unwrap();
    assert!(
        custom_subagents::validate_custom_subagent_role_conflicts_at(
            &config().subagent_roles,
            temp.path()
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(outside).unwrap(),
        "name = 'native_external'\n"
    );
}

#[cfg(unix)]
#[test]
fn linked_runtime_directory_is_rejected_without_touching_external_files() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let marker = temp.path().join("state/lease.json");
    let constraints = marker.with_file_name(CODEY_CONSTRAINTS_DIR);
    let outside = temp.path().join("outside");
    let linked_directory = constraints.join(CODEY_RUNTIME_AGENTS_DIR);
    fs::create_dir_all(linked_directory.parent().unwrap()).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("custom_agent_1.toml"), "external").unwrap();
    std::os::unix::fs::symlink(&outside, linked_directory).unwrap();
    assert!(start(&home, &marker, &config()).is_err());
    assert_eq!(
        fs::read_to_string(outside.join("custom_agent_1.toml")).unwrap(),
        "external"
    );
    assert!(!marker.exists());
}
