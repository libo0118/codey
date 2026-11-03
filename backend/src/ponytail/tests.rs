// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
use super::*;
use std::fs;

const REGISTERED: &str = "[marketplaces.ponytail]\nsource_type = 'git'\nsource = 'https://github.com/DietrichGebert/ponytail.git'\n";

fn cached(home: &Path, version: &str) {
    let root = home.join("plugins/cache/ponytail/ponytail").join(version);
    for directory in [".codex-plugin", "skills/ponytail", "hooks"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    fs::write(
        root.join(".codex-plugin/plugin.json"),
        json!({"name":"ponytail","version":version}).to_string(),
    )
    .unwrap();
    fs::write(root.join("skills/ponytail/SKILL.md"), "fixture").unwrap();
    fs::write(root.join("hooks/claude-codex-hooks.json"), "{}").unwrap();
}

#[test]
fn official_paths_honor_xdg_then_platform_and_reject_relative_paths() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let xdg = home.join("xdg");
    let appdata = home.join("roaming");
    for windows in [false, true] {
        assert_eq!(
            config_path(home, windows, Some(&xdg), Some(&appdata)).unwrap(),
            xdg.join("ponytail/config.json")
        );
        assert!(config_path(home, windows, Some(Path::new("relative")), None).is_err());
    }
    assert_eq!(
        config_path(home, true, None, Some(&appdata)).unwrap(),
        appdata.join("ponytail/config.json")
    );
    assert_eq!(
        config_path(home, true, None, None).unwrap(),
        home.join("AppData/Roaming/ponytail/config.json")
    );
    assert_eq!(
        config_path(home, false, None, None).unwrap(),
        home.join(".config/ponytail/config.json")
    );
}

#[test]
fn settings_match_official_precedence_and_preserve_unknown_fields_and_bom() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    assert_eq!(settings(&path, None).unwrap()["defaultMode"], "full");
    assert!(!path.exists());
    fs::write(
        &path,
        "\u{feff}{\"defaultMode\":\" LITE \",\"quietStartup\":true,\"nested\":{\"keep\":1}}",
    )
    .unwrap();
    let before = settings(&path, Some(" ULTRA ")).unwrap();
    assert_eq!(before["defaultMode"], "lite");
    assert_eq!(before["effectiveMode"], "ultra");
    assert_eq!(
        settings(&path, Some("review")).unwrap()["effectiveMode"],
        "lite"
    );
    save_settings(&path, Mode::Off, before["revision"].as_str().unwrap()).unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!({"defaultMode":"off","quietStartup":true,"nested":{"keep":1}})
    );
    assert_eq!(
        settings(&path, Some("off")).unwrap()["environmentOverride"],
        "off"
    );
    assert!(normalized_mode("review").is_none());
    for mode in ["lite", "full", "ultra", "off"] {
        assert!(normalized_mode(mode).is_some());
    }
}

#[test]
fn invalid_stale_or_oversized_settings_never_overwrite_original_or_expose_content() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    for value in ["{private-secret", "null", "[]", "\"private-secret\""] {
        fs::write(&path, value).unwrap();
        let err = settings(&path, None).unwrap_err().to_string();
        assert!(!err.contains("private-secret"));
        assert!(save_settings(&path, Mode::Lite, &revision(Some(value.as_bytes()))).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), value);
    }
    fs::write(&path, "{}").unwrap();
    let before = settings(&path, None).unwrap();
    fs::write(&path, "{\"other\":true}").unwrap();
    assert!(save_settings(&path, Mode::Lite, before["revision"].as_str().unwrap()).is_err());
    assert!(save_settings(&path, Mode::Full, "missing").is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\"other\":true}");
    fs::write(&path, vec![b' '; MAX_CONFIG as usize + 1]).unwrap();
    assert!(settings(&path, None).is_err());
}

#[test]
fn settings_creation_and_concurrent_save_lock_are_safe() {
    use fs2::FileExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ponytail/config.json");
    save_settings(&path, Mode::Ultra, "missing").unwrap();
    let snapshot = settings(&path, None).unwrap();
    assert_eq!(snapshot["defaultMode"], "ultra");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.parent().unwrap().join(".codey-config.lock"))
        .unwrap();
    FileExt::lock_exclusive(&lock).unwrap();
    assert!(save_settings(&path, Mode::Off, snapshot["revision"].as_str().unwrap()).is_err());
    assert_eq!(settings(&path, None).unwrap()["defaultMode"], "ultra");
}

#[test]
fn plugin_status_requires_cache_and_toggle_preserves_all_other_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let path = home.join("settings.json");
    assert_eq!(status(home, &path, None).unwrap()["installed"], false);
    assert!(!home.join("config.toml").exists());
    fs::write(home.join("config.toml"), format!("# keep\nmodel = 'user-model'\n{REGISTERED}\n[plugins.'ponytail@ponytail']\nenabled = false\ncustom = 'keep'\n[plugins.'other@market']\nenabled = true\n")).unwrap();
    assert_eq!(status(home, &path, None).unwrap()["installed"], false);
    cached(home, "5.3.2");
    let snapshot = status(home, &path, None).unwrap();
    assert_eq!(snapshot["version"], "5.3.2");
    assert_eq!(snapshot["installed"], true);
    assert_eq!(snapshot["enabled"], false);
    set_enabled(home, true).unwrap();
    set_enabled(home, false).unwrap();
    let config = fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains("# keep"));
    assert!(config.contains("user-model"));
    assert!(config.contains("custom = 'keep'"));
    let doc: DocumentMut = config.parse().unwrap();
    assert_eq!(
        doc["plugins"]["other@market"]["enabled"].as_bool(),
        Some(true)
    );
    assert_eq!(doc["plugins"][PLUGIN]["enabled"].as_bool(), Some(false));
    assert!(cached_version(home).unwrap().is_some());
}

