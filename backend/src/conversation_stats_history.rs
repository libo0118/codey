//! 从当前 Codex 数据目录读取原生累计快照，不累加每轮或运行期统计。
use super::{
    ConversationStatsRegistry, ConversationStatsSnapshot, LastTurnStatsSnapshot,
    RuntimeHistoryBoundary, TurnRequestTotals, TurnWindow, lock_unpoisoned, normalize_session_id,
};
#[cfg(test)]
use crate::route_request_log::{SessionGenerationTotals, SessionRecordedTiming};
use codey_runtime_core::codex_sqlite::codex_session_db_paths_from_home;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

#[cfg(test)]
const MAX_TAIL_BYTES: u64 = 8 * 1024 * 1024;
const LOOKUP_TTL: Duration = Duration::from_secs(60);
type CacheKey = (PathBuf, String);
type Cache = HashMap<CacheKey, Arc<Mutex<CachedHistory>>>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

#[derive(Default)]
struct CachedHistory {
    lookup_at: Option<Instant>,
    path: Option<PathBuf>,
    version: Option<FileVersion>,
    append_checkpoint: Option<AppendCheckpoint>,
    snapshot: Option<ConversationStatsSnapshot>,
    activity: super::activity::Activity,
}

// 仅文件版本变化时读取首尾固定窗口；缓存只保存摘要，不保存会话正文。
const APPEND_SAMPLE_BYTES: usize = 4096;

#[derive(PartialEq, Eq)]
struct AppendCheckpoint {
    len: u64,
    digest: [u8; 32],
}

impl AppendCheckpoint {
    fn read(path: &Path, len: u64) -> std::io::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let mut hash = Sha256::new();
        let mut buffer = [0; APPEND_SAMPLE_BYTES];
        let head_len = len.min(APPEND_SAMPLE_BYTES as u64);
        let tail_start = head_len.max(len.saturating_sub(APPEND_SAMPLE_BYTES as u64));
        for (start, size) in [(0, head_len), (tail_start, len - tail_start)] {
            if size == 0 {
                continue;
            }
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(&mut buffer[..size as usize])?;
            hash.update(start.to_le_bytes());
            hash.update(&buffer[..size as usize]);
        }
        Ok(Self {
            len,
            digest: hash.finalize().into(),
        })
    }

    fn still_matches(&self, path: &Path) -> bool {
        Self::read(path, self.len).is_ok_and(|current| current == *self)
    }
}

#[derive(PartialEq, Eq)]
struct FileVersion {
    len: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
    #[cfg(windows)]
    identity: (u32, u64),
}

impl FileVersion {
    fn from_path(path: &Path) -> Option<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(windows)]
        let file = std::fs::File::open(path).ok()?;
        #[cfg(windows)]
        let metadata = file.metadata().ok()?;
        #[cfg(not(windows))]
        let metadata = path.metadata().ok()?;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok()?,
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
            #[cfg(windows)]
            identity: {
                use std::os::windows::io::AsRawHandle;
                use windows::Win32::Foundation::HANDLE;
                use windows::Win32::Storage::FileSystem::{
                    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
                };
                let mut information = BY_HANDLE_FILE_INFORMATION::default();
                // 创建时间可能相同或被保留，卷号和文件索引才能识别同路径替换。
                // file 在调用期间持有有效句柄，information 提供完整的可写输出空间。
                unsafe {
                    GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)
                }
                .ok()?;
                (
                    information.dwVolumeSerialNumber,
                    (u64::from(information.nFileIndexHigh) << 32)
                        | u64::from(information.nFileIndexLow),
                )
            },
        })
    }
}

pub(crate) async fn snapshot_with_history(
    home: PathBuf,
    session_id: String,
    registry: Option<Arc<ConversationStatsRegistry>>,
) -> Option<ConversationStatsSnapshot> {
    tokio::task::spawn_blocking(move || {
        let mut history = cached_history(&home, &session_id);
        let latest_native = crate::conversation_stats_native::latest_turn(&home, &session_id);
        let route_active = registry
            .as_ref()
            .and_then(|registry| registry.snapshot(&session_id))
            .is_some_and(|snapshot| snapshot.in_flight.unwrap_or(0) > 0);
        let last_turn = select_last_turn(
            history
                .as_ref()
                .and_then(|snapshot| snapshot.last_turn.clone()),
            latest_native.as_ref(),
            route_active,
        );
        if let Some(last_turn) = &last_turn {
            history
                .get_or_insert_with(|| ConversationStatsSnapshot {
                    usage_source: "unknown",
                    usage_scope: "unknown",
                    ..Default::default()
                })
                .last_turn = Some(last_turn.clone());
        }
        let runtime =
            registry.and_then(|registry| registry.snapshot_with_turn(&session_id, last_turn));
        let mut snapshot = merge(history, runtime)?;
        if let Some(turn) = &snapshot.last_turn
            && let Some(window) = TurnWindow::from_snapshot(turn)
        {
            let recorded = cached_turn(&home, &session_id, &window, turn.runtime_boundary.as_ref());
            apply_recorded_turn(&mut snapshot, recorded);
        }
        let native = snapshot.last_turn.as_ref().and_then(|turn| {
            if latest_native
                .as_ref()
                .is_some_and(|native| native.turn_id == turn.id)
            {
                latest_native
            } else {
                crate::conversation_stats_native::snapshot(&home, &session_id, &turn.id)
            }
        });
        apply_native_turn(&mut snapshot, native);
        Some(snapshot)
    })
    .await
    .ok()
    .flatten()
}

fn select_last_turn(
    last_turn: Option<LastTurnStatsSnapshot>,
    native: Option<&crate::conversation_stats_native::NativeTurnStats>,
    route_active: bool,
) -> Option<LastTurnStatsSnapshot> {
    let Some(native) = native else {
        return last_turn;
    };
    if route_active {
        return last_turn;
    }
    if let Some(turn) = &last_turn {
        if turn.id == native.turn_id
            || !turn.started_at_unix_ms.is_some_and(|start| {
                native.latest_sample_started_at_unix_ms > turn.completed_at_unix_ms.unwrap_or(start)
            })
        {
            return last_turn;
        }
    }
    // Sampling times identify freshness, never the user's turn start or completion.
    Some(LastTurnStatsSnapshot {
        id: native.turn_id.clone(),
        status: "unknown",
        coverage: "partial",
        ..Default::default()
    })
}

fn apply_native_turn(
    snapshot: &mut ConversationStatsSnapshot,
    native: Option<crate::conversation_stats_native::NativeTurnStats>,
) {
    let Some(turn) = &mut snapshot.last_turn else {
        return;
    };
    let Some(native) =
        native.filter(|native| native.turn_id == turn.id && native.request_count > 0)
    else {
        return;
    };
    if turn.in_flight.unwrap_or(0) > 0 {
        return;
    }
    let mut applied = false;
    if (turn.model_duration_ms.is_none() && turn.average_ttft_ms.is_none())
        || turn
            .route_timing_updated_at_unix_ms
            .is_some_and(|at| native.updated_at_unix_ms > at)
    {
        turn.model_duration_ms = Some(native.model_duration_ms);
        turn.average_ttft_ms = native
            .average_ttft_ms
            .filter(|_| native.ttft_sample_count > 0);
        turn.timing_source = Some("codex");
        turn.ttft_boundary = turn.average_ttft_ms.map(|_| "first_output_item");
        applied = true;
    }
    if let Some(speed) = native
        .average_output_tokens_per_second
        .filter(|speed| speed.is_finite() && *speed >= 0.0)
        && native.average_speed_request_count > 0
        && (turn.average_output_tokens_per_second.is_none()
            || turn.route_speed_updated_at_unix_ms.is_some_and(|at| {
                native
                    .average_speed_updated_at_unix_ms
                    .is_some_and(|native_at| native_at > at)
            }))
    {
        turn.average_output_tokens_per_second = Some(speed);
        turn.average_speed_source = Some("codex");
        turn.speed_is_estimate = false;
        applied = true;
    }
    if applied {
        turn.coverage = "partial";
    }
}

#[derive(Default)]
struct CachedTurn {
    key: Option<(TurnWindow, Option<RuntimeHistoryBoundary>)>,
    at: Option<Instant>,
    value: Option<TurnRequestTotals>,
}

impl CachedTurn {
    fn get_or_query(
        &mut self,
        window: &TurnWindow,
        boundary: Option<&RuntimeHistoryBoundary>,
        now: Instant,
        query: impl FnOnce() -> Option<TurnRequestTotals>,
    ) -> Option<TurnRequestTotals> {
        if self
            .at
            .is_some_and(|at| now.saturating_duration_since(at) < LOG_LOOKUP_TTL)
            && self.key.as_ref().is_some_and(|(old_window, old_boundary)| {
                old_window == window && old_boundary.as_ref() == boundary
            })
        {
            return self.value;
        }
        self.value = query();
        self.at = Some(now);
        self.key = Some((window.clone(), boundary.cloned()));
        self.value
    }
}

type TurnCache = HashMap<(PathBuf, PathBuf, String), Arc<Mutex<CachedTurn>>>;
static TURN_CACHE: OnceLock<Mutex<TurnCache>> = OnceLock::new();

