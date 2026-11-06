use std::io::Write;

use super::*;

const SESSION: &str = "failed-spawn-transcript";
const RUNTIME: &str = "runtime-a";
const TASK: &str = "harmony_svg_capability";

fn record(at: u64, payload: Value) -> Value {
    json!({"timestamp": chrono::DateTime::from_timestamp_millis(at as i64).unwrap().to_rfc3339(), "type": "response_item", "payload": payload})
}

fn call(id: &str, at: u64) -> Value {
    record(
        at,
        json!({"type": "function_call", "namespace": "agents", "name": "spawn_agent", "call_id": id, "arguments": json!({"task_name": TASK}).to_string()}),
    )
}

fn response(id: &str, at: u64, output: Value) -> Value {
    record(
        at,
        json!({"type": "function_call_output", "call_id": id, "output": output}),
    )
}

fn failure(id: &str, at: u64) -> Value {
    response(
        id,
        at,
        json!("collab spawn failed: agent thread limit reached"),
    )
}

fn transcript(home: &Path, meta: Value, records: &[Value]) -> PathBuf {
    let path = home
        .join("sessions")
        .join(format!("rollout-probe-{SESSION}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!("{}\n", json!({"type": "session_meta", "payload": meta})),
    )
    .unwrap();
    append(&path, records);
    path
}

fn append(path: &Path, records: &[Value]) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    for value in records {
        writeln!(file, "{value}").unwrap();
    }
}

fn reserve(root: &Path, at: u64, call_id: Option<&str>) {
    write_test_runtime_policy(root);
    let mut spawn = input("PreToolUse", SESSION);
    spawn.turn_id = Some("root-turn-a".into());
    spawn.tool_name = Some("agents.spawn_agent".into());
    spawn.tool_use_id = call_id.map(str::to_string);
    spawn.tool_input = Some(
        json!({"task_name": TASK, "agent_type": "codey_quick_scan", "message": "Read the bounded source"}),
    );
    assert_eq!(
        handle_hook_for_runtime_at(&spawn, root, RUNTIME, at).unwrap(),
        json!({})
    );
}

fn trigger(root: &Path, path: &Path, event: &str) -> Value {
    let mut hook = input(event, SESSION);
    hook.transcript_path = Some(path.to_string_lossy().into_owned());
    hook.tool_name = Some("functions.exec_command".into());
    handle_hook_for_runtime_at(&hook, root, RUNTIME, 200).unwrap()
}

#[test]
fn failed_spawn_without_post_releases_on_next_root_hook() {
    for event in ["Stop", "PreToolUse", "UserPromptSubmit"] {
        for binding in [Some("spawn-1"), None] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(STATE_DIRECTORY);
            reserve(&root, 100, binding);
            let path = transcript(
                temp.path(),
                json!({"id": SESSION, "source": "vscode"}),
                &[call("spawn-1", 98), failure("spawn-1", 110)],
            );
            assert_eq!(trigger(&root, &path, event), json!({}));
            assert_eq!(
                active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
                0
            );
            let ledger_path = session_state_dir(&root, SESSION).join("orchestrator-ledger-v1.json");
            if event == "Stop" {
                assert!(!ledger_path.exists());
            } else {
                let ledger: Value =
                    serde_json::from_slice(&fs::read(ledger_path).unwrap()).unwrap();
                assert_eq!(ledger["reservations"][TASK]["outcome"], "failed");
                assert_eq!(ledger["updated_at_ms"], 200);
            }
        }
    }
}

