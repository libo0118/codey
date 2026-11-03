use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn configured() -> CodeyConfig {
    let mut config = CodeyConfig::default();
    config.profiles[0].name = "测试线路".into();
    config.profiles[0].base_url = "https://api.example.com/v1".into();
    config.profiles[0].api_key = "test-backup-secret".into();
    config.normalize()
}

#[test]
fn backup_roundtrip_preserves_credentials_and_preferences() {
    let mut config = configured();
    config.stream_max_retries = 3;
    let id = config.profiles[0].id.clone();
    config
        .selected_models_by_provider
        .insert(id.clone(), vec!["test-model".into()]);
    let backup = parse(&export(config.clone()).unwrap()).unwrap();
    assert_eq!(backup.summary(7)["settingsRevision"], 7);
    assert_eq!(backup.summary(7)["containsSecrets"], true);
    let restored = backup.prepare(&CodeyConfig::default()).unwrap();
    assert_eq!(restored.profiles[0].api_key, "test-backup-secret");
    assert_eq!(
        restored.selected_models_by_provider[&id],
        vec!["test-model"]
    );
    assert_eq!(restored.stream_max_retries, 3);
}

#[test]
fn invalid_and_incompatible_backups_fail_without_echoing_input() {
    for content in ["", "test-backup-secret", "{}", "[]"] {
        assert!(parse(content).is_err());
    }
    assert!(check_size(MAX_BACKUP_BYTES).is_ok());
    assert!(check_size(MAX_BACKUP_BYTES + 1).is_err());
    let valid: Value = serde_json::from_str(&export(configured()).unwrap()).unwrap();
    for (key, value) in [
        ("version", json!(99)),
        ("format", json!("other")),
        ("createdAt", json!("bad")),
    ] {
        let mut changed = valid.clone();
        changed[key] = value;
        assert!(parse(&changed.to_string()).is_err());
    }
    let mut changed = valid.clone();
    changed["config"]
        .as_object_mut()
        .unwrap()
        .remove("profiles");
    assert!(parse(&changed.to_string()).is_err());
    changed = valid.clone();
    changed["config"]["profiles"][0]["baseUrl"] = json!("https://test-backup-secret@host.invalid");
    let error = parse(&changed.to_string()).err().unwrap();
    assert!(!error.contains("test-backup-secret"));
    assert!(parse(&format!("\u{feff}{}", valid)).is_ok());
}

#[test]
fn restore_preserves_local_managed_routes_models_and_paths() {
    let mut previous = configured();
    previous.profiles[0].plugin_owner_id = Some("local-plugin".into());
    let key = previous.profiles[0].provider_id().to_string();
    previous
        .selected_models_by_provider
        .insert(key.clone(), vec!["plugin-model".into()]);
    previous.codex_app_path = "local-app".into();
    previous.user_scripts = vec!["local-script".into()];
    previous.codex_launch_proxy = "http://127.0.0.1:3128".into();
    previous.settings_revision = 9;
    let mut foreign = configured();
    foreign.profiles[0].plugin_owner_id = Some("foreign-plugin".into());
    foreign.codex_app_path = "foreign-app".into();
    foreign.user_scripts = vec!["foreign-script".into()];
    let next = parse(&export(foreign).unwrap())
        .unwrap()
        .prepare(&previous)
        .unwrap();
    assert_eq!(next.profiles, previous.profiles);
    assert_eq!(next.selected_models_by_provider[&key], vec!["plugin-model"]);
    assert_eq!(next.codex_app_path, "local-app");
    assert_eq!(next.user_scripts, vec!["local-script"]);
    assert_eq!(next.codex_launch_proxy, previous.codex_launch_proxy);
    assert_eq!(next.settings_revision, 9);
    let empty = parse(&export(CodeyConfig::default()).unwrap())
        .unwrap()
        .prepare(&previous)
        .unwrap();
    assert_eq!(empty.profiles, previous.profiles);
}

