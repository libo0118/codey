use super::super::{Projection, compact_turn};
use super::*;
use crate::remote_control::protocol;

fn native_history(count: usize, width: usize) -> Value {
    json!({"id":"thread","cwd":"/synthetic-workspace","turns":[{"turnId":"turn","status":"completed","items":(0..count).map(|id|json!({"id":format!("item-{id}"),"type":"agentMessage","text":"x".repeat(width)})).collect::<Vec<_>>()}]})
}

fn snapshot(
    projection: &mut Projection,
    state: &Value,
    window: &mut MessageWindow,
) -> Arc<PublicState> {
    Arc::new(projection.public_state(state, StreamVersion::V2, window))
}

fn view(state: &PublicState) -> &Value {
    match state {
        PublicState::V2(view) => view,
        _ => panic!("expected v2 public state"),
    }
}

fn ids(state: &Value) -> Vec<&str> {
    state["turns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|turn| turn["messages"].as_array().unwrap())
        .map(|message| message["id"].as_str().unwrap())
        .collect()
}

fn apply(projection: &mut Projection, state: &mut Value, patches: Value) {
    projection.invalidate(state, &patches).unwrap();
    protocol::apply_patches(state, &patches).unwrap();
}

fn receive(
    encoder: &mut Encoder,
    state: Arc<PublicState>,
    client: &mut Value,
    revision: &mut u64,
) -> String {
    let frame = encoder.prepare(state.clone()).unwrap().unwrap();
    let text = frame.text.clone();
    let value: Value = serde_json::from_str(&text).unwrap();
    if *revision == 0 {
        assert_eq!(value["type"], "state");
        assert_eq!(value["streamVersion"], 2);
        *client = value["state"].clone();
    } else {
        assert_eq!(value["type"], "delta");
        assert_eq!(value["baseRevision"], *revision);
        protocol::apply_patches(client, &value["patches"]).unwrap();
    }
    *revision += 1;
    assert_eq!(value["revision"], *revision);
    assert_eq!(client, view(&state));
    encoder.delivered(frame);
    text
}

#[test]
fn version_negotiation_is_explicit_and_does_not_accept_query_credentials() {
    assert_eq!(
        StreamVersion::from_query(None).unwrap(),
        StreamVersion::Legacy
    );
    assert_eq!(
        StreamVersion::from_query(Some("stream=2")).unwrap(),
        StreamVersion::V2
    );
    for query in [
        "stream=1",
        "stream=3",
        "stream=2&stream=2",
        "token=secret",
        "stream=2&token=secret",
    ] {
        assert!(StreamVersion::from_query(Some(query)).is_err());
    }
}

#[test]
fn large_history_is_bounded_then_one_item_update_is_a_small_public_delta() {
    let mut state = native_history(200, 32 * 1024);
    let mut projection = Projection::default();
    let legacy = projection.frame(&state);
    assert!(legacy.len() > 6 * 1024 * 1024);
    let mut window = MessageWindow::default();
    let mut encoder = Encoder::new(StreamVersion::V2);
    let (mut client, mut revision) = (Value::Null, 0);
    let current = snapshot(&mut projection, &state, &mut window);
    let first = receive(&mut encoder, current.clone(), &mut client, &mut revision);
    assert!(first.len() < MESSAGE_BUDGET + 2048);
    assert!(ids(&client).len() < INITIAL_BODIES);
    assert_eq!(client["hasEarlierMessages"], true);
    assert!(encoder.prepare(current).unwrap().is_none());
    apply(
        &mut projection,
        &mut state,
        json!([{"op":"replace","path":"/turns/0/items/199/text","value":"final record"}]),
    );
    let current = snapshot(&mut projection, &state, &mut window);
    let delta = receive(&mut encoder, current, &mut client, &mut revision);
    assert!(delta.len() < 1024, "{delta}");
    assert!(delta.len() * 1000 < legacy.len());
    apply(
        &mut projection,
        &mut state,
        json!([{"op":"add","path":"/privateConfig","value":"NATIVE_PRIVATE_CANARY"}]),
    );
    assert!(
        encoder
            .prepare(snapshot(&mut projection, &state, &mut window))
            .unwrap()
            .is_none()
    );
    assert!(!client.to_string().contains("NATIVE_PRIVATE_CANARY"));
}

#[test]
fn only_successfully_delivered_states_advance_revisions_and_reconnect_starts_fresh() {
    let mut encoder = Encoder::new(StreamVersion::V2);
    let first = Arc::new(PublicState::V2(json!({"public":"first"})));
    let frame = encoder.prepare(first.clone()).unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&frame.text).unwrap()["revision"],
        1
    );
    // Preparing a frame twice is not delivery.
    assert_eq!(encoder.prepare(first.clone()).unwrap().unwrap().revision, 1);
    encoder.delivered(frame);
    let dropped = Arc::new(PublicState::V2(json!({"public":"not delivered"})));
    assert_eq!(encoder.prepare(dropped).unwrap().unwrap().revision, 2);
    let final_state = Arc::new(PublicState::V2(json!({"public":"final"})));
    let frame = encoder.prepare(final_state.clone()).unwrap().unwrap();
    let value: Value = serde_json::from_str(&frame.text).unwrap();
    assert_eq!(value["baseRevision"], 1);
    assert_eq!(value["revision"], 2);
    let mut rebuilt = view(&first).clone();
    protocol::apply_patches(&mut rebuilt, &value["patches"]).unwrap();
    assert_eq!(&rebuilt, view(&final_state));
    let fresh = Encoder::new(StreamVersion::V2)
        .prepare(final_state)
        .unwrap()
        .unwrap();
    let fresh: Value = serde_json::from_str(&fresh.text).unwrap();
    assert_eq!(fresh["type"], "state");
    assert_eq!(fresh["revision"], 1);
}