#[test]
fn transcript_failure_requires_matching_root_identity_and_native_call() {
    let cases = [
        (
            json!({"id": "other", "source": "vscode"}),
            vec![call("a", 98), failure("a", 110)],
        ),
        (
            json!({"id": SESSION, "source": {"subagent": {}}}),
            vec![call("a", 98), failure("a", 110)],
        ),
        (
            json!({"id": SESSION, "source": "vscode", "parent_thread_id": "other"}),
            vec![call("a", 98), failure("a", 110)],
        ),
        (
            json!({"id": SESSION, "source": "vscode"}),
            vec![call("a", 98), failure("wrong-id", 110)],
        ),
        (
            json!({"id": SESSION, "source": "vscode"}),
            vec![call("a", 98), failure("a", 99)],
        ),
        (
            json!({"id": SESSION, "source": "vscode"}),
            vec![
                call("a", 98),
                response("a", 110, json!({"agent_id": "running-child"})),
            ],
        ),
        (
            json!({"id": SESSION, "source": "vscode"}),
            vec![call("a", 98)],
        ),
        (
            json!({"id": SESSION, "source": "vscode"}),
            vec![
                call("a", 98),
                record(
                    110,
                    json!({"type": "message", "call_id": "a", "content": "collab spawn failed: agent thread limit reached"}),
                ),
            ],
        ),
        (
            json!({"id": SESSION, "source": "vscode"}),
            vec![call("a", 98), call("a", 99), failure("a", 110)],
        ),
    ];
    for (meta, records) in cases {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(STATE_DIRECTORY);
        reserve(&root, 100, None);
        let path = transcript(temp.path(), meta.clone(), &records);
        assert_eq!(
            trigger(&root, &path, "Stop")["decision"],
            "block",
            "{meta}: {records:?}"
        );
        assert_eq!(
            active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
            1
        );
    }
}

#[test]
fn transcript_recovery_is_incremental_and_waits_for_complete_lines() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, Some("a"));
    let path = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "cli"}),
        &[call("a", 98)],
    );
    assert_eq!(trigger(&root, &path, "Stop")["decision"], "block");
    let cache = session_auxiliary_path(
        &session_state_dir(&root, SESSION),
        RUNTIME,
        "failed-spawn-transcript.json",
    );
    let before: Value = serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
    assert_eq!(before["offset"], fs::metadata(&path).unwrap().len());
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    write!(file, "{}", failure("a", 110)).unwrap();
    assert_eq!(trigger(&root, &path, "Stop")["decision"], "block");
    writeln!(file).unwrap();
    assert_eq!(trigger(&root, &path, "Stop"), json!({}));
}

#[test]
fn late_failure_from_old_same_name_call_cannot_settle_a_new_attempt() {
    for binding in [None, Some("new")] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(STATE_DIRECTORY);
        reserve(&root, 100, binding);
        let path = transcript(
            temp.path(),
            json!({"id": SESSION, "source": "vscode"}),
            &[call("old", 10), call("new", 98), failure("old", 110)],
        );
        assert_eq!(trigger(&root, &path, "Stop")["decision"], "block");
        append(&path, &[failure("new", 120)]);
        assert_eq!(
            trigger(&root, &path, "Stop") == json!({}),
            binding.is_some()
        );
    }
}

#[test]
fn bound_reservations_and_child_hooks_are_not_recovered() {
    for bound in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(STATE_DIRECTORY);
        reserve(&root, 100, Some("a"));
        let path = transcript(
            temp.path(),
            json!({"id": SESSION, "source": "vscode"}),
            &[call("a", 98), failure("a", 110)],
        );
        if bound {
            let mut post = input("PostToolUse", SESSION);
            post.tool_name = Some("agents.spawn_agent".into());
            post.tool_input = Some(json!({"task_name": TASK}));
            post.tool_response = Some(json!({"agent_id": "running-child"}));
            handle_hook_for_runtime_at(&post, &root, RUNTIME, 105).unwrap();
        } else {
            let mut child = input("Stop", SESSION);
            child.agent_id = Some("some-child".into());
            child.transcript_path = Some(path.to_string_lossy().into_owned());
            handle_hook_for_runtime_at(&child, &root, RUNTIME, 150).unwrap();
        }
        assert_eq!(
            active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
            1
        );
        if bound {
            assert_eq!(trigger(&root, &path, "Stop")["decision"], "block");
        }
    }
}

