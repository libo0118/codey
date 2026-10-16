use super::*;
use serde_json::json;
use tempfile::tempdir;

fn spawn(root: &Path, task: &str, agent: &str, now_ms: u64) {
    let input = json!({
        "task_name": task,
        "agent_type": "codey_worker",
        "message": "Complete the assigned task"
    });
    assert_eq!(
        pre_spawn(root, "runtime-a", "session-a", Some(&input), 0, now_ms).unwrap(),
        None
    );
    post_spawn(
        root,
        "runtime-a",
        "session-a",
        Some(&input),
        Some(&json!({"agent_id": agent})),
        now_ms + 1,
    )
    .unwrap();
}

#[test]
fn root_stop_retains_terminal_identity_against_late_start_and_pending_snapshot() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    spawn(root, "read_task", "opaque-old", 10);
    subagent_stopped(root, "runtime-a", "session-a", "opaque-old", 20).unwrap();
    settle_turn(root, "runtime-a", "session-a", 21).unwrap();

    assert!(
        !subagent_started_with_context(
            root,
            "runtime-a",
            "session-a",
            "opaque-old",
            None,
            None,
            22,
        )
        .unwrap()
    );
    assert_eq!(
        reconcile_pending_init_status_response(
            root,
            "runtime-a",
            "session-a",
            Some(&json!({"agents": [
                {"agent_name": "/root/read_task", "agent_status": "pending_init"}
            ]})),
            23,
            600_000,
        )
        .unwrap(),
        None
    );
    assert_eq!(
        active_reservation_count(root, "runtime-a", "session-a", 24).unwrap(),
        Some(0)
    );
}

#[test]
fn old_child_cannot_bind_to_a_new_round_reusing_its_task_name() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("codey-subagent-gate-v3");
    spawn(&root, "read_task", "opaque-old", 10);
    subagent_stopped(&root, "runtime-a", "session-a", "opaque-old", 20).unwrap();
    settle_turn(&root, "runtime-a", "session-a", 21).unwrap();

    let input = json!({
        "task_name": "read_task",
        "agent_type": "codey_worker",
        "message": "A new bounded task"
    });
    assert_eq!(
        pre_spawn(&root, "runtime-a", "session-a", Some(&input), 0, 30).unwrap(),
        None
    );
    let sessions = temp.path().join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    let transcript = sessions.join("rollout-probe-opaque-old.jsonl");
    fs::write(
        &transcript,
        json!({"type": "session_meta", "payload": {
            "id": "opaque-old", "parent_thread_id": "session-a",
            "agent_path": "/root/read_task", "agent_role": "codey_worker"
        }})
        .to_string(),
    )
    .unwrap();
    let old_context = || ChildToolContext {
        agent_id: "opaque-old",
        agent_type: Some("codey_worker"),
        transcript_path: transcript.to_str(),
        tool_name: "apply_patch",
        tool_input: None,
    };
    assert!(
        authorize_child_tool(&root, "runtime-a", "session-a", old_context(), 31)
            .unwrap()
            .is_some_and(|reason| reason.contains(UNBOUND_ATTEMPT_ERROR_CODE))
    );
    post_spawn(
        &root,
        "runtime-a",
        "session-a",
        Some(&input),
        Some(&json!({"agent_id": "opaque-new"})),
        32,
    )
    .unwrap();
    assert!(
        subagent_started_with_context(
            &root,
            "runtime-a",
            "session-a",
            "opaque-new",
            None,
            None,
            33,
        )
        .unwrap()
    );
    assert_eq!(
        authorize_child_tool(
            &root,
            "runtime-a",
            "session-a",
            ChildToolContext {
                agent_id: "opaque-new",
                transcript_path: None,
                ..old_context()
            },
            34,
        )
        .unwrap(),
        None
    );
    assert_eq!(
        active_reservation_count(&root, "runtime-a", "session-a", 35).unwrap(),
        Some(1)
    );
}