#[test]
fn recent_window_counts_messages_not_turns_and_history_expansion_has_a_stable_anchor() {
    let mut state = native_history(125, 8);
    state["turnsPagination"] = json!({"hasLoadedOldest":false});
    let mut projection = Projection::default();
    let mut window = MessageWindow::default();
    let mut encoder = Encoder::new(StreamVersion::V2);
    let (mut client, mut revision) = (Value::Null, 0);
    receive(
        &mut encoder,
        snapshot(&mut projection, &state, &mut window),
        &mut client,
        &mut revision,
    );
    assert_eq!(ids(&client).len(), 20);
    assert_eq!(ids(&client)[0], "item-105");
    assert_eq!(client["historyComplete"], false);
    window.expand();
    receive(
        &mut encoder,
        snapshot(&mut projection, &state, &mut window),
        &mut client,
        &mut revision,
    );
    assert_eq!(ids(&client).len(), 70);
    assert_eq!(ids(&client)[0], "item-55");
    apply(
        &mut projection,
        &mut state,
        json!([{"op":"add","path":"/turns/0/items/-","value":{"id":"new","type":"agentMessage","text":"new message"}}]),
    );
    receive(
        &mut encoder,
        snapshot(&mut projection, &state, &mut window),
        &mut client,
        &mut revision,
    );
    assert_eq!(ids(&client).len(), 71);
    assert_eq!(ids(&client)[0], "item-55");
    apply(
        &mut projection,
        &mut state,
        json!([{"op":"remove","path":"/turns/0/items/55"}]),
    );
    receive(
        &mut encoder,
        snapshot(&mut projection, &state, &mut window),
        &mut client,
        &mut revision,
    );
    assert_eq!(ids(&client).len(), 70);
    assert_eq!(ids(&client)[0], "item-56");
    let mut reordered = state["turns"][0]["items"].as_array().unwrap().clone();
    let moved = reordered.pop().unwrap();
    reordered.insert(10, moved);
    apply(
        &mut projection,
        &mut state,
        json!([{"op":"replace","path":"/turns/0/items","value":reordered}]),
    );
    receive(
        &mut encoder,
        snapshot(&mut projection, &state, &mut window),
        &mut client,
        &mut revision,
    );
    assert_eq!(ids(&client)[0], "new");
    assert!(ids(&client).contains(&"item-56"));
    window.expand();
    receive(
        &mut encoder,
        snapshot(&mut projection, &state, &mut window),
        &mut client,
        &mut revision,
    );
    assert_eq!(client["hasEarlierMessages"], false);
    assert_eq!(ids(&client).len(), 125);
}