fn cached_turn(
    home: &Path,
    session_id: &str,
    window: &TurnWindow,
    boundary: Option<&RuntimeHistoryBoundary>,
) -> Option<TurnRequestTotals> {
    let session_id = normalize_session_id(session_id);
    uuid::Uuid::parse_str(session_id).ok()?;
    let root = codey_runtime_core::paths::default_app_state_dir();
    let key = (
        home.canonicalize().ok()?,
        root.clone(),
        session_id.to_owned(),
    );
    let entry = {
        let mut cache = lock_unpoisoned(TURN_CACHE.get_or_init(Default::default));
        if cache.len() >= 128
            && !cache.contains_key(&key)
            && let Some(old) = cache.keys().next().cloned()
        {
            cache.remove(&old);
        }
        Arc::clone(cache.entry(key).or_default())
    };
    lock_unpoisoned(&entry).get_or_query(window, boundary, Instant::now(), || {
        crate::route_request_log::last_turn_recorded_stats(&root, session_id, window, boundary)
    })
}

fn apply_recorded_turn(
    snapshot: &mut ConversationStatsSnapshot,
    recorded: Option<TurnRequestTotals>,
) {
    let Some(turn) = &mut snapshot.last_turn else {
        return;
    };
    let mut totals = turn.runtime_totals;
    let has_runtime = totals.requests > 0 || turn.in_flight.unwrap_or(0) > 0;
    if let Some(recorded) = recorded {
        totals.add(recorded);
    }
    totals.apply(turn);
    let incomplete = turn.runtime_partial
        || turn.activity_partial
        || recorded.is_some_and(TurnRequestTotals::incomplete)
        || totals.incomplete()
        || turn.step_count.is_some_and(|steps| steps > totals.requests);
    turn.coverage = if incomplete {
        "partial"
    } else {
        match (has_runtime, recorded.is_some()) {
            (true, true) => "recorded_and_runtime",
            (true, false) => "runtime",
            (false, true) => "recorded",
            (false, false) => "unknown",
        }
    };
}

#[cfg(test)]
type CachedAverage = CachedLog<SessionGenerationTotals>;
const LOG_LOOKUP_TTL: Duration = Duration::from_secs(30);

#[cfg(test)]
#[derive(Default)]
struct CachedLog<T> {
    at: Option<Instant>,
    boundary: Option<RuntimeHistoryBoundary>,
    value: Option<T>,
}

#[cfg(test)]
impl<T: Copy> CachedLog<T> {
    fn get_or_query(
        &mut self,
        boundary: Option<&RuntimeHistoryBoundary>,
        now: Instant,
        query: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        if self
            .at
            .is_some_and(|at| now.saturating_duration_since(at) < LOG_LOOKUP_TTL)
            && self.boundary.as_ref() == boundary
        {
            return self.value;
        }
        self.value = query();
        self.boundary = boundary.cloned();
        self.at = Some(now);
        self.value
    }
}

#[cfg(test)]
fn apply_recorded_average(
    snapshot: Option<ConversationStatsSnapshot>,
    recorded: Option<SessionGenerationTotals>,
) -> Option<ConversationStatsSnapshot> {
    let Some(recorded) = recorded else {
        return snapshot;
    };
    let mut snapshot = snapshot.unwrap_or_else(|| ConversationStatsSnapshot {
        usage_source: "unknown",
        usage_scope: "unknown",
        ..Default::default()
    });
    let mut totals = snapshot.average_runtime_totals;
    let has_runtime = totals.requests > 0;
    totals.add(recorded);
    snapshot.average_output_tokens_per_second = totals.speed();
    snapshot.average_speed_request_count = Some(totals.requests);
    snapshot.average_speed_source = Some(if has_runtime {
        "runtime_and_request_log"
    } else {
        "request_log"
    });
    snapshot.average_speed_scope = Some(if has_runtime {
        "recorded_and_runtime"
    } else {
        "recorded_requests"
    });
    Some(snapshot)
}

#[cfg(test)]
fn apply_recorded_timing(
    snapshot: Option<ConversationStatsSnapshot>,
    timing: Option<SessionRecordedTiming>,
) -> Option<ConversationStatsSnapshot> {
    let Some(timing) = timing else {
        return snapshot;
    };
    let mut snapshot = snapshot.unwrap_or_else(|| ConversationStatsSnapshot {
        usage_source: "unknown",
        usage_scope: "unknown",
        ..Default::default()
    });
    if snapshot.step_source != Some("rollout") && snapshot.step_count.unwrap_or(0) == 0 {
        snapshot.step_count = Some(timing.requests);
        snapshot.step_source = Some("request_log");
        snapshot.step_scope = Some("recorded_requests");
    }
    // 历史查询已排除运行期跟踪的请求，两段计时可以相加；每个字段单独保留未知值。
    let has_runtime = snapshot.model_duration_ms.is_some() || snapshot.average_ttft_ms.is_some();
    let has_recorded = timing.model_duration_ms.is_some() || timing.average_ttft_ms.is_some();
    if let Some(duration) = timing.model_duration_ms {
        snapshot.model_duration_ms = Some(
            snapshot
                .model_duration_ms
                .unwrap_or(0)
                .saturating_add(duration),
        );
        snapshot.timing_request_count = Some(
            snapshot
                .timing_request_count
                .unwrap_or(0)
                .saturating_add(timing.timing_request_count),
        );
    }
    if let Some(average) = timing.average_ttft_ms
        && timing.ttft_sample_count > 0
    {
        let runtime_samples = snapshot
            .average_ttft_ms
            .map_or(0, |_| snapshot.ttft_sample_count.unwrap_or(0));
        let samples = runtime_samples.saturating_add(timing.ttft_sample_count);
        snapshot.average_ttft_ms = Some(
            (snapshot.average_ttft_ms.unwrap_or(0.0) * runtime_samples as f64
                + average * timing.ttft_sample_count as f64)
                / samples as f64,
        );
        snapshot.ttft_sample_count = Some(samples);
    }
    if has_recorded {
        snapshot.timing_source = Some(if has_runtime {
            "runtime_and_request_log"
        } else {
            "request_log"
        });
        snapshot.timing_scope = Some(if has_runtime {
            "recorded_and_runtime"
        } else {
            "recorded_requests"
        });
    }
    Some(snapshot)
}

#[cfg(test)]
fn apply_recorded_speed(
    snapshot: Option<ConversationStatsSnapshot>,
    speed: Option<(f64, u64)>,
) -> Option<ConversationStatsSnapshot> {
    let Some((speed, recorded_at)) = speed else {
        return snapshot;
    };
    let mut snapshot = snapshot.unwrap_or_else(|| ConversationStatsSnapshot {
        usage_source: "unknown",
        usage_scope: "unknown",
        ..Default::default()
    });
    if snapshot.in_flight.unwrap_or(0) == 0 && snapshot.output_tokens_per_second.is_none() {
        snapshot.output_tokens_per_second = Some(speed);
        snapshot.speed_source = Some("request_log");
        snapshot.speed_scope = Some("latest_recorded_request");
        snapshot.speed_recorded_at_unix_ms = Some(recorded_at);
    }
    Some(snapshot)
}

fn merge(
    history: Option<ConversationStatsSnapshot>,
    runtime: Option<ConversationStatsSnapshot>,
) -> Option<ConversationStatsSnapshot> {
    let Some(mut history) = history else {
        return runtime;
    };
    if let Some(runtime) = runtime {
        if runtime.last_turn.is_some() {
            history.last_turn = runtime.last_turn;
        }
        // 原生文件可能只有活动事件，尚无 Token 累计；此时保留运行期用量。
        if history.usage_source == "unknown" {
            history.input_tokens = runtime.input_tokens;
            history.output_tokens = runtime.output_tokens;
            history.cached_input_tokens = runtime.cached_input_tokens;
            history.cache_write_input_tokens = runtime.cache_write_input_tokens;
            history.total_tokens = runtime.total_tokens;
            history.usage_source = runtime.usage_source;
            history.usage_scope = runtime.usage_scope;
        }
        history.requests = runtime.requests;
        history.in_flight = runtime.in_flight;
        history.requests_scope = runtime.requests_scope;
        // 请求字段只描述本次运行。rollout 的写入时间通常晚于请求开始时间，
        // 不能用两者比较来判断模型和速度是否属于最近的运行期请求。
        history.model = runtime.model;
        history.last_status = runtime.last_status;
        history.output_tokens_per_second = runtime.output_tokens_per_second;
        history.speed_source = runtime.speed_source;
        history.speed_scope = runtime.speed_scope;
        history.speed_recorded_at_unix_ms = runtime.speed_recorded_at_unix_ms;
        history.speed_is_estimate = runtime.speed_is_estimate;
        history.speed_state = runtime.speed_state;
        history.average_output_tokens_per_second = runtime.average_output_tokens_per_second;
        history.average_speed_is_estimate = runtime.average_speed_is_estimate;
        history.average_speed_source = runtime.average_speed_source;
        history.average_speed_scope = runtime.average_speed_scope;
        history.average_speed_request_count = runtime.average_speed_request_count;
        history.average_runtime_totals = runtime.average_runtime_totals;
        history.average_history_boundary = runtime.average_history_boundary;
        history.updated_at_unix_ms = runtime.updated_at_unix_ms;
        history.model_duration_ms = runtime.model_duration_ms;
        history.average_ttft_ms = runtime.average_ttft_ms;
        history.ttft_boundary = runtime.ttft_boundary;
        history.timing_source = runtime.timing_source;
        history.timing_scope = runtime.timing_scope;
        history.timing_request_count = runtime.timing_request_count;
        history.ttft_sample_count = runtime.ttft_sample_count;
        if history.step_count.is_none() {
            history.step_count = runtime.step_count;
            history.step_source = runtime.step_source;
            history.step_scope = runtime.step_scope;
        }
    }
    Some(history)
}

