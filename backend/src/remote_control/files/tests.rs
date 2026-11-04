use super::*;
use serde_json::json;
use std::io::{Read, Write};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    thread: String,
    registry: Arc<Mutex<Registry>>,
    binding: BindingGuard,
    state: Value,
}

fn fixture(text: &str) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("workspace");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("outputs")).unwrap();
    let thread = uuid::Uuid::new_v4().to_string();
    let state = json!({"id":thread,"cwd":root,"turns":[{"turnId":"turn","items":[{"id":"answer","type":"agentMessage","text":text}]}]});
    let registry = Arc::new(Mutex::new(Registry::with_protection(
        None,
        vec![temp.path().join("synthetic-codex-home")],
    )));
    let binding = BindingGuard::new(registry.clone(), "token", &thread, &state).unwrap();
    Fixture {
        _temp: temp,
        root,
        thread,
        registry,
        binding,
        state,
    }
}

fn select(fixture: &Fixture, reference: &str) -> Selection {
    fixture
        .registry
        .lock()
        .unwrap()
        .selection("token", &fixture.thread, reference)
        .unwrap()
}

fn snapshot(fixture: &Fixture, reference: &str) -> Snapshot {
    let descriptor = descriptor(fixture, reference);
    prepare_snapshot(&fixture.registry, &descriptor, || Ok(())).unwrap()
}

fn descriptor(fixture: &Fixture, reference: &str) -> Descriptor {
    describe(select(fixture, reference), || Ok(())).unwrap()
}

#[test]
fn public_activity_file_links_use_the_same_projection_without_widening_authority() {
    let message = "[结果](outputs/activity.txt) [越界](../outside.txt)";
    for item in [
        json!({"id":"activity","type":"collabAgentToolCall","tool":"wait","status":"completed","receiverThreadIds":["child"],"agentsStates":{"child":{"status":"completed","message":message}},"prompt":"[private](outputs/private.txt)"}),
        json!({"id":"activity","type":"mcpToolCall","tool":"inspect_local_file","result":{"content":[{"type":"text","text":message}],"_meta":{"text":"[private](outputs/private.txt)"}}}),
        json!({"id":"activity","type":"dynamicToolCall","tool":"test","contentItems":[{"type":"inputText","text":message}],"arguments":{"private":"[private](outputs/private.txt)"}}),
        json!({"id":"activity","type":"reasoning","summary":[message],"content":"[private](outputs/private.txt)"}),
    ] {
        let mut fixture = fixture("");
        std::fs::write(fixture.root.join("outputs/activity.txt"), b"public result").unwrap();
        std::fs::write(
            fixture.root.parent().unwrap().join("outside.txt"),
            b"outside",
        )
        .unwrap();
        fixture.state["turns"][0]["items"] = json!([item]);
        fixture.binding.update(&fixture.state).unwrap();
        assert!(
            super::super::protocol::view(&fixture.state)
                .to_string()
                .contains("outputs/activity.txt")
        );
        assert_eq!(
            snapshot(&fixture, "outputs/activity.txt").read(0).unwrap(),
            b"public result"
        );
        assert!(describe(select(&fixture, "../outside.txt"), || Ok(())).is_err());
        assert!(
            fixture
                .registry
                .lock()
                .unwrap()
                .selection("token", &fixture.thread, "outputs/private.txt")
                .is_err()
        );
    }
    let mut fixture = fixture("");
    fixture.state["turns"][0]["items"] = json!([{"id":"unknown","type":"collabAgentToolCall","tool":"futureAction","receiverThreadIds":["child"],"agentsStates":{"child":{"status":"completed","message":message}}}]);
    fixture.binding.update(&fixture.state).unwrap();
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/activity.txt")
            .is_err()
    );
}

