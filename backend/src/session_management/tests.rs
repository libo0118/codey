use super::*;
use rusqlite::{Connection, params};
use serde_json::json;
use tempfile::TempDir;

pub(super) const A: &str = "11111111-1111-4111-8111-111111111111";
pub(super) const B: &str = "22222222-2222-4222-8222-222222222222";
pub(super) const C: &str = "33333333-3333-4333-8333-333333333333";

pub(super) fn rollout(home: &Path, id: &str, archived: bool) -> std::path::PathBuf {
    let folder = home.join(if archived {
        "archived_sessions/2026/10/17"
    } else {
        "sessions/2026/10/17"
    });
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join(format!("rollout-test-{id}.jsonl"));
    fs::write(
        &path,
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta", "payload":{"id":id,"cwd":"/test/project"}}),
            json!({"type":"response_item", "payload":{"text":format!("private-{id}")}})
        ),
    )
    .unwrap();
    path
}

pub(super) fn fixture() -> TempDir {
    let home = tempfile::tempdir().unwrap();
    rollout(home.path(), A, false);
    rollout(home.path(), B, true);
    let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, cwd TEXT, updated_at INTEGER, archived INTEGER);
        CREATE TABLE thread_attachments (thread_id TEXT REFERENCES threads(id), payload TEXT);
        CREATE TABLE thread_spawn_edges (parent_thread_id TEXT, child_thread_id TEXT);
        CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT);
        INSERT INTO projects VALUES ('project', 'keep project');").unwrap();
    for (id, archive) in [(A, 0), (B, 1)] {
        db.execute(
            "INSERT INTO threads VALUES (?1, ?2, '/test/project', 1000, ?3)",
            params![id, format!("title-{id}"), archive],
        )
        .unwrap();
        db.execute(
            "INSERT INTO thread_attachments VALUES (?1, 'keep-or-delete')",
            [id],
        )
        .unwrap();
    }
    db.execute("INSERT INTO thread_spawn_edges VALUES (?1, ?2)", [A, B])
        .unwrap();
    for name in [
        "thread_history_1.sqlite",
        "goals_1.sqlite",
        "queue_1.sqlite",
        "memories_1.sqlite",
    ] {
        let db = Connection::open(home.path().join(name)).unwrap();
        db.execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE records (thread_id TEXT, content TEXT);",
        )
        .unwrap();
        for id in [A, B] {
            db.execute(
                "INSERT INTO records VALUES (?1, ?2)",
                [id, &format!("sensitive-{id}")],
            )
            .unwrap();
        }
    }
    fs::create_dir(home.path().join("sqlite")).unwrap();
    let db = Connection::open(home.path().join("sqlite/codex-dev.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE local_thread_catalog (host_id TEXT, thread_id TEXT, display_title TEXT);
        CREATE TABLE thread_timeline_ledger (host_id TEXT, thread_id TEXT, payload TEXT);",
    )
    .unwrap();
    for (host, id) in [("local", A), ("local", B), ("remote", A)] {
        db.execute(
            "INSERT INTO local_thread_catalog VALUES (?1, ?2, 'catalog title')",
            [host, id],
        )
        .unwrap();
        db.execute(
            "INSERT INTO thread_timeline_ledger VALUES (?1, ?2, 'timeline')",
            [host, id],
        )
        .unwrap();
    }
    let lines = [A, B]
        .map(|id| {
            format!(
                "{}\n",
                json!({"id":id, "thread_name":format!("index-{id}")})
            )
        })
        .concat();
    fs::write(home.path().join("session_index.jsonl"), &lines).unwrap();
    let backup = home.path().join("backups_state/provider-sync/old");
    fs::create_dir_all(&backup).unwrap();
    fs::write(backup.join("session_index.jsonl"), lines).unwrap();
    fs::write(
        home.path().join("history.jsonl"),
        [A, B]
            .map(|id| format!("{}\n", json!({"session_id":id, "text":"history"})))
            .concat(),
    )
    .unwrap();
    let global = json!({"theme":"dark", "thread-titles":{"titles":{A:"remove",B:"keep"}, "order":[A,B]}, "pinned-thread-ids":[A,B], "drafts":{A:"private draft",B:"keep draft"}});
    for name in [".codex-global-state.json", ".codex-global-state.json.bak"] {
        fs::write(home.path().join(name), global.to_string()).unwrap();
    }
    fs::write(home.path().join("config.toml"), "keep = true").unwrap();
    fs::write(home.path().join("auth.json"), "fixture-only").unwrap();
    home
}