#[test]
fn initial_window_counts_visible_bodies_not_interleaved_activity_or_empty_messages() {
    let mut state = json!({"id":"thread","turns":(0..45).map(|index| {
        let body = match index % 6 {
            0 => json!({"id":format!("body-{index}"),"type":"userMessage","content":[{"type":"text","text":"question"}]}),
            1 => json!({"id":format!("body-{index}"),"type":"steeringUserMessage","input":[{"type":"text","text":"follow-up"}]}),
            2 => json!({"id":format!("body-{index}"),"type":"agentMessage","text":"answer"}),
            3 => json!({"id":format!("body-{index}"),"type":"assistantMessage","text":"answer"}),
            4 => json!({"id":format!("body-{index}"),"type":"planImplementation","planContent":"plan"}),
            _ => json!({"id":format!("body-{index}"),"type":"error","message":"error"}),
        };
        let body = if index == 37 { json!({"id":"body-37","type":"userMessage","content":[{"type":"file","path":"/public/image.png"}]}) } else { body };
        json!({"turnId":format!("turn-{index}"),"status":"completed","items":[
            body,
            {"id":format!("empty-{index}"),"type":"agentMessage","text":"  "},
            {"id":format!("reason-{index}"),"type":"reasoning","summary":["r".repeat(6000)]},
            {"id":format!("command-{index}"),"type":"commandExecution","command":"test","aggregatedOutput":"c".repeat(6000)},
            {"id":format!("tool-{index}"),"type":"mcpToolCall","tool":"lookup","result":{"content":[{"type":"text","text":"t".repeat(6000)}]}}
        ]})
    }).collect::<Vec<_>>()});
    let mut projection = Projection::default();
    let mut window = MessageWindow::default();
    let mut encoder = Encoder::new(StreamVersion::V2);
    let (mut client, mut revision) = (Value::Null, 0);
    let frame = receive(
        &mut encoder,
        snapshot(&mut projection, &state, &mut window),
        &mut client,
        &mut revision,
    );
    assert!(
        frame.len() > MESSAGE_BUDGET,
        "activities must not exhaust the body budget"
    );
    assert_eq!(ids(&client).len(), 100);
    assert_eq!(ids(&client).first(), Some(&"body-25"));
    assert_eq!(ids(&client).last(), Some(&"tool-44"));
    assert_eq!(client["hasEarlierMessages"], true);
    let image_only = client["turns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|turn| turn["messages"].as_array().unwrap())
        .find(|message| message["id"] == "body-37")
        .unwrap();
    assert_eq!(image_only["text"], "");
    assert_eq!(image_only["attachments"].as_array().unwrap().len(), 1);

    apply(
        &mut projection,
        &mut state,
        json!([
            {"op":"add","path":"/turns/44/items/-","value":{"id":"live-activity","type":"commandExecution","command":"live"}},
            {"op":"add","path":"/turns/44/items/-","value":{"id":"live-body","type":"agentMessage","text":"live answer"}}
        ]),
    );
    receive(
        &mut encoder,
        snapshot(&mut projection, &state, &mut window),
        &mut client,
        &mut revision,
    );
    assert_eq!(ids(&client).first(), Some(&"body-25"));
    assert_eq!(
        &ids(&client)[ids(&client).len() - 2..],
        &["live-activity", "live-body"]
    );
    for (first, more) in [("body-15", true), ("body-5", true), ("body-0", false)] {
        window.expand();
        receive(
            &mut encoder,
            snapshot(&mut projection, &state, &mut window),
            &mut client,
            &mut revision,
        );
        assert_eq!(ids(&client).first(), Some(&first));
        assert_eq!(ids(&client).last(), Some(&"live-body"));
        assert_eq!(client["hasEarlierMessages"], more);
    }
    assert_eq!(ids(&client).len(), 227);
}

#[test]
fn activity_only_initial_window_is_still_bounded_and_history_remains_available() {
    let small = json!({"turns":[{"turnId":"turn","status":"inProgress","items":(0..35).map(|id| json!({"id":format!("small-{id}"),"type":"reasoning","summary":["public"]})).collect::<Vec<_>>()}]});
    let mut projection = Projection::default();
    let small_view = snapshot(&mut projection, &small, &mut MessageWindow::default());
    assert_eq!(ids(view(&small_view)).len(), 35);
    assert_eq!(view(&small_view)["hasEarlierMessages"], false);
    assert_eq!(view(&small_view)["turns"][0]["status"], "inProgress");

    let mut state = json!({"turns":[{"turnId":"turn","items":(0..10).map(|id| json!({
        "id":format!("activity-{id}"),"type":"fileChange",
        "changes":(0..8).map(|part| json!({"path":format!("{part}.txt"),"diff":"x".repeat(128 * 1024)})).collect::<Vec<_>>()
    })).collect::<Vec<_>>()}]});
    projection.reset();
    let mut window = MessageWindow::default();
    let first = snapshot(&mut projection, &state, &mut window);
    let initial_ids = ids(view(&first));
    assert!(initial_ids.len() > 1 && initial_ids.len() < 10);
    assert_eq!(initial_ids.last(), Some(&"activity-9"));
    assert_eq!(view(&first)["hasEarlierMessages"], true);
    assert!(view(&first).to_string().len() <= 8 * 1024 * 1024 + 2048);
    let mut previous_count = initial_ids.len();
    while previous_count < 10 {
        window.expand();
        let expanded = snapshot(&mut projection, &state, &mut window);
        let expanded_ids = ids(view(&expanded));
        assert!(expanded_ids.len() > previous_count);
        assert_eq!(expanded_ids.last(), Some(&"activity-9"));
        previous_count = expanded_ids.len();
        assert_eq!(view(&expanded)["hasEarlierMessages"], previous_count < 10);
    }

    state["turns"][0]["items"].as_array_mut().unwrap().insert(
        0,
        json!({"id":"earlier-body","type":"agentMessage","text":"earlier"}),
    );
    projection.reset();
    let mut fresh = MessageWindow::default();
    let limited = snapshot(&mut projection, &state, &mut fresh);
    assert!(!ids(view(&limited)).contains(&"earlier-body"));
    assert_eq!(view(&limited)["hasEarlierMessages"], true);
    let mut reached_body = false;
    for _ in 0..10 {
        fresh.expand();
        let more = snapshot(&mut projection, &state, &mut fresh);
        if ids(view(&more)).contains(&"earlier-body") {
            assert_eq!(view(&more)["hasEarlierMessages"], false);
            reached_body = true;
            break;
        }
        assert_eq!(view(&more)["hasEarlierMessages"], true);
    }
    assert!(reached_body);
}

#[test]
fn history_has_a_soft_extra_byte_budget_but_never_discards_one_oversized_item() {
    let state = native_history(30, 64 * 1024);
    let mut projection = Projection::default();
    let mut window = MessageWindow::default();
    let first = snapshot(&mut projection, &state, &mut window);
    assert_eq!(ids(view(&first)).len(), 3);
    window.expand();
    let more = snapshot(&mut projection, &state, &mut window);
    assert_eq!(ids(view(&more)).len(), 6);
    let state = json!({"turns":[{"turnId":"big","items":[{"id":"old","type":"agentMessage","text":"earlier"},{"id":"big","type":"fileChange","changes":(0..65).map(|id|json!({"path":format!("{id}.txt"),"diff":"x".repeat(128 * 1024)})).collect::<Vec<_>>()}]}]});
    let mut window = MessageWindow::default();
    projection.reset();
    let big = snapshot(&mut projection, &state, &mut window);
    assert_eq!(ids(view(&big)), ["big"]);
    assert_eq!(
        view(&big)["turns"][0]["messages"][0]["changes"]
            .as_array()
            .unwrap()
            .len(),
        65
    );
    assert!(view(&big).to_string().len() > MESSAGE_BUDGET);
    assert_eq!(view(&big)["hasEarlierMessages"], true);
    window.expand();
    assert_eq!(
        ids(view(&snapshot(&mut projection, &state, &mut window))),
        ["old", "big"]
    );
}

#[test]
fn efficient_projection_removes_only_redundant_text_and_verified_inline_image_bodies() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut bytes = vec![0; 4 * 1024 * 1024];
    bytes[..6].copy_from_slice(b"GIF89a");
    let url = format!("data:image/gif;base64,{}", STANDARD.encode(bytes));
    let state = json!({"turns":[{"turnId":"turn","items":[
        {"id":"image","type":"userMessage","content":[{"type":"file","path":"/public/report.txt"},{"type":"image","url":url},{"type":"image","url":"https://example.test/public.png"}]},
        {"id":"command","type":"commandExecution","command":"test","aggregatedOutput":"public output"},
        {"id":"diff","type":"fileChange","changes":[{"path":"a","diff":"+public"}]},
        {"id":"tool","type":"mcpToolCall","tool":"lookup","result":{"content":[{"type":"text","text":"public tool result"}],"_meta":"PRIVATE_IMAGE_CANARY"}},
        {"id":"summary","type":"reasoning","summary":["public summary"],"content":"PRIVATE_IMAGE_CANARY"},
        {"id":"unknown","type":"futurePrivateType","config":"PRIVATE_IMAGE_CANARY","text":"PRIVATE_IMAGE_CANARY"}
    ]}]});
    let legacy = protocol::view(&state);
    let mut projection = Projection::default();
    let result = snapshot(&mut projection, &state, &mut MessageWindow::default());
    let result = view(&result);
    let messages = result["turns"][0]["messages"].as_array().unwrap();
    let image = &messages[0]["attachments"][1];
    assert_eq!(image["name"], "图片");
    assert_eq!(
        image["id"],
        crate::remote_control::files::inline_image_reference_id(
            &json!("turn"),
            &json!("image"),
            1,
            &url
        )
    );
    assert!(image.get("url").is_none());
    assert_eq!(
        messages[0]["attachments"][2]["url"],
        "https://example.test/public.png"
    );
    assert!(result.to_string().len() < 4096);
    for message in messages.iter().take(4).skip(1) {
        assert_eq!(message["text"], "");
    }
    assert_eq!(messages[1]["command"]["output"], "public output");
    assert_eq!(messages[2]["changes"][0]["diff"], "+public");
    assert_eq!(messages[3]["tool"]["output"], "public tool result");
    assert_eq!(messages[4]["text"], "public summary");
    assert_eq!(messages[5]["text"], "此内容请在电脑端查看");
    assert!(!result.to_string().contains("PRIVATE_IMAGE_CANARY"));
    assert_eq!(
        legacy["turns"][0]["messages"][0]["attachments"][1]["url"],
        url
    );
    let PublicState::Legacy(frame) =
        projection.public_state(&state, StreamVersion::Legacy, &mut MessageWindow::default())
    else {
        panic!("legacy frame");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&frame).unwrap(),
        json!({"type":"state","state":legacy})
    );
    let mut encoder = Encoder::new(StreamVersion::Legacy);
    let state = Arc::new(PublicState::Legacy(frame));
    let frame = encoder.prepare(state.clone()).unwrap().unwrap();
    encoder.delivered(frame);
    assert!(encoder.prepare(state).unwrap().is_none());
}

