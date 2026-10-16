use std::collections::BTreeMap;

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
    Ok(
        serde_json::from_slice(&crate::fs_util::read_bounded(path, MAX_SCAN_BYTES)?)
            .unwrap_or_default(),
    )
}

fn save(path: &Path, state: &RecoveryState) -> Result<()> {
    crate::fs_util::atomic_write_private_with_parent(path, &serde_json::to_vec(state)?)
}

pub(super) fn note_spawn_call(
    input: &HookInput,
    root: &Path,
    runtime: &str,
    now_ms: u64,
) -> Result<()> {
    let Some(call_id) = nonempty(input.tool_use_id.as_deref()) else {
        return Ok(());
    };
    let Some(task) = input
        .tool_input
        .as_ref()
        .and_then(|args| args.get("task_name").or_else(|| args.get("taskName")))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    let path = cache_path(input, root, runtime);
    let mut state = load(&path)?;
    state
        .bindings
        .insert(task.trim().into(), (now_ms, call_id.into()));
    save(&path, &state)
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
            .is_some_and(|p| p.ends_with(&format!("-{}.jsonl", input.session_id)))
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
            "agent_role",
            "agent_id",
            "agent_type",
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
            continue;
        };
        if record["type"] != "response_item" {
            continue;
        }
        let payload = &record["payload"];
        let Some(id) = payload["call_id"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        let id = hash_component(id);
        match payload["type"].as_str() {
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
                let args = raw_args
                    .as_str()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .unwrap_or_else(|| raw_args.clone());
                let task = args["task_name"].as_str();
                let call = (payload["namespace"] == "agents" && payload["name"] == "spawn_agent")
                    .then(|| task.zip(timestamp(&record)))
                    .flatten()
                    .map(|(task, called_at_ms)| SpawnCall {
                        task: task.into(),
                        called_at_ms,
                        returned_at_ms: None,
                        failure: None,
                    });
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
                call.returned_at_ms = Some(timestamp(&record).unwrap_or(0));
                let output = if payload["output"].is_array() {
                    json!({"content": payload["output"]})
                } else {
                    payload["output"].clone()
                };
                if protocol::response_is_explicit_spawn_failure(&output)
                    && output.to_string().len() <= MAX_RENDERED_TOOL_RESULT_CHARS
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
    if state.ambiguous || state.offset < length {
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
        let Some(returned) = call.returned_at_ms.filter(|returned| {
            *returned >= *created && *returned >= call.called_at_ms && *returned <= now_ms
        }) else {
            continue;
        };
        let Some(response) = call.failure.as_ref() else {
            continue;
        };
        let matched = match state.bindings.get(&call.task) {
            Some((_, binding)) => hash_component(binding) == *id,
            None => {
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
                returned,
            )?;
        }
    }
    Ok(())
}