#[test]
fn conflicting_managed_identity_is_rejected() {
    let incoming = configured();
    let mut previous = incoming.clone();
    previous.profiles[0].plugin_owner_id = Some("owned".into());
    assert!(
        parse(&export(incoming).unwrap())
            .unwrap()
            .prepare(&previous)
            .is_err()
    );
}

#[test]
fn recovery_snapshot_is_independent_and_does_not_replace_primary() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("config.json");
    std::fs::write(&primary, "unchanged").unwrap();
    let first = save_recovery(&primary, configured()).unwrap();
    let second = save_recovery(&primary, configured()).unwrap();
    assert_ne!(first, second);
    assert!(parse(&std::fs::read_to_string(first).unwrap()).is_ok());
    assert_eq!(std::fs::read_to_string(&primary).unwrap(), "unchanged");
    assert!(save_recovery(&primary.join("config.json"), configured()).is_err());
}

fn settings(url: &str) -> webdav::Settings {
    webdav::Settings {
        url: url.into(),
        username: "user".into(),
        password: "secret".into(),
        ..Default::default()
    }
}

#[test]
fn webdav_password_is_private_retained_only_for_same_identity_and_clearable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("webdav.json");
    let mut input = settings("https://dav.example.com/backup.json");
    let public = webdav::save(&path, input.clone()).unwrap();
    assert!(public.get("password").is_none());
    assert_eq!(public["passwordConfigured"], true);
    let mut edit = input.clone();
    edit.revision = public["revision"].as_str().unwrap().into();
    edit.password.clear();
    let public = webdav::save(&path, edit.clone()).unwrap();
    assert_eq!(webdav::load(&path).unwrap().password, "secret");
    edit.username = "new-user".into();
    edit.revision = public["revision"].as_str().unwrap().into();
    webdav::save(&path, edit).unwrap();
    assert!(webdav::load(&path).unwrap().password.is_empty());
    input.revision = webdav::load(&path).unwrap().revision;
    let public = webdav::save(&path, input.clone()).unwrap();
    let mut cleared = input;
    cleared.revision = public["revision"].as_str().unwrap().into();
    cleared.clear_password = true;
    webdav::save(&path, cleared).unwrap();
    assert!(webdav::load(&path).unwrap().password.is_empty());
}

#[test]
fn webdav_stale_settings_and_private_write_failures_preserve_existing_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("webdav.json");
    assert_eq!(webdav::load(&path).unwrap().revision, "");
    assert!(!path.exists());
    let original = settings("https://dav.example.com/backup.json");
    let public = webdav::save(&path, original.clone()).unwrap();
    assert!(!public["revision"].as_str().unwrap().is_empty());
    let before = std::fs::read(&path).unwrap();
    assert!(webdav::save(&path, original.clone()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let blocker = temp.path().join("blocker");
    std::fs::write(&blocker, "keep").unwrap();
    assert!(webdav::save(&blocker.join("settings.json"), original).is_err());
    assert_eq!(std::fs::read_to_string(blocker).unwrap(), "keep");
    assert!(!export(configured()).unwrap().contains("passwordConfigured"));
}

#[test]
fn webdav_rejects_linked_settings_without_reading_or_changing_the_target() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("original.json");
    std::fs::write(&target, "do not disclose or overwrite").unwrap();
    let path = temp.path().join("webdav.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &path).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &path).unwrap();
    assert!(webdav::load(&path).is_err());
    assert!(webdav::save(&path, settings("https://dav.example.com/backup.json")).is_err());
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "do not disclose or overwrite"
    );
}

