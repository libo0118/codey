use super::*;
use serde_json::json;
use std::io::{Read, Write};

fn provenance(revision: u64) -> super::super::desktop::NativeProvenance {
    super::super::desktop::NativeProvenance {
        owner: "synthetic-owner".into(),
        revision: Some(revision),
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    thread: String,
    registry: Arc<Mutex<Registry>>,
    binding: BindingGuard,
    state: Value,
    revision: u64,
}

impl Fixture {
    fn update(&mut self) {
        self.revision += 1;
        self.binding
            .update(&self.state, &provenance(self.revision))
            .unwrap();
    }
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
    let binding =
        BindingGuard::new(registry.clone(), "token", &thread, &state, &provenance(1)).unwrap();
    Fixture {
        _temp: temp,
        root,
        thread,
        registry,
        binding,
        state,
        revision: 1,
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

fn compact_view(state: &Value) -> Value {
    use super::super::projection::{MessageWindow, Projection, PublicState, StreamVersion};
    match Projection::default().public_state(
        state,
        StreamVersion::V2,
        &mut MessageWindow::default(),
    ) {
        PublicState::V2(state) => state,
        PublicState::Legacy(_) => panic!("expected v2 projection"),
    }
}

fn inline_fixture(item: Value) -> Fixture {
    let mut fixture = fixture("");
    fixture.state["cwd"] = Value::Null;
    fixture.state["turns"][0]["items"] = json!([item]);
    fixture.update();
    fixture
}

#[test]
fn public_inline_images_keep_legacy_urls_and_resolve_v2_ids_without_copying_or_snapshotting() {
    for (mime, bytes) in [
        ("image/png", b"\x89PNG\r\n\x1a\nsynthetic png".as_slice()),
        ("image/jpeg", b"\xff\xd8\xffsynthetic jpeg".as_slice()),
        ("image/gif", b"GIF89asynthetic gif".as_slice()),
        ("image/webp", b"RIFF1234WEBPsynthetic webp".as_slice()),
    ] {
        let encoded = STANDARD.encode(bytes);
        let url = format!("data:{mime};base64,{encoded}");
        for item in [
            json!({"id":"user","type":"userMessage","content":[{"type":"file","path":"outputs/not-opened.bin"},{"type":"image","url":url}]}),
            json!({"id":"tool","type":"mcpToolCall","result":{"content":[{"type":"image","mimeType":mime,"data":encoded}]}}),
            json!({"id":123,"type":"dynamicToolCall","contentItems":[{"type":"image","mimeType":mime,"data":encoded}]}),
        ] {
            let fixture = inline_fixture(item);
            let legacy = super::super::protocol::view(&fixture.state);
            let attachments = legacy["turns"][0]["messages"][0]["attachments"]
                .as_array()
                .unwrap();
            let slot = attachments.len() - 1;
            assert_eq!(attachments[slot]["url"], url);
            let compact = compact_view(&fixture.state);
            let attachment = &compact["turns"][0]["messages"][0]["attachments"][slot];
            assert!(attachment.get("url").is_none());
            let id = attachment["id"].as_str().unwrap();
            assert_eq!(
                id,
                inline_image_reference_id(
                    &legacy["turns"][0]["id"],
                    &legacy["turns"][0]["messages"][0]["id"],
                    slot,
                    &url
                )
            );
            let selected = select(&fixture, id);
            assert!(selected_path(&selected).is_err());
            let references::Reference::InlineImage { url: source, .. } = &selected.value else {
                panic!("not an inline image")
            };
            let source = Arc::clone(source);
            let file = describe(selected, || Ok(())).unwrap();
            let references::Reference::InlineImage { url: described, .. } = &file.selection.value
            else {
                panic!("not an inline image")
            };
            assert!(Arc::ptr_eq(&source, described));
            assert!(matches!(&file.source, DescriptorSource::InlineImage));
            assert_eq!(file.size, bytes.len() as u64);
            assert_eq!(file.mime, mime);
            assert_eq!(
                fixture.registry.lock().unwrap().budget.available_permits(),
                CACHE_KIB
            );
            let resolved = fixture.registry.lock().unwrap().insert(file).unwrap();
            assert_eq!(resolved["file"]["url"], resolved["file"]["downloadUrl"]);
            assert!(!resolved.to_string().contains("data:"));
            assert!(!resolved.to_string().contains("workspace"));
            let file = fixture
                .registry
                .lock()
                .unwrap()
                .cached(
                    "token",
                    &fixture.thread,
                    resolved["file"]["id"].as_str().unwrap(),
                )
                .unwrap();
            let snapshot = prepare_snapshot(&fixture.registry, &file, || Ok(())).unwrap();
            assert_eq!(snapshot.read(0).unwrap(), bytes);
            assert_eq!(snapshot.digest, crate::fs_util::sha256_hex(bytes));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    snapshot.file.metadata().unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
}

#[test]
fn inline_image_identity_uses_public_turn_message_fallbacks_and_absolute_attachment_slots() {
    let url = "data:image/gif;base64,R0lGODlh";
    for turn_id in [
        Value::Null,
        json!(15),
        json!({"private":"not a public id"}),
        json!("turn"),
    ] {
        for item in [
            json!({"type":"userMessage","content":[{"type":"file","path":"outputs/a.bin"},{"type":"image","url":url}]}),
            json!({"id":{"private":"not a public id"},"type":"steeringUserMessage","input":[{"type":"image","url":url}]}),
        ] {
            let mut fixture = inline_fixture(item);
            fixture.state["turns"][0]["turnId"] = turn_id.clone();
            fixture.update();
            let compact = compact_view(&fixture.state);
            let attachments = compact["turns"][0]["messages"][0]["attachments"]
                .as_array()
                .unwrap();
            let id = attachments.last().unwrap()["id"].as_str().unwrap();
            assert_eq!(snapshot(&fixture, id).read(0).unwrap(), b"GIF89a");
        }
    }
    let mut fixture = fixture("");
    fixture.state["cwd"] = Value::Null;
    fixture.state["turns"][0]["items"] = json!([]);
    fixture.state["turns"][0]["params"]["input"] = json!([{"type":"image","url":url}]);
    fixture.update();
    let compact = compact_view(&fixture.state);
    let id = compact["turns"][0]["messages"][0]["attachments"][0]["id"]
        .as_str()
        .unwrap();
    assert_eq!(snapshot(&fixture, id).read(0).unwrap(), b"GIF89a");
    assert_ne!(
        inline_image_reference_id(&json!(1), &json!("m"), 0, url),
        inline_image_reference_id(&json!(2), &json!("m"), 0, url)
    );
    assert_ne!(
        inline_image_reference_id(&json!("t"), &json!("m"), 0, url),
        inline_image_reference_id(&json!("t"), &json!("other"), 0, url)
    );
}

#[test]
fn inline_metadata_preserves_standard_base64_magic_and_the_existing_encoded_size_limit() {
    for suffix in 0..72 {
        let bytes = [b"GIF89a".as_slice(), &vec![0xff; suffix]].concat();
        let encoded = STANDARD.encode(&bytes);
        for encoded in [
            encoded.clone(),
            encoded.trim_end_matches('=').into(),
            format!("{encoded}\n"),
            encoded.replace('/', "_"),
            format!("{}!", &encoded[..encoded.len() - 1]),
        ] {
            let url = format!("data:image/gif;base64,{encoded}");
            let accepted = STANDARD
                .decode(&encoded)
                .is_ok_and(|bytes| fs::image_mime(&bytes) == Some("image/gif"));
            assert_eq!(
                public_image_url(&json!(url)).is_some(),
                accepted,
                "{encoded}"
            );
            if accepted {
                assert_eq!(
                    inline_image_metadata(&url).unwrap().size,
                    bytes.len() as u64
                );
            }
        }
    }
    for url in [
        "data:image/gif;base64,R0lGODlhZh==", // nonzero unused padding bits
        "data:image/gif;base64,R0l=ODlh",
        "data:image/gif;base64,R0lGODlh====",
        "data:image/gif;base64,",
        "data:image/png;base64,R0lGODlh",
        "data:image/svg+xml;base64,PHN2Zz4=",
        "data:text/html;base64,PGh0bWw+",
        "data:image/gif;BASE64,R0lGODlh",
    ] {
        assert!(inline_image_metadata(url).is_err(), "{url}");
        assert!(public_image_url(&json!(url)).is_none(), "{url}");
        let fixture = inline_fixture(
            json!({"id":"user","type":"userMessage","content":[{"type":"image","url":url}]}),
        );
        assert!(
            fixture
                .registry
                .lock()
                .unwrap()
                .selection(
                    "token",
                    &fixture.thread,
                    &inline_image_reference_id(&json!("turn"), &json!("user"), 0, url)
                )
                .is_err()
        );
    }
    assert!(fs::snapshot_inline_image("R0lGODlh", 6, "image/png", || Ok(())).is_err());
    assert!(fs::snapshot_inline_image("R0lGODlh", 7, "image/gif", || Ok(())).is_err());
    assert!(fs::snapshot_inline_image("R0lGODlh!", 6, "image/gif", || Ok(())).is_err());
    // Keep the Native encoded-length cap, including its existing final quartet rounding.
    let mut bytes = vec![0; MAX_INLINE_IMAGE_ENCODED / 4 * 3];
    bytes[..6].copy_from_slice(b"GIF89a");
    let url = format!("data:image/gif;base64,{}", STANDARD.encode(&bytes));
    assert_eq!(
        inline_image_metadata(&url).unwrap().size,
        bytes.len() as u64
    );
    let fixture = inline_fixture(
        json!({"id":"user","type":"userMessage","content":[{"type":"image","url":url}]}),
    );
    let compact = compact_view(&fixture.state);
    let id = compact["turns"][0]["messages"][0]["attachments"][0]["id"]
        .as_str()
        .unwrap();
    assert!(url.len() > 4096);
    assert_eq!(descriptor(&fixture, id).size, bytes.len() as u64);
    bytes.push(0);
    let oversized = format!("data:image/gif;base64,{}", STANDARD.encode(bytes));
    assert!(
        inline_image_metadata(&oversized)
            .err()
            .unwrap()
            .contains("上限")
    );
    assert!(public_image_url(&json!(oversized)).is_none());
}

#[test]
fn only_projected_image_attachments_authorize_inline_bytes_never_metadata_markdown_or_urls() {
    let url = "data:image/gif;base64,R0lGODlh";
    for item in [
        json!({"id":"hidden","type":"futurePrivateItem","content":[{"type":"image","url":url}]}),
        json!({"id":"hidden","type":"mcpToolCall","arguments":{"content":[{"type":"image","mimeType":"image/gif","data":"R0lGODlh"}]}}),
        json!({"id":"hidden","type":"mcpToolCall","result":{"_meta":{"content":[{"type":"image","mimeType":"image/gif","data":"R0lGODlh"}]}}}),
        json!({"id":"hidden","type":"mcpToolCall","result":{"structuredContent":{"type":"image","mimeType":"image/gif","data":"R0lGODlh"}}}),
        json!({"id":"hidden","type":"reasoning","content":[{"type":"image","url":url}]}),
        json!({"id":"hidden","type":"collabAgentToolCall","tool":"wait","receiverThreads":[{"threadId":"hidden-child","thread":{"content":[{"type":"image","url":url}]}}]}),
        json!({"id":"hidden","type":"agentMessage","text":format!("![not a native attachment]({url})")}),
        json!({"id":"hidden","type":"userMessage","content":[{"type":"file","url":url}]}),
        json!({"id":"hidden","type":"userMessage","content":[{"type":"localImage","url":url}]}),
        json!({"id":"hidden","type":"userMessage","content":[{"type":"image","url":"https://example.invalid/no-proxy.gif"}]}),
    ] {
        let fixture = inline_fixture(item);
        let id = inline_image_reference_id(&json!("turn"), &json!("hidden"), 0, url);
        let registry = fixture.registry.lock().unwrap();
        assert!(registry.selection("token", &fixture.thread, &id).is_err());
        assert!(
            registry
                .selection(
                    "token",
                    &fixture.thread,
                    "https://example.invalid/no-proxy.gif"
                )
                .is_err()
        );
        if let Ok(selected) = registry.selection("token", &fixture.thread, url) {
            assert!(describe(selected, || Ok(())).is_err());
        }
    }
}

#[test]
fn inline_image_bindings_replacement_cancellation_disconnect_and_revocation_remain_scoped() {
    let mut bytes = vec![b'x'; CHUNK * 3];
    bytes[..6].copy_from_slice(b"GIF89a");
    let url = format!("data:image/gif;base64,{}", STANDARD.encode(&bytes));
    for close_old_first in [true, false] {
        let fixture = inline_fixture(
            json!({"id":"user","type":"userMessage","content":[{"type":"image","url":url}]}),
        );
        let reference = inline_image_reference_id(&json!("turn"), &json!("user"), 0, &url);
        let newer = BindingGuard::new(
            fixture.registry.clone(),
            "token",
            &fixture.thread,
            &fixture.state,
            &provenance(fixture.revision),
        )
        .unwrap();
        let file = descriptor(&fixture, &reference);
        let selection = file.selection();
        let response = fixture.registry.lock().unwrap().insert(file).unwrap();
        let id = response["file"]["id"].as_str().unwrap();
        let file = fixture
            .registry
            .lock()
            .unwrap()
            .cached("token", &fixture.thread, id)
            .unwrap();
        assert!(
            fixture
                .registry
                .lock()
                .unwrap()
                .selection("other-token", &fixture.thread, &reference)
                .is_err()
        );
        assert!(
            fixture
                .registry
                .lock()
                .unwrap()
                .selection("token", &uuid::Uuid::new_v4().to_string(), &reference)
                .is_err()
        );
        let budget = fixture
            .registry
            .lock()
            .unwrap()
            .budget((CACHE_KIB * 1024) as u64)
            .unwrap();
        assert!(
            prepare_snapshot(&fixture.registry, &file, || Ok(()))
                .err()
                .unwrap()
                .contains("预算")
        );
        drop(budget);
        assert!(
            fixture
                .registry
                .lock()
                .unwrap()
                .cached("other-token", &fixture.thread, id)
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
        let mut checks = 0;
        assert!(
            prepare_snapshot(&fixture.registry, &file, || {
                checks += 1;
                if checks == 4 {
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
        let download = prepare_snapshot(&fixture.registry, &file, || Ok(())).unwrap();
        assert_eq!(download.digest, crate::fs_util::sha256_hex(&bytes));
        assert_eq!(download.read(0).unwrap(), bytes[..CHUNK]);
        let (first, last) = if close_old_first {
            (fixture.binding, newer)
        } else {
            (newer, fixture.binding)
        };
        drop(first);
        assert!(fixture.registry.lock().unwrap().current(&selection));
        assert!(
            fixture
                .registry
                .lock()
                .unwrap()
                .cached("token", &fixture.thread, id)
                .is_ok()
        );
        drop(last);
        assert!(!fixture.registry.lock().unwrap().current(&selection));
        assert!(
            fixture
                .registry
                .lock()
                .unwrap()
                .cached("token", &fixture.thread, id)
                .is_err()
        );
        drop(download);
        assert_eq!(
            fixture.registry.lock().unwrap().budget.available_permits(),
            CACHE_KIB
        );
    }
    let mut fixture = inline_fixture(
        json!({"id":"user","type":"userMessage","content":[{"type":"image","url":url}]}),
    );
    let reference = inline_image_reference_id(&json!("turn"), &json!("user"), 0, &url);
    let file = descriptor(&fixture, &reference);
    let selection = file.selection();
    let old = prepare_snapshot(&fixture.registry, &file, || Ok(())).unwrap();
    fixture.state["turns"][0]["items"][0]["content"][0]["url"] =
        json!("data:image/gif;base64,R0lGODdh");
    fixture.update();
    assert!(!fixture.registry.lock().unwrap().current(&selection));
    assert!(
        prepare_snapshot(&fixture.registry, &file, || fixture
            .registry
            .lock()
            .unwrap()
            .current(&selection)
            .then_some(())
            .ok_or("replaced".into()))
        .is_err()
    );
    assert_eq!(old.read(0).unwrap(), bytes[..CHUNK]);
    let compact = compact_view(&fixture.state);
    let current_id = compact["turns"][0]["messages"][0]["attachments"][0]["id"]
        .as_str()
        .unwrap();
    let current = select(&fixture, current_id);
    let newer = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &fixture.state,
        &provenance(fixture.revision),
    )
    .unwrap();
    fixture.registry.lock().unwrap().prune(|_| false);
    assert!(!fixture.registry.lock().unwrap().current(&current));
    fixture.update();
    newer
        .update(&fixture.state, &provenance(fixture.revision))
        .unwrap();
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, current_id)
            .is_err()
    );
}

#[tokio::test]
async fn synthetic_inline_images_use_the_authenticated_resolve_and_get_routes() {
    use super::super::{
        actions::Ledger,
        auth::{Auth, Credentials},
        server::{self, Core},
    };
    use reqwest::StatusCode;
    let bytes = b"GIF89asynthetic route image\0bytes";
    let data = STANDARD.encode(bytes);
    let url = format!("data:image/gif;base64,{data}");
    let mut fixture = fixture("");
    fixture.state["cwd"] = Value::Null;
    fixture.state["turns"][0]["items"] = json!([
        {"id":"user","type":"userMessage","content":[{"type":"image","url":url}]},
        {"id":"tool","type":"mcpToolCall","result":{"content":[{"type":"image","mimeType":"image/gif","data":data}]}}
    ]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut auth = Auth::load(fixture._temp.path().join("synthetic-remote.json")).unwrap();
    auth.configure(
        "",
        "admin",
        Some(Credentials::new("admin", "synthetic-password-1234").unwrap()),
        address.port(),
        String::new(),
    )
    .unwrap();
    let revision = auth.revision().to_string();
    let login = auth.issue(&revision, false).unwrap();
    let other = auth.issue(&revision, false).unwrap();
    let cookie = format!("codey_remote={}", login.token);
    let core = Arc::new(Core {
        auth: Arc::new(Mutex::new(auth)),
        state: std::sync::Weak::new(),
        public_url: std::sync::RwLock::new(None),
        address,
        actions: tokio::sync::Mutex::new(Ledger::default()),
        shutdown: tokio::sync::watch::channel(false).0,
        streams: Arc::new(Semaphore::new(8)),
        logins: Arc::new(Semaphore::new(2)),
        files: fixture.registry.clone(),
        file_transfers: Arc::new(Semaphore::new(2)),
    });
    let binding = BindingGuard::new(
        fixture.registry.clone(),
        &login.token,
        &fixture.thread,
        &fixture.state,
        &provenance(fixture.revision),
    )
    .unwrap();
    let task = tokio::spawn(server::serve(listener, core.clone()));
    let base = format!("http://{address}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let compact = compact_view(&fixture.state);
    let mut last_url = String::new();
    for message in compact["turns"][0]["messages"].as_array().unwrap() {
        let reference = message["attachments"][0]["id"].as_str().unwrap();
        let resolve = || {
            client
                .post(format!("{base}/remote/files/resolve"))
                .header("origin", &base)
                .header("cookie", &cookie)
                .json(&json!({"threadId":fixture.thread,"reference":reference}))
        };
        assert_eq!(
            resolve().send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let reply = resolve()
            .header("x-csrf-token", login.session["csrfToken"].as_str().unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        let resolved: Value = reply.json().await.unwrap();
        assert_eq!(resolved["file"]["size"], bytes.len());
        assert_eq!(resolved["file"]["mime"], "image/gif");
        assert!(!resolved.to_string().contains("data:"));
        assert!(!resolved.to_string().contains("workspace"));
        assert_eq!(
            fixture.registry.lock().unwrap().budget.available_permits(),
            CACHE_KIB
        );
        assert_eq!(core.file_transfers.available_permits(), 2);
        last_url = format!(
            "{base}{}",
            resolved["file"]["downloadUrl"].as_str().unwrap()
        );
        assert_eq!(
            client.get(&last_url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(&last_url)
                .header("cookie", format!("codey_remote={}", other.token))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            client
                .get(last_url.replace(&fixture.thread, &uuid::Uuid::new_v4().to_string()))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            client
                .get(&last_url)
                .header("cookie", &cookie)
                .header("origin", "https://evil.invalid")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let reply = client
            .get(&last_url)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        assert_eq!(reply.headers()["content-type"], "image/gif");
        assert_eq!(reply.headers()["content-length"], bytes.len().to_string());
        assert_eq!(
            reply.headers()["etag"],
            format!("\"sha256-{}\"", crate::fs_util::sha256_hex(bytes))
        );
        assert_eq!(reply.headers()["cache-control"], "no-store");
        assert_eq!(reply.headers()["x-content-type-options"], "nosniff");
        assert_eq!(reply.bytes().await.unwrap().as_ref(), bytes);
    }
    assert!(core.state.upgrade().is_none());
    drop(binding);
    assert_eq!(
        client
            .get(&last_url)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    core.auth.lock().unwrap().revoke("*").unwrap();
    assert_eq!(
        client
            .get(&last_url)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    core.shutdown.send_replace(true);
    drop(client);
    task.await.unwrap();
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
        fixture.update();
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
    fixture.update();
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
    fixture.update();
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
            fixture.update();
            let view = super::super::protocol::view(&fixture.state);
            let attachments = view["turns"][0]["messages"][0]["attachments"]
                .as_array()
                .unwrap();
            assert_eq!(attachments.len(), paths.len());
            for (index, attachment) in attachments.iter().enumerate() {
                let selected = select(&fixture, attachment["id"].as_str().unwrap());
                assert!(matches!(
                    &selected.value,
                    references::Reference::File { native: true, .. }
                ));
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
    let parent = fixture.root.parent().unwrap().to_path_buf();
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
    fixture.update();
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
    fixture.update();
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
    let references::Reference::File { value, .. } = &mut selected.value else {
        panic!("expected file reference")
    };
    *value = "outputs/file.txt".into();
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
    fixture.update();
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
fn newer_native_revocations_survive_stale_snapshots_and_the_newer_window_closing() {
    for inline in [false, true] {
        let mut fixture = fixture("[file](outputs/a.txt)");
        let mut bytes = vec![b'x'; CHUNK * 3];
        bytes[..6].copy_from_slice(b"GIF89a");
        std::fs::write(fixture.root.join("outputs/a.txt"), &bytes).unwrap();
        if inline {
            let url = format!("data:image/gif;base64,{}", STANDARD.encode(&bytes));
            fixture.state["turns"][0]["items"] =
                json!([{"id":"image","type":"userMessage","content":[{"type":"image","url":url}]}]);
            fixture.update();
        }
        let reference = if inline {
            compact_view(&fixture.state)["turns"][0]["messages"][0]["attachments"][0]["id"]
                .as_str()
                .unwrap()
                .to_string()
        } else {
            "outputs/a.txt".to_string()
        };
        let stale_state = fixture.state.clone();
        let stale_revision = fixture.revision;
        let stale = BindingGuard::new(
            fixture.registry.clone(),
            "token",
            &fixture.thread,
            &stale_state,
            &provenance(stale_revision),
        )
        .unwrap();
        let file = descriptor(&fixture, &reference);
        let selection = file.selection();
        let prepared = prepare_snapshot(&fixture.registry, &file, || Ok(())).unwrap();
        let registry = fixture.registry.clone();
        let mut transfer = (0..prepared.size).step_by(CHUNK).map(|offset| {
            if !registry.lock().unwrap().current(&selection) {
                return Err("revoked".to_string());
            }
            prepared.read(offset)
        });
        assert_eq!(transfer.next().unwrap().unwrap(), bytes[..CHUNK]);
        let response = fixture.registry.lock().unwrap().insert(file).unwrap();
        let id = response["file"]["id"].as_str().unwrap();
        let file = fixture
            .registry
            .lock()
            .unwrap()
            .cached("token", &fixture.thread, id)
            .unwrap();
        fixture.state["turns"][0]["items"] = json!([]);
        fixture.update();
        stale
            .update(&stale_state, &provenance(stale_revision))
            .unwrap();
        drop(fixture.binding);
        let registry = fixture.registry.lock().unwrap();
        assert!(!registry.current(&selection));
        assert!(registry.cached("token", &fixture.thread, id).is_err());
        assert!(
            registry
                .selection("token", &fixture.thread, &reference)
                .is_err()
        );
        drop(registry);
        assert_eq!(transfer.next().unwrap().unwrap_err(), "revoked");
        assert!(
            prepare_snapshot(&fixture.registry, &file, || {
                fixture
                    .registry
                    .lock()
                    .unwrap()
                    .current(&selection)
                    .then_some(())
                    .ok_or("revoked".into())
            })
            .is_err()
        );
        assert!(!fixture.registry.lock().unwrap().current(&selection));
        drop(transfer);
        stale
            .update(&stale_state, &provenance(stale_revision + 2))
            .unwrap();
        assert!(
            fixture
                .registry
                .lock()
                .unwrap()
                .selection("token", &fixture.thread, &reference)
                .is_ok()
        );
        drop(stale);
        assert!(fixture.registry.lock().unwrap().bindings.is_empty());
    }
}

#[test]
fn native_counter_reset_blocks_all_old_leases_until_the_group_reconnects() {
    let fixture = fixture("[file](outputs/a.txt)");
    std::fs::write(fixture.root.join("outputs/a.txt"), b"synthetic").unwrap();
    fixture
        .binding
        .update(&fixture.state, &provenance(10))
        .unwrap();
    let other = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &fixture.state,
        &provenance(10),
    )
    .unwrap();
    let file = descriptor(&fixture, "outputs/a.txt");
    let selection = file.selection();
    let response = fixture.registry.lock().unwrap().insert(file).unwrap();
    let id = response["file"]["id"].as_str().unwrap();
    fixture
        .binding
        .update(&fixture.state, &provenance(1))
        .unwrap();
    drop(fixture.binding);
    {
        let registry = fixture.registry.lock().unwrap();
        assert!(!registry.current(&selection));
        assert!(
            registry
                .selection("token", &fixture.thread, "outputs/a.txt")
                .unwrap_err()
                .contains("重置")
        );
        assert!(registry.cached("token", &fixture.thread, id).is_err());
    }
    other.update(&fixture.state, &provenance(11)).unwrap();
    let still_blocked = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &fixture.state,
        &provenance(12),
    )
    .unwrap();
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/a.txt")
            .is_err()
    );
    drop(still_blocked);
    drop(other);
    let reconnected = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &fixture.state,
        &provenance(1),
    )
    .unwrap();
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/a.txt")
            .is_ok()
    );
    let other_owner = NativeProvenance {
        owner: "different-native-owner".into(),
        revision: Some(100),
    };
    assert!(
        BindingGuard::new(
            fixture.registry.clone(),
            "token",
            &fixture.thread,
            &fixture.state,
            &other_owner
        )
        .is_err()
    );
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .selection("token", &fixture.thread, "outputs/a.txt")
            .unwrap_err()
            .contains("窗口已变化")
    );
    drop(reconnected);
    assert!(
        BindingGuard::new(
            fixture.registry.clone(),
            "token",
            &fixture.thread,
            &fixture.state,
            &other_owner
        )
        .is_ok()
    );
}

#[test]
fn unknown_native_file_revision_revokes_without_failing_the_chat_binding() {
    let fixture = fixture("[file](outputs/a.txt)");
    let selection = select(&fixture, "outputs/a.txt");
    let unknown = NativeProvenance {
        owner: "synthetic-owner".into(),
        revision: None,
    };
    fixture.binding.update(&fixture.state, &unknown).unwrap();
    let another = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &fixture.state,
        &unknown,
    )
    .unwrap();
    let registry = fixture.registry.lock().unwrap();
    assert!(!registry.current(&selection));
    assert!(
        registry
            .selection("token", &fixture.thread, "outputs/a.txt")
            .unwrap_err()
            .contains("版本未知")
    );
    drop(registry);
    fixture
        .binding
        .update(&fixture.state, &provenance(2))
        .unwrap();
    assert!(!fixture.registry.lock().unwrap().current(&selection));
    drop(another);
}

#[test]
fn native_authority_tracks_only_latest_or_live_scopes_and_does_not_resurrect_unknown_ones() {
    let mut fixture = fixture("[file](outputs/a.txt)");
    let old = fixture.state.clone();
    let selection = select(&fixture, "outputs/a.txt");
    let stale = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &old,
        &provenance(1),
    )
    .unwrap();
    for index in 0..8 {
        fixture.state["turns"][0]["items"] = json!([{"id":format!("new-{index}"),"type":"agentMessage","text":format!("[new](outputs/{index}.txt)")}]);
        fixture.update();
        let registry = fixture.registry.lock().unwrap();
        assert!(registry.bindings.values().next().unwrap().scopes.len() <= 2);
        assert!(!registry.current(&selection));
    }
    drop(stale);
    assert_eq!(
        fixture
            .registry
            .lock()
            .unwrap()
            .bindings
            .values()
            .next()
            .unwrap()
            .scopes
            .len(),
        1
    );
    fixture.state["turns"][0]["items"] = json!([]);
    fixture.update();
    assert!(
        fixture
            .registry
            .lock()
            .unwrap()
            .bindings
            .values()
            .next()
            .unwrap()
            .scopes
            .is_empty()
    );
    let late = BindingGuard::new(
        fixture.registry.clone(),
        "token",
        &fixture.thread,
        &old,
        &provenance(1),
    )
    .unwrap();
    assert!(!fixture.registry.lock().unwrap().current(&selection));
    late.update(&old, &provenance(fixture.revision)).unwrap();
    assert!(!fixture.registry.lock().unwrap().current(&selection));
    late.update(&old, &provenance(fixture.revision + 1))
        .unwrap();
    assert!(fixture.registry.lock().unwrap().current(&selection));
}

#[test]
fn native_history_gaps_preserve_unloaded_items_but_complete_history_can_revoke_them() {
    for canonical in [false, true] {
        let fixture = fixture("[file](outputs/a.txt)");
        let selected = select(&fixture, "outputs/a.txt");
        let mut partial = fixture.state.clone();
        partial["turns"][0]["items"] =
            json!([{"id":"recent","type":"agentMessage","text":"no file"}]);
        partial["turns"][0]["itemsPagination"] = json!({"hasLoadedOldest":false});
        if canonical {
            partial["turns"][0]["items"] = json!({"isComplete":false,"entitiesByKey":{"recent":{"id":"recent","type":"agentMessage","text":"no file"}},"islands":[{"entries":[{"value":"recent"}]}]});
        }
        let newer = BindingGuard::new(
            fixture.registry.clone(),
            "token",
            &fixture.thread,
            &partial,
            &provenance(2),
        )
        .unwrap();
        assert!(fixture.registry.lock().unwrap().current(&selected));
        partial["turns"][0]["itemsPagination"]["hasLoadedOldest"] = json!(true);
        if canonical {
            partial["turns"][0]["items"]["isComplete"] = json!(true);
        }
        newer.update(&partial, &provenance(3)).unwrap();
        assert!(!fixture.registry.lock().unwrap().current(&selected));
        newer.update(&fixture.state, &provenance(4)).unwrap();
        assert!(fixture.registry.lock().unwrap().current(&selected));
        partial["turns"] = json!([]);
        partial["turnsPagination"] = json!({"hasLoadedOldest":false});
        if canonical {
            partial["turnHistory"] = json!({"kind":"canonical","history":{"isComplete":false,"entitiesByKey":{},"islands":[]}});
        }
        newer.update(&partial, &provenance(5)).unwrap();
        assert!(fixture.registry.lock().unwrap().current(&selected));
        partial["turnsPagination"]["hasLoadedOldest"] = json!(true);
        if canonical {
            partial["turnHistory"]["history"]["isComplete"] = json!(true);
        }
        newer.update(&partial, &provenance(6)).unwrap();
        assert!(!fixture.registry.lock().unwrap().current(&selected));
    }
}

#[test]
fn same_session_windows_keep_old_downloads_and_independent_history_until_their_own_disconnect() {
    let mut fixture = fixture("[shared](outputs/shared.bin)");
    fixture.state["turnsPagination"] = json!({"hasLoadedOldest":false});
    fixture.state["turns"].as_array_mut().unwrap().push(json!({"turnId":"old","items":[{"id":"old-answer","type":"agentMessage","text":"[old](outputs/old.txt)"}]}));
    fixture.update();
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
    newer_state["turns"][1] = json!({"turnId":"new","items":[{"id":"new-answer","type":"agentMessage","text":"[new](outputs/new.txt)"}]});
    let mut newer = Some(
        BindingGuard::new(
            fixture.registry.clone(),
            "token",
            &fixture.thread,
            &newer_state,
            &provenance(fixture.revision + 1),
        )
        .unwrap(),
    );
    fixture.revision += 1;
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
        json!("[shared](outputs/shared.bin) [late](outputs/late.txt)");
    fixture.update();
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
            &provenance(fixture.revision),
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
    let mut fixture = fixture("[file](outputs/a.txt)");
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
        &provenance(fixture.revision),
    )
    .unwrap();
    let _other_token = BindingGuard::new(
        fixture.registry.clone(),
        "other",
        &fixture.thread,
        &fixture.state,
        &provenance(fixture.revision),
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
        &provenance(fixture.revision),
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
    fixture.update();
    newer
        .update(&fixture.state, &provenance(fixture.revision))
        .unwrap();
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
        &provenance(fixture.revision),
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
    fixture.update();
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
    fixture.revision += 1;
    fixture
        .binding
        .update(&conflict, &provenance(fixture.revision))
        .unwrap();
    assert!(!fixture.registry.lock().unwrap().current(&selection));
    let path = fixture.root.parent().unwrap().join("user-selected.bin");
    std::fs::write(&path, b"user").unwrap();
    fixture.state["turns"][0]["items"] =
        json!([{"id":"same-item","type":"userMessage","content":[{"type":"file","path":path}]}]);
    fixture.update();
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
        &provenance(fixture.revision),
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
    assert!(!fixture.registry.lock().unwrap().current(&native));
    fixture.update();
    assert!(fixture.registry.lock().unwrap().current(&native));
    fixture.state["turns"][0]["items"] = json!([]);
    fixture.update();
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
    fixture.update();
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
    fixture.update();
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
    fixture.update();
    assert!(describe(select(&fixture, "auth.json"), || Ok(())).is_err());
    fixture.state["cwd"] = json!(fixture.root);
    fixture.state["turns"][0]["items"][0]["text"] = json!("[safe](outputs/safe.txt)");
    fixture.update();
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