fn request(home: &Path, ids: &[&str]) -> DeleteRequest {
    DeleteRequest {
        ids: ids.iter().map(|id| (*id).into()).collect(),
        revision: list(home).unwrap().revision,
        confirmed: true,
    }
}

fn count(path: &Path, table: &str, column: &str, id: &str) -> usize {
    Connection::open(path)
        .unwrap()
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE {column}=?1"),
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn lists_archived_and_active_without_exposing_contents() {
    let home = fixture();
    let inventory = list(home.path()).unwrap();
    assert_eq!(inventory.sessions.len(), 2);
    assert!(
        !inventory
            .sessions
            .iter()
            .find(|value| value.id == A)
            .unwrap()
            .archived
    );
    assert!(
        inventory
            .sessions
            .iter()
            .find(|value| value.id == B)
            .unwrap()
            .archived
    );
    assert!(
        !serde_json::to_string(&inventory)
            .unwrap()
            .contains("sensitive")
    );
    assert_eq!(inventory.revision, list(home.path()).unwrap().revision);
}

#[test]
fn selected_cleanup_removes_records_archives_indexes_and_wal_without_touching_other_data() {
    let home = fixture();
    let selected = rollout(home.path(), B, true);
    let before = fs::read(&selected).unwrap();
    let result = remove(home.path(), request(home.path(), &[B])).unwrap();
    assert_eq!(result.selected_count, 1);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.restart_required);
    assert_eq!(
        fs::read(&selected).unwrap(),
        before,
        "安排清理时不能删除活跃文件"
    );
    assert_eq!(
        count(&home.path().join("state_5.sqlite"), "threads", "id", B),
        1
    );
    finish_pending(home.path()).unwrap();
    let inventory = list(home.path()).unwrap();
    assert_eq!(
        inventory
            .sessions
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        [A]
    );
    for name in [
        "thread_history_1.sqlite",
        "goals_1.sqlite",
        "queue_1.sqlite",
        "memories_1.sqlite",
    ] {
        assert_eq!(count(&home.path().join(name), "records", "thread_id", B), 0);
        assert_eq!(count(&home.path().join(name), "records", "thread_id", A), 1);
        assert!(
            !String::from_utf8_lossy(&fs::read(home.path().join(name)).unwrap())
                .contains(&format!("sensitive-{B}"))
        );
    }
    for name in [
        "session_index.jsonl",
        "history.jsonl",
        "backups_state/provider-sync/old/session_index.jsonl",
        ".codex-global-state.json",
        ".codex-global-state.json.bak",
    ] {
        let text = fs::read_to_string(home.path().join(name)).unwrap();
        assert!(!text.contains(B), "{name}");
        assert!(text.contains(A), "{name}");
    }
    assert_eq!(
        fs::read_to_string(home.path().join("config.toml")).unwrap(),
        "keep = true"
    );
    assert_eq!(
        fs::read_to_string(home.path().join("auth.json")).unwrap(),
        "fixture-only"
    );
    assert_eq!(
        count(
            &home.path().join("state_5.sqlite"),
            "projects",
            "id",
            "project"
        ),
        1
    );
    assert!(!home.path().join(PENDING).exists());
}

#[test]
fn selected_cleanup_allows_activity_in_unselected_sessions() {
    use std::io::Write;

    let home = fixture();
    let path = rollout(home.path(), A, false);
    let request = request(home.path(), &[B]);
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{}",
        json!({"type":"response_item", "payload":{"text":"new activity"}})
    )
    .unwrap();
    let contents = fs::read(&path).unwrap();
    let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
    db.execute("UPDATE threads SET updated_at=2000 WHERE id=?1", [A])
        .unwrap();
    drop(db);

    let result = remove(home.path(), request).unwrap();
    assert_eq!(result.selected_count, 1);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    finish_pending(home.path()).unwrap();
    assert_eq!(fs::read(&path).unwrap(), contents);
    let inventory = list(home.path()).unwrap();
    assert_eq!(inventory.sessions.len(), 1);
    assert_eq!(inventory.sessions[0].id, A);
    assert_eq!(inventory.pending_count, 0);
}