#[test]
fn root_stop_releases_the_round_capacity_and_task_ids() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    spawn(root, "read_task", "opaque-old", 10);
    subagent_stopped(root, "runtime-a", "session-a", "opaque-old", 20).unwrap();
    {
        let store = LedgerStore::open(root, "session-a").unwrap();
        let mut ledger = store.load("runtime-a", "session-a", 21).unwrap().unwrap();
        let template = ledger.reservations["read_task"].clone();
        for index in 1..MAX_RESERVATIONS_PER_LEDGER {
            let mut reservation = template.clone();
            reservation.task_id = format!("task-{index}");
            reservation.agent_id_hash = None;
            ledger
                .reservations
                .insert(reservation.task_id.clone(), reservation);
        }
        store.save(&mut ledger, 21).unwrap();
        assert!(ledger_capacity_denial(&ledger).is_some());
    }
    settle_turn(root, "runtime-a", "session-a", 22).unwrap();
    spawn(root, "read_task", "opaque-new", 30);
}

#[test]
fn root_stop_retains_interrupt_fence_across_runtime_change_until_session_end() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    spawn(root, "read_task", "opaque-old", 10);
    let target = json!({"target": "opaque-old"});
    pre_interrupt_agent(root, "runtime-a", "session-a", Some(&target), 20).unwrap();
    assert!(
        settle_interrupt_acknowledgement(
            root,
            "runtime-a",
            "session-a",
            Some(&target),
            &InterruptAcknowledgement {
                prior_outcome: None,
                identifiers: Vec::new()
            },
            21,
        )
        .unwrap()
        .is_some()
    );
    settle_turn(root, "runtime-a", "session-a", 22).unwrap();
    assert!(
        !subagent_started_with_context(
            root,
            "runtime-b",
            "session-a",
            "opaque-old",
            None,
            None,
            23,
        )
        .unwrap()
    );
    assert!(
        subagent_started_with_context(
            root, "runtime-a", "session-a", "opaque-old", None, None, 24,
        )
        .unwrap_err()
        .to_string()
        .contains(STALE_RUNTIME_ERROR_CODE)
    );
    end_session(root, "runtime-b", "session-a", 25).unwrap();
    assert!(
        LedgerStore::open(root, "session-a")
            .unwrap()
            .load("runtime-b", "session-a", 26)
            .unwrap()
            .is_none()
    );
}

#[test]
fn spawn_cannot_reuse_a_settled_concrete_agent_identity() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    spawn(root, "old_task", "opaque-old", 10);
    subagent_stopped(root, "runtime-a", "session-a", "opaque-old", 20).unwrap();
    settle_turn(root, "runtime-a", "session-a", 21).unwrap();
    let input = json!({
        "task_name": "new_task", "agent_type": "codey_worker", "message": "Work"
    });
    assert_eq!(
        pre_spawn(root, "runtime-a", "session-a", Some(&input), 0, 30).unwrap(),
        None
    );
    assert!(
        post_spawn(
            root,
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({"agent_id": "opaque-old"})),
            31,
        )
        .unwrap_err()
        .to_string()
        .contains(AGENT_ID_COLLISION_ERROR_CODE)
    );
    assert_eq!(
        active_reservation_count(root, "runtime-a", "session-a", 32).unwrap(),
        Some(0)
    );
}

#[test]
fn pending_unbound_spawn_projection_excludes_bound_fenced_and_settled_attempts() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    assert!(
        pending_unbound_spawn_reservations(root, "runtime-a", "session-a", 1)
            .unwrap()
            .is_empty()
    );
    let input = json!({
        "task_name": "pending", "agent_type": "codey_worker", "message": "Work"
    });
    pre_spawn(root, "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
    {
        let store = LedgerStore::open(root, "session-a").unwrap();
        let mut ledger = store.load("runtime-a", "session-a", 11).unwrap().unwrap();
        let template = ledger.reservations["pending"].clone();
        for task in ["bound", "fenced", "settled"] {
            let mut reservation = template.clone();
            reservation.task_id = task.into();
            if task == "bound" {
                reservation.agent_id_hash = Some(hash_component("agent-bound"));
            }
            if task == "fenced" {
                reservation.fenced_at_ms = Some(11);
            }
            if task == "settled" {
                reservation.state = ReservationState::Terminal;
            }
            ledger.reservations.insert(task.into(), reservation);
        }
        store.save(&mut ledger, 11).unwrap();
    }
    assert_eq!(
        pending_unbound_spawn_reservations(root, "runtime-a", "session-a", 12).unwrap(),
        BTreeMap::from([("pending".into(), 10)])
    );
}
