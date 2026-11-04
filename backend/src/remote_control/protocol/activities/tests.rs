use super::super::{MAX_PUBLIC_TEXT, item_view, view};
use serde_json::{Value, json};

fn project(item: Value) -> Value {
    item_view(&item, &json!("turn"), 0)
}

#[test]
fn collab_spawn_wait_completion_and_failure_only_publish_names_states_and_results() {
    let secret = "PRIVATE_COLLAB_CANARY";
    let mut item = json!({"id":"call","type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress",
        "senderThreadId":"root","receiverThreadIds":["b","a"],"prompt":secret,"developerInstructions":secret,
        "arguments":{"message":secret},"_meta":{"auth":secret},"config":secret,
        "receiverThreads":[{"threadId":"a","thread":{"agentNickname":"Mock Reviewer","title":secret,"preview":secret,"turns":[{"text":secret}],"config":secret}}],
        "agentsStates":{"a":{"status":"pendingInit","message":null,"_meta":secret},"b":{"status":"running","message":null}}});
    let spawned = project(item.clone());
    assert_eq!(spawned["id"], "call");
    assert_eq!(spawned["kind"], "collabAgentToolCall");
    assert_eq!(spawned["role"], "activity");
    assert_eq!(spawned["text"], "");
    assert_eq!(spawned["status"], "inProgress");
    assert_eq!(
        spawned["collaboration"],
        json!({"action":"spawnAgent","agents":[{"id":"b","status":"running"},{"id":"a","name":"Mock Reviewer","status":"pendingInit"}]})
    );
    assert!(spawned.get("tool").is_none());
    assert!(!spawned.to_string().contains(secret));

    item["tool"] = json!("wait");
    item["status"] = json!("completed");
    item["agentsStates"] = json!({"a":{"status":"completed","message":"公开审查结果","auth":secret},"b":{"status":"errored","message":"公开错误"},"c":{"status":"shutdown","message":null}});
    let finished = project(item.clone());
    assert_eq!(finished["status"], "completed");
    assert_eq!(
        finished["collaboration"],
        json!({"action":"wait","agents":[{"id":"b","status":"errored","message":"公开错误"},{"id":"a","name":"Mock Reviewer","status":"completed","message":"公开审查结果"},{"id":"c","status":"shutdown"}]})
    );
    assert!(!finished.to_string().contains(secret));

    item["tool"] = json!("spawnAgent");
    item["status"] = json!("failed");
    let failed = project(item.clone());
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["collaboration"]["agents"][0]["status"], "errored");
    for action in [
        "sendInput",
        "resumeAgent",
        "closeAgent",
        "sendMessage",
        "followupTask",
        "interruptAgent",
        "listAgents",
    ] {
        item["tool"] = json!(action);
        assert_eq!(project(item.clone())["collaboration"]["action"], action);
    }
}

#[test]
fn native_lifecycle_items_keep_actions_and_public_task_names_without_reading_children() {
    for (action, status) in [
        ("started", Some("running")),
        ("interacted", None),
        ("completed", Some("completed")),
        ("interrupted", Some("interrupted")),
    ] {
        let item = project(
            json!({"id":"event","type":"subAgentActivity","kind":action,"agentThreadId":"child",
            "agentPath":"/root/mock_review","prompt":"PRIVATE_ACTIVITY_CANARY","developerInstructions":"PRIVATE_ACTIVITY_CANARY",
            "result":{"config":"PRIVATE_ACTIVITY_CANARY"},"_meta":"PRIVATE_ACTIVITY_CANARY"}),
        );
        let mut agent = json!({"id":"child","name":"mock_review"});
        if let Some(status) = status {
            agent["status"] = json!(status);
        }
        assert_eq!(
            item["collaboration"],
            json!({"action":action,"agents":[agent]})
        );
        assert_eq!(item["status"], Value::Null);
        assert!(!item.to_string().contains("PRIVATE_ACTIVITY_CANARY"));
    }
}

#[test]
fn empty_opaque_and_unknown_activities_do_not_dump_private_payloads() {
    let secret = "PRIVATE_OPAQUE_CANARY";
    let empty = project(
        json!({"type":"collabAgentToolCall","tool":"wait","receiverThreadIds":[],"agentsStates":{},"result":{"config":secret}}),
    );
    assert_eq!(empty["collaboration"], json!({"action":"wait","agents":[]}));
    assert!(!empty.to_string().contains(secret));
    let opaque = project(
        json!({"type":"collabAgentToolCall","tool":"spawnAgent","receiverThreadIds":["a",{"auth":secret}],
        "receiverThreads":[{"threadId":"a","thread":{"agentNickname":{"config":secret},"title":secret}}],
        "agentsStates":{"a":{"status":"completed","message":{"config":secret}},"b":{"status":"futureState","message":secret}}}),
    );
    assert_eq!(
        opaque["collaboration"],
        json!({"action":"spawnAgent","agents":[{"id":"a","status":"completed"},{"id":"b"}]})
    );
    assert!(!opaque.to_string().contains(secret));
    for item in [
        json!({"type":"collabAgentToolCall","tool":"futureAction","text":secret,"prompt":secret,"agentsStates":{"a":{"message":secret}}}),
        json!({"type":"subAgentActivity","kind":"futureAction","agentThreadId":"a","agentPath":secret}),
        json!({"type":"subAgentActivity","kind":"started","agentThreadId":{"auth":secret},"agentPath":secret}),
        json!({"type":"futurePrivateItem","text":secret,"tool":secret,"displayTitle":secret,"result":{"content":[{"type":"text","text":secret}]}}),
    ] {
        let item = project(item);
        assert_eq!(item["text"], "此内容请在电脑端查看");
        assert!(item.get("collaboration").is_none());
        assert!(item.get("tool").is_none());
        assert!(!item.to_string().contains(secret));
    }
}

#[test]
fn dynamic_tool_text_results_follow_the_native_input_text_schema() {
    let result = project(
        json!({"type":"dynamicToolCall","tool":"lookup","contentItems":[
        {"type":"inputText","text":"公开工具结果"},
        {"type":"opaque","text":"PRIVATE_DYNAMIC_CANARY"},
        {"type":"inputImage","imageUrl":"PRIVATE_DYNAMIC_CANARY"}
    ],"_meta":"PRIVATE_DYNAMIC_CANARY"}),
    );
    assert_eq!(result["tool"]["name"], "lookup");
    assert_eq!(result["tool"]["output"], "公开工具结果");
    assert_eq!(result["text"], "公开工具结果");
    assert!(!result.to_string().contains("PRIVATE_DYNAMIC_CANARY"));
}

#[test]
fn collaboration_fields_share_the_existing_utf8_public_text_bound() {
    let text = "测".repeat(MAX_PUBLIC_TEXT);
    let result = view(&json!({"turns":[{"turnId":"turn","items":[
        {"type":"collabAgentToolCall","tool":"wait","receiverThreadIds":["a"],
            "receiverThreads":[{"threadId":"a","thread":{"agentNickname":text}}],
            "agentsStates":{"a":{"status":"completed","message":text}}},
        {"type":"subAgentActivity","kind":"completed","agentThreadId":"a","agentPath":format!("/root/{text}")}
    ]}]}));
    for item in result["turns"][0]["messages"].as_array().unwrap() {
        assert_eq!(item["truncated"], true);
        let agent = &item["collaboration"]["agents"][0];
        assert!(agent["name"].as_str().unwrap().len() <= MAX_PUBLIC_TEXT);
        if let Some(message) = agent["message"].as_str() {
            assert!(message.len() <= MAX_PUBLIC_TEXT);
        }
    }
}