#[test]
fn inventory_scope_changes_require_confirmation_before_recording_intent() {
    for change in ["added", "removed", "archived"] {
        let home = tempfile::tempdir().unwrap();
        let active = rollout(home.path(), A, false);
        let selected = rollout(home.path(), B, false);
        let request = request(home.path(), &[B]);
        match change {
            "added" => {
                rollout(home.path(), C, false);
            }
            "removed" => fs::remove_file(active).unwrap(),
            _ => {
                let archived = rollout(home.path(), A, true);
                fs::rename(active, archived).unwrap();
            }
        }

        let error = remove(home.path(), request).unwrap_err();
        assert!(
            error.to_string().contains("会话列表已变化"),
            "{change}: {error}"
        );
        assert!(!home.path().join(PENDING).exists(), "{change}");
        assert!(selected.exists(), "{change}");
    }
}

#[test]
fn restart_replays_stale_writes_but_preserves_new_sessions_and_remote_rows() {
    let home = fixture();
    let result = remove(home.path(), request(home.path(), &[A, B])).unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    rollout(home.path(), A, true);
    rollout(home.path(), C, false);
    let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
    db.execute(
        "INSERT OR REPLACE INTO threads VALUES (?1, 'stale title', '', 1000, 0)",
        [A],
    )
    .unwrap();
    drop(db);
    finish_pending(home.path()).unwrap();
    assert_eq!(
        list(home.path())
            .unwrap()
            .sessions
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        [C]
    );
    // 相同 ID 的远程目录副本仍属于远程宿主。
    assert_eq!(
        count(
            &home.path().join("sqlite/codex-dev.db"),
            "local_thread_catalog",
            "thread_id",
            A
        ),
        1
    );
    finish_pending(home.path()).unwrap();
}

#[test]
fn validates_confirmation_selection_and_revision_before_recording_intent() {
    let home = fixture();
    for mode in ["unconfirmed", "empty", "invalid", "unknown", "stale"] {
        let mut value = request(home.path(), &[A]);
        match mode {
            "unconfirmed" => value.confirmed = false,
            "empty" => value.ids.clear(),
            "invalid" => value.ids = vec!["../../auth.json".into()],
            "unknown" => value.ids = vec![C.into()],
            _ => value.revision = "old".into(),
        }
        assert!(remove(home.path(), value).is_err(), "{mode}");
        assert!(!home.path().join(PENDING).exists());
        assert_eq!(list(home.path()).unwrap().sessions.len(), 2);
    }
}

#[test]
fn partial_failure_keeps_retry_and_does_not_claim_completion() {
    let home = fixture();
    let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER block_delete BEFORE DELETE ON threads BEGIN SELECT RAISE(ABORT, 'fixture locked'); END;").unwrap();
    let result = remove(home.path(), request(home.path(), &[A])).unwrap();
    assert!(result.errors.is_empty(), "安排阶段不应触发数据库删除");
    assert_eq!(
        count(&home.path().join("state_5.sqlite"), "threads", "id", A),
        1
    );
    assert!(home.path().join(PENDING).exists());
    assert!(finish_pending(home.path()).is_err());
    assert!(home.path().join(PENDING).exists());
    db.execute_batch("DROP TRIGGER block_delete").unwrap();
    drop(db);
    finish_pending(home.path()).unwrap();
    assert_eq!(list(home.path()).unwrap().pending_count, 0);
}

#[test]
fn duplicate_selection_and_missing_home_are_safe() {
    let home = fixture();
    assert_eq!(
        remove(home.path(), request(home.path(), &[A, A]))
            .unwrap()
            .selected_count,
        1
    );
    finish_pending(home.path()).unwrap();
    let absent = home.path().join("missing");
    assert!(list(&absent).unwrap().sessions.is_empty());
    finish_pending(&absent).unwrap();
    assert!(!absent.exists());
}

#[test]
fn corrupted_database_or_pending_manifest_is_not_silently_skipped() {
    let home = fixture();
    fs::write(home.path().join("queue_9.sqlite"), b"invalid database").unwrap();
    assert!(list(home.path()).is_err());
    fs::remove_file(home.path().join("queue_9.sqlite")).unwrap();
    fs::write(home.path().join(PENDING), "{broken").unwrap();
    assert!(finish_pending(home.path()).is_err());
    assert!(home.path().join(PENDING).exists());
}

#[test]
fn damaged_rollout_with_known_filename_and_orphan_history_can_be_cleaned() {
    let home = tempfile::tempdir().unwrap();
    let path = rollout(home.path(), A, true);
    fs::write(path, "partial write").unwrap();
    fs::write(
        home.path().join("history.jsonl"),
        format!("\n{}\n", json!({"session_id":B,"text":"orphan"})),
    )
    .unwrap();
    fs::create_dir(home.path().join("shell_snapshots")).unwrap();
    fs::write(
        home.path().join(format!("shell_snapshots/{C}.ps1")),
        "snapshot",
    )
    .unwrap();
    assert_eq!(list(home.path()).unwrap().sessions.len(), 3);
    assert!(
        remove(home.path(), request(home.path(), &[A, B, C]))
            .unwrap()
            .errors
            .is_empty()
    );
    finish_pending(home.path()).unwrap();
    assert!(list(home.path()).unwrap().sessions.is_empty());
}