#[test]
fn fenced_failed_spawn_settles_without_disturbing_a_live_sibling_or_late_events() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, Some("failed-call"));

    let mut sibling = input("PreToolUse", SESSION);
    sibling.turn_id = Some("root-turn-a".into());
    sibling.tool_name = Some("agents.spawn_agent".into());
    sibling.tool_input = Some(json!({"task_name": "sibling", "message": "Work independently"}));
    assert_eq!(
        handle_hook_for_runtime_at(&sibling, &root, RUNTIME, 101).unwrap(),
        json!({})
    );
    sibling.hook_event_name = "PostToolUse".into();
    sibling.tool_response = Some(json!({"agent_id": "/root/sibling"}));
    handle_hook_for_runtime_at(&sibling, &root, RUNTIME, 102).unwrap();
    let mut started = input("SubagentStart", SESSION);
    started.agent_id = Some("/root/sibling".into());
    handle_hook_for_runtime_at(&started, &root, RUNTIME, 103).unwrap();

    let mut interrupt = input("PreToolUse", SESSION);
    interrupt.turn_id = Some("root-turn-a".into());
    interrupt.tool_name = Some("agents.interrupt_agent".into());
    interrupt.tool_input = Some(json!({"target": format!("/root/{TASK}")}));
    handle_hook_for_runtime_at(&interrupt, &root, RUNTIME, 105).unwrap();
    interrupt.hook_event_name = "PostToolUse".into();
    interrupt.tool_response = Some(json!({"isError": true, "error": "agent not found"}));
    handle_hook_for_runtime_at(&interrupt, &root, RUNTIME, 106).unwrap();
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        2
    );

    let path = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "vscode"}),
        &[call("failed-call", 98), failure("failed-call", 110)],
    );
    let mut list = input("PostToolUse", SESSION);
    list.transcript_path = Some(path.to_string_lossy().into_owned());
    list.tool_name = Some("agents.list_agents".into());
    list.tool_input = Some(json!({}));
    list.tool_response = Some(json!({"agents": [
        {"agent_name": "/root", "status": "running"},
        {"agent_name": "/root/sibling", "status": "running"}
    ]}));
    let result = handle_hook_for_runtime_at(&list, &root, RUNTIME, 200).unwrap();
    assert_eq!(result["decision"], "block");
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        1
    );
    let ledger_path = session_state_dir(&root, SESSION).join("orchestrator-ledger-v1.json");
    let ledger: Value = serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    assert_eq!(ledger["reservations"][TASK]["outcome"], "failed");
    assert!(ledger["reservations"][TASK]["fenced_at_ms"].is_number());
    assert!(ledger["reservations"][TASK]["agent_id_hash"].is_null());
    assert!(ledger["reservations"][TASK]["started_at_ms"].is_null());

    let mut late = input("PostToolUse", SESSION);
    late.tool_name = Some("agents.spawn_agent".into());
    late.tool_input = Some(json!({"task_name": TASK}));
    late.tool_response = Some(json!({"agent_id": format!("/root/{TASK}")}));
    handle_hook_for_runtime_at(&late, &root, RUNTIME, 201).unwrap();
    started.agent_id = Some(format!("/root/{TASK}"));
    handle_hook_for_runtime_at(&started, &root, RUNTIME, 202).unwrap();
    assert!(
        !agent_marker_path(
            &session_state_dir(&root, SESSION),
            RUNTIME,
            &format!("/root/{TASK}")
        )
        .exists()
    );
    let mut child_write = input("PreToolUse", SESSION);
    child_write.agent_id = started.agent_id.clone();
    child_write.agent_type = Some("codey_quick_scan".into());
    child_write.tool_name = Some("apply_patch".into());
    attest_test_child(&child_write, &root, RUNTIME);
    let denied = handle_hook_for_runtime_at(&child_write, &root, RUNTIME, 203).unwrap();
    assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        denied["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("已终态、过期或被 fence")
    );
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        1
    );

    let mut stopped = input("SubagentStop", SESSION);
    stopped.agent_id = Some("/root/sibling".into());
    handle_hook_for_runtime_at(&stopped, &root, RUNTIME, 204).unwrap();
    let mut command = input("PreToolUse", SESSION);
    command.transcript_path = Some(path.to_string_lossy().into_owned());
    command.tool_name = Some("apply_patch".into());
    assert_eq!(
        handle_hook_for_runtime_at(&command, &root, RUNTIME, 205).unwrap(),
        json!({})
    );
    command.hook_event_name = "Stop".into();
    assert_eq!(
        handle_hook_for_runtime_at(&command, &root, RUNTIME, 206).unwrap(),
        json!({})
    );
}