#[test]
fn managed_routes_retain_fork_upstream_reasoning_metadata() {
    let mut previous = configured();
    previous.profiles[0].plugin_owner_id = Some("local-plugin".into());
    let provider = previous.profiles[0].provider_id().to_owned();
    previous
        .upstream_models_by_provider
        .insert(provider.clone(), vec!["test-model".into()]);
    previous
        .selected_models_by_provider
        .insert(provider.clone(), vec!["test-model".into()]);
    previous
        .upstream_model_reasoning_efforts_by_provider
        .insert(
            provider.clone(),
            BTreeMap::from([(
                "test-model".into(),
                vec![crate::config::ModelReasoningEffort {
                    level: "high".into(),
                    value: "high".into(),
                }],
            )]),
        );
    let previous = previous.normalize();
    assert!(
        previous
            .upstream_model_reasoning_efforts_by_provider
            .contains_key(&provider)
    );
    let restored = parse(&export(CodeyConfig::default()).unwrap())
        .unwrap()
        .prepare(&previous)
        .unwrap();
    assert_eq!(
        restored
            .upstream_model_reasoning_efforts_by_provider
            .get(&provider),
        previous
            .upstream_model_reasoning_efforts_by_provider
            .get(&provider)
    );
}

#[test]
fn webdav_rejects_unsafe_or_ambiguous_addresses_and_oversized_settings() {
    for url in [
        "http://dav.example.com/a.json",
        "https://user:secret@dav.example.com/a.json",
        "file:///a.json",
        "https://dav.example.com/",
        "https://dav.example.com/a.json?key=secret",
        "https://169.254.169.254/a.json",
    ] {
        assert!(settings(url).merge(Default::default()).is_err());
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("webdav.json");
    std::fs::write(&path, vec![b' '; 16 * 1024 + 1]).unwrap();
    assert!(webdav::load(&path).is_err());
    let mut input = settings("http://127.0.0.1/a.json");
    input.allow_http = true;
    assert!(input.merge(Default::default()).is_ok());
}

async fn server(
    status: &str,
    body: String,
    headers: &str,
) -> (webdav::Settings, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut input = settings(&format!(
        "http://{}/backup.json",
        listener.local_addr().unwrap()
    ));
    input.allow_http = true;
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}",
        body.len()
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let size = socket.read(&mut chunk).await.unwrap();
            if size == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..size]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let size = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|size| size.parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= end + 4 + size {
                    break;
                }
            }
        }
        let _ = socket.write_all(response.as_bytes()).await;
        String::from_utf8(request).unwrap()
    });
    (input, task)
}

#[tokio::test]
async fn webdav_put_and_get_use_json_and_basic_auth() {
    let content = export(configured()).unwrap();
    let (input, request) = server("201 Created", String::new(), "").await;
    assert!(
        webdav::transfer(&input, Some(content.clone()))
            .await
            .unwrap()
            .is_none()
    );
    let request = request.await.unwrap();
    assert!(request.starts_with("PUT /backup.json HTTP/1.1"));
    assert!(request.contains("Basic dXNlcjpzZWNyZXQ="));
    assert!(request.contains("application/json"));
    assert!(request.ends_with(&content));
    let (input, request) = server("200 OK", content.clone(), "").await;
    assert_eq!(webdav::transfer(&input, None).await.unwrap(), Some(content));
    assert!(request.await.unwrap().starts_with("GET /backup.json"));
}

#[tokio::test]
async fn webdav_rejects_failures_redirects_and_invalid_success_bodies() {
    for (status, body) in [
        ("401 Unauthorized", "secret"),
        ("404 Not Found", "secret"),
        ("302 Found", "secret"),
        ("207 Multi-Status", "secret"),
        ("500 Server Error", "secret"),
        ("200 OK", "secret"),
    ] {
        let (input, request) = server(
            status,
            body.into(),
            "Location: https://example.com/other.json\r\n",
        )
        .await;
        let error = webdav::transfer(&input, None).await.unwrap_err();
        assert!(!error.contains("secret"));
        request.await.unwrap();
    }
}

#[tokio::test]
async fn webdav_download_is_bounded() {
    let (input, request) = server("200 OK", " ".repeat(MAX_BACKUP_BYTES + 1), "").await;
    assert!(
        webdav::transfer(&input, None)
            .await
            .unwrap_err()
            .contains("4 MiB")
    );
    // The client may close before the mock finishes writing an oversized body.
    let _ = request.await;
}
