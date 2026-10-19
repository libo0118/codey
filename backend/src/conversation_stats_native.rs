//! Native Codex trace timing. This listener receives telemetry only, never model traffic.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot};

const DB_NAME: &str = "native-session-stats.sqlite";
const AUTH_HEADER: &str = "x-codey-native-stats-key";
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_HEADER: usize = 8 * 1024;
const MAX_BATCH_SPANS: usize = 4096;
const MAX_TRACES: usize = 256;
const MAX_SPANS_PER_TRACE: usize = 8192;
const MAX_PENDING_SPANS: usize = 32768;
const PENDING_TTL: Duration = Duration::from_secs(2 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub(crate) struct NativeTurnStats {
    pub turn_id: String,
    pub latest_sample_started_at_unix_ms: u64,
    pub model_duration_ms: u64,
    pub average_ttft_ms: Option<f64>,
    pub average_output_tokens_per_second: Option<f64>,
    pub request_count: u64,
    pub ttft_sample_count: u64,
    pub average_speed_request_count: u64,
    pub average_speed_updated_at_unix_ms: Option<u64>,
    pub updated_at_unix_ms: u64,
}

struct RunningSink {
    config: String,
    task: tokio::task::JoinHandle<()>,
    worker: tokio::task::JoinHandle<()>,
}

static SINKS: Mutex<BTreeMap<PathBuf, RunningSink>> = Mutex::const_new(BTreeMap::new());

/// Process overrides leave the user's config and model endpoint untouched.
pub(crate) async fn runtime_overrides(home: PathBuf) -> anyhow::Result<Vec<String>> {
    let home = home.canonicalize().context("定位原生统计目录失败")?;
    let mut sinks = SINKS.lock().await;
    sinks.retain(|_, sink| !sink.task.is_finished() && !sink.worker.is_finished());
    if let Some(sink) = sinks.get(&home) {
        return Ok(vec![sink.config.clone()]);
    }
    ensure!(sinks.len() < 8, "原生统计监听目录达到上限");
    let db_home = home.clone();
    let connection = tokio::task::spawn_blocking(move || open_database(&db_home)).await??;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let auth = uuid::Uuid::new_v4().simple().to_string();
    // All substituted values are generated ASCII, so these are valid TOML basic strings.
    let config = format!(
        "otel.trace_exporter={{otlp-http={{endpoint=\"http://127.0.0.1:{port}/v1/traces\",protocol=\"json\",headers={{\"{AUTH_HEADER}\"=\"{auth}\"}}}}}}"
    );
    let (sender, mut receiver) = mpsc::channel::<Batch>(16);
    let worker = tokio::task::spawn_blocking(move || {
        let mut connection = connection;
        let mut pending = Pending::default();
        while let Some(batch) = receiver.blocking_recv() {
            let result = pending.ingest(&mut connection, batch.spans).is_ok();
            let _ = batch.reply.send(result);
        }
    });
    let task = tokio::spawn(async move {
        let permits = Arc::new(Semaphore::new(16));
        let auth = Arc::new(auth);
        while let Ok((stream, address)) = listener.accept().await {
            if !address.ip().is_loopback() {
                continue;
            }
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                continue;
            };
            let sender = sender.clone();
            let auth = auth.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = tokio::time::timeout(REQUEST_TIMEOUT, serve(stream, &auth, sender)).await;
            });
        }
    });
    sinks.insert(
        home,
        RunningSink {
            config: config.clone(),
            task,
            worker,
        },
    );
    Ok(vec![config])
}

pub(crate) fn latest_turn(home: &Path, session_id: &str) -> Option<NativeTurnStats> {
    let session_id = session_uuid(session_id)?;
    let connection = open_snapshot_connection(home)?;
    let turn_id: String = connection
        .query_row(
            "SELECT turn_id FROM native_samples INDEXED BY native_samples_session_turn
         WHERE session_id = ?1 AND valid = 1 AND turn_id IS NOT NULL
         ORDER BY start_ns DESC, turn_id DESC LIMIT 1",
            [&session_id],
            |row| row.get(0),
        )
        .ok()?;
    snapshot(home, &session_id, &turn_id)
}

