use super::*;
use serde_json::json;

const OWN: &str = "01a1431a-c590-70c1-8cb3-6dbdfff19cc0";
const FOREIGN: &str = "01a1431a-c590-70c1-8cb3-6dbdfff19cc1";
const TRACE: &str = "00112233445566778899aabbccddeeff";
const BASE: i64 = 1_790_000_000_000_000_000;

fn snapshot(home: &Path, session: &str) -> Option<NativeTurnStats> {
    latest_turn(home, session)
}

fn attr(key: &str, value: Value) -> Value {
    json!({"key":key,"value":value})
}
fn text(key: &str, value: &str) -> Value {
    attr(key, json!({"stringValue":value}))
}

fn span(
    id: u64,
    parent: u64,
    name: &str,
    start_ms: i64,
    end_ms: i64,
    attributes: Vec<Value>,
) -> Value {
    json!({
        "traceId": TRACE, "spanId": format!("{id:016x}"), "parentSpanId": format!("{parent:016x}"),
        "name":name, "startTimeUnixNano":(BASE + start_ms * 1_000_000).to_string(),
        "endTimeUnixNano":(BASE + end_ms * 1_000_000).to_string(), "attributes":attributes,
        "events":[{"name":"sensitive event content must not survive"}]
    })
}

fn fixture() -> Vec<Value> {
    vec![
        span(
            1,
            99,
            "try_run_sampling_request",
            0,
            20_000,
            vec![text("turn_id", "turn-1")],
        ),
        span(
            2,
            1,
            "codex.sampling",
            1_000,
            4_000,
            vec![text("conversation.id", OWN), text("turn.id", "turn-1")],
        ),
        span(3, 1, "receiving_stream", 1_100, 20_000, vec![]),
        span(
            4,
            3,
            "reasoning",
            1_200,
            1_910,
            vec![text("from", "output_item_added")],
        ),
        span(5, 4, "receiving", 1_200, 1_900, vec![]),
        span(
            6,
            3,
            "completed",
            3_800,
            3_910,
            vec![attr(
                "gen_ai.usage.output_tokens",
                json!({"intValue":"100"}),
            )],
        ),
        span(7, 6, "receiving", 3_800, 3_900, vec![]),
    ]
}

fn batch(spans: &[Value]) -> Vec<u8> {
    serde_json::to_vec(&json!({"resourceSpans":[{
        "resource":{"attributes":[text("prompt","sensitive resource content must not survive")]},
        "scopeSpans":[{"scope":{"name":"codex"}, "spans":spans}]
    }]}))
    .unwrap()
}

fn ingest(pending: &mut Pending, connection: &mut Connection, spans: &[Value]) {
    pending
        .ingest(connection, parse_batch(&batch(spans)).unwrap())
        .unwrap();
}

#[test]
fn installed_cli_otlp_capture_recovers_timing_without_otel_name_rewrites() {
    // Captured from CLI 0.163.0-alpha.10.2 against a loopback fake API; timing/ID allowlist only.
    let capture = include_bytes!("../../../tests/fixtures/codex-native-stats-0.163.0.json");
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    let spans = parse_batch(capture).unwrap();
    assert!(spans.iter().any(|span| span.kind == Kind::First));
    assert!(spans.iter().any(|span| span.kind == Kind::Completed));
    pending.ingest(&mut connection, spans.clone()).unwrap();
    pending.ingest(&mut connection, spans).unwrap();
    let stats = snapshot(home.path(), "01a151e5-ff6c-7de2-90d3-5af417ad92a1").unwrap();
    assert_eq!(stats.request_count, 1);
    assert_eq!(stats.model_duration_ms, 209);
    assert!((stats.average_ttft_ms.unwrap() - 98.8877).abs() < 1e-6);
    assert!((stats.average_output_tokens_per_second.unwrap() - 2.0 / 0.1097888).abs() < 1e-6);
    assert!(snapshot(home.path(), OWN).is_none());
}