#[test]
fn refuses_conflicting_marketplace_and_invalid_plugin_tables() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    for source in [
        "source_type='local'\nsource='user-directory'",
        "source_type='git'\nsource='https://example.com/private-token'",
    ] {
        let config = format!(
            "[marketplaces.ponytail]\n{source}\n[plugins.'ponytail@ponytail']\nenabled=true\n"
        );
        fs::write(home.join("config.toml"), &config).unwrap();
        let error = set_enabled(home, false).unwrap_err().to_string();
        assert!(!error.contains("private-token"));
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            config
        );
    }
    fs::write(
        home.join("config.toml"),
        "plugins = { 'ponytail@ponytail' = false }",
    )
    .unwrap();
    assert!(set_enabled(home, false).is_err());
    assert!(status(home, &home.join("settings.json"), None).is_err());
    fs::write(home.join("config.toml"), "").unwrap();
    assert!(set_enabled(home, true).is_err());
    assert!(set_enabled(home, false).is_ok());
}

#[tokio::test]
async fn official_install_commands_are_ordered_and_partial_failure_is_retryable() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let mut calls = Vec::new();
    let failure = install_with(home, |args, _| {
        calls.push(args.to_vec());
        let result = if args.contains(&"marketplace") {
            fs::write(home.join("config.toml"), REGISTERED).unwrap();
            Ok(())
        } else {
            Err(anyhow::anyhow!("mock download failure"))
        };
        std::future::ready(result)
    })
    .await;
    assert!(failure.is_err());
    assert_eq!(
        calls,
        vec![
            vec!["plugin", "marketplace", "add", SOURCE],
            vec!["plugin", "add", PLUGIN]
        ]
    );
    calls.clear();
    install_with(home, |args, _| {
        calls.push(args.to_vec());
        cached(home, "5.3.2");
        fs::write(
            home.join("config.toml"),
            format!("{REGISTERED}\n[plugins.'ponytail@ponytail']\nenabled = true\n"),
        )
        .unwrap();
        std::future::ready(Ok(()))
    })
    .await
    .unwrap();
    assert_eq!(calls, vec![vec!["plugin", "add", PLUGIN]]);
}

#[tokio::test]
async fn fake_success_without_files_and_conflicts_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    assert!(
        install_with(home, |_, _| std::future::ready(Ok(())))
            .await
            .is_err()
    );
    fs::write(
        home.join("config.toml"),
        "[marketplaces.ponytail]\nsource_type='local'\nsource='custom'\n",
    )
    .unwrap();
    let mut called = false;
    assert!(
        install_with(home, |_, _| {
            called = true;
            std::future::ready(Ok(()))
        })
        .await
        .is_err()
    );
    assert!(!called);
}

#[tokio::test]
async fn command_start_errors_are_actionable() {
    let temp = tempfile::tempdir().unwrap();
    let error = run(&temp.path().join("missing"), temp.path(), &[], "检查 Codex")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("检查 Codex无法启动"));
}

#[test]
#[ignore = "仅作为 Ponytail 子进程测试夹具运行"]
fn cli_fixture() {
    match std::env::var("CODEY_PONYTAIL_TEST_MODE").as_deref() {
        Ok("fail") => {
            eprintln!("private-fixture-output");
            std::process::exit(7);
        }
        Ok("wait") => std::thread::sleep(Duration::from_secs(30)),
        _ => {}
    }
}

#[test]
fn command_success_exit_failure_and_timeout_are_bounded_and_redacted() {
    for mode in ["ok", "fail", "wait"] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "ponytail::tests::cli_fixture", "--ignored"])
            .env("CODEY_PONYTAIL_TEST_MODE", mode)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let timeout = if mode == "wait" {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(20)
        };
        let result = run_command(&mut command, "fixture", timeout);
        if mode == "ok" {
            result.unwrap();
        } else {
            let error = result.unwrap_err().to_string();
            assert!(error.contains(if mode == "wait" {
                "超时"
            } else {
                "退出码"
            }));
            assert!(!error.contains("private-fixture-output"));
        }
    }
}

#[test]
fn invalid_codex_toml_does_not_expose_config_contents() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("config.toml"), "api_key = private-secret").unwrap();
    for error in [
        status(temp.path(), &temp.path().join("settings.json"), None).unwrap_err(),
        set_enabled(temp.path(), false).unwrap_err(),
    ] {
        assert!(!format!("{error:#}").contains("private-secret"));
    }
}

#[tokio::test]
#[ignore = "从官方仓库下载 Ponytail 到临时 CODEX_HOME，需 Node.js、Git 与网络"]
async fn official_plugin_smoke() {
    let temp = tempfile::tempdir().unwrap();
    let executable = cli("").unwrap();
    install(&executable, temp.path()).await.unwrap();
    let path = temp.path().join("ponytail-settings/config.json");
    let before = status(temp.path(), &path, None).unwrap();
    assert_eq!(before["installed"], true);
    assert_eq!(before["enabled"], true);
    save_settings(&path, Mode::Lite, "missing").unwrap();
    set_enabled(temp.path(), false).unwrap();
    let after = status(temp.path(), &path, None).unwrap();
    assert_eq!(after["installed"], true);
    assert_eq!(after["enabled"], false);
    assert_eq!(after["settings"]["defaultMode"], "lite");
}