#[test]
fn generated_markdown_download_is_a_private_exact_byte_snapshot() {
    let fixture = fixture("[文档](outputs/result.docx) [空格](<outputs/a b.pdf>)");
    let bytes = b"PK\x03\x04synthetic document\0bytes";
    let path = fixture.root.join("outputs/result.docx");
    std::fs::write(&path, bytes).unwrap();
    std::fs::write(fixture.root.join("outputs/a b.pdf"), b"pdf").unwrap();
    let source_before = std::fs::read(&path).unwrap();
    let mut file = snapshot(&fixture, "outputs/result.docx");
    assert_eq!(file.digest, crate::fs_util::sha256_hex(bytes));
    assert_eq!(file.size, bytes.len() as u64);
    let mut actual = Vec::new();
    file.file.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            file.file.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(file);
    let descriptor = descriptor(&fixture, "outputs/result.docx");
    let response = fixture.registry.lock().unwrap().insert(descriptor).unwrap();
    let attachment = &response["file"];
    let id = attachment["id"].as_str().unwrap();
    assert_eq!(
        download_route(attachment["downloadUrl"].as_str().unwrap()),
        Some((fixture.thread.as_str(), id))
    );
    assert_eq!(attachment["name"], "result.docx");
    assert!(attachment.get("url").is_none());
    assert!(!response.to_string().contains("workspace"));
    let file = fixture
        .registry
        .lock()
        .unwrap()
        .cached("token", &fixture.thread, id)
        .unwrap();
    let snapshot = prepare_snapshot(&fixture.registry, &file, || Ok(())).unwrap();
    assert_eq!(snapshot.read(0).unwrap(), bytes);
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .cached("other", &fixture.thread, id)
            .is_err()
    );
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .cached("token", &uuid::Uuid::new_v4().to_string(), id)
            .is_err()
    );
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .cached("token", &fixture.thread, &"f".repeat(64))
            .is_err()
    );
    assert!(describe(select(&fixture, "outputs/a%20b.pdf"), || Ok(())).is_ok());
    assert_eq!(std::fs::read(path).unwrap(), source_before);
}

#[test]
fn native_user_attachment_has_only_opaque_identity_and_exact_outside_workspace_authority() {
    let mut fixture = fixture("");
    let outside = fixture.root.parent().unwrap().join("private-user-file.bin");
    std::fs::write(&outside, b"user-selected").unwrap();
    fixture.state["turns"][0]["items"] = json!([
        {"id":"user","type":"userMessage","content":[{"type":"file","path":outside}]},
        {"id":"model","type":"agentMessage","text":format!("[not authorized](<{}>)",outside.display()),"content":[{"type":"file","path":outside}]}
    ]);
    fixture.binding.update(&fixture.state).unwrap();
    let view = super::super::protocol::view(&fixture.state);
    let attachment = &view["turns"][0]["messages"][0]["attachments"][0];
    assert_eq!(attachment["name"], "private-user-file.bin");
    assert!(
        !view["turns"][0]["messages"][0]
            .to_string()
            .contains(fixture.root.parent().unwrap().to_str().unwrap())
    );
    assert_eq!(
        snapshot(&fixture, attachment["id"].as_str().unwrap())
            .read(0)
            .unwrap(),
        b"user-selected"
    );
    assert!(describe(select(&fixture, outside.to_str().unwrap()), || Ok(())).is_err());
    let fake = &view["turns"][0]["messages"][1]["attachments"][0]["id"];
    assert!(describe(select(&fixture, fake.as_str().unwrap()), || Ok(())).is_err());
    // A client cannot invent native attachment authority from a filesystem path.
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "private-user-file.bin")
            .is_err()
    );
}

#[test]
fn native_absolute_attachments_protect_config_but_allow_exact_home_and_downloads_files() {
    let mut fixture = fixture("");
    let home = fixture.root.parent().unwrap().join("synthetic-home");
    let protected = fixture.root.parent().unwrap().join("custom-codex-home");
    std::fs::create_dir_all(home.join("Downloads")).unwrap();
    std::fs::create_dir(&protected).unwrap();
    let mut paths = vec![home.join("attachment.bin"), home.join("Downloads/file.bin")];
    for name in [".codex", ".ssh", ".gnupg"] {
        std::fs::create_dir(home.join(name)).unwrap();
        paths.push(home.join(name).join("synthetic-secret"));
    }
    paths.push(protected.join("auth.json"));
    for path in &paths {
        std::fs::write(path, b"SYNTHETIC_ATTACHMENT").unwrap();
    }
    fixture.registry.lock().unwrap().protection = Protection {
        home: Some(home.clone()),
        roots: vec![protected],
    };
    // A home cwd is intentionally too broad for Markdown, not for an exact user attachment.
    fixture.state["cwd"] = json!(home);
    for opening in [false, true] {
        for kind in ["file", "localImage", "image"] {
            let parts: Vec<_> = paths
                .iter()
                .map(|path| json!({"type":kind,"path":path}))
                .collect();
            let model = json!({"id":"model","type":"agentMessage","text":format!("[not authorized](<{}>)",paths[0].display())});
            fixture.state["turns"][0]["items"] = if opening {
                json!([model])
            } else {
                json!([{"id":"user","type":"userMessage","content":parts},model])
            };
            fixture.state["turns"][0]["params"]["input"] = json!(parts);
            fixture.binding.update(&fixture.state).unwrap();
            let view = super::super::protocol::view(&fixture.state);
            let attachments = view["turns"][0]["messages"][0]["attachments"]
                .as_array()
                .unwrap();
            assert_eq!(attachments.len(), paths.len());
            for (index, attachment) in attachments.iter().enumerate() {
                let selected = select(&fixture, attachment["id"].as_str().unwrap());
                assert!(selected.value.native);
                if index < 2 {
                    let file = describe(selected, || Ok(())).unwrap();
                    assert_eq!(
                        prepare_snapshot(&fixture.registry, &file, || Ok(()))
                            .unwrap()
                            .read(0)
                            .unwrap(),
                        b"SYNTHETIC_ATTACHMENT"
                    );
                } else {
                    assert!(
                        describe(selected, || Ok(())).is_err(),
                        "{opening}: {kind}: {}",
                        paths[index].display()
                    );
                }
            }
            assert!(describe(select(&fixture, paths[0].to_str().unwrap()), || Ok(())).is_err());
        }
    }
}