#[test]
fn real_shape_parent_chain_excludes_tool_wait_and_first_wait_from_speed() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    let mut spans = fixture();
    // A handle may finish processing much later than its receiving child.
    spans[3]["endTimeUnixNano"] = json!((BASE + 19_000_000_000).to_string());
    ingest(&mut pending, &mut connection, &spans);
    let stats = snapshot(home.path(), &format!(" local:{OWN} ")).unwrap();
    assert_eq!(stats.model_duration_ms, 3_000);
    assert_eq!(stats.average_ttft_ms, Some(900.0));
    assert_eq!(stats.average_output_tokens_per_second, Some(50.0));
    assert_eq!(stats.request_count, 1);
    assert_eq!(stats.ttft_sample_count, 1);
    assert_eq!(stats.updated_at_unix_ms, (BASE / 1_000_000 + 4_000) as u64);
    assert!(snapshot(home.path(), FOREIGN).is_none());
    assert!(snapshot(home.path(), "not-a-session").is_none());
}

#[test]
fn out_of_order_batches_replay_restart_and_late_earlier_output_are_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    let spans = fixture();
    for index in [6, 5, 4, 3, 1, 2] {
        ingest(&mut pending, &mut connection, &[spans[index].clone()]);
        assert!(snapshot(home.path(), OWN).is_none());
    }
    ingest(&mut pending, &mut connection, &[spans[0].clone()]);
    ingest(&mut pending, &mut connection, &spans);
    let late = [
        span(
            8,
            3,
            "agent_message",
            1_300,
            1_510,
            vec![text("from", "output_item_added")],
        ),
        span(9, 8, "receiving", 1_300, 1_500, vec![]),
    ];
    ingest(&mut pending, &mut connection, &late);
    let stats = snapshot(home.path(), OWN).unwrap();
    assert_eq!(stats.request_count, 1);
    assert_eq!(stats.average_ttft_ms, Some(500.0));
    assert!((stats.average_output_tokens_per_second.unwrap() - 100.0 / 2.4).abs() < 1e-9);
    drop(connection);
    let mut connection = open_database(home.path()).unwrap();
    ingest(&mut Pending::default(), &mut connection, &spans);
    let replayed = snapshot(home.path(), OWN).unwrap();
    assert_eq!(replayed.request_count, 1);
    assert_eq!(replayed.average_ttft_ms, Some(500.0));
}

#[test]
fn missing_first_output_stays_unknown_and_late_first_populates_it() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    let spans = fixture();
    let without_first: Vec<_> = spans
        .iter()
        .enumerate()
        .filter(|(i, _)| ![3, 4].contains(i))
        .map(|(_, v)| v.clone())
        .collect();
    ingest(&mut pending, &mut connection, &without_first);
    let stats = snapshot(home.path(), OWN).unwrap();
    assert_eq!(stats.request_count, 1);
    assert_eq!(stats.model_duration_ms, 3_000);
    assert_eq!(stats.average_ttft_ms, None);
    assert_eq!(stats.ttft_sample_count, 0);
    assert_eq!(stats.average_output_tokens_per_second, None);
    ingest(&mut pending, &mut connection, &spans[3..5]);
    assert_eq!(
        snapshot(home.path(), OWN).unwrap().average_ttft_ms,
        Some(900.0)
    );
}

#[test]
fn samples_without_decode_timing_do_not_refresh_the_speed_source() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    let spans = fixture();
    ingest(&mut pending, &mut connection, &spans);
    let later: Vec<_> = spans
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut span)| {
            if [3, 4].contains(&index) {
                return None;
            }
            span["traceId"] = json!("112233445566778899aabbccddeeff00");
            for field in ["startTimeUnixNano", "endTimeUnixNano"] {
                let timestamp = span[field].as_str().unwrap().parse::<i64>().unwrap();
                span[field] = json!((timestamp + 10_000_000_000).to_string());
            }
            Some(span)
        })
        .collect();
    ingest(&mut pending, &mut connection, &later);
    let stats = snapshot(home.path(), OWN).unwrap();
    assert_eq!(stats.request_count, 2);
    assert_eq!(stats.average_speed_request_count, 1);
    assert_eq!(stats.updated_at_unix_ms, (BASE / 1_000_000 + 14_000) as u64);
    assert_eq!(
        stats.average_speed_updated_at_unix_ms,
        Some((BASE / 1_000_000 + 4_000) as u64)
    );
    assert_eq!(stats.average_output_tokens_per_second, Some(50.0));
}