#[test]
fn bound_or_started_pending_reservations_are_not_replayed() {
    for field in ["agent_id_hash", "started_at_ms"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(STATE_DIRECTORY);
        reserve(&root, 100, Some("a"));
        let ledger_path = session_state_dir(&root, SESSION).join("orchestrator-ledger-v1.json");
        let mut ledger: Value = serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        ledger["reservations"][TASK][field] = if field == "agent_id_hash" {
            json!(hash_component("already-bound"))
        } else {
            json!(101)
        };
        fs::write(&ledger_path, ledger.to_string()).unwrap();
        let path = transcript(
            temp.path(),
            json!({"id": SESSION, "source": "vscode"}),
            &[call("a", 98), failure("a", 110)],
        );
        assert_eq!(
            trigger(&root, &path, "Stop")["decision"],
            "block",
            "{field}"
        );
        assert_eq!(
            active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
            1
        );
    }
}

#[test]
fn rejected_duplicate_spawn_does_not_replace_the_admitted_call_binding() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, Some("admitted"));
    let mut duplicate = input("PreToolUse", SESSION);
    duplicate.turn_id = Some("root-turn-a".into());
    duplicate.tool_name = Some("agents.spawn_agent".into());
    duplicate.tool_use_id = Some("rejected".into());
    duplicate.tool_input = Some(json!({"task_name": TASK, "message": "duplicate"}));
    let denied = handle_hook_for_runtime_at(&duplicate, &root, RUNTIME, 101).unwrap();
    assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny");
    let path = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "vscode"}),
        &[
            call("admitted", 98),
            call("rejected", 101),
            failure("rejected", 110),
        ],
    );
    assert_eq!(trigger(&root, &path, "Stop")["decision"], "block");
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        1
    );
    append(&path, &[failure("admitted", 120)]);
    assert_eq!(trigger(&root, &path, "Stop"), json!({}));
}

#[test]
fn reused_hook_call_id_cannot_create_a_second_reservation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, Some("same-id"));
    let mut spawn = input("PreToolUse", SESSION);
    spawn.turn_id = Some("root-turn-a".into());
    spawn.tool_name = Some("agents.spawn_agent".into());
    spawn.tool_use_id = Some("same-id".into());
    spawn.tool_input = Some(json!({"task_name": "other", "message": "work"}));
    assert!(handle_hook_for_runtime_at(&spawn, &root, RUNTIME, 101).is_err());
    let ledger: Value = serde_json::from_slice(
        &fs::read(session_state_dir(&root, SESSION).join("orchestrator-ledger-v1.json")).unwrap(),
    )
    .unwrap();
    assert!(ledger["reservations"].get("other").is_none());
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        1
    );
}

#[test]
fn stale_binding_after_stop_does_not_require_cross_turn_call_id_uniqueness() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, Some("reused-id"));
    let path = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "cli"}),
        &[call("reused-id", 98), failure("reused-id", 110)],
    );
    assert_eq!(trigger(&root, &path, "Stop"), json!({}));
    let mut next = input("PreToolUse", SESSION);
    next.turn_id = Some("next-turn".into());
    next.tool_name = Some("agents.spawn_agent".into());
    next.tool_use_id = Some("reused-id".into());
    next.tool_input = Some(json!({"task_name": "next", "message": "work"}));
    assert_eq!(
        handle_hook_for_runtime_at(&next, &root, RUNTIME, 201).unwrap(),
        json!({})
    );
    let cache = session_auxiliary_path(
        &session_state_dir(&root, SESSION),
        RUNTIME,
        "failed-spawn-transcript.json",
    );
    let saved: Value = serde_json::from_slice(&fs::read(cache).unwrap()).unwrap();
    assert!(saved["bindings"].get(TASK).is_none());
    assert!(saved["bindings"].get("next").is_some());
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        1
    );
}