fn cached_history(home: &Path, session_id: &str) -> Option<ConversationStatsSnapshot> {
    let session_id = normalize_session_id(session_id);
    // 原生会话使用 UUID，拒绝路径形态及非本地会话标识。
    uuid::Uuid::parse_str(session_id).ok()?;
    let home = home.canonicalize().ok()?;
    let key = (home.clone(), session_id.to_string());
    let entry = {
        let mut cache = lock_unpoisoned(CACHE.get_or_init(Default::default));
        if cache.len() >= 128 && !cache.contains_key(&key) {
            // 有界缓存只影响重新定位频率，不影响数据来源。
            if let Some(old) = cache.keys().next().cloned() {
                cache.remove(&old);
            }
        }
        Arc::clone(cache.entry(key).or_default())
    };
    let mut cached = lock_unpoisoned(&entry);
    if cached.lookup_at.is_none_or(|at| at.elapsed() >= LOOKUP_TTL) {
        let path = locate_rollout(&home, session_id);
        if cached.path != path {
            cached.version = None;
            cached.append_checkpoint = None;
            cached.snapshot = None;
            cached.activity = Default::default();
        }
        cached.path = path;
        cached.lookup_at = Some(Instant::now());
    }
    cached.path.as_ref()?;
    // 文件可能刚被归档移动，不等待定位缓存到期；每次重新验证路径，
    // 也拒绝替换为外部符号链接的文件。
    let path = cached
        .path
        .as_ref()
        .and_then(|path| canonical_rollout(&home, path));
    let path = match path {
        Some(path) => path,
        None => {
            cached.path = locate_rollout(&home, session_id);
            cached.lookup_at = Some(Instant::now());
            cached.version = None;
            cached.append_checkpoint = None;
            cached.snapshot = None;
            cached.activity = Default::default();
            canonical_rollout(&home, cached.path.as_ref()?)?
        }
    };
    let version = FileVersion::from_path(&path)?;
    if cached.version.as_ref() != Some(&version) {
        let appended = cached.version.as_ref().is_some_and(|old| {
            #[cfg(any(unix, windows))]
            let same_file =
                old.identity.0 == version.identity.0 && old.identity.1 == version.identity.1;
            #[cfg(not(any(unix, windows)))]
            let same_file = old.created == version.created;
            same_file
                && version.len > old.len
                && cached
                    .append_checkpoint
                    .as_ref()
                    .is_some_and(|checkpoint| checkpoint.still_matches(&path))
        });
        if !appended {
            cached.activity = Default::default();
        }
        let append_checkpoint = AppendCheckpoint::read(&path, version.len).ok();
        let activity_ok = cached.activity.read_append(&path, session_id).is_ok();
        let mut snapshot = activity_ok
            .then(|| cached.activity.usage_snapshot.clone())
            .flatten()
            .unwrap_or_else(|| ConversationStatsSnapshot {
                usage_source: "unknown",
                usage_scope: "unknown",
                ..Default::default()
            });
        if activity_ok {
            cached.activity.apply(&mut snapshot);
        }
        cached.snapshot = (snapshot.total_tokens.is_some()
            || snapshot.input_tokens.is_some()
            || snapshot.output_tokens.is_some()
            || snapshot.turn_count.is_some()
            || snapshot.step_count.is_some()
            || snapshot.tool_duration_ms.is_some()
            || snapshot.last_turn.is_some())
        .then_some(snapshot);
        // 读取中失败不能固化偏移和版本，下次轮询重新读取。
        cached.version = activity_ok.then_some(version);
        cached.append_checkpoint = activity_ok.then_some(append_checkpoint).flatten();
    }
    cached.snapshot.clone()
}

fn canonical_rollout(home: &Path, path: &Path) -> Option<PathBuf> {
    path.canonicalize()
        .ok()
        .filter(|path| path.starts_with(home) && path.is_file())
}