#[test]
fn missing_usage_reversed_time_invalid_ids_and_wrong_owners_do_not_yield_samples() {
    for variant in 0..9 {
        let home = tempfile::tempdir().unwrap();
        let mut connection = open_database(home.path()).unwrap();
        let mut spans = fixture();
        match variant {
            0 => spans[5]["attributes"] = json!([]),
            1 => spans[6]["startTimeUnixNano"] = json!((BASE + 5_000_000_000).to_string()),
            2 => spans[1]["spanId"] = json!("invalid-span-id"),
            3 => spans[0]["attributes"]
                .as_array_mut()
                .unwrap()
                .push(text("conversation.id", FOREIGN)),
            4 => spans[1]["attributes"][0] = text("conversation.id", "invalid-session"),
            5 => spans[1]["traceId"] = json!("00000000000000000000000000000000"),
            6 => {
                spans[5]["attributes"][0] =
                    attr("gen_ai.usage.output_tokens", json!({"intValue":"-1"}))
            }
            7 => {
                spans[5]["attributes"][0] =
                    attr("gen_ai.usage.output_tokens", json!({"doubleValue":10.5}))
            }
            8 => {
                spans[3]["startTimeUnixNano"] = json!((BASE + 3_900_000_000).to_string());
                spans[3]["endTimeUnixNano"] = json!((BASE + 3_990_000_000).to_string());
                spans[4]["startTimeUnixNano"] = json!((BASE + 3_900_000_000).to_string());
                spans[4]["endTimeUnixNano"] = json!((BASE + 3_980_000_000).to_string());
            }
            _ => unreachable!(),
        }
        ingest(&mut Pending::default(), &mut connection, &spans);
        assert!(snapshot(home.path(), OWN).is_none(), "variant {variant}");
    }
}

#[test]
fn span_collision_and_ambiguous_sampling_revoke_persisted_results() {
    for collision in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let mut connection = open_database(home.path()).unwrap();
        let mut pending = Pending::default();
        let spans = fixture();
        ingest(&mut pending, &mut connection, &spans);
        assert!(snapshot(home.path(), OWN).is_some());
        let mut other = spans[1].clone();
        if collision {
            other["attributes"][0] = text("conversation.id", FOREIGN);
        } else {
            other["spanId"] = json!("000000000000000a");
        }
        ingest(&mut pending, &mut connection, &[other]);
        assert!(snapshot(home.path(), OWN).is_none());
        assert!(snapshot(home.path(), FOREIGN).is_none());
        ingest(&mut Pending::default(), &mut connection, &spans);
        assert!(snapshot(home.path(), OWN).is_none());
    }
}

#[test]
fn only_numeric_summary_and_ids_survive_ingestion() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    let mut spans = fixture();
    let secret = "PRIVATE_PROMPT_TOOL_RAW_RESPONSE_9ddfa846";
    for span in &mut spans {
        span["attributes"]
            .as_array_mut()
            .unwrap()
            .push(text("raw_request", secret));
        span["attributes"]
            .as_array_mut()
            .unwrap()
            .push(text("tool_name", secret));
    }
    ingest(&mut pending, &mut connection, &spans);
    assert!(!format!("{:?}", pending.traces[TRACE].spans).contains(secret));
    drop(connection);
    for entry in std::fs::read_dir(home.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(
            !bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes())
        );
        assert!(
            !bytes
                .windows(23)
                .any(|window| window == b"sensitive event content")
        );
    }
}