#[test]
fn wrong_hook_call_id_does_not_use_the_unique_task_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, Some("expected"));
    let path = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "cli"}),
        &[call("different", 98), failure("different", 110)],
    );
    assert_eq!(trigger(&root, &path, "Stop")["decision"], "block");
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        1
    );
}

#[test]
fn admitted_call_without_id_removes_a_stale_same_timestamp_binding() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    let mut stale = input("PreToolUse", SESSION);
    stale.tool_use_id = Some("old".into());
    failed_spawn_recovery::note_spawn_call(&stale, &root, RUNTIME, TASK, &Default::default(), 100)
        .unwrap();
    reserve(&root, 100, None);
    let cache = session_auxiliary_path(
        &session_state_dir(&root, SESSION),
        RUNTIME,
        "failed-spawn-transcript.json",
    );
    let saved: Value = serde_json::from_slice(&fs::read(cache).unwrap()).unwrap();
    assert!(saved["bindings"].get(TASK).is_none());
    let path = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "vscode"}),
        &[call("old", 90), call("new", 98), failure("old", 110)],
    );
    assert_eq!(trigger(&root, &path, "Stop")["decision"], "block");
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        1
    );
}

#[test]
fn corrupt_records_and_unidentified_spawns_make_unique_task_fallback_ambiguous() {
    let mut bad_timestamp = call("bad-time", 97);
    bad_timestamp["timestamp"] = Value::Null;
    let mut missing_id = call("missing", 97);
    missing_id["payload"]
        .as_object_mut()
        .unwrap()
        .remove("call_id");
    let mut bad_args = call("bad-args", 97);
    bad_args["payload"]["arguments"] = json!("{broken");
    let mut conflicting_names = call("bad-alias", 97);
    conflicting_names["payload"]["arguments"] =
        json!(json!({"task_name": TASK, "taskName": "other"}).to_string());
    for bad in [
        Some(bad_timestamp),
        Some(missing_id),
        Some(bad_args),
        Some(conflicting_names),
        None,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(STATE_DIRECTORY);
        reserve(&root, 100, None);
        let path = transcript(temp.path(), json!({"id": SESSION, "source": "cli"}), &[]);
        if let Some(bad) = bad {
            append(&path, &[bad]);
        } else {
            writeln!(
                fs::OpenOptions::new().append(true).open(&path).unwrap(),
                "{{broken"
            )
            .unwrap();
        }
        append(&path, &[call("good", 98), failure("good", 110)]);
        assert_eq!(trigger(&root, &path, "Stop")["decision"], "block");
        assert_eq!(
            active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
            1
        );
    }
}

#[test]
fn matching_task_binding_conflicting_with_failure_is_not_replayed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, Some("a"));
    let path = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "vscode"}),
        &[
            call("a", 98),
            response(
                "a",
                110,
                json!({"task_name": format!("/root/{TASK}"), "isError": true}),
            ),
        ],
    );
    assert_eq!(trigger(&root, &path, "Stop")["decision"], "block");
    let ledger: Value = serde_json::from_slice(
        &fs::read(session_state_dir(&root, SESSION).join("orchestrator-ledger-v1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(ledger["reservations"][TASK]["state"], "pending");
    assert!(ledger["reservations"][TASK]["agent_id_hash"].is_null());
}

#[test]
fn resumed_root_suffix_and_consistent_task_name_alias_are_supported() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, None);
    let mut aliased = call("a", 98);
    aliased["payload"]["arguments"] =
        json!(json!({"taskName": TASK, "task_name": TASK}).to_string());
    let original = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "cli"}),
        &[aliased, failure("a", 110)],
    );
    let path = original.with_file_name(format!("rollout-probe-{SESSION}_resume-suffix.jsonl"));
    fs::rename(original, &path).unwrap();
    assert_eq!(trigger(&root, &path, "PreToolUse"), json!({}));
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        0
    );
}