fn locate_rollout(home: &Path, session_id: &str) -> Option<PathBuf> {
    for db in codex_session_db_paths_from_home(home) {
        let Ok(connection) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        else {
            continue;
        };
        let _ = connection.busy_timeout(Duration::from_millis(100));
        let path = connection
            .query_row(
                "SELECT rollout_path FROM threads WHERE id=?1 LIMIT 1",
                [session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten();
        if let Some(path) = path {
            let path = home.join(path);
            if let Some(path) = canonical_rollout(home, &path) {
                return Some(path);
            }
        }
    }
    // 没有目录映射时，同一会话可能保留多个恢复副本；选择最近写入的副本。
    crate::message_delete::find_rollout_files_by_session_id(home, session_id)
        .into_iter()
        .filter_map(|path| {
            let path = canonical_rollout(home, &path)?;
            let modified = path.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max()
        .map(|(_, path)| path)
}

#[cfg(test)]
fn read_rollout(
    path: &Path,
    session_id: &str,
) -> std::io::Result<Option<ConversationStatsSnapshot>> {
    let mut activity = super::activity::Activity::default();
    activity.read_append(path, session_id)?;
    Ok(activity.usage_snapshot)
}

#[cfg(test)]
fn parse_tail(bytes: &[u8], session_id: &str) -> Option<ConversationStatsSnapshot> {
    let mut activity = super::activity::Activity::default();
    for line in bytes.split(|byte| *byte == b'\n') {
        if let Ok(record) = serde_json::from_slice::<Value>(line) {
            activity.record(&record, session_id);
        }
    }
    activity.usage_snapshot
}

pub(super) fn usage_from_record(
    record: &Value,
    session_id: &str,
) -> Option<ConversationStatsSnapshot> {
    let payload = &record["payload"];
    let usage = match record["type"].as_str() {
        Some("event_msg") if payload["type"] == "token_count" => {
            &payload["info"]["total_token_usage"]
        }
        Some("token_usage_record")
            if payload["session_id"]
                .as_str()
                .is_some_and(|id| normalize_session_id(id).eq_ignore_ascii_case(session_id)) =>
        {
            &payload["thread_token_usage"]
        }
        _ => return None,
    };
    let input = usage["input_tokens"].as_u64();
    let output = usage["output_tokens"].as_u64();
    let total = usage["total_tokens"]
        .as_u64()
        .or_else(|| Some(input?.saturating_add(output?)));
    if input.is_none() && output.is_none() && total.is_none() {
        return None;
    }
    Some(ConversationStatsSnapshot {
        input_tokens: input,
        output_tokens: output,
        total_tokens: total,
        cached_input_tokens: usage["cached_input_tokens"].as_u64(),
        cache_write_input_tokens: usage["cache_write_input_tokens"].as_u64(),
        usage_source: "rollout",
        usage_scope: "session",
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route_request_log::{SessionGenerationTotals, SessionRecordedTiming};
    use serde_json::json;
    const ID: &str = "01a1471c-962b-7d00-a9c4-0658dd26dd46";

    #[test]
    fn last_turn_history_restores_window_and_merges_runtime_once() {
        use crate::conversation_stats::ConversationStatsRegistry;
        let home = tempfile::tempdir().unwrap();
        let log_root = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-test-{ID}.jsonl"));
        let record = |second: u32, payload: Value| json!({"type":"event_msg", "timestamp":format!("2026-10-18T00:00:{second:02}Z"), "payload":payload});
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n{}\n",
                record(0, json!({"type":"task_started","turn_id":"old"})),
                record(5, json!({"type":"task_complete","turn_id":"old"})),
                record(10, json!({"type":"task_started","turn_id":"last"})),
                record(
                    20,
                    json!({"type":"task_complete","turn_id":"last","duration_ms":10000})
                )
            ),
        )
        .unwrap();
        let history = cached_history(home.path(), ID).unwrap();
        assert_eq!(history.turn_count, Some(2));
        let last = history.last_turn.as_ref().unwrap();
        let start = last.started_at_unix_ms.unwrap();
        let connection = Connection::open(log_root.path().join("route-requests.sqlite3")).unwrap();
        connection.execute_batch("CREATE TABLE route_request_logs (
            request_id TEXT PRIMARY KEY, codex_session_id TEXT, timestamp_unix_ms INTEGER,
            request_kind TEXT, codex_session_is_parent INTEGER, total_duration_ms INTEGER,
            router_pre_upstream_ms INTEGER, downstream_first_content_ms INTEGER,
            upstream_first_byte_ms INTEGER, output_tokens INTEGER);
            CREATE INDEX idx_route_request_logs_session_time ON route_request_logs(codex_session_id, timestamp_unix_ms);").unwrap();
        let insert = |id: &str, timestamp: u64, output: u64, duration: u64, ttft: u64| {
            connection.execute("INSERT INTO route_request_logs VALUES (?1,?2,?3,'responses',0,?4,100,?5,200,?6)",
                rusqlite::params![id, ID, timestamp, duration+300, ttft, output]).unwrap();
        };
        insert("old", start - 5000, 99999, 99999, 999);
        insert("historical-step", start + 1000, 100, 1000, 100);
        let registry = ConversationStatsRegistry::default();
        let mut entry = super::super::tests::sample_entry(Some(ID), "responses");
        entry.request_id = "runtime-step".into();
        entry.timestamp_unix_ms = start + 5000;
        entry.router_pre_upstream_ms = Some(100);
        entry.upstream_first_byte_ms = Some(200);
        entry.total_duration_ms = 3300;
        entry.downstream_first_content_ms = Some(300);
        entry.token_usage.output_tokens = Some(600);
        registry.finish_request(&entry);
        let runtime = registry.snapshot_with_turn(ID, history.last_turn.clone());
        let mut merged = merge(Some(history.clone()), runtime).unwrap();
        assert_eq!(merged.turn_count, Some(2));
        assert_eq!(merged.activity_scope, Some("session"));
        let last = merged.last_turn.as_ref().unwrap();
        let window = TurnWindow::from_snapshot(last).unwrap();
        let boundary = last.runtime_boundary.clone();
        let historical = crate::route_request_log::last_turn_recorded_stats(
            log_root.path(),
            ID,
            &window,
            boundary.as_ref(),
        );
        apply_recorded_turn(&mut merged, historical);
        let result = merged.last_turn.as_ref().unwrap();
        assert_eq!(result.average_output_tokens_per_second, Some(175.0));
        assert_eq!(result.model_duration_ms, Some(4400));
        assert_eq!(result.average_ttft_ms, Some(200.0));
        assert_eq!(result.step_count, Some(2));
        assert_eq!(result.coverage, "recorded_and_runtime");
        insert("runtime-step", start + 5000, 600, 3000, 300);
        let after_flush = crate::route_request_log::last_turn_recorded_stats(
            log_root.path(),
            ID,
            &window,
            boundary.as_ref(),
        );
        assert_eq!(historical, after_flush);
        let mut restored = history;
        apply_recorded_turn(
            &mut restored,
            crate::route_request_log::last_turn_recorded_stats(log_root.path(), ID, &window, None),
        );
        let restored = restored.last_turn.unwrap();
        assert_eq!(restored.average_output_tokens_per_second, Some(175.0));
        assert_eq!(restored.model_duration_ms, Some(4400));
        assert_eq!(restored.coverage, "recorded");
    }

    #[test]
    fn last_turn_cache_hits_avoid_queries_and_invalidate_window_or_runtime_boundary() {
        let mut cache = CachedTurn::default();
        let now = Instant::now();
        let mut window = TurnWindow {
            id: "last".into(),
            started_at_unix_ms: 100,
            completed_at_unix_ms: None,
        };
        let mut queries = 0;
        for _ in 0..10_000 {
            cache.get_or_query(&window, None, now, || {
                queries += 1;
                None
            });
        }
        assert_eq!(queries, 1);
        window.completed_at_unix_ms = Some(500);
        cache.get_or_query(&window, None, now, || {
            queries += 1;
            None
        });
        window.id = "next".into();
        window.started_at_unix_ms = 600;
        window.completed_at_unix_ms = None;
        cache.get_or_query(&window, None, now, || {
            queries += 1;
            None
        });
        let boundary = RuntimeHistoryBoundary {
            timestamp_unix_ms: 650,
            request_ids: std::collections::HashSet::from(["request".into()]),
            exclude_entire_millisecond: false,
        };
        cache.get_or_query(&window, Some(&boundary), now, || {
            queries += 1;
            None
        });
        assert_eq!(queries, 4);
        cache.get_or_query(
            &window,
            Some(&boundary),
            now + Duration::from_secs(30),
            || {
                queries += 1;
                None
            },
        );
        assert_eq!(queries, 5);
    }

    fn event(total: u64) -> String {
        json!({"type":"event_msg","timestamp":"2026-10-17T00:00:00Z","payload":{
        "type":"token_count","info":{"total_token_usage":{
            "input_tokens":total-20,"output_tokens":20,"total_tokens":total,
            "cached_input_tokens":10
        }}}})
        .to_string()
    }

    fn native_stats(id: &str) -> crate::conversation_stats_native::NativeTurnStats {
        crate::conversation_stats_native::NativeTurnStats {
            turn_id: id.into(),
            latest_sample_started_at_unix_ms: 1_000,
            model_duration_ms: 4_000,
            average_ttft_ms: Some(250.0),
            average_output_tokens_per_second: Some(75.0),
            request_count: 2,
            ttft_sample_count: 2,
            average_speed_request_count: 2,
            average_speed_updated_at_unix_ms: Some(2_000),
            updated_at_unix_ms: 2_000,
        }
    }

    fn native_target(id: &str) -> ConversationStatsSnapshot {
        ConversationStatsSnapshot {
            last_turn: Some(LastTurnStatsSnapshot {
                id: id.into(),
                started_at_unix_ms: Some(100),
                status: "running",
                coverage: "unknown",
                ..Default::default()
            }),
            usage_source: "unknown",
            usage_scope: "unknown",
            ..Default::default()
        }
    }

    #[test]
    fn native_turn_fills_only_matching_metrics_without_recounting_rollout() {
        let mut snapshot = parse_tail(event(1_000).as_bytes(), ID).unwrap();
        snapshot.turn_count = Some(3);
        snapshot.step_count = Some(4);
        snapshot.tool_duration_ms = Some(6_000);
        snapshot.last_turn = native_target("turn").last_turn;
        let turn = snapshot.last_turn.as_mut().unwrap();
        turn.step_count = Some(4);
        turn.tool_duration_ms = Some(6_000);
        apply_native_turn(&mut snapshot, Some(native_stats("turn")));
        assert_eq!(snapshot.total_tokens, Some(1_000));
        assert_eq!(snapshot.output_tokens, Some(20));
        assert_eq!(snapshot.turn_count, Some(3));
        assert_eq!(snapshot.step_count, Some(4));
        assert_eq!(snapshot.tool_duration_ms, Some(6_000));
        assert_eq!(snapshot.average_output_tokens_per_second, None);
        let turn = snapshot.last_turn.unwrap();
        assert_eq!(turn.step_count, Some(4));
        assert_eq!(turn.tool_duration_ms, Some(6_000));
        assert_eq!(turn.model_duration_ms, Some(4_000));
        assert_eq!(turn.average_ttft_ms, Some(250.0));
        assert_eq!(turn.average_output_tokens_per_second, Some(75.0));
        assert_eq!(turn.coverage, "partial");
        let json = serde_json::to_value(turn).unwrap();
        assert_eq!(json["timingSource"], "codex");
        assert_eq!(json["averageSpeedSource"], "codex");
        assert_eq!(json["ttftBoundary"], "first_output_item");
        assert!(json.get("routeTimingUpdatedAtUnixMs").is_none());
    }

    #[test]
    fn native_and_route_turn_sources_choose_latest_valid_groups_independently() {
        for (route_at, native_at, source) in [
            (1_000, 2_000, Some("codex")),
            (3_000, 2_000, None),
            (2_000, 2_000, None),
        ] {
            let mut snapshot = native_target("turn");
            let turn = snapshot.last_turn.as_mut().unwrap();
            turn.model_duration_ms = Some(1_000);
            turn.average_ttft_ms = Some(100.0);
            turn.average_output_tokens_per_second = Some(100.0);
            turn.route_timing_updated_at_unix_ms = Some(route_at);
            turn.route_speed_updated_at_unix_ms = Some(route_at);
            let mut native = native_stats("turn");
            native.updated_at_unix_ms = native_at;
            native.average_speed_updated_at_unix_ms = Some(native_at);
            apply_native_turn(&mut snapshot, Some(native));
            let turn = snapshot.last_turn.unwrap();
            assert_eq!(turn.timing_source, source);
            assert_eq!(turn.average_speed_source, source);
            assert_eq!(
                turn.model_duration_ms,
                Some(if source.is_some() { 4_000 } else { 1_000 })
            );
            assert_eq!(
                turn.average_output_tokens_per_second,
                Some(if source.is_some() { 75.0 } else { 100.0 })
            );
        }
        let mut snapshot = native_target("turn");
        let turn = snapshot.last_turn.as_mut().unwrap();
        turn.model_duration_ms = Some(1_000);
        turn.average_output_tokens_per_second = Some(100.0);
        turn.route_timing_updated_at_unix_ms = Some(2_000);
        turn.route_speed_updated_at_unix_ms = Some(2_000);
        let mut native = native_stats("turn");
        native.updated_at_unix_ms = 3_000;
        native.average_speed_updated_at_unix_ms = Some(1_000);
        apply_native_turn(&mut snapshot, Some(native));
        let turn = snapshot.last_turn.unwrap();
        assert_eq!(turn.timing_source, Some("codex"));
        assert_eq!(turn.average_speed_source, None);
        assert_eq!(turn.average_output_tokens_per_second, Some(100.0));
    }

    #[test]
    fn native_turn_never_overwrites_active_routes_or_invents_missing_or_foreign_metrics() {
        let mut snapshot = native_target("new");
        for native in [None, Some(native_stats("old"))] {
            apply_native_turn(&mut snapshot, native);
            let turn = snapshot.last_turn.as_ref().unwrap();
            assert_eq!(turn.model_duration_ms, None);
            assert_eq!(turn.average_output_tokens_per_second, None);
        }
        snapshot.last_turn.as_mut().unwrap().in_flight = Some(1);
        apply_native_turn(&mut snapshot, Some(native_stats("new")));
        let turn = snapshot.last_turn.unwrap();
        assert_eq!(turn.model_duration_ms, None);
        assert_eq!(turn.average_output_tokens_per_second, None);
        for speed in [None, Some(f64::NAN), Some(f64::INFINITY), Some(-1.0)] {
            let mut snapshot = native_target("new");
            let mut native = native_stats("new");
            native.average_output_tokens_per_second = speed;
            apply_native_turn(&mut snapshot, Some(native));
            assert_eq!(
                snapshot.last_turn.unwrap().average_output_tokens_per_second,
                None
            );
        }
    }

    #[test]
    fn native_only_turn_has_no_fabricated_user_boundaries_and_cannot_replace_unknown_or_newer_rollout()
     {
        let native = native_stats("native");
        assert!(select_last_turn(None, Some(&native), true).is_none());
        let turn = select_last_turn(None, Some(&native), false).unwrap();
        assert_eq!(turn.id, "native");
        assert_eq!(turn.status, "unknown");
        assert_eq!(turn.started_at_unix_ms, None);
        assert_eq!(turn.completed_at_unix_ms, None);
        assert_eq!(turn.coverage, "partial");
        for (start, active, id) in [
            (None, false, "rollout"),
            (Some(3_000), false, "rollout"),
            (Some(100), true, "rollout"),
            (Some(100), false, "native"),
        ] {
            let mut turn = native_target("rollout").last_turn.unwrap();
            turn.started_at_unix_ms = start;
            assert_eq!(
                select_last_turn(Some(turn), Some(&native), active)
                    .unwrap()
                    .id,
                id
            );
        }
    }

    #[test]
    fn average_cache_reuses_history_until_ttl_or_boundary_changes() {
        let mut cache = CachedAverage::default();
        let now = Instant::now();
        let mut queries = 0;
        let totals = SessionGenerationTotals {
            output_tokens: 10.0,
            generation_ms: 1000.0,
            requests: 1,
        };
        let mut boundary = RuntimeHistoryBoundary {
            timestamp_unix_ms: 1000,
            request_ids: std::collections::HashSet::from(["a".to_owned()]),
            exclude_entire_millisecond: false,
        };
        {
            let mut query = || {
                queries += 1;
                Some(totals)
            };
            for second in 0..30 {
                assert_eq!(
                    cache.get_or_query(
                        Some(&boundary),
                        now + Duration::from_secs(second),
                        &mut query
                    ),
                    Some(totals)
                );
            }
        }
        assert_eq!(queries, 1);
        let hit_started = Instant::now();
        for _ in 0..10_000 {
            cache.get_or_query(Some(&boundary), now + Duration::from_secs(29), || {
                queries += 1;
                Some(totals)
            });
        }
        eprintln!(
            "session average: 10000 in-memory cache hits: {:?}; query calls including first lookup: {queries}",
            hit_started.elapsed()
        );
        assert_eq!(queries, 1);
        // 同毫秒新增运行期请求必须立刻更新排除条件，不能等待 TTL。
        boundary.request_ids.insert("b".into());
        cache.get_or_query(Some(&boundary), now + Duration::from_secs(29), || {
            queries += 1;
            Some(totals)
        });
        assert_eq!(queries, 2);
        cache.get_or_query(Some(&boundary), now + Duration::from_secs(59), || {
            queries += 1;
            None
        });
        cache.get_or_query(Some(&boundary), now + Duration::from_secs(60), || {
            queries += 1;
            Some(totals)
        });
        assert_eq!(queries, 3, "missing logs are also cached");
        boundary.timestamp_unix_ms = 999;
        cache.get_or_query(Some(&boundary), now + Duration::from_secs(60), || {
            queries += 1;
            Some(totals)
        });
        assert_eq!(queries, 4);
    }

    #[test]
    fn average_history_and_runtime_merge_without_recounting_delayed_logs() {
        use crate::conversation_stats::{ConversationStatsRegistry, LiveSpeedSample};
        use crate::route_request_log::session_recorded_average;
        let root = tempfile::tempdir().unwrap();
        let connection = Connection::open(root.path().join("route-requests.sqlite3")).unwrap();
        connection.execute_batch("CREATE TABLE route_request_logs (
            request_id TEXT PRIMARY KEY, codex_session_id TEXT, timestamp_unix_ms INTEGER,
            request_kind TEXT, codex_session_is_parent INTEGER, output_tokens INTEGER,
            total_duration_ms INTEGER, router_pre_upstream_ms INTEGER, upstream_first_byte_ms INTEGER);
            CREATE INDEX idx_route_request_logs_session_time ON route_request_logs(codex_session_id, timestamp_unix_ms);").unwrap();
        let insert = |id: &str, session: &str, timestamp: u64, output: u64, duration: u64| {
            connection
                .execute(
                    "INSERT INTO route_request_logs VALUES (?1,?2,?3,'responses',0,?4,?5,100,200)",
                    rusqlite::params![id, session, timestamp, output, duration + 300],
                )
                .unwrap();
        };
        insert("historical", ID, 100, 100, 1000);
        insert("foreign", "foreign", 100, 9999, 1);
        let historical = session_recorded_average(root.path(), ID, None);
        assert_eq!(
            apply_recorded_average(None, historical)
                .unwrap()
                .average_output_tokens_per_second,
            Some(100.0)
        );
        let registry = ConversationStatsRegistry::default();
        registry.begin_request(ID, "new", 200);
        let waiting = registry.snapshot(ID).unwrap();
        let boundary = waiting.average_history_boundary.clone().unwrap();
        let mut cache = CachedAverage::default();
        let now = Instant::now();
        let recorded = cache.get_or_query(Some(&boundary), now, || {
            session_recorded_average(root.path(), ID, Some(&boundary))
        });
        let waiting = apply_recorded_average(Some(waiting), recorded).unwrap();
        assert_eq!(waiting.average_output_tokens_per_second, Some(100.0));
        assert_eq!(waiting.average_speed_scope, Some("recorded_requests"));
        registry.update_request_speed(
            ID,
            "new",
            LiveSpeedSample {
                output_tokens: 300.0,
                generation_started_at: now - Duration::from_secs(1),
                is_estimate: true,
            },
            201,
        );
        let running = registry.snapshot_at(ID, now).unwrap();
        let running = apply_recorded_average(
            merge(parse_tail(event(1_000_000).as_bytes(), ID), Some(running)),
            recorded,
        )
        .unwrap();
        assert_eq!(running.average_output_tokens_per_second, Some(200.0));
        assert_eq!(running.average_speed_scope, Some("recorded_and_runtime"));
        assert!(running.average_speed_is_estimate);
        assert_eq!(running.total_tokens, Some(1_000_000));
        let mut entry = super::super::tests::sample_entry(Some(ID), "responses");
        entry.request_id = "new".into();
        entry.timestamp_unix_ms = 200;
        entry.token_usage.output_tokens = Some(600);
        entry.router_pre_upstream_ms = Some(100);
        entry.upstream_first_byte_ms = Some(200);
        entry.total_duration_ms = 3300;
        registry.finish_request(&entry);
        // 完成后日志尚未落盘，运行期已能立即给出整个会话的加权值。
        let completed = apply_recorded_average(registry.snapshot(ID), recorded).unwrap();
        assert_eq!(completed.average_output_tokens_per_second, Some(175.0));
        assert!(!completed.average_speed_is_estimate);
        insert("new", ID, 200, 600, 3000);
        // 同毫秒的历史请求仍保留，只排除跟踪到的那个请求 ID。
        insert("same-ms-historical", ID, 200, 0, 1000);
        let refreshed = cache.get_or_query(Some(&boundary), now + Duration::from_secs(30), || {
            session_recorded_average(root.path(), ID, Some(&boundary))
        });
        let done = apply_recorded_average(registry.snapshot(ID), refreshed).unwrap();
        assert_eq!(done.average_output_tokens_per_second, Some(140.0));
        assert_eq!(done.average_speed_request_count, Some(3));
        assert_eq!(done.average_speed_source, Some("runtime_and_request_log"));
        registry.begin_request(ID, "next", 300);
        let next = apply_recorded_average(registry.snapshot(ID), refreshed).unwrap();
        assert_eq!(next.average_output_tokens_per_second, Some(140.0));
        assert_eq!(next.speed_state, Some("waiting"));
        // 丢失或关闭日志时保留真实运行期累计，范围明确为 runtime。
        let runtime_only = apply_recorded_average(registry.snapshot(ID), None).unwrap();
        assert_eq!(runtime_only.average_output_tokens_per_second, Some(200.0));
        assert_eq!(runtime_only.average_speed_scope, Some("runtime"));
    }

    #[test]
    fn activity_only_history_keeps_runtime_usage_and_combines_disjoint_timing() {
        let history = ConversationStatsSnapshot {
            turn_count: Some(2),
            step_count: Some(3),
            step_source: Some("rollout"),
            usage_source: "unknown",
            usage_scope: "unknown",
            ..Default::default()
        };
        let runtime = ConversationStatsSnapshot {
            total_tokens: Some(10),
            usage_source: "runtime",
            usage_scope: "runtime",
            model_duration_ms: Some(1000),
            average_ttft_ms: Some(200.0),
            timing_request_count: Some(1),
            ttft_sample_count: Some(1),
            timing_source: Some("runtime"),
            timing_scope: Some("runtime"),
            ..Default::default()
        };
        let merged = merge(Some(history), Some(runtime));
        let timing = crate::route_request_log::SessionRecordedTiming {
            requests: 4,
            model_duration_ms: Some(5000),
            average_ttft_ms: Some(300.0),
            timing_request_count: 4,
            ttft_sample_count: 4,
        };
        let snapshot = apply_recorded_timing(merged, Some(timing)).unwrap();
        assert_eq!(snapshot.total_tokens, Some(10));
        assert_eq!(snapshot.turn_count, Some(2));
        assert_eq!(snapshot.step_count, Some(3));
        assert_eq!(snapshot.model_duration_ms, Some(6000));
        assert_eq!(snapshot.average_ttft_ms, Some(280.0));
        assert_eq!(snapshot.timing_request_count, Some(5));
        assert_eq!(snapshot.ttft_sample_count, Some(5));
        assert_eq!(snapshot.timing_scope, Some("recorded_and_runtime"));
    }

    #[test]
    fn recorded_timing_preserves_zero_duration_and_weights_valid_ttft_samples() {
        let timing = crate::route_request_log::SessionRecordedTiming {
            requests: 100,
            model_duration_ms: Some(500_000),
            average_ttft_ms: Some(3000.0),
            timing_request_count: 100,
            ttft_sample_count: 2,
        };
        let recovered = apply_recorded_timing(None, Some(timing)).unwrap();
        assert_eq!(recovered.step_count, Some(100));
        assert_eq!(recovered.model_duration_ms, Some(500_000));
        assert_eq!(recovered.timing_scope, Some("recorded_requests"));
        assert_eq!(recovered.total_tokens, None);

        let runtime = ConversationStatsSnapshot {
            step_count: Some(1),
            step_source: Some("runtime"),
            step_scope: Some("runtime"),
            model_duration_ms: Some(0),
            average_ttft_ms: Some(0.0),
            timing_request_count: Some(1),
            ttft_sample_count: Some(1),
            timing_source: Some("runtime"),
            timing_scope: Some("runtime"),
            ..Default::default()
        };
        let current = apply_recorded_timing(Some(runtime), Some(timing)).unwrap();
        assert_eq!(current.step_count, Some(1));
        assert_eq!(current.step_scope, Some("runtime"));
        assert_eq!(current.model_duration_ms, Some(500_000));
        assert_eq!(current.average_ttft_ms, Some(2000.0));
        assert_eq!(current.timing_scope, Some("recorded_and_runtime"));
        assert_eq!(current.timing_request_count, Some(101));
        assert_eq!(current.ttft_sample_count, Some(3));
    }

    #[test]
    fn recorded_timing_merges_each_available_field_without_filling_unknowns_with_zero() {
        let runtime_model = ConversationStatsSnapshot {
            model_duration_ms: Some(500),
            timing_request_count: Some(1),
            ..Default::default()
        };
        let recorded_ttft = SessionRecordedTiming {
            requests: 3,
            average_ttft_ms: Some(200.0),
            ttft_sample_count: 2,
            ..Default::default()
        };
        let merged = apply_recorded_timing(Some(runtime_model), Some(recorded_ttft)).unwrap();
        assert_eq!(merged.model_duration_ms, Some(500));
        assert_eq!(merged.timing_request_count, Some(1));
        assert_eq!(merged.average_ttft_ms, Some(200.0));
        assert_eq!(merged.ttft_sample_count, Some(2));
        assert_eq!(merged.timing_source, Some("runtime_and_request_log"));

        let runtime_ttft = ConversationStatsSnapshot {
            average_ttft_ms: Some(800.0),
            ttft_sample_count: Some(1),
            ..Default::default()
        };
        let recorded_model = SessionRecordedTiming {
            requests: 3,
            model_duration_ms: Some(1000),
            timing_request_count: 2,
            ..Default::default()
        };
        let merged = apply_recorded_timing(Some(runtime_ttft), Some(recorded_model)).unwrap();
        assert_eq!(merged.model_duration_ms, Some(1000));
        assert_eq!(merged.timing_request_count, Some(2));
        assert_eq!(merged.average_ttft_ms, Some(800.0));
        assert_eq!(merged.ttft_sample_count, Some(1));

        let unknown = apply_recorded_timing(
            None,
            Some(SessionRecordedTiming {
                requests: 1,
                ..Default::default()
            }),
        )
        .unwrap();
        assert_eq!(unknown.model_duration_ms, None);
        assert_eq!(unknown.average_ttft_ms, None);
        assert_eq!(unknown.timing_request_count, None);
        assert_eq!(unknown.ttft_sample_count, None);
        let only_model = apply_recorded_timing(None, Some(recorded_model)).unwrap();
        assert_eq!(only_model.average_ttft_ms, None);
        assert_eq!(only_model.ttft_sample_count, None);
        let only_ttft = apply_recorded_timing(None, Some(recorded_ttft)).unwrap();
        assert_eq!(only_ttft.model_duration_ms, None);
        assert_eq!(only_ttft.timing_request_count, None);
    }

    #[test]
    fn historical_timing_survives_short_runtime_request_log_flush_and_next_step() {
        use crate::conversation_stats::ConversationStatsRegistry;
        use crate::route_request_log::session_recorded_timing;
        let root = tempfile::tempdir().unwrap();
        let connection = Connection::open(root.path().join("route-requests.sqlite3")).unwrap();
        connection.execute_batch("CREATE TABLE route_request_logs (
            request_id TEXT PRIMARY KEY, codex_session_id TEXT, timestamp_unix_ms INTEGER,
            request_kind TEXT, codex_session_is_parent INTEGER, total_duration_ms INTEGER,
            router_pre_upstream_ms INTEGER, downstream_first_content_ms INTEGER);
            CREATE INDEX idx_route_request_logs_session_time ON route_request_logs(codex_session_id, timestamp_unix_ms);").unwrap();
        let insert = |id: &str, timestamp: u64, duration: u64, ttft: u64| {
            connection
                .execute(
                    "INSERT INTO route_request_logs VALUES (?1,?2,?3,'responses',0,?4,100,?5)",
                    rusqlite::params![id, ID, timestamp, duration + 100, ttft],
                )
                .unwrap();
        };
        insert("historical-1", 100, 10_000, 100);
        insert("historical-2", 150, 20_000, 300);
        let registry = ConversationStatsRegistry::default();
        registry.begin_request(ID, "runtime", 200);
        let waiting = registry.snapshot(ID).unwrap();
        let boundary = waiting.average_history_boundary.clone().unwrap();
        let recorded = session_recorded_timing(root.path(), ID, Some(&boundary));
        let waiting = apply_recorded_timing(Some(waiting), recorded).unwrap();
        assert_eq!(waiting.model_duration_ms, Some(30_000));
        assert_eq!(waiting.average_ttft_ms, Some(200.0));
        assert_eq!(waiting.timing_scope, Some("recorded_requests"));
        assert_eq!(waiting.speed_state, Some("waiting"));

        let mut entry = super::super::tests::sample_entry(Some(ID), "responses");
        entry.request_id = "runtime".into();
        entry.timestamp_unix_ms = 200;
        entry.router_pre_upstream_ms = Some(100);
        entry.total_duration_ms = 600;
        entry.downstream_first_content_ms = Some(500);
        registry.finish_request(&entry);
        let finished = apply_recorded_timing(registry.snapshot(ID), recorded).unwrap();
        assert_eq!(finished.model_duration_ms, Some(30_500));
        assert_eq!(finished.average_ttft_ms, Some(300.0));
        assert_eq!(finished.timing_request_count, Some(3));
        assert_eq!(finished.ttft_sample_count, Some(3));
        assert_eq!(finished.timing_scope, Some("recorded_and_runtime"));
        insert("runtime", 200, 500, 500);
        let flushed = session_recorded_timing(root.path(), ID, Some(&boundary));
        let after_flush = apply_recorded_timing(registry.snapshot(ID), flushed).unwrap();
        assert_eq!(after_flush.model_duration_ms, finished.model_duration_ms);
        assert_eq!(after_flush.average_ttft_ms, finished.average_ttft_ms);
        assert_eq!(after_flush.ttft_sample_count, finished.ttft_sample_count);
        registry.begin_request(ID, "next", 300);
        let next = apply_recorded_timing(registry.snapshot(ID), flushed).unwrap();
        assert_eq!(next.model_duration_ms, Some(30_500));
        assert_eq!(next.average_ttft_ms, Some(300.0));
        assert_eq!(next.speed_state, Some("waiting"));
        let runtime_only = apply_recorded_timing(registry.snapshot(ID), None).unwrap();
        assert_eq!(runtime_only.model_duration_ms, Some(500));
        assert_eq!(runtime_only.average_ttft_ms, Some(500.0));
        assert_eq!(runtime_only.timing_scope, Some("runtime"));
        assert_eq!(runtime_only.timing_source, Some("runtime"));
    }

    #[test]
    fn timing_cache_reuses_results_and_invalidates_every_boundary_change() {
        let mut cache = CachedLog::<SessionRecordedTiming>::default();
        let now = Instant::now();
        let mut queries = 0;
        let mut query = || {
            queries += 1;
            Some(SessionRecordedTiming {
                model_duration_ms: Some(500),
                ..Default::default()
            })
        };
        let mut boundary = RuntimeHistoryBoundary {
            timestamp_unix_ms: 200,
            request_ids: std::collections::HashSet::from(["a".into()]),
            exclude_entire_millisecond: false,
        };
        for second in 0..30 {
            let timing = cache
                .get_or_query(
                    Some(&boundary),
                    now + Duration::from_secs(second),
                    &mut query,
                )
                .unwrap();
            assert_eq!(timing.model_duration_ms, Some(500));
        }
        assert_eq!(queries, 1);
        for change in 0..3 {
            match change {
                0 => {
                    boundary.request_ids.insert("b".into());
                }
                1 => boundary.exclude_entire_millisecond = true,
                _ => boundary.timestamp_unix_ms = 199,
            }
            cache.get_or_query(Some(&boundary), now + Duration::from_secs(29), || {
                queries += 1;
                None
            });
        }
        assert_eq!(queries, 4);
        cache.get_or_query(Some(&boundary), now + Duration::from_secs(58), || {
            panic!("missing result should be cached")
        });
        cache.get_or_query(Some(&boundary), now + Duration::from_secs(59), || {
            queries += 1;
            None
        });
        assert_eq!(queries, 5);
    }

    #[test]
    fn recorded_speed_is_independent_of_usage_and_runtime_request_state() {
        let speed = Some((25.0, 123));
        let recovered = apply_recorded_speed(None, speed).unwrap();
        assert_eq!(recovered.output_tokens_per_second, Some(25.0));
        assert_eq!(recovered.speed_source, Some("request_log"));
        assert_eq!(recovered.speed_scope, Some("latest_recorded_request"));
        assert_eq!(recovered.speed_recorded_at_unix_ms, Some(123));
        assert_eq!(recovered.model, None);
        assert_eq!(recovered.total_tokens, None);
        let history = parse_tail(event(100).as_bytes(), ID);
        let result = apply_recorded_speed(merge(history, None), speed).unwrap();
        assert_eq!(result.total_tokens, Some(100));
        assert_eq!(result.output_tokens_per_second, Some(25.0));
        let running = ConversationStatsSnapshot {
            in_flight: Some(1),
            ..Default::default()
        };
        assert_eq!(
            apply_recorded_speed(Some(running), speed)
                .unwrap()
                .output_tokens_per_second,
            None
        );
        let completed_without_speed = ConversationStatsSnapshot {
            in_flight: Some(0),
            ..Default::default()
        };
        assert_eq!(
            apply_recorded_speed(Some(completed_without_speed), speed)
                .unwrap()
                .speed_scope,
            Some("latest_recorded_request")
        );
        let live = ConversationStatsSnapshot {
            output_tokens_per_second: Some(50.0),
            speed_source: Some("runtime"),
            ..Default::default()
        };
        let result = apply_recorded_speed(Some(live), speed).unwrap();
        assert_eq!(result.output_tokens_per_second, Some(50.0));
        assert_eq!(result.speed_source, Some("runtime"));
        assert!(apply_recorded_speed(None, None).is_none());
    }

    #[test]
    fn latest_cumulative_record_replaces_duplicates_and_decreases() {
        let data = format!("{}\n{}\n{}\n{{\"type\":", event(100), event(100), event(60));
        let snapshot = parse_tail(data.as_bytes(), ID).unwrap();
        assert_eq!(snapshot.total_tokens, Some(60));
        assert_eq!(snapshot.input_tokens, Some(40));
        assert_eq!(snapshot.cached_input_tokens, Some(10));
        assert_eq!(snapshot.cache_write_input_tokens, None);
        assert_eq!(snapshot.requests, None);
        assert_eq!(snapshot.in_flight, None);
        assert_eq!(snapshot.model, None);
    }

    #[test]
    fn thread_usage_records_require_own_session_and_ignore_per_turn_values() {
        let own = json!({"type":"token_usage_record","payload":{
            "session_id":ID,"usage":{"total_tokens":800},
            "turn_token_usage":{"total_tokens":900},"thread_token_usage":{"total_tokens":42}}});
        let foreign = json!({"type":"token_usage_record","payload":{
            "session_id":"other","thread_token_usage":{"total_tokens":9999}}});
        let data = format!("{}\n{own}\n{foreign}\n", event(100));
        let snapshot = parse_tail(data.as_bytes(), ID).unwrap();
        assert_eq!(snapshot.total_tokens, Some(42));
        assert_eq!(snapshot.input_tokens, None);
        assert_eq!(snapshot.cached_input_tokens, None);
    }

    #[test]
    fn native_usage_ignores_foreign_segments_and_explicit_foreign_ids() {
        let foreign_meta = json!({"type":"session_meta","payload":{"id":"other"}});
        let own_meta = json!({"type":"session_meta","payload":{"id":ID}});
        let foreign_count = json!({"type":"event_msg","payload":{
            "type":"token_count","thread_id":"other",
            "info":{"total_token_usage":{"total_tokens":9999}}
        }});
        let data = format!(
            "{foreign_meta}\n{}\n{own_meta}\n{}\n{foreign_count}\n{foreign_meta}\n{}\n",
            event(900),
            event(100),
            event(800)
        );
        assert_eq!(
            parse_tail(data.as_bytes(), ID).unwrap().total_tokens,
            Some(100)
        );
        assert!(parse_tail(format!("{foreign_meta}\n{}\n", event(900)).as_bytes(), ID).is_none());
    }

    #[test]
    fn database_mapping_to_another_sessions_file_does_not_return_its_usage() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("other-session.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                json!({"type":"session_meta","payload":{"id":"other"}}),
                event(900)
            ),
        )
        .unwrap();
        let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
        db.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO threads VALUES (?1, ?2)",
            [ID, path.to_str().unwrap()],
        )
        .unwrap();
        assert!(cached_history(home.path(), ID).is_none());
    }

    #[test]
    fn merge_uses_history_without_double_counting_and_exposes_runtime_scope() {
        let mut history = parse_tail(event(100).as_bytes(), ID).unwrap();
        // 模拟历史落盘晚于请求开始，不能因此丢掉运行期字段或串用历史时间。
        history.updated_at_unix_ms = 2_000;
        let runtime = ConversationStatsSnapshot {
            total_tokens: Some(20),
            requests: Some(1),
            in_flight: Some(0),
            requests_scope: Some("runtime"),
            model: Some("runtime-model".into()),
            output_tokens_per_second: Some(12.0),
            updated_at_unix_ms: 1_000,
            ..Default::default()
        };
        let merged = merge(Some(history), Some(runtime)).unwrap();
        assert_eq!(merged.total_tokens, Some(100));
        assert_eq!(merged.requests, Some(1));
        assert_eq!(merged.requests_scope, Some("runtime"));
        assert_eq!(merged.model.as_deref(), Some("runtime-model"));
        assert_eq!(merged.output_tokens_per_second, Some(12.0));
        assert_eq!(merged.updated_at_unix_ms, 1_000);
        assert_eq!(merged.usage_scope, "session");
    }

    #[test]
    fn history_survives_empty_runtime_and_cache_invalidates_on_file_change() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions/2026/10/17");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-2026-10-17T00-00-00-{ID}.jsonl"));
        std::fs::write(&path, event(100)).unwrap();
        assert_eq!(
            cached_history(home.path(), ID).unwrap().total_tokens,
            Some(100)
        );
        std::fs::write(&path, format!("{}\n{}\n", event(100), event(200))).unwrap();
        let snapshot = cached_history(home.path(), ID).unwrap();
        assert_eq!(merge(Some(snapshot), None).unwrap().total_tokens, Some(200));
        assert!(cached_history(home.path(), "../secrets").is_none());
    }

    fn cached_read_metrics(home: &Path) -> (u64, u64, u64) {
        let key = (home.canonicalize().unwrap(), ID.to_owned());
        let entry = {
            let cache = lock_unpoisoned(CACHE.get().unwrap());
            Arc::clone(cache.get(&key).unwrap())
        };
        let cached = lock_unpoisoned(&entry);
        (
            cached.activity.last_read_start,
            cached.activity.last_read_bytes,
            cached.activity.offset,
        )
    }

    #[test]
    fn total_turn_count_reads_appends_once_and_rebuilds_after_truncation() {
        use std::io::Write;
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-turn-count-{ID}.jsonl"));
        let start = |id: &str, second: u32| json!({"type":"event_msg","timestamp":format!("2026-10-18T00:00:{second:02}Z"),"payload":{"type":"task_started","turn_id":id}});
        let initial = format!("{}\n{}\n", start("first", 0), start("second", 1));
        std::fs::write(&path, &initial).unwrap();
        assert_eq!(cached_history(home.path(), ID).unwrap().turn_count, Some(2));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let appended = start("third", 2).to_string();
        file.write_all(appended.as_bytes()).unwrap();
        assert_eq!(cached_history(home.path(), ID).unwrap().turn_count, Some(3));
        assert_eq!(
            cached_read_metrics(home.path()),
            (
                initial.len() as u64,
                appended.len() as u64,
                initial.len() as u64
            )
        );
        file.write_all(b"\n").unwrap();
        assert_eq!(cached_history(home.path(), ID).unwrap().turn_count, Some(3));
        let metrics = cached_read_metrics(home.path());
        for _ in 0..10 {
            assert_eq!(cached_history(home.path(), ID).unwrap().turn_count, Some(3));
            assert_eq!(cached_read_metrics(home.path()), metrics);
        }
        drop(file);
        let truncated = format!("{}\n", start("only", 3));
        std::fs::write(&path, &truncated).unwrap();
        assert_eq!(cached_history(home.path(), ID).unwrap().turn_count, Some(1));
        assert_eq!(
            cached_read_metrics(home.path()),
            (0, truncated.len() as u64, truncated.len() as u64)
        );
    }

    #[test]
    fn history_cache_restarts_after_same_inode_longer_rewrite() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-rewrite-{ID}.jsonl"));
        // 首部保持相同，要求文件尾部样本也能发现原地截断重写。
        let prefix = format!("{}\n", json!({"type":"padding","payload":"x".repeat(9000)}));
        let start = |id: &str| json!({"type":"event_msg","timestamp":"2026-10-18T00:00:00Z","payload":{"type":"task_started","turn_id":id}});
        let tool = |id: &str, seconds: u64| json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":id,"item":{"type":"McpToolCall","id":"tool","duration":{"secs":seconds,"nanos":0}}}});
        for suffix in ["\n".to_owned(), String::new(), "\n{\"type\":".to_owned()] {
            let old = format!("{prefix}{}\n{}{suffix}", start("old"), tool("old", 1));
            std::fs::write(&path, old).unwrap();
            let first = cached_history(home.path(), ID).unwrap();
            assert_eq!(first.turn_count, Some(1));
            let first = first.last_turn.unwrap();
            assert_eq!(first.id, "old");
            assert_eq!(first.tool_duration_ms, Some(1000));
            let before = FileVersion::from_path(&path).unwrap();

            let rewritten = format!("{prefix}{}\n{}\n{prefix}", start("new"), tool("new", 2));
            std::fs::write(&path, rewritten).unwrap();
            let after = FileVersion::from_path(&path).unwrap();
            assert!(after.len > before.len);
            #[cfg(any(unix, windows))]
            assert_eq!(
                (before.identity.0, before.identity.1),
                (after.identity.0, after.identity.1)
            );
            let current = cached_history(home.path(), ID).unwrap();
            assert_eq!(current.turn_count, Some(1));
            let current = current.last_turn.unwrap();
            assert_eq!(current.id, "new");
            assert_eq!(current.tool_duration_ms, Some(2000));
            assert_eq!(cached_read_metrics(home.path()), (0, after.len, after.len));
        }
    }

    #[test]
    fn history_cache_parses_only_appended_bytes_and_retries_unfinished_tail() {
        use std::io::Write;
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-append-{ID}.jsonl"));
        let prefix = format!(
            "{}\n{}\n",
            json!({"type":"padding","payload":"x".repeat(9000)}),
            json!({"type":"event_msg","timestamp":"2026-10-18T00:00:00Z","payload":{"type":"task_started","turn_id":"current"}})
        );
        let partial = "{\"type\":\"event_msg\",\"payload\":";
        std::fs::write(&path, format!("{prefix}{partial}")).unwrap();
        let first = cached_history(home.path(), ID).unwrap().last_turn.unwrap();
        assert_eq!(first.tool_duration_ms, None);
        let prefix_len = prefix.len() as u64;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let suffix = format!(
            "{}}}",
            json!({"type":"item_completed","turn_id":"current","item":{"type":"McpToolCall","id":"first","duration":{"secs":1,"nanos":0}}})
        );
        file.write_all(suffix.as_bytes()).unwrap();
        let current = cached_history(home.path(), ID).unwrap().last_turn.unwrap();
        assert_eq!(current.tool_duration_ms, Some(1000));
        let tail_len = (partial.len() + suffix.len()) as u64;
        assert_eq!(
            cached_read_metrics(home.path()),
            (prefix_len, tail_len, prefix_len)
        );

        file.write_all(b"\n").unwrap();
        let current = cached_history(home.path(), ID).unwrap().last_turn.unwrap();
        assert_eq!(current.tool_duration_ms, Some(1000));
        let completed_len = prefix_len + tail_len + 1;
        assert_eq!(
            cached_read_metrics(home.path()),
            (prefix_len, tail_len + 1, completed_len)
        );

        let appended = format!(
            "{}\n",
            json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"current","item":{"type":"McpToolCall","id":"second","duration":{"secs":2,"nanos":0}}}})
        );
        file.write_all(appended.as_bytes()).unwrap();
        let current = cached_history(home.path(), ID).unwrap().last_turn.unwrap();
        assert_eq!(current.tool_duration_ms, Some(3000));
        let expected = (
            completed_len,
            appended.len() as u64,
            completed_len + appended.len() as u64,
        );
        assert_eq!(cached_read_metrics(home.path()), expected);
        for _ in 0..10 {
            assert_eq!(
                cached_history(home.path(), ID)
                    .unwrap()
                    .last_turn
                    .unwrap()
                    .tool_duration_ms,
                Some(3000)
            );
            assert_eq!(cached_read_metrics(home.path()), expected);
        }
    }

    #[test]
    fn history_merge_preserves_current_request_speed_metadata() {
        let history = parse_tail(event(100).as_bytes(), ID).unwrap();
        let runtime = ConversationStatsSnapshot {
            in_flight: Some(1),
            requests_scope: Some("runtime"),
            output_tokens_per_second: Some(42.0),
            speed_source: Some("runtime"),
            speed_scope: Some("current_request"),
            speed_state: Some("streaming"),
            speed_is_estimate: true,
            ..Default::default()
        };
        let merged =
            apply_recorded_speed(merge(Some(history), Some(runtime)), Some((99.0, 1000))).unwrap();
        assert_eq!(merged.total_tokens, Some(100));
        assert_eq!(merged.output_tokens_per_second, Some(42.0));
        assert_eq!(merged.speed_scope, Some("current_request"));
        assert_eq!(merged.speed_state, Some("streaming"));
        assert!(merged.speed_is_estimate);
    }

    #[test]
    fn history_before_large_tool_output_is_still_available() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "{}", event(300)).unwrap();
        // 超大但无统计的记录，以及末尾未写完的内容，均不覆盖已有累计。
        file.write_all(&vec![b'x'; MAX_TAIL_BYTES as usize + 100])
            .unwrap();
        file.write_all(b"\n{\"type\":").unwrap();
        assert_eq!(
            read_rollout(file.path(), ID).unwrap().unwrap().total_tokens,
            Some(300)
        );
    }

    #[test]
    fn fallback_selects_newest_rollout_and_detects_same_size_replacement() {
        use std::fs::{FileTimes, OpenOptions};
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join(format!("rollout-2026-01-01-{ID}.jsonl"));
        let current = dir.join(format!("rollout-2026-01-02-{ID}.jsonl"));
        std::fs::write(&old, event(100)).unwrap();
        std::fs::write(&current, event(200)).unwrap();
        let old_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let current_time = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
        OpenOptions::new()
            .write(true)
            .open(&old)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old_time))
            .unwrap();
        OpenOptions::new()
            .write(true)
            .open(&current)
            .unwrap()
            .set_times(FileTimes::new().set_modified(current_time))
            .unwrap();
        assert_eq!(
            cached_history(home.path(), ID).unwrap().total_tokens,
            Some(200)
        );
        let initial_version = FileVersion::from_path(&current).unwrap();
        let replacement = dir.join("replacement.jsonl");
        std::fs::write(&replacement, event(300)).unwrap();
        let replacement_file = OpenOptions::new().write(true).open(&replacement).unwrap();
        replacement_file
            .set_times(FileTimes::new().set_modified(current_time))
            .unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
            use windows::Win32::Foundation::{FILETIME, HANDLE};
            use windows::Win32::Storage::FileSystem::SetFileTime;
            let created = current.metadata().unwrap().creation_time();
            let created = FILETIME {
                dwLowDateTime: created as u32,
                dwHighDateTime: (created >> 32) as u32,
            };
            // 显式保留创建时间，避免测试结果依赖 Windows 的时间戳精度。
            // replacement_file 和 created 在调用期间保持有效。
            unsafe {
                SetFileTime(
                    HANDLE(replacement_file.as_raw_handle()),
                    Some(&created),
                    None,
                    None,
                )
            }
            .unwrap();
        }
        drop(replacement_file);
        // Windows rename 不覆盖目标；先移走旧文件，同路径替换仍保留相同大小和 mtime。
        std::fs::rename(&current, dir.join("previous.jsonl")).unwrap();
        std::fs::rename(&replacement, &current).unwrap();
        let replaced_version = FileVersion::from_path(&current).unwrap();
        assert_eq!(initial_version.len, replaced_version.len);
        assert_eq!(initial_version.modified, replaced_version.modified);
        #[cfg(windows)]
        assert_eq!(initial_version.created, replaced_version.created);
        #[cfg(any(unix, windows))]
        assert_ne!(initial_version.identity, replaced_version.identity);
        assert_eq!(
            cached_history(home.path(), ID).unwrap().total_tokens,
            Some(300)
        );
        std::fs::remove_file(&current).unwrap();
        std::fs::remove_file(&old).unwrap();
        assert!(cached_history(home.path(), ID).is_none());
    }

    #[test]
    fn database_path_cannot_read_outside_selected_home() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), event(500)).unwrap();
        let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
        db.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO threads VALUES (?1, ?2)",
            [ID, outside.path().to_str().unwrap()],
        )
        .unwrap();
        assert!(cached_history(home.path(), ID).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn fallback_does_not_follow_symlinks_outside_home() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), event(500)).unwrap();
        let dir = home.path().join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.join(format!("rollout-test-{ID}.jsonl")))
            .unwrap();
        assert!(cached_history(home.path(), ID).is_none());
    }
}