#[test]
fn suffixed_rollouts_keep_the_original_identity_during_listing_and_cleanup() {
    for archived in [false, true] {
        for damaged in [false, true] {
            let home = fixture();
            let original = rollout(home.path(), A, false);
            let folder = if archived {
                home.path().join("archived_sessions/2026/10/17")
            } else {
                original.parent().unwrap().to_path_buf()
            };
            let path = folder.join(format!("rollout-2026-10-18T16-20-49-{A}_{B}.jsonl"));
            fs::rename(original, &path).unwrap();
            if damaged {
                fs::write(&path, "partial write").unwrap();
            }

            let inventory = list(home.path()).unwrap();
            assert_eq!(inventory.sessions.len(), 2);
            let session = inventory
                .sessions
                .iter()
                .find(|entry| entry.id == A)
                .unwrap();
            assert_eq!(session.archived, archived);
            assert_eq!(session.bytes, fs::metadata(&path).unwrap().len());

            let result = remove(home.path(), request(home.path(), &[B])).unwrap();
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            finish_pending(home.path()).unwrap();
            assert!(path.exists(), "附加 UUID 不应决定会话文件归属");

            let result = remove(home.path(), request(home.path(), &[A])).unwrap();
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            finish_pending(home.path()).unwrap();
            assert!(!path.exists());
            assert!(list(home.path()).unwrap().sessions.is_empty());
        }
    }
}

#[test]
fn conflicting_rollout_identity_is_rejected_before_mutation() {
    for suffix in ["", B, C] {
        let home = tempfile::tempdir().unwrap();
        let mut path = rollout(home.path(), A, false);
        if !suffix.is_empty() {
            let renamed = path.with_file_name(format!("rollout-test-{A}_{suffix}.jsonl"));
            fs::rename(&path, &renamed).unwrap();
            path = renamed;
        }
        fs::write(
            &path,
            json!({"type":"session_meta", "payload":{"id":B}}).to_string(),
        )
        .unwrap();
        assert!(list(home.path()).is_err());
        assert!(path.exists());
        assert!(!home.path().join(PENDING).exists());
    }
}

#[test]
fn database_rollout_outside_the_owned_directories_is_rejected() {
    let home = fixture();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("private.jsonl");
    fs::write(&file, "keep outside").unwrap();
    let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
    db.execute_batch("ALTER TABLE threads ADD COLUMN rollout_path TEXT")
        .unwrap();
    db.execute(
        "UPDATE threads SET rollout_path=?1 WHERE id=?2",
        params![file.to_str().unwrap(), A],
    )
    .unwrap();
    assert!(list(home.path()).is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "keep outside");
    assert!(!home.path().join(PENDING).exists());
}

#[test]
fn database_accepts_canonical_rollout_paths() {
    let home = fixture();
    let path = rollout(home.path(), A, false).canonicalize().unwrap();
    let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
    db.execute_batch("ALTER TABLE threads ADD COLUMN rollout_path TEXT")
        .unwrap();
    db.execute(
        "UPDATE threads SET rollout_path=?1 WHERE id=?2",
        params![path.to_str().unwrap(), A],
    )
    .unwrap();
    drop(db);
    assert!(
        remove(home.path(), request(home.path(), &[A]))
            .unwrap()
            .errors
            .is_empty()
    );
    finish_pending(home.path()).unwrap();
    assert!(!path.exists());
    assert_eq!(list(home.path()).unwrap().sessions.len(), 1);
}

#[test]
fn startup_cleans_message_tombstones_and_global_state_temporary_copies() {
    let home = fixture();
    let tombstones = home.path().join(".codey-message-delete-tombstones-v1");
    fs::create_dir(&tombstones).unwrap();
    for id in [A, B] {
        fs::write(
            tombstones.join(format!("{id}.json")),
            json!({"session_id":id,"message_id":"fixture"}).to_string(),
        )
        .unwrap();
    }
    let temporary = home.path().join("..codex-global-state.json.tmp-test");
    fs::write(&temporary, "partial global cache").unwrap();
    remove(home.path(), request(home.path(), &[A])).unwrap();
    assert!(tombstones.join(format!("{A}.json")).exists());
    assert!(tombstones.join(format!("{B}.json")).exists());
    assert!(temporary.exists());
    finish_pending(home.path()).unwrap();
    assert!(!tombstones.join(format!("{A}.json")).exists());
    assert!(tombstones.join(format!("{B}.json")).exists());
    assert!(!temporary.exists());
}