#[cfg(any(unix, windows))]
#[test]
fn native_absolute_attachments_deny_protected_alias_targets_and_all_file_or_parent_links() {
    let mut fixture = fixture("");
    let parent = fixture.root.parent().unwrap();
    let protected = parent.join("custom-config");
    std::fs::create_dir(&protected).unwrap();
    std::fs::write(protected.join("auth.json"), b"SYNTHETIC_CONFIG").unwrap();
    let safe = fixture.root.join("outputs/safe.bin");
    std::fs::write(&safe, b"safe").unwrap();
    let alias = parent.join("configured-home-alias");
    let file_link = parent.join("file-link");
    let directory_link = parent.join("directory-link");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&protected, &alias).unwrap();
        std::os::unix::fs::symlink(&safe, &file_link).unwrap();
        std::os::unix::fs::symlink(fixture.root.join("outputs"), &directory_link).unwrap();
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(&protected, &alias).unwrap();
        std::os::windows::fs::symlink_file(&safe, &file_link).unwrap();
        std::os::windows::fs::symlink_dir(fixture.root.join("outputs"), &directory_link).unwrap();
    }
    fixture.registry.lock().unwrap().protection = Protection {
        home: None,
        roots: vec![alias.clone()],
    };
    let paths = [
        protected.join("auth.json"),
        alias.join("auth.json"),
        file_link,
        directory_link.join("safe.bin"),
        safe,
    ];
    let parts: Vec<_> = paths
        .iter()
        .map(|path| json!({"type":"file","path":path}))
        .collect();
    fixture.state["turns"][0]["items"] =
        json!([{"id":"user","type":"userMessage","content":parts}]);
    fixture.binding.update(&fixture.state).unwrap();
    let view = super::super::protocol::view(&fixture.state);
    let attachments = view["turns"][0]["messages"][0]["attachments"]
        .as_array()
        .unwrap();
    for attachment in &attachments[..4] {
        assert!(
            describe(select(&fixture, attachment["id"].as_str().unwrap()), || Ok(
                ()
            ))
            .is_err()
        );
    }
    let safe_id = attachments[4]["id"].as_str().unwrap();
    assert_eq!(snapshot(&fixture, safe_id).read(0).unwrap(), b"safe");
    #[cfg(unix)]
    {
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(parent.join("missing-target"), &alias).unwrap();
    }
    #[cfg(windows)]
    {
        std::fs::remove_dir(&alias).unwrap();
        std::os::windows::fs::symlink_dir(parent.join("missing-target"), &alias).unwrap();
    }
    assert!(
        describe(select(&fixture, safe_id), || Ok(()))
            .err()
            .unwrap()
            .contains("真实目标")
    );
}

