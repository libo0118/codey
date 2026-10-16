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
            let ledger: Value = serde_json::from_slice(
                &fs::read(session_state_dir(&root, SESSION).join("orchestrator-ledger-v1.json"))
                    .unwrap(),
            )
            .unwrap();
            if event == "Stop" {
                assert!(ledger["reservations"].as_object().unwrap().is_empty());
            } else {
                assert_eq!(ledger["reservations"][TASK]["outcome"], "failed");
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
fn bound_or_fenced_reservations_and_child_hooks_are_not_recovered() {
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