#[tokio::test]
async fn loopback_json_sink_authenticates_reads_split_body_and_reuses_runtime_override() {
    let home = tempfile::tempdir().unwrap();
    let config = runtime_overrides(home.path().to_path_buf()).await.unwrap();
    assert_eq!(
        config,
        runtime_overrides(home.path().to_path_buf()).await.unwrap()
    );
    let document = config[0].parse::<toml_edit::DocumentMut>().unwrap();
    let exporter = &document["otel"]["trace_exporter"]["otlp-http"];
    let endpoint = exporter["endpoint"].as_str().unwrap();
    let auth = exporter["headers"][AUTH_HEADER].as_str().unwrap();
    assert_eq!(exporter["protocol"].as_str(), Some("json"));
    let url = reqwest::Url::parse(endpoint).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    let port = url.port().unwrap();
    let body = batch(&fixture());
    let post = |key: &str| {
        format!(
            "POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{AUTH_HEADER}: {key}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
    };
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    stream.write_all(post("wrong").as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 401"));
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    stream.write_all(post(auth).as_bytes()).await.unwrap();
    stream.write_all(&body[..11]).await.unwrap();
    tokio::task::yield_now().await;
    stream.write_all(&body[11..]).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    assert_eq!(snapshot(home.path(), OWN).unwrap().model_duration_ms, 3_000);
    let key = home.path().canonicalize().unwrap();
    let sink = SINKS.lock().await.remove(&key).unwrap();
    sink.task.abort();
    let _ = sink.task.await;
    sink.worker.await.unwrap();
}

#[test]
fn aggregate_speed_weights_generation_windows_and_keeps_distinct_traces_separate() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    ingest(&mut pending, &mut connection, &fixture());
    let mut second = fixture();
    for value in &mut second {
        value["traceId"] = json!("112233445566778899aabbccddeeff00");
    }
    second[1]["endTimeUnixNano"] = json!((BASE + 3_000_000_000).to_string());
    second[5]["startTimeUnixNano"] = json!((BASE + 2_800_000_000).to_string());
    second[5]["endTimeUnixNano"] = json!((BASE + 2_910_000_000).to_string());
    second[5]["attributes"][0] = attr("gen_ai.usage.output_tokens", json!({"intValue":400}));
    second[6]["startTimeUnixNano"] = json!((BASE + 2_800_000_000).to_string());
    second[6]["endTimeUnixNano"] = json!((BASE + 2_900_000_000).to_string());
    ingest(&mut pending, &mut connection, &second);
    let stats = snapshot(home.path(), OWN).unwrap();
    assert_eq!(stats.request_count, 2);
    assert_eq!(stats.ttft_sample_count, 2);
    assert_eq!(stats.model_duration_ms, 5_000);
    assert_eq!(stats.average_ttft_ms, Some(900.0));
    assert!((stats.average_output_tokens_per_second.unwrap() - 500.0 / 3.0).abs() < 1e-9);
}

#[test]
fn exact_turn_queries_do_not_mix_different_turn_speeds_or_foreign_sessions() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    ingest(&mut pending, &mut connection, &fixture());
    let mut second = fixture();
    for value in &mut second {
        value["traceId"] = json!("112233445566778899aabbccddeeff00");
        for field in ["startTimeUnixNano", "endTimeUnixNano"] {
            let timestamp = value[field].as_str().unwrap().parse::<i64>().unwrap();
            value[field] = json!((timestamp + 30_000_000_000).to_string());
        }
        for attribute in value["attributes"].as_array_mut().unwrap() {
            if matches!(attribute["key"].as_str(), Some("turn.id" | "turn_id")) {
                attribute["value"]["stringValue"] = json!("turn-2");
            }
        }
    }
    second[5]["attributes"][0] = attr("gen_ai.usage.output_tokens", json!({"intValue":400}));
    ingest(&mut pending, &mut connection, &second);
    for (turn, speed) in [("turn-1", 50.0), ("turn-2", 200.0)] {
        let stats = super::snapshot(home.path(), OWN, turn).unwrap();
        assert_eq!(stats.turn_id, turn);
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.model_duration_ms, 3_000);
        assert_eq!(stats.average_output_tokens_per_second, Some(speed));
    }
    assert_eq!(latest_turn(home.path(), OWN).unwrap().turn_id, "turn-2");
    assert!(super::snapshot(home.path(), OWN, "turn-3").is_none());
    assert!(super::snapshot(home.path(), FOREIGN, "turn-2").is_none());
    let plan = connection.prepare("EXPLAIN QUERY PLAN SELECT COUNT(*) FROM native_samples INDEXED BY native_samples_session_turn WHERE session_id = ?1 AND turn_id = ?2 AND valid = 1").unwrap()
        .query_map(params![OWN, "turn-2"], |row| row.get::<_, String>(3)).unwrap()
        .collect::<rusqlite::Result<Vec<_>>>().unwrap().join(" ");
    assert!(plan.contains("SEARCH native_samples USING"), "{plan}");
    assert!(!plan.contains("SCAN native_samples"), "{plan}");
}