#[test]
fn passive_resolution_allocates_no_snapshot_and_native_percent_paths_remain_exact() {
    let mut fixture = fixture("");
    let literal = fixture.root.join("outputs/a%20b.txt");
    std::fs::write(&literal, b"literal percent path").unwrap();
    std::fs::write(fixture.root.join("outputs/a b.txt"), b"different file").unwrap();
    fixture.state["turns"][0]["items"] = json!([
        {"id":"user","type":"userMessage","content":[{"type":"file","path":literal}]},
        {"id":"answer","type":"agentMessage","text":format!("[absolute](<{}>)",literal.display())}
    ]);
    fixture.binding.update(&fixture.state).unwrap();
    let view = super::super::protocol::view(&fixture.state);
    let id = view["turns"][0]["messages"][0]["attachments"][0]["id"]
        .as_str()
        .unwrap();
    let descriptor = descriptor(&fixture, id);
    assert_eq!(
        fixture.registry.lock().unwrap().budget.available_permits(),
        CACHE_KIB
    );
    assert_eq!(
        prepare_snapshot(&fixture.registry, &descriptor, || Ok(()))
            .unwrap()
            .read(0)
            .unwrap(),
        b"literal percent path"
    );
    let first = snapshot(&fixture, id);
    let second = snapshot(&fixture, id);
    assert!(fixture.registry.lock().unwrap().budget.available_permits() < CACHE_KIB);
    drop((first, second));
    assert_eq!(
        fixture.registry.lock().unwrap().budget.available_permits(),
        CACHE_KIB
    );
}

#[test]
fn file_resolution_requires_a_bound_public_candidate_and_independent_safe_root() {
    let fixture = fixture(
        "[traversal](../outside) [remote](https://evil.invalid/a) [UNC](//evil.invalid/share) [sandbox](sandbox:/mnt/data/a)",
    );
    for reference in [
        "../outside",
        "https://evil.invalid/a",
        "//evil.invalid/share",
        "sandbox:/mnt/data/a",
    ] {
        let selected = select(&fixture, reference);
        assert!(describe(selected, || Ok(())).is_err());
    }
    for reference in ["not-public.txt", "./outputs/not-public.txt"] {
        assert!(
            fixture
                .registry
                .lock()
                .unwrap()
                .selection("token", &fixture.thread, reference)
                .is_err()
        );
    }
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &uuid::Uuid::new_v4().to_string(), "anything")
            .unwrap_err()
            .contains("先打开")
    );
    let mut selected = select(&fixture, "../outside");
    selected.value.value = "outputs/file.txt".into();
    selected.cwd = Some(if cfg!(windows) { "C:/" } else { "/" }.into());
    assert!(selected_path(&selected).is_err());
}

#[test]
fn reference_replacement_source_changes_disappearance_and_disconnect_invalidate_downloads() {
    let fixture = fixture("[a](outputs/a.txt)");
    let path = fixture.root.join("outputs/a.txt");
    std::fs::write(&path, b"old").unwrap();
    let old_descriptor = descriptor(&fixture, "outputs/a.txt");
    let old = snapshot(&fixture, "outputs/a.txt");
    std::fs::write(&path, b"recreated different content").unwrap();
    assert!(old_descriptor.validate_source().is_err());
    assert!(prepare_snapshot(&fixture.registry, &old_descriptor, || Ok(())).is_err());
    assert_eq!(old.read(0).unwrap(), b"old");
    let current = descriptor(&fixture, "outputs/a.txt");
    std::fs::remove_file(&path).unwrap();
    assert!(current.validate_source().is_err());
    std::fs::write(&path, b"new content").unwrap();
    assert!(current.validate_source().is_err());
}

#[test]
fn replaced_links_and_disconnected_bindings_drop_only_owned_cache_files() {
    let mut fixture = fixture("[a](outputs/a.txt)");
    let path = fixture.root.join("outputs/a.txt");
    std::fs::write(&path, b"keep").unwrap();
    let file = descriptor(&fixture, "outputs/a.txt");
    let response = fixture.registry.lock().unwrap().insert(file).unwrap();
    let id = response["file"]["id"].as_str().unwrap().to_string();
    fixture.state["turns"][0]["items"][0]["text"] = json!("[replacement](outputs/b.txt)");
    fixture.binding.update(&fixture.state).unwrap();
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .cached("token", &fixture.thread, &id)
            .is_err()
    );
    assert_eq!(
        fixture.registry.lock().unwrap().budget.available_permits(),
        CACHE_KIB
    );
    drop(fixture.binding);
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/b.txt")
            .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"keep");
}