fn open_snapshot_connection(home: &Path) -> Option<Connection> {
    let connection =
        Connection::open_with_flags(home.join(DB_NAME), OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    connection.busy_timeout(Duration::from_millis(50)).ok()?;
    Some(connection)
}

pub(crate) fn snapshot(home: &Path, session_id: &str, turn_id: &str) -> Option<NativeTurnStats> {
    let session_id = session_uuid(session_id)?;
    let connection = open_snapshot_connection(home)?;
    connection.query_row(
        "SELECT COUNT(*), SUM((end_ns - start_ns) / 1000000.0),
                AVG((first_ns - start_ns) / 1000000.0), COUNT(first_ns),
                SUM(CASE WHEN completed_ns > first_ns THEN output_tokens * 1.0 END),
                SUM(CASE WHEN completed_ns > first_ns THEN (completed_ns - first_ns) / 1000000000.0 END),
                MAX(end_ns / 1000000),
                COUNT(CASE WHEN completed_ns > first_ns THEN 1 END),
                MAX(CASE WHEN completed_ns > first_ns THEN end_ns / 1000000 END),
                MAX(start_ns / 1000000)
         FROM native_samples INDEXED BY native_samples_session_turn
         WHERE session_id = ?1 AND turn_id = ?2 AND valid = 1",
        params![session_id, turn_id],
        |row| {
            let request_count: u64 = row.get(0)?;
            let duration: Option<f64> = row.get(1)?;
            let output: Option<f64> = row.get(4)?;
            let seconds: Option<f64> = row.get(5)?;
            Ok((request_count > 0).then(|| NativeTurnStats {
                turn_id: turn_id.to_owned(),
                latest_sample_started_at_unix_ms: row.get(9).unwrap_or_default(),
                model_duration_ms: duration.unwrap_or_default() as u64,
                average_ttft_ms: row.get(2).ok().flatten(),
                average_output_tokens_per_second: output.zip(seconds)
                    .filter(|(_, seconds)| *seconds > 0.0)
                    .map(|(tokens, seconds)| tokens / seconds),
                request_count,
                ttft_sample_count: row.get(3).unwrap_or_default(),
                average_speed_request_count: row.get(7).unwrap_or_default(),
                average_speed_updated_at_unix_ms: row.get(8).ok().flatten(),
                updated_at_unix_ms: row.get(6).unwrap_or_default(),
            }))
        },
    ).ok().flatten()
}

struct Batch {
    spans: Vec<Span>,
    reply: oneshot::Sender<bool>,
}

async fn respond(stream: &mut TcpStream, status: &str) -> std::io::Result<()> {
    stream.write_all(format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    ).as_bytes()).await
}

async fn serve(
    mut stream: TcpStream,
    auth: &str,
    sender: mpsc::Sender<Batch>,
) -> anyhow::Result<()> {
    let mut buffer = Vec::new();
    let head_end = loop {
        if let Some(end) = memchr::memmem::find(&buffer, b"\r\n\r\n") {
            if end + 4 <= MAX_HEADER {
                break end + 4;
            }
        }
        if buffer.len() >= MAX_HEADER {
            respond(&mut stream, "431 Request Header Fields Too Large").await?;
            return Ok(());
        }
        let mut chunk = [0u8; 4096];
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..count]);
    };
    let length = match request_length(&buffer[..head_end], auth) {
        Ok(length) => length,
        Err(status) => {
            respond(&mut stream, status).await?;
            return Ok(());
        }
    };
    if buffer.len() - head_end > length {
        respond(&mut stream, "400 Bad Request").await?;
        return Ok(());
    }
    let received = buffer.len();
    buffer.resize(head_end + length, 0);
    stream.read_exact(&mut buffer[received..]).await?;
    let spans = parse_batch(&buffer[head_end..]);
    let spans = match spans {
        Ok(spans) => spans,
        Err(_) => {
            respond(&mut stream, "400 Bad Request").await?;
            return Ok(());
        }
    };
    let (reply, result) = oneshot::channel();
    if sender.try_send(Batch { spans, reply }).is_err() {
        respond(&mut stream, "503 Service Unavailable").await?;
    } else if result.await.unwrap_or(false) {
        respond(&mut stream, "200 OK").await?;
    } else {
        respond(&mut stream, "503 Service Unavailable").await?;
    }
    Ok(())
}