#[test]
fn malformed_pending_intent_cannot_delete_existing_session_files() {
    for intent in [
        json!({"version":99,"ids":[A]}),
        json!({"version":1,"ids":["../../auth.json"]}),
        json!({"version":2,"ids":[],"automatic":{A:{"updated_at":1,"policy_revision":"a".repeat(64),"phase":"unknown"}}}),
    ] {
        let home = fixture();
        let selected = rollout(home.path(), A, false);
        let before = fs::read(&selected).unwrap();
        let marker = home.path().join(PENDING);
        fs::write(&marker, intent.to_string()).unwrap();
        assert!(finish_pending(home.path()).is_err());
        assert!(
            remove(
                home.path(),
                DeleteRequest {
                    ids: vec![A.into()],
                    revision: "unsupported".into(),
                    confirmed: true,
                },
            )
            .is_err()
        );
        assert_eq!(fs::read(selected).unwrap(), before);
        assert_eq!(fs::read_to_string(marker).unwrap(), intent.to_string());
    }
}

#[test]
fn legacy_pending_requires_current_revision_and_manual_reconfirmation() {
    let home = fixture();
    let path = rollout(home.path(), A, false);
    let marker = home.path().join(PENDING);
    let legacy = json!({"version":1,"ids":[A,B]}).to_string();
    fs::write(&marker, &legacy).unwrap();
    let inventory = list(home.path()).unwrap();
    assert_eq!(inventory.pending_count, 0);
    assert_eq!(inventory.legacy_pending_ids, [A, B]);
    let public = serde_json::to_value(&inventory).unwrap();
    assert_eq!(public["pendingCount"], 0);
    assert_eq!(public["legacyPendingIds"], json!([A, B]));
    assert!(
        finish_pending(home.path())
            .unwrap_err()
            .to_string()
            .contains("重新确认")
    );
    assert!(path.exists());
    assert_eq!(fs::read_to_string(&marker).unwrap(), legacy);
    let mut unconfirmed = request(home.path(), &[A]);
    unconfirmed.confirmed = false;
    assert!(remove(home.path(), unconfirmed).is_err());
    let mut stale = request(home.path(), &[A]);
    stale.revision = "stale".into();
    assert!(remove(home.path(), stale).is_err());
    assert_eq!(fs::read_to_string(&marker).unwrap(), legacy);
    let result = remove(home.path(), request(home.path(), &[A])).unwrap();
    assert_eq!(result.selected_count, 1);
    assert_eq!(result.scheduled_count, 1);
    assert_eq!(result.cleared_legacy_count, 0);
    assert!(result.restart_required);
    assert_eq!(read_pending(home.path()).unwrap().version, 2);
    assert_eq!(list(home.path()).unwrap().pending_count, 1);
    assert_eq!(list(home.path()).unwrap().legacy_pending_ids, [B]);
    assert!(finish_pending(home.path()).is_err());
    assert!(!path.exists(), "重新确认的手动目标不应被其他旧任务阻断");
    assert!(
        list(home.path())
            .unwrap()
            .sessions
            .iter()
            .any(|entry| entry.id == B)
    );
    remove(home.path(), request(home.path(), &[B])).unwrap();
    finish_pending(home.path()).unwrap();
    assert!(!path.exists());
    assert!(!marker.exists());
}