#[test]
fn same_session_windows_keep_old_downloads_and_independent_history_until_their_own_disconnect() {
    let mut fixture = fixture("[old](outputs/old.txt) [shared](outputs/shared.bin)");
    let bytes = vec![b's'; CHUNK * 3];
    std::fs::write(fixture.root.join("outputs/old.txt"), b"old").unwrap();
    std::fs::write(fixture.root.join("outputs/new.txt"), b"new").unwrap();
    std::fs::write(fixture.root.join("outputs/late.txt"), b"late").unwrap();
    let path = fixture.root.join("outputs/shared.bin");
    std::fs::write(&path, &bytes).unwrap();
    let file = descriptor(&fixture, "outputs/shared.bin");
    let selection = file.selection();
    let response = fixture.registry.lock().unwrap().insert(file).unwrap();
    let id = response["file"]["id"].as_str().unwrap();
    let mut newer_state = fixture.state.clone();
    newer_state["turns"][0]["items"][0]["text"] =
        json!("[shared](outputs/shared.bin) [new](outputs/new.txt)");
    let mut newer = Some(
        BindingGuard::new(
            fixture.registry.clone(),
            "token",
            &fixture.thread,
            &newer_state,
        )
        .unwrap(),
    );
    assert!(fixture.registry.lock().unwrap().current(&selection));
    assert_eq!(
        snapshot(&fixture, "outputs/old.txt").read(0).unwrap(),
        b"old"
    );
    assert_eq!(
        snapshot(&fixture, "outputs/new.txt").read(0).unwrap(),
        b"new"
    );
    fixture.state["turns"][0]["items"][0]["text"] =
        json!("[old](outputs/old.txt) [shared](outputs/shared.bin) [late](outputs/late.txt)");
    fixture.binding.update(&fixture.state).unwrap();
    assert_eq!(
        snapshot(&fixture, "outputs/late.txt").read(0).unwrap(),
        b"late"
    );
    let file = fixture
        .registry
        .lock()
        .unwrap()
        .cached("token", &fixture.thread, id)
        .unwrap();
    let mut checks = 0;
    let download = prepare_snapshot(&fixture.registry, &file, || {
        checks += 1;
        if checks == 3 {
            drop(newer.take());
        }
        fixture
            .registry
            .lock()
            .unwrap()
            .current(&selection)
            .then_some(())
            .ok_or("disconnected".into())
    })
    .unwrap();
    assert!(newer.is_none());
    assert_eq!(download.digest, crate::fs_util::sha256_hex(&bytes));
    drop(download);
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .cached("token", &fixture.thread, id)
            .is_ok()
    );
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/new.txt")
            .is_err()
    );
    assert_eq!(
        snapshot(&fixture, "outputs/old.txt").read(0).unwrap(),
        b"old"
    );
    let registry = fixture.registry.clone();
    let mut last = Some(fixture.binding);
    checks = 0;
    assert!(
        prepare_snapshot(&registry, &file, || {
            checks += 1;
            if checks == 3 {
                drop(last.take());
            }
            registry
                .lock()
                .unwrap()
                .current(&selection)
                .then_some(())
                .ok_or("disconnected".into())
        })
        .is_err()
    );
    assert!(last.is_none());
    let registry = registry.lock().unwrap();
    assert!(!registry.current(&selection));
    assert!(registry.cached("token", &fixture.thread, id).is_err());
    assert!(
        registry
            .selection("token", &fixture.thread, "outputs/old.txt")
            .is_err()
    );
    assert_eq!(registry.budget.available_permits(), CACHE_KIB);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn shared_download_stream_survives_either_window_closing_until_the_last_binding() {
    for close_old_first in [true, false] {
        let fixture = fixture("[shared](outputs/a.txt)");
        let bytes = vec![b's'; CHUNK * 3];
        std::fs::write(fixture.root.join("outputs/a.txt"), &bytes).unwrap();
        let newer = BindingGuard::new(
            fixture.registry.clone(),
            "token",
            &fixture.thread,
            &fixture.state,
        )
        .unwrap();
        let file = descriptor(&fixture, "outputs/a.txt");
        let selection = file.selection();
        let response = fixture.registry.lock().unwrap().insert(file).unwrap();
        let id = response["file"]["id"].as_str().unwrap();
        let registry = fixture.registry.clone();
        let file = registry
            .lock()
            .unwrap()
            .cached("token", &fixture.thread, id)
            .unwrap();
        let snapshot = prepare_snapshot(&registry, &file, || Ok(())).unwrap();
        let mut stream = (0..snapshot.size).step_by(CHUNK).map(|offset| {
            if !registry.lock().unwrap().current(&selection) {
                return Err("disconnected".to_string());
            }
            snapshot.read(offset)
        });
        assert_eq!(stream.next().unwrap().unwrap(), bytes[..CHUNK]);
        let (first, last) = if close_old_first {
            (fixture.binding, newer)
        } else {
            (newer, fixture.binding)
        };
        drop(first);
        assert!(registry.lock().unwrap().current(&selection));
        assert!(
            registry
                .lock()
                .unwrap()
                .cached("token", &fixture.thread, id)
                .is_ok()
        );
        assert_eq!(stream.next().unwrap().unwrap(), bytes[CHUNK..CHUNK * 2]);
        drop(last);
        assert!(!registry.lock().unwrap().current(&selection));
        assert!(
            registry
                .lock()
                .unwrap()
                .cached("token", &fixture.thread, id)
                .is_err()
        );
        assert_eq!(stream.next().unwrap().unwrap_err(), "disconnected");
        drop(stream);
        drop(snapshot);
        assert_eq!(
            registry.lock().unwrap().budget.available_permits(),
            CACHE_KIB
        );
    }
}

