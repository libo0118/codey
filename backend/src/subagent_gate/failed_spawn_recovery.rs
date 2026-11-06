use std::collections::{BTreeMap, BTreeSet};

use super::*;

const CACHE_FILE: &str = "failed-spawn-transcript.json";
const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CALLS: usize = 16 * 1024;

#[derive(Default, Deserialize, Serialize)]
struct RecoveryState {
    path_hash: String,
    header_hash: String,
    offset: u64,
    anchor_hash: String,
    #[serde(default)]
    ambiguous: bool,
    calls: BTreeMap<String, Option<SpawnCall>>,
    bindings: BTreeMap<String, (u64, String)>,
}

#[derive(Deserialize, Serialize)]
struct SpawnCall {
    task: String,
    called_at_ms: u64,
    returned_at_ms: Option<u64>,
    failure: Option<Value>,
}

fn cache_path(input: &HookInput, root: &Path, runtime: &str) -> PathBuf {
    session_auxiliary_path(
        &session_state_dir(root, &input.session_id),
        runtime,
        CACHE_FILE,
    )
}

fn load(path: &Path) -> Result<RecoveryState> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "子代理失败回执缓存不是普通文件"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecoveryState::default());
        }
        Err(error) => return Err(error.into()),
    }
    serde_json::from_slice(&crate::fs_util::read_bounded(path, MAX_SCAN_BYTES)?)
        .with_context(|| format!("解析子代理失败回执缓存失败：{}", path.display()))
}

fn save(path: &Path, state: &RecoveryState) -> Result<()> {
    let bytes = serde_json::to_vec(state)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_SCAN_BYTES,
        "子代理失败回执缓存超过上限"
    );
    crate::fs_util::atomic_write_private_with_parent(path, &bytes)
}

pub(super) fn note_spawn_call(
    input: &HookInput,
    root: &Path,
    runtime: &str,
    task: &str,
    issued_tasks: &BTreeSet<String>,
    now_ms: u64,
) -> Result<()> {
    let path = cache_path(input, root, runtime);
    let mut state = load(&path)?;
    let original_bindings = state.bindings.len();
    state
        .bindings
        .retain(|bound_task, _| issued_tasks.contains(bound_task));
    if let Some(call_id) = nonempty(input.tool_use_id.as_deref()) {
        anyhow::ensure!(
            !state
                .bindings
                .iter()
                .any(|(other_task, (_, other_id))| { other_task != task && other_id == call_id }),
            "子代理派发 Hook 的 tool_use_id 已绑定其他任务"
        );
        state.bindings.insert(task.into(), (now_ms, call_id.into()));
        save(&path, &state)
    } else if state.bindings.remove(task).is_some() || state.bindings.len() != original_bindings {
        save(&path, &state)
    } else {
        Ok(())
    }
}

fn spawn_task(args: &Value) -> Option<&str> {
    let args = args.as_object()?;
    let mut task = None;
    for key in ["task_name", "taskName"] {
        if let Some(value) = args.get(key) {
            let value = value.as_str().filter(|value| !value.trim().is_empty())?;
            if task.is_some_and(|previous| previous != value) {
                return None;
            }
            task = Some(value);
        }
    }
    task
}

fn timestamp(record: &Value) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(record["timestamp"].as_str()?)
        .ok()?
        .timestamp_millis()
        .try_into()
        .ok()
}