#[test]
fn missing_legacy_confirmation_only_clears_marker_and_never_schedules_future_files() {
    for other_pending in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let other = rollout(home.path(), B, false);
        fs::write(
            home.path().join(PENDING),
            json!({"version":1,"ids":[A]}).to_string(),
        )
        .unwrap();
        if other_pending {
            remove(home.path(), request(home.path(), &[B])).unwrap();
        }
        let before = fs::read(&other).unwrap();
        let state = home.path().join(".codex-global-state.json");
        let state_bytes = json!({"thread_ids":[A]}).to_string().into_bytes();
        fs::write(&state, &state_bytes).unwrap();
        let result = remove(home.path(), request(home.path(), &[A, A])).unwrap();
        assert_eq!(result.selected_count, 1);
        assert_eq!(result.scheduled_count, 0);
        assert_eq!(result.cleared_legacy_count, 1);
        assert_eq!(result.restart_required, other_pending);
        assert_eq!(fs::read(&other).unwrap(), before);
        assert_eq!(fs::read(&state).unwrap(), state_bytes);
        let pending = read_pending(home.path()).unwrap();
        assert!(!pending.ids.contains(A));
        assert!(!pending.automatic.contains_key(A));
        assert!(pending.legacy_ids.is_empty());
        let future = rollout(home.path(), A, false);
        let future_bytes = fs::read(&future).unwrap();
        finish_pending(home.path()).unwrap();
        assert_eq!(fs::read(future).unwrap(), future_bytes);
        assert_eq!(fs::read(state).unwrap(), state_bytes);
        assert_eq!(other.exists(), !other_pending);
        assert!(!home.path().join(PENDING).exists());
    }
}

#[test]
fn legacy_unknown_or_stale_selection_preserves_all_markers_and_data() {
    let home = tempfile::tempdir().unwrap();
    let path = rollout(home.path(), A, false);
    let before = fs::read(&path).unwrap();
    let marker = home.path().join(PENDING);
    let legacy = json!({"version":1,"ids":[A]}).to_string();
    fs::write(&marker, &legacy).unwrap();
    assert!(remove(home.path(), request(home.path(), &[A, C])).is_err());
    assert_eq!(fs::read_to_string(&marker).unwrap(), legacy);
    let stale = request(home.path(), &[A]);
    let changed = json!({"version":1,"ids":[A,B]}).to_string();
    fs::write(&marker, &changed).unwrap();
    assert_ne!(stale.revision, list(home.path()).unwrap().revision);
    assert!(remove(home.path(), stale).is_err());
    assert_eq!(fs::read_to_string(marker).unwrap(), changed);
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn inventory_revision_binds_every_pending_source_and_automatic_phase() {
    let home = fixture();
    let marker = home.path().join(PENDING);
    let original = json!({
        "version":2, "ids":[A], "legacy_ids":[C],
        "automatic":{B:{"updated_at":1,"policy_revision":"a".repeat(64),"phase":"queued"}},
    });
    for field in [
        "ids",
        "legacy_ids",
        "automatic",
        "updated_at",
        "policy_revision",
        "phase",
    ] {
        fs::write(&marker, original.to_string()).unwrap();
        let stale = request(home.path(), &[A]);
        let mut changed = original.clone();
        match field {
            "ids" | "legacy_ids" => changed[field] = json!([]),
            "automatic" => changed[field] = json!({}),
            "updated_at" => changed["automatic"][B][field] = json!(2),
            "policy_revision" => changed["automatic"][B][field] = json!("b".repeat(64)),
            _ => changed["automatic"][B][field] = json!("started"),
        }
        let bytes = changed.to_string();
        fs::write(&marker, &bytes).unwrap();
        assert_ne!(
            stale.revision,
            list(home.path()).unwrap().revision,
            "{field}"
        );
        assert!(remove(home.path(), stale).is_err(), "{field}");
        assert_eq!(fs::read_to_string(&marker).unwrap(), bytes, "{field}");
        assert_eq!(list(home.path()).unwrap().sessions.len(), 2);
    }
}

#[test]
fn whole_session_cleanup_precedes_existing_message_delete_replay() {
    let home = tempfile::tempdir().unwrap();
    let selected = rollout(home.path(), A, false);
    let tombstones = home.path().join(".codey-message-delete-tombstones-v1");
    fs::create_dir(&tombstones).unwrap();
    let tombstone = tombstones.join("selected.json");
    fs::write(
        &tombstone,
        json!({"version":1,"sessionId":A,"messageId":"old-message"}).to_string(),
    )
    .unwrap();
    remove(home.path(), request(home.path(), &[A])).unwrap();
    assert!(selected.exists());
    assert!(tombstone.exists());
    finish_pending(home.path()).unwrap();
    let replay = crate::message_delete::reapply_persisted_deletions(home.path()).unwrap();
    assert!(replay.failures.is_empty());
    assert!(!selected.exists());
    assert!(!tombstone.exists());
}

#[test]
fn refuses_linked_session_directory() {
    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = rollout(outside.path(), A, false);
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        outside.path().join("sessions"),
        home.path().join("sessions"),
    )
    .unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(
        outside.path().join("sessions"),
        home.path().join("sessions"),
    )
    .unwrap();
    assert!(list(home.path()).is_err());
    assert!(file.exists());
}