#[test]
fn multiple_connection_leases_remain_token_and_thread_scoped_and_revoked_guards_cannot_rebind() {
    let fixture = fixture("[file](outputs/a.txt)");
    std::fs::write(fixture.root.join("outputs/a.txt"), b"safe").unwrap();
    let selection = select(&fixture, "outputs/a.txt");
    let file = descriptor(&fixture, "outputs/a.txt");
    let response = fixture.registry.lock().unwrap().insert(file).unwrap();
    let id = response["file"]["id"].as_str().unwrap();
    let newer = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &fixture.state,
    )
    .unwrap();
    let _other_token = BindingGuard::new(
        fixture.registry.clone(),
        "other",
        &fixture.thread,
        &fixture.state,
    )
    .unwrap();
    let mut other_state = fixture.state.clone();
    let other_thread = uuid::Uuid::new_v4().to_string();
    other_state["id"] = json!(other_thread);
    let _other_thread = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &other_thread,
        &other_state,
    )
    .unwrap();
    {
        let mut registry = fixture.registry.lock().unwrap();
        assert!(registry.cached("other", &fixture.thread, id).is_err());
        assert!(registry.cached("token", &other_thread, id).is_err());
        assert!(
            registry
                .selection("other", &other_thread, "outputs/a.txt")
                .is_err()
        );
        assert!(registry.current(&selection));
        registry.prune(|token| token != "token");
        assert!(!registry.current(&selection));
        assert!(registry.cached("token", &fixture.thread, id).is_err());
        assert!(
            registry
                .selection("token", &other_thread, "outputs/a.txt")
                .is_err()
        );
        assert!(
            registry
                .selection("other", &fixture.thread, "outputs/a.txt")
                .is_ok()
        );
    }
    fixture.binding.update(&fixture.state).unwrap();
    newer.update(&fixture.state).unwrap();
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/a.txt")
            .is_err()
    );
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("other", &fixture.thread, "outputs/a.txt")
            .is_ok()
    );
}

#[test]
fn multiple_connections_reject_conflicting_roots_or_authority_and_real_updates_expire_old_references()
 {
    let mut fixture = fixture("[file](outputs/a.txt)");
    std::fs::write(fixture.root.join("outputs/a.txt"), b"safe").unwrap();
    let selection = select(&fixture, "outputs/a.txt");
    let file = descriptor(&fixture, "outputs/a.txt");
    let response = fixture.registry.lock().unwrap().insert(file).unwrap();
    let id = response["file"]["id"].as_str().unwrap();
    let mut conflict = fixture.state.clone();
    conflict["cwd"] = json!(fixture.root.join("other-workspace"));
    let newer = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &conflict,
    )
    .unwrap();
    assert!(!fixture.registry.lock().unwrap().current(&selection));
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/a.txt")
            .is_err()
    );
    drop(newer);
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .cached("token", &fixture.thread, id)
            .is_err()
    );
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/a.txt")
            .is_ok()
    );
    fixture.binding.update(&conflict).unwrap();
    assert!(!fixture.registry.lock().unwrap().current(&selection));
    let path = fixture.root.parent().unwrap().join("user-selected.bin");
    std::fs::write(&path, b"user").unwrap();
    fixture.state["turns"][0]["items"] =
        json!([{"id":"same-item","type":"userMessage","content":[{"type":"file","path":path}]}]);
    fixture.binding.update(&fixture.state).unwrap();
    let view = super::super::protocol::view(&fixture.state);
    let reference = view["turns"][0]["messages"][0]["attachments"][0]["id"]
        .as_str()
        .unwrap();
    let native = select(&fixture, reference);
    conflict = fixture.state.clone();
    conflict["turns"][0]["items"][0]["type"] = json!("agentMessage");
    let newer = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &conflict,
    )
    .unwrap();
    assert!(!fixture.registry.lock().unwrap().current(&native));
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, reference)
            .is_err()
    );
    drop(newer);
    assert!(fixture.registry.lock().unwrap().current(&native));
    fixture.state["turns"][0]["items"] = json!([]);
    fixture.binding.update(&fixture.state).unwrap();
    assert!(!fixture.registry.lock().unwrap().current(&native));
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, reference)
            .is_err()
    );
}