fn request_length(head: &[u8], auth: &str) -> Result<usize, &'static str> {
    let head = std::str::from_utf8(head).map_err(|_| "400 Bad Request")?;
    let mut lines = head.split("\r\n");
    if lines.next() != Some("POST /v1/traces HTTP/1.1") {
        return Err("404 Not Found");
    }
    let mut length = None;
    let mut authorized = false;
    let mut saw_auth = false;
    let mut json = false;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("400 Bad Request")?;
        let value = value.trim();
        if name.eq_ignore_ascii_case(AUTH_HEADER) {
            if saw_auth {
                return Err("400 Bad Request");
            }
            saw_auth = true;
            authorized = value == auth;
        } else if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() || !value.bytes().all(|v| v.is_ascii_digit()) {
                return Err("400 Bad Request");
            }
            length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "413 Content Too Large")?,
            );
        } else if name.eq_ignore_ascii_case("content-type") {
            json = value
                .split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-encoding")
                && !value.eq_ignore_ascii_case("identity"))
        {
            return Err("415 Unsupported Media Type");
        }
    }
    if !authorized {
        return Err("401 Unauthorized");
    }
    if !json {
        return Err("415 Unsupported Media Type");
    }
    let length = length.ok_or("411 Length Required")?;
    if length > MAX_BODY {
        return Err("413 Content Too Large");
    }
    Ok(length)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Sampling,
    Request,
    Stream,
    Receiving,
    First,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Span {
    trace: String,
    id: String,
    parent: String,
    kind: Kind,
    start: i64,
    end: i64,
    session: Option<String>,
    turn: Option<String>,
    output_tokens: Option<i64>,
}

fn session_uuid(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.strip_prefix("local:").unwrap_or(value).trim();
    // Accept the rollout UUID format only, never arbitrary thread-like strings.
    (value.len() == 36).then_some(())?;
    let uuid = uuid::Uuid::parse_str(value).ok()?;
    (!uuid.is_nil()).then(|| uuid.hyphenated().to_string())
}

fn hex_id(value: &Value, digits: usize) -> Option<String> {
    let value = value.as_str()?;
    (value.len() == digits
        && value.bytes().all(|c| c.is_ascii_hexdigit())
        && value.bytes().any(|c| c != b'0'))
    .then(|| value.to_ascii_lowercase())
}

fn integer(value: &Value) -> Option<i64> {
    value.as_i64().filter(|value| *value >= 0).or_else(|| {
        let value = value.as_str()?;
        (!value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()))
            .then(|| value.parse::<i64>().ok())
            .flatten()
    })
}

fn attribute<'a>(span: &'a Value, key: &str) -> Option<&'a Value> {
    let mut matches = span
        .get("attributes")?
        .as_array()?
        .iter()
        .filter(|attribute| attribute["key"].as_str() == Some(key));
    let value = matches.next()?.get("value")?;
    matches.next().is_none().then_some(value)
}