fn anchor(file: &mut fs::File, offset: u64) -> Result<String> {
    let start = offset.saturating_sub(64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (offset - start) as usize];
    file.read_exact(&mut bytes)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn recover(input: &HookInput, root: &Path, runtime: &str, now_ms: u64) -> Result<()> {
    if input_has_subagent_context(input) {
        return Ok(());
    }
    let Some(transcript) = input.transcript_path.as_deref().map(Path::new) else {
        return Ok(());
    };
    let Some(home) = root.parent() else {
        return Ok(());
    };
    let (Ok(sessions), Ok(path), Ok(metadata)) = (
        fs::canonicalize(home.join("sessions")),
        fs::canonicalize(transcript),
        fs::symlink_metadata(transcript),
    ) else {
        return Ok(());
    };
    if !transcript.is_absolute()
        || !metadata.is_file()
        || metadata.file_type().is_symlink()
        || !path.starts_with(sessions)
        || !path
            .file_name()
            .and_then(|p| p.to_str())
            .is_some_and(|p| p.starts_with("rollout-") && p.ends_with(".jsonl"))
    {
        return Ok(());
    }
    let mut file = fs::File::open(path.clone())?;
    let mut header = Vec::new();
    BufReader::new((&mut file).take(64 * 1024)).read_until(b'\n', &mut header)?;
    let Ok(meta) = serde_json::from_slice::<Value>(&header) else {
        return Ok(());
    };
    let payload = &meta["payload"];
    if !header.ends_with(b"\n")
        || meta["type"] != "session_meta"
        || payload["id"].as_str() != Some(input.session_id.as_str())
        || !matches!(payload["source"].as_str(), Some("cli" | "vscode"))
        || [
            "parent_thread_id",
            "parentThreadId",
            "agent_path",
            "agentPath",
            "agent_role",
            "agentRole",
            "agent_id",
            "agentId",
            "agent_type",
            "agentType",
            "subagent",
        ]
        .iter()
        .any(|key| payload.get(*key).is_some_and(|v| !v.is_null()))
    {
        return Ok(());
    }
    let pending = crate::subagent_orchestrator::pending_unbound_spawn_reservations(
        root,
        runtime,
        &input.session_id,
        now_ms,
    )?;
    if pending.is_empty() {
        return Ok(());
    }
    let cache = cache_path(input, root, runtime);
    let mut state = load(&cache)?;
    let path_hash = hash_component(&path.to_string_lossy());
    let header_hash = format!("{:x}", Sha256::digest(&header));
    let length = file.metadata()?.len();
    if state.path_hash != path_hash
        || state.header_hash != header_hash
        || state.offset > length
        || anchor(&mut file, state.offset)? != state.anchor_hash
    {
        state.path_hash = path_hash;
        state.header_hash = header_hash;
        state.offset = header.len() as u64;
        state.calls.clear();
        state.ambiguous = false;
    }
    state
        .bindings
        .retain(|task, (created, _)| pending.get(task) == Some(created));
    file.seek(SeekFrom::Start(state.offset))?;
    let mut reader =
        BufReader::new((&mut file).take(length.saturating_sub(state.offset).min(MAX_SCAN_BYTES)));
    loop {
        let mut line = Vec::new();
        let count = reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if count == 0 || !line.ends_with(b"\n") || count as u64 > MAX_RECORD_BYTES {
            break;
        }
        state.offset += count as u64;
        let Ok(record) = serde_json::from_slice::<Value>(&line) else {
            state.ambiguous = true;
            continue;
        };
        let Some(record_type) = record["type"].as_str() else {
            state.ambiguous = true;
            continue;
        };
        if record_type != "response_item" {
            continue;
        }
        let payload = &record["payload"];
        let kind = payload["type"].as_str();
        if !payload.is_object() || kind.is_none() {
            state.ambiguous = true;
            continue;
        }
        let is_call = matches!(kind, Some("function_call" | "custom_tool_call"));
        let is_output = matches!(
            kind,
            Some("function_call_output" | "custom_tool_call_output")
        );
        if !is_call && !is_output {
            continue;
        }
        let Some(id) = payload["call_id"]
            .as_str()
            .filter(|id| !id.trim().is_empty())
        else {
            state.ambiguous = true;
            continue;
        };
        let id = hash_component(id);
        match kind {
            Some("function_call" | "custom_tool_call") => {
                if state.calls.len() >= MAX_CALLS {
                    // shortcut: unusually large or ambiguous histories keep the gate, add indexed lookup if needed.
                    state.ambiguous = true;
                    break;
                }
                if state.calls.contains_key(&id) {
                    state.ambiguous = true;
                    state.calls.insert(id, None);
                    continue;
                }
                let raw_args = if payload["type"] == "custom_tool_call" {
                    &payload["input"]
                } else {
                    &payload["arguments"]
                };
                let name = payload["name"].as_str();
                let namespace = payload["namespace"].as_str();
                if (namespace == Some("agents") && name.is_none())
                    || (name == Some("spawn_agent") && namespace != Some("agents"))
                {
                    state.ambiguous = true;
                }
                let call = if namespace == Some("agents") && name == Some("spawn_agent") {
                    let args = match raw_args.as_str() {
                        Some(text) => serde_json::from_str::<Value>(text).ok(),
                        None => Some(raw_args.clone()),
                    };
                    match args.as_ref().and_then(spawn_task).zip(timestamp(&record)) {
                        Some((task, called_at_ms)) => Some(SpawnCall {
                            task: task.into(),
                            called_at_ms,
                            returned_at_ms: None,
                            failure: None,
                        }),
                        None => {
                            state.ambiguous = true;
                            None
                        }
                    }
                } else {
                    None
                };
                state.calls.insert(id, call);
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                let Some(Some(call)) = state.calls.get_mut(&id) else {
                    continue;
                };
                if call.returned_at_ms.is_some() {
                    state.ambiguous = true;
                    state.calls.insert(id, None);
                    continue;
                }
                let Some(returned_at_ms) = timestamp(&record) else {
                    state.ambiguous = true;
                    continue;
                };
                call.returned_at_ms = Some(returned_at_ms);
                let output = if payload["output"].is_array() {
                    json!({"content": payload["output"]})
                } else {
                    payload["output"].clone()
                };
                if crate::subagent_orchestrator::spawn_response_is_unbound_failure(
                    &output, &call.task,
                ) && output.to_string().len() <= MAX_RENDERED_TOOL_RESULT_CHARS
                {
                    call.failure = Some(output);
                }
            }
            _ => {}
        }
    }
    state.anchor_hash = anchor(&mut file, state.offset)?;
    // Persist the cursor before settlement; a retry can replay the same verified failure.
    save(&cache, &state)?;
    if state.ambiguous || state.offset < length || file.metadata()?.len() != length {
        return Ok(());
    }
    for (id, call) in state
        .calls
        .iter()
        .filter_map(|(id, call)| call.as_ref().map(|call| (id, call)))
    {
        let Some(created) = pending.get(&call.task) else {
            continue;
        };
        if !call.returned_at_ms.is_some_and(|returned| {
            returned >= *created && returned >= call.called_at_ms && returned <= now_ms
        }) {
            continue;
        }
        let Some(response) = call.failure.as_ref() else {
            continue;
        };
        let matched = match state.bindings.get(&call.task) {
            Some((_, binding)) => hash_component(binding) == *id,
            None => {
                // Without a Hook call ID, only a complete transcript with one
                // task call whose interval contains the reservation is evidence.
                call.called_at_ms <= *created
                    && state
                        .calls
                        .values()
                        .flatten()
                        .filter(|other| other.task == call.task)
                        .count()
                        == 1
            }
        };
        if matched {
            crate::subagent_orchestrator::post_spawn(
                root,
                runtime,
                &input.session_id,
                Some(&json!({"task_name": call.task})),
                Some(response),
                now_ms,
            )?;
        }
    }
    Ok(())
}