#[test]
fn size_mutation_and_cancellation_are_bounded_and_never_write_sources() {
    let fixture = fixture("[large](outputs/large.bin) [small](outputs/small.bin)");
    let path = fixture.root.join("outputs/large.bin");
    File::create(&path)
        .unwrap()
        .set_len(fs::MAX_FILE + 1)
        .unwrap();
    assert!(
        describe(select(&fixture, "outputs/large.bin"), || Ok(()))
            .err()
            .unwrap()
            .contains("200 MiB")
    );
    assert_eq!(
        fixture.registry.lock().unwrap().budget.available_permits(),
        CACHE_KIB
    );
    let path = fixture.root.join("outputs/small.bin");
    let bytes = vec![b'x'; CHUNK * 3];
    std::fs::write(&path, &bytes).unwrap();
    let descriptor = descriptor(&fixture, "outputs/small.bin");
    let mut checks = 0;
    assert!(
        prepare_snapshot(&fixture.registry, &descriptor, || {
            checks += 1;
            if checks >= 3 {
                Err("cancel".into())
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert_eq!(
        fixture.registry.lock().unwrap().budget.available_permits(),
        CACHE_KIB
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let mut source = File::open(&path).unwrap();
    let expected = fs::stamp(&source).unwrap();
    let mut checks = 0;
    assert!(
        fs::snapshot(&mut source, &expected, || {
            checks += 1;
            if checks == 2 {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"changed")
                    .unwrap();
            }
            Ok(())
        })
        .is_err()
    );
}

#[cfg(any(unix, windows))]
#[test]
fn symlink_file_and_parent_are_never_followed() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    let fixture = fixture("[file](outputs/link) [parent](outputs/directory/file.txt)");
    let outside = fixture.root.parent().unwrap().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("file.txt"), b"secret").unwrap();
    #[cfg(unix)]
    {
        symlink(outside.join("file.txt"), fixture.root.join("outputs/link")).unwrap();
        symlink(&outside, fixture.root.join("outputs/directory")).unwrap();
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(
            outside.join("file.txt"),
            fixture.root.join("outputs/link"),
        )
        .unwrap();
        std::os::windows::fs::symlink_dir(&outside, fixture.root.join("outputs/directory"))
            .unwrap();
    }
    assert!(describe(select(&fixture, "outputs/link"), || Ok(())).is_err());
    assert!(describe(select(&fixture, "outputs/directory/file.txt"), || Ok(())).is_err());
    assert_eq!(std::fs::read(outside.join("file.txt")).unwrap(), b"secret");
}

#[test]
fn absolute_workspace_links_image_metadata_ttl_and_native_identity_are_stable() {
    let mut fixture = fixture("");
    let path = fixture.root.join("outputs/image.png");
    std::fs::write(&path, b"\x89PNG\r\n\x1a\nimage").unwrap();
    fixture.state["turns"][0]["items"][0]["text"] =
        json!(format!("![image](<{}>)", path.display()));
    fixture.binding.update(&fixture.state).unwrap();
    let mut file = descriptor(&fixture, path.to_str().unwrap());
    assert_eq!(file.mime, "image/png");
    assert_eq!(
        fixture.registry.lock().unwrap().budget.available_permits(),
        CACHE_KIB
    );
    let attachment = file.attachment(&"a".repeat(64));
    assert_eq!(attachment["url"], attachment["downloadUrl"]);
    file.created = Instant::now() - LIFETIME;
    let expired = fixture.registry.lock().unwrap().insert(file).unwrap();
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .cached(
                "token",
                &fixture.thread,
                expired["file"]["id"].as_str().unwrap()
            )
            .is_err()
    );
    assert_eq!(
        reference_id(&json!("t"), &json!("item"), 0, 0, "private"),
        reference_id(&json!("t"), &json!("item"), 7, 0, "private")
    );
    fixture.state["turns"][0]["items"][0]["text"] = json!(format!(
        "{}[hidden](outputs/hidden.txt)",
        "x".repeat(super::super::protocol::MAX_PUBLIC_TEXT)
    ));
    fixture.binding.update(&fixture.state).unwrap();
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/hidden.txt")
            .is_err()
    );
}