fn parse_span(span: &Value) -> Option<Span> {
    let name = span["name"].as_str()?;
    let first = attribute(span, "from").and_then(|v| v["stringValue"].as_str())
        == Some("output_item_added");
    let output_tokens =
        attribute(span, "gen_ai.usage.output_tokens").and_then(|v| integer(&v["intValue"]));
    let kind = match name {
        "codex.sampling" => Kind::Sampling,
        "try_run_sampling_request" => Kind::Request,
        "receiving_stream" => Kind::Stream,
        "receiving" => Kind::Receiving,
        // The installed CLI retains this name even after recording `otel.name`.
        "handle_responses" if first && output_tokens.is_none() => Kind::First,
        "handle_responses" if !first && output_tokens.is_some() => Kind::Completed,
        "completed" => Kind::Completed,
        "reasoning"
        | "agent_message"
        | "function_call"
        | "local_shell_call"
        | "custom_tool_call"
        | "web_search_call"
        | "image_generation_call"
        | "tool_search_call"
        | "function_call_output"
        | "custom_tool_call_output"
        | "tool_search_output"
        | "compaction"
        | "configuration_update"
        | "compaction_trigger"
        | "context_compaction"
        | "additional_tools"
        | "message_from_assistant"
        | "other"
            if first =>
        {
            Kind::First
        }
        _ => return None,
    };
    let start = integer(&span["startTimeUnixNano"])?;
    let end = integer(&span["endTimeUnixNano"])?;
    if start == 0 || end < start {
        return None;
    }
    let session = match attribute(span, "conversation.id") {
        Some(value) => Some(session_uuid(value["stringValue"].as_str()?)?),
        None => None,
    };
    let turn = match attribute(span, "turn.id").or_else(|| attribute(span, "turn_id")) {
        Some(v) => {
            let value = v["stringValue"].as_str()?;
            if value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            {
                return None;
            }
            Some(value.to_string())
        }
        None => None,
    };
    if kind == Kind::Sampling && (session.is_none() || turn.is_none() || end == start) {
        return None;
    }
    Some(Span {
        trace: hex_id(&span["traceId"], 32)?,
        id: hex_id(&span["spanId"], 16)?,
        parent: hex_id(&span["parentSpanId"], 16)?,
        kind,
        start,
        end,
        session,
        turn,
        output_tokens,
    })
}

fn parse_batch(body: &[u8]) -> anyhow::Result<Vec<Span>> {
    ensure!(body.len() <= MAX_BODY, "trace batch too large");
    let value: Value = serde_json::from_slice(body).context("invalid trace JSON")?;
    let resources = value
        .get("resourceSpans")
        .and_then(Value::as_array)
        .context("missing resourceSpans")?;
    let mut spans = Vec::new();
    let mut count = 0;
    for resource in resources {
        let scopes = resource
            .get("scopeSpans")
            .and_then(Value::as_array)
            .context("missing scopeSpans")?;
        for scope in scopes {
            for span in scope
                .get("spans")
                .and_then(Value::as_array)
                .context("missing spans")?
            {
                count += 1;
                ensure!(count <= MAX_BATCH_SPANS, "too many spans");
                if let Some(span) = parse_span(span) {
                    spans.push(span);
                }
            }
        }
    }
    Ok(spans)
}

struct Trace {
    spans: HashMap<String, Span>,
    touched: Instant,
    poisoned: bool,
}

#[derive(Default)]
struct Pending {
    traces: HashMap<String, Trace>,
}

struct Sample<'a> {
    span: &'a Span,
    request: &'a Span,
    stream: &'a Span,
    completed: &'a Span,
    first: Option<i64>,
    output_tokens: i64,
}