#[test]
fn matching_root_metadata_outside_sessions_is_not_trusted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, Some("a"));
    let inside = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "vscode"}),
        &[call("a", 98), failure("a", 110)],
    );
    let outside = temp.path().join(format!("rollout-probe-{SESSION}.jsonl"));
    fs::copy(inside, &outside).unwrap();
    assert_eq!(trigger(&root, &outside, "Stop")["decision"], "block");
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        1
    );
}

#[test]
fn corrupt_cache_fails_closed_and_does_not_replace_the_original() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    reserve(&root, 100, Some("a"));
    let cache = session_auxiliary_path(
        &session_state_dir(&root, SESSION),
        RUNTIME,
        "failed-spawn-transcript.json",
    );
    fs::write(&cache, b"{broken").unwrap();
    let path = transcript(
        temp.path(),
        json!({"id": SESSION, "source": "vscode"}),
        &[call("a", 98), failure("a", 110)],
    );
    let mut hook = input("PreToolUse", SESSION);
    hook.transcript_path = Some(path.to_string_lossy().into_owned());
    hook.tool_name = Some("apply_patch".into());
    let error = handle_hook_for_runtime_at(&hook, &root, RUNTIME, 200).unwrap_err();
    assert_eq!(
        fail_closed_output(&hook, &error)["hookSpecificOutput"]["permissionDecision"],
        "deny"
    );
    assert_eq!(fs::read(cache).unwrap(), b"{broken");
    assert_eq!(
        active_agent_count_for_runtime(&root, RUNTIME, SESSION).unwrap(),
        1
    );
}

#[test]
fn reservation_is_not_written_when_pre_save_callback_fails() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    write_test_runtime_policy(&root);
    let called = std::cell::Cell::new(false);
    let fail = |_: &str, _: &std::collections::BTreeSet<String>| -> Result<()> {
        called.set(true);
        anyhow::bail!("synthetic cache save failure")
    };
    let result = crate::subagent_orchestrator::pre_spawn_with_workspace_and_turn(
        &root,
        RUNTIME,
        SESSION,
        Some(&json!({"task_name": TASK, "message": "work"})),
        crate::subagent_orchestrator::RootHookContext::new(Some("/repo"), 0, 100)
            .with_before_reservation_save(&fail),
    );
    assert!(
        called.get(),
        "admission must reach the injected storage failure"
    );
    assert!(result.is_err());
    assert_eq!(
        crate::subagent_orchestrator::active_reservation_count(&root, RUNTIME, SESSION, 101)
            .unwrap(),
        None
    );
}

#[cfg(windows)]
#[test]
fn cache_replace_failure_does_not_write_a_reservation() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(STATE_DIRECTORY);
    write_test_runtime_policy(&root);
    let cache = session_auxiliary_path(
        &session_state_dir(&root, SESSION),
        RUNTIME,
        "failed-spawn-transcript.json",
    );
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    let original = br#"{"path_hash":"","header_hash":"","offset":0,"anchor_hash":"","calls":{},"bindings":{}}"#;
    fs::write(&cache, original).unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&cache)
        .unwrap();
    let mut spawn = input("PreToolUse", SESSION);
    spawn.tool_use_id = Some("a".into());
    spawn.tool_name = Some("agents.spawn_agent".into());
    spawn.tool_input = Some(json!({"task_name": TASK, "message": "work"}));
    assert!(handle_hook_for_runtime_at(&spawn, &root, RUNTIME, 100).is_err());
    assert_eq!(fs::read(&cache).unwrap(), original);
    assert_eq!(
        crate::subagent_orchestrator::active_reservation_count(&root, RUNTIME, SESSION, 101)
            .unwrap(),
        None
    );
    drop(held);
}