#[test]
fn all_active_turns_and_current_empty_turn_keep_composer_control_metadata() {
    let mut state = native_history(40, 4);
    state["latestThreadSettings"] = json!({"model":"native-model","effort":"high","serviceTier":"priority","permissions":":workspace","approvalPolicy":"on-request"});
    state["threadRuntimeStatus"] = json!({"type":"running"});
    state["requests"] = json!([{"id":1,"method":"item/commandExecution/requestApproval","params":{"command":"public command","cwd":"/synthetic-workspace"}}]);
    state["turns"][0]["status"] = json!("inProgress");
    state["turns"].as_array_mut().unwrap().insert(
        0,
        json!({"turnId":"older-active","status":"inProgress","items":[]}),
    );
    state["turns"]
        .as_array_mut()
        .unwrap()
        .push(json!({"turnId":"current-empty","status":"completed","items":[]}));
    let projected = snapshot(
        &mut Projection::default(),
        &state,
        &mut MessageWindow::default(),
    );
    let turns = view(&projected)["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0]["id"], "older-active");
    assert_eq!(turns[0]["status"], "inProgress");
    assert!(turns[0]["messages"].as_array().unwrap().is_empty());
    assert_eq!(turns[2]["id"], "current-empty");
    assert_eq!(ids(view(&projected)).len(), 20);
    let legacy = protocol::view(&state);
    for field in [
        "id",
        "cwd",
        "model",
        "effort",
        "serviceTier",
        "permissionMode",
        "status",
        "requests",
        "historyComplete",
    ] {
        assert_eq!(view(&projected)[field], legacy[field]);
    }
    assert_eq!(
        turns
            .iter()
            .rev()
            .find(|turn| turn["status"] == "inProgress")
            .unwrap()["id"],
        "turn"
    );
}

#[test]
fn a_partially_supported_file_change_keeps_public_text_not_represented_by_structured_changes() {
    let turn = json!({"turnId":"turn","items":[{"type":"fileChange","changes":[{"path":"public path","diff":[{"text":"public noncanonical diff"}]}]}]});
    let compact = compact_turn(&turn);
    assert!(
        compact["messages"][0]["changes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        compact["messages"][0]["text"],
        "public path\npublic noncanonical diff"
    );
}

#[test]
fn missing_or_repeated_ids_do_not_expand_an_unchanged_window_into_all_history() {
    for turn_id in [Value::Null, json!("same")] {
        let state = json!({"turns":(0..40).map(|index|json!({"turnId":turn_id,"items":[{"id":"same-item","type":"agentMessage","text":format!("public-{index}")}]})).collect::<Vec<_>>()});
        let mut projection = Projection::default();
        let mut window = MessageWindow::default();
        let first = snapshot(&mut projection, &state, &mut window);
        assert_eq!(ids(view(&first)).len(), 20);
        let second = snapshot(&mut projection, &state, &mut window);
        assert!(first == second);
    }
}

#[test]
fn canonical_reordering_snapshots_empty_turns_and_duplicate_ids_cannot_reuse_stale_projection() {
    let mut state = json!({"id":"t","turnHistory":{"kind":"canonical","history":{"entitiesByKey":{"a":{"turnId":"a","items":[{"id":"i","type":"agentMessage","text":"old"}]},"b":{"turnId":"b","items":[]}},"islands":[{"entries":[{"value":"a"},{"value":"b"}]}]}}});
    let mut projection = Projection::default();
    let mut window = MessageWindow::default();
    let mut encoder = Encoder::new(StreamVersion::V2);
    let (mut client, mut revision) = (Value::Null, 0);
    receive(
        &mut encoder,
        snapshot(&mut projection, &state, &mut window),
        &mut client,
        &mut revision,
    );
    for patches in [
        json!([{"op":"replace","path":"/turnHistory/history/entitiesByKey/a/items/0/text","value":"new"}]),
        json!([{"op":"replace","path":"/turnHistory/history/islands/0/entries","value":[{"value":"b"},{"value":"a"}]}]),
        json!([{"op":"remove","path":"/turnHistory/history/entitiesByKey/a/items/0"}]),
    ] {
        apply(&mut projection, &mut state, patches);
        receive(
            &mut encoder,
            snapshot(&mut projection, &state, &mut window),
            &mut client,
            &mut revision,
        );
    }
    projection.reset();
    let duplicate = json!({"turns":[{"turnId":"same","items":[{"id":"a","type":"agentMessage","text":"one"}]},{"turnId":"same","items":[{"id":"b","type":"agentMessage","text":"two"}]}]});
    let value = snapshot(&mut projection, &duplicate, &mut MessageWindow::default());
    assert_eq!(ids(view(&value)), ["a", "b"]);
    assert_eq!(
        view(&value)["turns"][1],
        compact_turn(&duplicate["turns"][1])
    );
}

#[test]
fn patches_escape_json_pointers_and_bound_large_structural_changes_to_one_public_root_replace() {
    let before = json!({"a/b":{"~c":[1,2,3]},"remove":true});
    let after = json!({"a/b":{"~c":[2,{"public":true}]},"add":false});
    let mut patches = Vec::new();
    diff(&before, &after, "", &mut patches);
    let mut rebuilt = before;
    protocol::apply_patches(&mut rebuilt, &json!(patches)).unwrap();
    assert_eq!(rebuilt, after);
    let before = json!({"turns":[{"messages":(0..5000).map(|_|json!({"text":"before"})).collect::<Vec<_>>()}]});
    let after = json!({"turns":[{"messages":(0..5000).map(|_|json!({"text":"after"})).collect::<Vec<_>>()}]});
    let mut encoder = Encoder::new(StreamVersion::V2);
    let first = encoder
        .prepare(Arc::new(PublicState::V2(before.clone())))
        .unwrap()
        .unwrap();
    encoder.delivered(first);
    let frame = encoder
        .prepare(Arc::new(PublicState::V2(after.clone())))
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(&frame.text).unwrap();
    assert_eq!(value["patches"].as_array().unwrap().len(), 1);
    assert_eq!(value["patches"][0]["path"], "");
    let mut rebuilt = before;
    protocol::apply_patches(&mut rebuilt, &value["patches"]).unwrap();
    assert_eq!(rebuilt, after);
}