impl Pending {
    fn ingest(&mut self, connection: &mut Connection, spans: Vec<Span>) -> anyhow::Result<()> {
        let now = Instant::now();
        self.traces
            .retain(|_, trace| now.duration_since(trace.touched) <= PENDING_TTL);
        let mut affected = HashSet::new();
        for span in spans {
            let id = span.trace.clone();
            let trace = self.traces.entry(id.clone()).or_insert_with(|| Trace {
                spans: HashMap::new(),
                touched: now,
                poisoned: false,
            });
            trace.touched = now;
            if trace.poisoned {
                continue;
            }
            if let Some(previous) = trace.spans.get(&span.id) {
                trace.poisoned |= previous != &span;
            } else if trace.spans.len() < MAX_SPANS_PER_TRACE {
                trace.spans.insert(span.id.clone(), span);
            } else {
                // Capacity loss is missing coverage, not evidence that earlier samples are invalid.
                continue;
            }
            affected.insert(id);
        }
        let transaction = connection.transaction()?;
        for id in affected {
            let trace = &self.traces[&id];
            if trace.poisoned {
                transaction.execute(
                    "INSERT OR IGNORE INTO native_rejected_traces VALUES (?1)",
                    [&id],
                )?;
                transaction.execute("DELETE FROM native_samples WHERE trace_id = ?1", [&id])?;
                continue;
            }
            if transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_rejected_traces WHERE trace_id = ?1)",
                [&id],
                |row| row.get::<_, bool>(0),
            )? {
                continue;
            }
            let (samples, rejected_requests) = trace.samples();
            for request in rejected_requests {
                transaction.execute(
                    "INSERT OR IGNORE INTO native_rejected_requests VALUES (?1, ?2)",
                    params![id, request],
                )?;
                transaction.execute("UPDATE native_samples SET valid = 0 WHERE trace_id = ?1 AND request_span_id = ?2", params![id, request])?;
            }
            for sample in samples {
                if transaction.query_row("SELECT EXISTS(SELECT 1 FROM native_rejected_requests WHERE trace_id = ?1 AND request_span_id = ?2)", params![id, sample.request.id], |row| row.get::<_, bool>(0))? {
                    continue;
                }
                transaction.execute(
                    "INSERT INTO native_samples
                        (trace_id, sampling_span_id, request_span_id, stream_span_id, completed_span_id,
                         session_id, turn_id, start_ns, end_ns, completed_ns, first_ns, output_tokens, valid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1)
                     ON CONFLICT(trace_id, sampling_span_id) DO UPDATE SET
                        valid = native_samples.valid AND
                            native_samples.request_span_id = excluded.request_span_id AND
                            native_samples.stream_span_id = excluded.stream_span_id AND
                            native_samples.completed_span_id = excluded.completed_span_id AND
                            native_samples.session_id = excluded.session_id AND
                            (native_samples.turn_id IS NULL OR native_samples.turn_id = excluded.turn_id) AND
                            native_samples.start_ns = excluded.start_ns AND native_samples.end_ns = excluded.end_ns AND
                            native_samples.completed_ns = excluded.completed_ns AND native_samples.output_tokens = excluded.output_tokens,
                        turn_id = COALESCE(native_samples.turn_id, excluded.turn_id),
                        first_ns = CASE WHEN native_samples.first_ns IS NULL THEN excluded.first_ns
                                        WHEN excluded.first_ns IS NULL THEN native_samples.first_ns
                                        ELSE MIN(native_samples.first_ns, excluded.first_ns) END",
                    params![id, sample.span.id, sample.request.id, sample.stream.id, sample.completed.id,
                        sample.span.session, sample.span.turn, sample.span.start, sample.span.end,
                        sample.completed.end, sample.first, sample.output_tokens],
                )?;
            }
        }
        transaction.commit()?;
        // shortcut: evicted incomplete traces cannot yield samples; increase these caps if native batches grow.
        while self.traces.len() > MAX_TRACES
            || self
                .traces
                .values()
                .map(|trace| trace.spans.len())
                .sum::<usize>()
                > MAX_PENDING_SPANS
        {
            let Some(oldest) = self
                .traces
                .iter()
                .min_by_key(|(_, trace)| trace.touched)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.traces.remove(&oldest);
        }
        Ok(())
    }
}