#[test]
fn old_database_migration_preserves_samples_without_guessing_their_turn() {
    let home = tempfile::tempdir().unwrap();
    let connection = Connection::open(home.path().join(DB_NAME)).unwrap();
    connection.execute_batch("CREATE TABLE native_samples (
        trace_id TEXT NOT NULL, sampling_span_id TEXT NOT NULL,
        request_span_id TEXT NOT NULL, stream_span_id TEXT NOT NULL, completed_span_id TEXT NOT NULL,
        session_id TEXT NOT NULL, start_ns INTEGER NOT NULL, end_ns INTEGER NOT NULL,
        completed_ns INTEGER NOT NULL, first_ns INTEGER, output_tokens INTEGER NOT NULL,
        valid INTEGER NOT NULL, PRIMARY KEY (trace_id, sampling_span_id));").unwrap();
    connection.execute("INSERT INTO native_samples VALUES (?1,'0000000000000002','0000000000000001','0000000000000003','0000000000000007',?2,?3,?4,?5,?6,100,1)",
        params![TRACE, OWN, BASE + 1_000_000_000, BASE + 4_000_000_000, BASE + 3_900_000_000, BASE + 1_900_000_000]).unwrap();
    assert!(latest_turn(home.path(), OWN).is_none());
    assert!(
        !crate::sqlite_util::table_columns(&connection, "native_samples")
            .unwrap()
            .contains("turn_id")
    );
    drop(connection);
    let mut connection = open_database(home.path()).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM native_samples WHERE turn_id IS NULL",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        1
    );
    assert!(super::snapshot(home.path(), OWN, "turn-1").is_none());
    // A full, matching span replay supplies direct ownership evidence for this old sample.
    ingest(&mut Pending::default(), &mut connection, &fixture());
    assert_eq!(
        super::snapshot(home.path(), OWN, "turn-1")
            .unwrap()
            .request_count,
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM native_samples", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
    drop(connection);
    assert_eq!(
        open_database(home.path())
            .unwrap()
            .query_row("SELECT COUNT(*) FROM native_samples", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn native_only_history_returns_last_turn_without_claiming_user_boundaries_or_tokens() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    ingest(&mut Pending::default(), &mut connection, &fixture());
    let snapshot =
        crate::conversation_stats::snapshot_with_history(home.path().into(), OWN.into(), None)
            .await
            .unwrap();
    assert_eq!(snapshot.total_tokens, None);
    assert_eq!(snapshot.turn_count, None);
    assert_eq!(snapshot.step_count, None);
    assert_eq!(snapshot.tool_duration_ms, None);
    assert_eq!(snapshot.average_output_tokens_per_second, None);
    let turn = snapshot.last_turn.unwrap();
    assert_eq!(turn.id, "turn-1");
    assert_eq!(turn.status, "unknown");
    assert_eq!(turn.started_at_unix_ms, None);
    assert_eq!(turn.completed_at_unix_ms, None);
    assert_eq!(turn.step_count, None);
    assert_eq!(turn.average_output_tokens_per_second, Some(50.0));
    assert_eq!(turn.coverage, "partial");
}

#[tokio::test]
async fn history_new_rollout_turn_clears_previous_native_values_without_exact_match() {
    use std::io::Write;
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    ingest(&mut Pending::default(), &mut connection, &fixture());
    let sessions = home.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    let rollout = sessions.join(format!("rollout-test-{OWN}.jsonl"));
    let started = |id: &str, offset: i64| {
        json!({"type":"event_msg", "timestamp":
        chrono::DateTime::from_timestamp_millis(BASE / 1_000_000 + offset).unwrap().to_rfc3339(),
        "payload":{"type":"task_started","turn_id":id}})
    };
    std::fs::write(&rollout, format!("{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":OWN}}), started("turn-1", 0),
        json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":80,"output_tokens":20,"total_tokens":100}}}}))).unwrap();
    let first =
        crate::conversation_stats::snapshot_with_history(home.path().into(), OWN.into(), None)
            .await
            .unwrap();
    assert_eq!(first.total_tokens, Some(100));
    assert_eq!(first.turn_count, Some(1));
    assert_eq!(
        first.last_turn.unwrap().average_output_tokens_per_second,
        Some(50.0)
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(rollout)
        .unwrap();
    writeln!(file, "{}", started("turn-2", 30_000)).unwrap();
    file.flush().unwrap();
    let next =
        crate::conversation_stats::snapshot_with_history(home.path().into(), OWN.into(), None)
            .await
            .unwrap();
    assert_eq!(next.total_tokens, Some(100));
    assert_eq!(next.turn_count, Some(2));
    let turn = next.last_turn.unwrap();
    assert_eq!(turn.id, "turn-2");
    assert_eq!(turn.model_duration_ms, None);
    assert_eq!(turn.average_output_tokens_per_second, None);
    assert_eq!(turn.average_ttft_ms, None);
    assert_eq!(turn.timing_source, None);
    assert_eq!(turn.average_speed_source, None);
}

#[test]
fn pending_trace_count_and_batch_size_are_bounded() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    let template = parse_batch(&batch(&fixture())).unwrap().remove(0);
    let spans = (1..MAX_TRACES + 10)
        .map(|index| {
            let mut span = template.clone();
            span.trace = format!("{index:032x}");
            span
        })
        .collect();
    pending.ingest(&mut connection, spans).unwrap();
    assert_eq!(pending.traces.len(), MAX_TRACES);
    assert!(parse_batch(&vec![b' '; MAX_BODY + 1]).is_err());
}

#[test]
fn reaching_pending_span_capacity_keeps_previously_valid_samples() {
    let home = tempfile::tempdir().unwrap();
    let mut connection = open_database(home.path()).unwrap();
    let mut pending = Pending::default();
    let spans = fixture();
    ingest(&mut pending, &mut connection, &spans);
    let template = parse_batch(&batch(&spans)).unwrap().remove(0);
    let extra = (100..MAX_SPANS_PER_TRACE + 200)
        .map(|index| {
            let mut span = template.clone();
            span.id = format!("{index:016x}");
            span
        })
        .collect();
    pending.ingest(&mut connection, extra).unwrap();
    assert_eq!(pending.traces[TRACE].spans.len(), MAX_SPANS_PER_TRACE);
    assert!(!pending.traces[TRACE].poisoned);
    assert_eq!(snapshot(home.path(), OWN).unwrap().request_count, 1);
}

#[test]
fn http_limits_and_unsupported_payloads_are_rejected_before_ingestion() {
    let head = |headers: &str| {
        format!(
            "POST /v1/traces HTTP/1.1\r\n{AUTH_HEADER}: key\r\nContent-Type: application/json\r\n{headers}\r\n"
        )
    };
    assert_eq!(
        request_length(head("Content-Length: 10\r\n").as_bytes(), "key"),
        Ok(10)
    );
    assert!(
        request_length(
            head("Content-Length: 10\r\nContent-Length: 10\r\n").as_bytes(),
            "key"
        )
        .is_err()
    );
    assert!(request_length(head("Transfer-Encoding: chunked\r\n").as_bytes(), "key").is_err());
    assert!(
        request_length(
            head(&format!("Content-Length: {}\r\n", MAX_BODY + 1)).as_bytes(),
            "key"
        )
        .is_err()
    );
    assert!(parse_batch(br#"{"resourceLogs":[]}"#).is_err());
    assert!(parse_batch(b"not-json").is_err());
}