#[cfg(any(unix, windows))]
#[test]
fn protected_real_targets_are_denied_even_below_a_parent_workspace_and_unresolved_roots_fail_closed()
 {
    let mut fixture = fixture("[secret](custom-config/auth.json) [safe](outputs/safe.txt)");
    let protected = fixture.root.join("custom-config");
    std::fs::create_dir(&protected).unwrap();
    std::fs::write(protected.join("auth.json"), b"SYNTHETIC_PRIVATE_CONFIG").unwrap();
    std::fs::write(fixture.root.join("outputs/safe.txt"), b"safe").unwrap();
    let alias = fixture.root.parent().unwrap().join("configured-home-alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&protected, &alias).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&protected, &alias).unwrap();
    fixture.registry.lock().unwrap().protection = Protection {
        home: None,
        roots: vec![alias.clone()],
    };
    assert!(describe(select(&fixture, "custom-config/auth.json"), || Ok(())).is_err());
    assert!(describe(select(&fixture, "outputs/safe.txt"), || Ok(())).is_ok());
    fixture.state["cwd"] = json!(protected);
    fixture.state["turns"][0]["items"][0]["text"] = json!("[secret](auth.json)");
    fixture.binding.update(&fixture.state).unwrap();
    assert!(describe(select(&fixture, "auth.json"), || Ok(())).is_err());
    fixture.state["cwd"] = json!(fixture.root);
    fixture.state["turns"][0]["items"][0]["text"] = json!("[safe](outputs/safe.txt)");
    fixture.binding.update(&fixture.state).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&alias).unwrap();
    #[cfg(windows)]
    std::fs::remove_dir(&alias).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(fixture.root.join("missing-target"), &alias).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(fixture.root.join("missing-target"), &alias).unwrap();
    assert!(
        describe(select(&fixture, "outputs/safe.txt"), || Ok(()))
            .err()
            .unwrap()
            .contains("真实目标")
    );
    assert_eq!(
        std::fs::read(protected.join("auth.json")).unwrap(),
        b"SYNTHETIC_PRIVATE_CONFIG"
    );
}

#[test]
fn cache_budget_route_filename_and_mime_are_explicit_and_bounded() {
    let fixture = fixture("[safe](outputs/a.txt)");
    std::fs::write(fixture.root.join("outputs/a.txt"), b"<svg onload='evil'>").unwrap();
    let file = descriptor(&fixture, "outputs/a.txt");
    assert_eq!(file.mime, "application/octet-stream");
    let response = fixture.registry.lock().unwrap().insert(file).unwrap();
    assert!(response["file"].get("url").is_none());
    let mut registry = fixture.registry.lock().unwrap();
    let first = registry.budget(fs::MAX_FILE).unwrap();
    let second = registry.budget(fs::MAX_FILE).unwrap();
    assert!(registry.budget(1).is_err());
    drop((first, second));
    assert_eq!(registry.budget.available_permits(), CACHE_KIB);
    registry.prune(|_| false);
    assert!(registry.bindings.is_empty());
    assert!(registry.descriptors.is_empty());
    let header = disposition("坏\";x\r\nInjected: yes.txt");
    assert!(!header.contains(['\r', '\n']));
    assert!(header.contains("filename*=UTF-8''"));
    assert!(header.contains("%0D%0A"));
    assert_eq!(fs::image_mime(b"\x89PNG\r\n\x1a\n"), Some("image/png"));
    assert!(public_image_url(&json!({"url":"secret"})).is_none());
    assert!(public_image_url(&json!("data:image/svg+xml;base64,PHN2Zz4=")).is_none());
    assert!(public_image_url(&json!("https://user:password@evil.invalid/a.png")).is_none());
    assert!(
        download_route(&format!(
            "/api/sessions/{}/files/{}",
            fixture.thread,
            "f".repeat(64)
        ))
        .is_some()
    );
    assert!(
        download_route(&format!(
            "/api/sessions/{}/files/{}",
            fixture.thread,
            "F".repeat(64)
        ))
        .is_none()
    );
    assert!(download_route("/api/sessions/../files/anything").is_none());
}