impl Trace {
    fn samples(&self) -> (Vec<Sample<'_>>, HashSet<&str>) {
        let mut children: HashMap<&str, Vec<&Span>> = HashMap::new();
        for span in self.spans.values() {
            children.entry(&span.parent).or_default().push(span);
        }
        let mut samples = Vec::new();
        let mut rejected = HashSet::new();
        for span in self
            .spans
            .values()
            .filter(|span| span.kind == Kind::Sampling)
        {
            let Some(request) = self
                .spans
                .get(&span.parent)
                .filter(|v| v.kind == Kind::Request)
            else {
                continue;
            };
            let siblings = &children[span.parent.as_str()];
            let streams: Vec<_> = siblings
                .iter()
                .filter(|v| v.kind == Kind::Stream)
                .copied()
                .collect();
            if siblings.iter().filter(|v| v.kind == Kind::Sampling).count() != 1
                || streams.len() > 1
            {
                rejected.insert(request.id.as_str());
                continue;
            }
            let Some(stream) = streams.first().copied() else {
                continue;
            };
            if request.start > span.start
                || request.end < span.end
                || stream.start < span.start
                || stream.end > request.end
                || !same_owner(span, request)
                || !same_owner(span, stream)
            {
                rejected.insert(request.id.as_str());
                continue;
            }
            let events = children
                .get(stream.id.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let completions: Vec<_> = events
                .iter()
                .filter(|v| v.kind == Kind::Completed)
                .copied()
                .collect();
            if completions.len() > 1 {
                rejected.insert(request.id.as_str());
                continue;
            }
            let Some(completed) = completions.first().copied() else {
                continue;
            };
            let receiving = |event: &Span| -> Result<Option<&Span>, ()> {
                let Some(waits) = children.get(event.id.as_str()) else {
                    return Ok(None);
                };
                let mut waits = waits.iter().filter(|v| v.kind == Kind::Receiving);
                let Some(wait) = waits.next().copied() else {
                    return Ok(None);
                };
                if waits.next().is_some()
                    || !same_owner(span, event)
                    || !same_owner(span, wait)
                    || wait.start < span.start
                    || wait.end > span.end
                    || wait.start < event.start
                    || wait.end > event.end
                    || event.start < stream.start
                    || event.end > stream.end
                {
                    return Err(());
                }
                Ok(Some(wait))
            };
            if !same_owner(span, completed) {
                rejected.insert(request.id.as_str());
                continue;
            }
            let completed_wait = match receiving(completed) {
                Ok(Some(wait)) => wait,
                Ok(None) => continue,
                Err(()) => {
                    rejected.insert(request.id.as_str());
                    continue;
                }
            };
            let Some(output_tokens) = completed.output_tokens else {
                continue;
            };
            let mut first = None;
            let mut invalid = false;
            for event in events.iter().filter(|v| v.kind == Kind::First) {
                if !same_owner(span, event) {
                    invalid = true;
                    break;
                }
                match receiving(event) {
                    Ok(Some(wait)) => {
                        if wait.end > completed_wait.end {
                            invalid = true;
                            break;
                        }
                        first =
                            Some(first.map_or(wait.end, |previous: i64| previous.min(wait.end)));
                    }
                    Ok(None) => {}
                    Err(()) => {
                        invalid = true;
                        break;
                    }
                }
            }
            if invalid {
                rejected.insert(request.id.as_str());
                continue;
            }
            samples.push(Sample {
                span,
                request,
                stream,
                completed: completed_wait,
                first,
                output_tokens,
            });
        }
        (samples, rejected)
    }
}

fn same_owner(sampling: &Span, other: &Span) -> bool {
    other
        .session
        .as_ref()
        .is_none_or(|session| sampling.session.as_ref() == Some(session))
        && other
            .turn
            .as_ref()
            .is_none_or(|turn| sampling.turn.as_ref() == Some(turn))
}

fn open_database(home: &Path) -> anyhow::Result<Connection> {
    let connection = Connection::open(home.join(DB_NAME))?;
    connection.busy_timeout(Duration::from_millis(250))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS native_samples (
             trace_id TEXT NOT NULL, sampling_span_id TEXT NOT NULL,
             request_span_id TEXT NOT NULL, stream_span_id TEXT NOT NULL, completed_span_id TEXT NOT NULL,
             session_id TEXT NOT NULL, turn_id TEXT, start_ns INTEGER NOT NULL, end_ns INTEGER NOT NULL,
             completed_ns INTEGER NOT NULL, first_ns INTEGER, output_tokens INTEGER NOT NULL,
             valid INTEGER NOT NULL, PRIMARY KEY (trace_id, sampling_span_id)
         );
         CREATE INDEX IF NOT EXISTS native_samples_session ON native_samples(session_id, valid);
         CREATE TABLE IF NOT EXISTS native_rejected_traces (trace_id TEXT PRIMARY KEY NOT NULL);
         CREATE TABLE IF NOT EXISTS native_rejected_requests (
             trace_id TEXT NOT NULL, request_span_id TEXT NOT NULL,
             PRIMARY KEY (trace_id, request_span_id)
         );"
    )?;
    // Old samples have no provable turn owner; only a validated replay may fill it in.
    if !crate::sqlite_util::table_columns(&connection, "native_samples")?.contains("turn_id") {
        connection.execute("ALTER TABLE native_samples ADD COLUMN turn_id TEXT", [])?;
    }
    connection.execute("CREATE INDEX IF NOT EXISTS native_samples_session_turn ON native_samples(session_id, turn_id, valid, start_ns)", [])?;
    Ok(connection)
}

#[cfg(test)]
#[path = "conversation_stats_native/tests.rs"]
mod tests;
