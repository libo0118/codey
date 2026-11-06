use super::*;
use crate::config::{CodeyConfig, ConfigStore, SubagentAccess};

fn custom() -> SubagentRoleConfig {
    serde_json::from_value(serde_json::json!({
        "name": "测试", "description": "只读检查", "developerInstructions": "检查后返回证据。",
        "model": "provider/model", "reasoningEffort": "ultra"
    }))
    .unwrap()
}

#[test]
fn subagent_count_defaults_roundtrips_and_rejects_invalid_values_before_writing() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    let store = ConfigStore::new(path.clone());
    assert_eq!(CodeyConfig::default().subagent_max_threads, 2);
    assert_eq!(
        super::super::store::parse_config_contents("{}", &path)
            .unwrap()
            .subagent_max_threads,
        2
    );
    let mut config = CodeyConfig::default().normalize();
    for limit in [1, 2, 8, 64] {
        config.subagent_max_threads = limit;
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap().subagent_max_threads, limit);
    }
    let bytes = std::fs::read(&path).unwrap();
    for limit in [0, 65, u32::MAX] {
        config.subagent_max_threads = limit;
        assert!(store.save(&config).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    for value in [
        serde_json::json!(0),
        serde_json::json!(65),
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!("4"),
        serde_json::json!(true),
        serde_json::Value::Null,
        serde_json::json!(4_294_967_296_u64),
    ] {
        assert!(
            super::super::store::parse_config_contents(
                &serde_json::json!({"subagentMaxThreads": value}).to_string(),
                &path
            )
            .is_err()
        );
    }
}

#[test]
fn custom_roles_roundtrip_and_missing_access_is_read_only_without_changing_legacy_builtins() {
    let mut config = CodeyConfig::default();
    config
        .subagent_roles
        .insert("custom_agent_1".into(), custom());
    let config = config.normalize();
    assert_eq!(
        config.subagent_roles["custom_agent_1"].access,
        Some(SubagentAccess::ReadOnly)
    );
    let legacy: CodeyConfig = serde_json::from_value(serde_json::json!({
        "subagentRoles": {"codey_worker": {"model": "legacy", "reasoningEffort": "max"}}
    }))
    .unwrap();
    assert_eq!(
        legacy.normalize().subagent_roles["codey_worker"].access,
        Some(SubagentAccess::ReadWrite)
    );
    let temp = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.json"));
    store.save(&config).unwrap();
    assert_eq!(store.load().unwrap(), config);
}

#[test]
fn validation_rejects_path_native_windows_reserved_ids_and_byte_limits_without_truncation() {
    for id in [
        "",
        "../outside",
        "a/b",
        "a\\b",
        "a.b",
        "A",
        "con",
        "nul",
        "com1",
        "lpt9",
        "worker",
        "explorer",
        "max_threads",
    ] {
        assert!(
            validate_subagent_roles(&BTreeMap::from([(id.into(), custom())])).is_err(),
            "{id}"
        );
    }
    assert!(!valid_subagent_role_id(&"a".repeat(65)));
    for (field, limit) in [
        ("name", 80),
        ("description", 1000),
        ("developerInstructions", 4000),
    ] {
        for value in [
            " ".to_string(),
            "文".repeat(limit / 3 + 1),
            "x\0y".to_string(),
        ] {
            let mut json = serde_json::to_value(custom()).unwrap();
            json[field] = serde_json::json!(value);
            let selection = serde_json::from_value(json).unwrap();
            assert!(
                validate_subagent_roles(&BTreeMap::from([("custom_agent_1".into(), selection)]))
                    .is_err(),
                "{field}"
            );
        }
    }
    let roles = (0..33)
        .map(|i| (format!("custom_agent_{i}"), custom()))
        .collect();
    assert!(validate_subagent_roles(&roles).is_err());
    for (field, value) in [("model", ""), ("reasoningEffort", "invalid")] {
        let mut json = serde_json::to_value(custom()).unwrap();
        json[field] = serde_json::json!(value);
        assert!(
            validate_subagent_roles(&BTreeMap::from([(
                "custom_agent_1".into(),
                serde_json::from_value(json).unwrap()
            )]))
            .is_err()
        );
    }
}

#[test]
fn partial_updates_preserve_missing_roles_and_only_explicit_custom_deletions_remove_them() {
    let previous = BTreeMap::from([("custom_agent_1".into(), custom())]);
    assert_eq!(
        merge_subagent_role_updates(&previous, &BTreeMap::new(), &[]).unwrap(),
        previous
    );
    let deleted = vec!["custom_agent_1".into()];
    let empty = merge_subagent_role_updates(&previous, &BTreeMap::new(), &deleted).unwrap();
    assert!(empty.is_empty());
    assert!(
        merge_subagent_role_updates(&empty, &BTreeMap::new(), &deleted)
            .unwrap()
            .is_empty()
    );
    for role in ["default", "codey_worker", "../bad", "worker"] {
        assert!(merge_subagent_role_updates(&previous, &BTreeMap::new(), &[role.into()]).is_err());
    }
    assert!(merge_subagent_role_updates(&previous, &previous, &deleted).is_err());
    assert_eq!(previous.len(), 1);
}

#[test]
fn invalid_custom_role_cannot_overwrite_a_valid_store() {
    let temp = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.json"));
    let config = CodeyConfig::default().normalize();
    store.save(&config).unwrap();
    let bytes = std::fs::read(store.path()).unwrap();
    let mut invalid = config.clone();
    invalid.subagent_roles.insert("../outside".into(), custom());
    assert!(store.save(&invalid).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), bytes);
    assert_eq!(store.load().unwrap(), config);
}

#[test]
fn old_builtin_roles_migrate_without_dropping_custom_roles_on_store_load() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    let old = serde_json::json!({"subagentRoles": {
        "codey_quick_scan": {"enabled": true, "model": "old-model", "reasoningEffort": "high"},
        "custom_agent_1": custom()
    }});
    std::fs::write(&path, old.to_string()).unwrap();
    let loaded = ConfigStore::new(path).load().unwrap();
    assert_eq!(loaded.subagent_roles["codey_explorer"].model, "old-model");
    assert!(!loaded.subagent_roles.contains_key("codey_quick_scan"));
    assert_eq!(
        loaded.subagent_roles["custom_agent_1"].access,
        Some(SubagentAccess::ReadOnly)
    );
}

#[test]
fn legacy_partial_role_updates_reach_their_successor() {
    let previous = crate::config::default_subagent_roles();
    let incoming = BTreeMap::from([(
        "codey_quick_scan".into(),
        SubagentRoleConfig::new("updated-model", "high"),
    )]);
    let merged = merge_subagent_role_updates(&previous, &incoming, &[]).unwrap();
    assert_eq!(merged["codey_explorer"].model, "updated-model");
    assert!(!merged.contains_key("codey_quick_scan"));
    assert!(
        merge_subagent_role_updates(&previous, &BTreeMap::new(), &["codey_quick_scan".into()])
            .is_err()
    );
}
