//! Desktop follower protocol, adapted from codex-mobile-bridge (MIT).
//! See third_party/codex-mobile-bridge.LICENSE.
use serde_json::{Value, json};

mod activities;
mod approvals;

pub(super) const MAX_PUBLIC_TEXT: usize = 128 * 1024;

pub(super) fn patch_path(patch: &Value) -> Result<Vec<String>, String> {
    match &patch["path"] {
        Value::String(path) if path.is_empty() || path.starts_with('/') => Ok(path
            .split('/')
            .skip(1)
            .map(|s| s.replace("~1", "/").replace("~0", "~"))
            .collect()),
        Value::Array(path) => path
            .iter()
            .map(|part| match part {
                Value::String(s) => Ok(s.clone()),
                Value::Number(n) if n.is_u64() => Ok(n.to_string()),
                _ => Err("桌面增量路径无效".to_string()),
            })
            .collect(),
        _ => Err("桌面增量路径无效".into()),
    }
}

pub(super) fn apply_patches(state: &mut Value, patches: &Value) -> Result<(), String> {
    let patches = patches.as_array().ok_or("桌面增量格式无效")?;
    if patches.len() > 10_000 {
        return Err("桌面增量过大".into());
    }
    for patch in patches {
        let path = patch_path(patch)?;
        let op = patch["op"].as_str().ok_or("桌面增量操作无效")?;
        if !matches!(op, "add" | "replace" | "remove") {
            return Err("不支持的桌面增量操作".into());
        }
        if op != "remove" && patch.get("value").is_none() {
            return Err("桌面增量缺少内容".into());
        }
        if path.is_empty() {
            if op != "replace" {
                return Err("不支持的桌面根增量".into());
            }
            *state = patch["value"].clone();
            continue;
        }
        let mut target = &mut *state;
        for key in &path[..path.len() - 1] {
            target = match target {
                Value::Array(items) => {
                    items.get_mut(key.parse::<usize>().map_err(|_| "增量下标无效")?)
                }
                Value::Object(map) => map.get_mut(key),
                _ => None,
            }
            .ok_or("桌面增量路径不存在")?;
        }
        let key = path.last().unwrap();
        match target {
            Value::Array(items) => {
                let index = if key == "-" && op == "add" {
                    items.len()
                } else {
                    key.parse().map_err(|_| "增量下标无效")?
                };
                if index > items.len() || (index == items.len() && op != "add") {
                    return Err("增量下标越界".into());
                }
                match op {
                    "add" => items.insert(index, patch["value"].clone()),
                    "remove" => {
                        items.remove(index);
                    }
                    _ => items[index] = patch["value"].clone(),
                }
            }
            Value::Object(map) => {
                if op != "add" && !map.contains_key(key) {
                    return Err("桌面增量字段不存在".into());
                }
                if op == "remove" {
                    map.remove(key);
                } else {
                    map.insert(key.clone(), patch["value"].clone());
                }
            }
            _ => return Err("桌面增量目标无效".into()),
        }
    }
    Ok(())
}

pub(super) fn ordered_items(value: &Value) -> Vec<&Value> {
    if let Some(items) = value.as_array() {
        return items.iter().collect();
    }
    if let Some(entities) = value["entitiesByKey"].as_object() {
        return value["islands"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|island| island["entries"].as_array().into_iter().flatten())
            .filter_map(|entry| entry["value"].as_str().and_then(|key| entities.get(key)))
            .collect();
    }
    value["items"].as_array().into_iter().flatten().collect()
}

pub(super) fn turns(state: &Value) -> Vec<&Value> {
    if state["turnHistory"]["kind"] == "canonical" {
        ordered_items(&state["turnHistory"]["history"])
    } else {
        ordered_items(&state["turns"])
    }
}

pub(super) fn text_content(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| {
            part.as_str().or_else(|| {
                matches!(
                    part["type"].as_str(),
                    None | Some(
                        "text" | "inputText" | "input_text" | "output_text" | "summary_text"
                    )
                )
                .then_some(part["text"].as_str())
                .flatten()
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn scalar_id(value: &Value) -> Value {
    if value.is_string() || value.as_i64().is_some() || value.as_u64().is_some() {
        value.clone()
    } else {
        Value::Null
    }
}

fn error_text(value: &Value) -> Option<&str> {
    value.as_str().or(value["message"].as_str())
}

pub(super) fn changes_view(value: &Value) -> Vec<Value> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|change| {
            let path = change["path"].as_str()?;
            let diff = change["diff"].as_str()?;
            let mut result = json!({"path":path,"diff":diff});
            if let Some(kind) = change["kind"].as_str().or(change["kind"]["type"].as_str()) {
                result["kind"] = json!(kind);
            }
            Some(result)
        })
        .collect()
}

fn tool_output(item: &Value) -> String {
    let result = &item["result"];
    let mut parts = Vec::new();
    for content in [
        result.get("content"),
        item.get("contentItems"),
        item.get("content"),
    ]
    .into_iter()
    .flatten()
    {
        let text = text_content(content);
        if !text.is_empty() {
            parts.push(text);
        }
    }
    if let Some(content) = result
        .get("structuredContent")
        .or(item.get("structuredContent"))
    {
        // Only the protocol's explicitly public result, never its RPC envelope.
        parts.push(content.to_string());
    }
    if let Some(text) = result.as_str() {
        parts.push(text.to_string());
    }
    parts.join("\n")
}

fn public_tool_arguments(value: &Value) -> Value {
    match value {
        Value::Object(fields) => fields
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.to_ascii_lowercase().as_str(),
                    "_meta" | "auth" | "env" | "config"
                )
            })
            .map(|(key, value)| (key.clone(), public_tool_arguments(value)))
            .collect(),
        Value::Array(values) => values.iter().map(public_tool_arguments).collect(),
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(value) if value.is_object() || value.is_array() => {
                json!(public_tool_arguments(&value).to_string())
            }
            _ => value.clone(),
        },
        _ => value.clone(),
    }
}

pub(super) fn item_view(item: &Value, turn_id: &Value, index: usize) -> Value {
    let kind = item["type"].as_str().unwrap_or("unknown");
    let collaboration = activities::collaboration(item);
    let (role, text) = match kind {
        "userMessage" => ("user", text_content(&item["content"])),
        "steeringUserMessage" => ("user", text_content(&item["input"])),
        "agentMessage" | "assistantMessage" => ("assistant", text_content(&item["text"])),
        "commandExecution" => (
            "activity",
            format!(
                "{}\n{}",
                text_content(&item["command"]),
                text_content(&item["aggregatedOutput"])
            ),
        ),
        "fileChange" => (
            "activity",
            item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| format!("{}\n{}", text_content(&c["path"]), text_content(&c["diff"])))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        "reasoning" => ("activity", text_content(&item["summary"])),
        "planImplementation" => ("assistant", text_content(&item["planContent"])),
        "plan" => ("activity", String::new()),
        "mcpToolCall" | "dynamicToolCall" => ("activity", tool_output(item)),
        "collabAgentToolCall" | "subAgentActivity" if collaboration.is_some() => {
            ("activity", String::new())
        }
        "error" => ("error", text_content(&item["message"])),
        _ => ("activity", "此内容请在电脑端查看".to_string()),
    };
    let content = if matches!(
        kind,
        "userMessage" | "steeringUserMessage" | "agentMessage" | "assistantMessage"
    ) {
        item.get("content")
            .or(item.get("input"))
            .unwrap_or(&Value::Null)
    } else {
        &Value::Null
    };
    let mut attachments: Vec<Value> = content
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(part_index, part)| match part["type"].as_str()? {
            "localImage" | "file" | "image" => {
                if part["type"] == "image" && let Some(url) = super::files::public_image_url(&part["url"]) {
                    return Some(json!({"name":"图片","url":url}));
                }
                let path = part["path"].as_str().or(part["url"].as_str())?;
                Some(json!({"name":super::files::basename(path),"id":super::files::reference_id(turn_id, &item["id"], index, part_index, path)}))
            }
            _ => None,
        })
        .collect();
    if matches!(kind, "mcpToolCall" | "dynamicToolCall") {
        for content in [
            item["result"].get("content"),
            item.get("contentItems"),
            item.get("content"),
        ]
        .into_iter()
        .flatten()
        {
            for part in content
                .as_array()
                .into_iter()
                .flatten()
                .filter(|part| part["type"] == "image")
            {
                if let (Some(mime), Some(data)) = (part["mimeType"].as_str(), part["data"].as_str())
                {
                    let url = format!("data:{mime};base64,{data}");
                    if super::files::public_image_url(&json!(url)).is_some() {
                        attachments.push(json!({"name":"工具图片","url":url}));
                    }
                }
            }
        }
    }
    let id = scalar_id(&item["id"]);
    let mut result = json!({"id": if id.is_null() { json!(format!("{}:item:{index}",turn_id.as_str().unwrap_or("turn"))) } else { id }, "kind": kind, "role": role, "text": text, "status": item["status"].as_str(), "attachments":attachments});
    match kind {
        "commandExecution" => {
            result["command"] = json!({"command":text_content(&item["command"]),"output":text_content(&item["aggregatedOutput"])});
            if let Some(cwd) = item["cwd"].as_str() {
                result["command"]["cwd"] = json!(cwd);
            }
            if let Some(code) = item["exitCode"].as_i64() {
                result["command"]["exitCode"] = json!(code);
            }
        }
        "fileChange" => result["changes"] = json!(changes_view(&item["changes"])),
        "mcpToolCall" | "dynamicToolCall" => {
            let name = item["tool"]
                .as_str()
                .or(item["toolName"].as_str())
                .unwrap_or("工具");
            result["tool"] = json!({"name":name,"output":tool_output(item)});
            if let Some(input) = item.get("arguments") {
                let input = public_tool_arguments(input);
                result["tool"]["input"] = json!(
                    input
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| input.to_string())
                );
            }
            if let Some(error) = error_text(&item["error"]) {
                result["tool"]["error"] = json!(error);
            }
        }
        "plan" => {
            result["plan"] = item["plan"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|step| {
                    let mut result = json!({"step":step["step"].as_str()?});
                    if let Some(status) = step["status"].as_str() {
                        result["status"] = json!(status);
                    }
                    Some(result)
                })
                .collect()
        }
        _ => {}
    }
    if let Some(collaboration) = collaboration {
        result["collaboration"] = collaboration;
    }
    fn bound(value: &mut Value) -> bool {
        match value {
            Value::String(text) if text.len() > MAX_PUBLIC_TEXT => {
                let mut end = MAX_PUBLIC_TEXT;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                true
            }
            Value::Array(values) => values
                .iter_mut()
                .fold(false, |cut, value| bound(value) || cut),
            Value::Object(values) => values
                .iter_mut()
                .filter(|(key, _)| key.as_str() != "url")
                .fold(false, |cut, (_, value)| bound(value) || cut),
            _ => false,
        }
    }
    if bound(&mut result) {
        result["truncated"] = json!(true);
    }
    result
}

fn permission_mode(settings: &Value) -> &'static str {
    let profile = settings["activePermissionProfile"]["id"]
        .as_str()
        .or(settings["permissions"].as_str());
    if settings["approvalsReviewer"]
        .as_str()
        .is_some_and(|value| value != "user")
    {
        return "custom";
    }
    match (
        profile,
        settings["sandboxPolicy"]["type"].as_str(),
        settings["approvalPolicy"].as_str(),
    ) {
        (Some(":read-only"), _, Some("on-request"))
        | (None, Some("readOnly"), Some("on-request")) => "read-only",
        (Some(":workspace"), _, Some("on-request"))
        | (None, Some("workspaceWrite"), Some("on-request")) => "auto",
        (Some(":danger-full-access"), _, Some("never"))
        | (None, Some("dangerFullAccess"), Some("never")) => "full-access",
        _ => "custom",
    }
}

pub(super) fn request_supported(state: &Value, request: &Value) -> bool {
    approvals::params(state, request).is_some()
}

fn question_replies(text: &str) -> Vec<Value> {
    text.trim()
        .strip_prefix("<send_user_message_question_reply>")
        .and_then(|s| s.strip_suffix("</send_user_message_question_reply>"))
        .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
        .unwrap_or_default()
}

pub(super) fn async_requests(state: &Value) -> Vec<Value> {
    let turns = turns(state);
    let Some(turn) = turns
        .last()
        .filter(|t| t["status"] == "inProgress" || t["status"] == "completed")
    else {
        return Vec::new();
    };
    let items = ordered_items(&turn["items"]);
    let answered: std::collections::HashSet<String> = items
        .iter()
        .filter(|item| {
            item["type"] == "userMessage"
                || (item["type"] == "steeringUserMessage" && item["status"] == "accepted")
        })
        .flat_map(|item| {
            question_replies(&text_content(item.get("content").unwrap_or(&item["input"])))
        })
        .filter_map(|reply| reply["questionItemId"].as_str().map(str::to_string))
        .collect();
    items.into_iter().filter(|item|item["type"] == "agentMessage").filter_map(|item| {
        let id = item["id"].as_str()?;
        let source = item.get("questions").filter(|value|!value.is_null())?;
        // A question group is one capability. Never drop an unknown unanswered
        // question/choice and then authorize answers to only its visible subset.
        let questions = source.as_array().and_then(|questions|questions.iter().enumerate()
            .filter(|(index,_)| !answered.contains(&json!(["request_user_input_async",id,index]).to_string()))
            .map(|(index,question)| {
                let title = question["title"].as_str().filter(|title|!title.trim().is_empty())?;
                let options = match question.get("options").filter(|value|!value.is_null()) {
                    None => Vec::new(),
                    Some(options) => options.as_array()?.iter().map(|option|
                        option.as_str().map(|label|json!({"label":label}))).collect::<Option<Vec<_>>>()?,
                };
                Some(json!({"id":json!(["request_user_input_async",id,index]).to_string(),"question":title,"options":options}))
            }).collect::<Option<Vec<_>>>());
        if questions.as_ref().is_some_and(Vec::is_empty) { return None; }
        Some(json!({"id":format!("async:{}:{id}",turn["turnId"].as_str().unwrap_or_default()),"method":"codey/requestUserInputAsync",
            "supported":questions.is_some(),"params": match questions {
                Some(questions) => json!({"questions":questions}),
                None => json!({"message":"请在电脑端处理此类请求"}),
            }}))
    }).collect()
}

pub(super) fn view(state: &Value) -> Value {
    let mut result = view_metadata(state);
    result["turns"] = turns(state).into_iter().map(turn_view).collect();
    result
}

pub(super) fn turn_view(turn: &Value) -> Value {
    let mut messages: Vec<Value> = ordered_items(&turn["items"])
        .into_iter()
        .enumerate()
        .map(|(index, item)| item_view(item, &turn["turnId"], index))
        .collect();
    if !messages.iter().any(|item| item["role"] == "user") {
        let opening = item_view(
            &json!({"type":"userMessage", "content":turn["params"]["input"]}),
            &turn["turnId"],
            usize::MAX,
        );
        if opening["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
            || opening["attachments"]
                .as_array()
                .is_some_and(|images| !images.is_empty())
        {
            messages.insert(0, opening);
        }
    }
    json!({"id":scalar_id(&turn["turnId"]), "status":turn["status"].as_str(), "messages":messages, "error":error_text(&turn["error"]),
            "startedAt":turn["turnStartedAtMs"].as_i64(), "completedAt":turn["turnCompletedAtMs"].as_i64()})
}

pub(super) fn view_metadata(state: &Value) -> Value {
    let mut requests: Vec<Value> = state["requests"].as_array().into_iter().flatten().map(|request| {
        let params = approvals::params(state, request);
        json!({"id": scalar_id(&request["id"]), "method": request["method"].as_str(), "supported":params.is_some(),
            "params": params.unwrap_or_else(||json!({"message":"请在电脑端处理此类请求"}))})
    }).collect();
    requests.extend(async_requests(state));
    json!({"id":scalar_id(&state["id"]), "title":state["title"].as_str(), "cwd":state["cwd"].as_str(),
        "model":state["latestThreadSettings"]["model"].as_str().or(state["latestModel"].as_str()),
        "effort":state["latestThreadSettings"]["effort"].as_str().or(state["latestReasoningEffort"].as_str()),
        "serviceTier":state["latestThreadSettings"]["serviceTier"].as_str(),
        "permissionMode":permission_mode(&state["latestThreadSettings"]),
        "status":state["threadRuntimeStatus"]["type"].as_str(), "requests":requests,
        "historyComplete":state["turnHistory"]["history"]["isComplete"].as_bool().or(state["turnsPagination"]["hasLoadedOldest"].as_bool()).unwrap_or(true)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_projection_never_serializes_unknown_rpc_or_nested_private_fields() {
        let secret = "PRIVATE_RPC_CANARY";
        let state = json!({"id":"thread","title":{"config":secret},"cwd":{"env":secret},
            "latestThreadSettings":{"serviceTier":{"auth":secret}},
            "turns":[{"turnId":"turn","status":{"auth":secret},"error":{"config":secret},"items":[
                {"id":"unknown","type":"futurePrivateItem","config":secret,"text":secret,"content":[{"type":"file","path":secret}]},
                {"id":"summary","type":"reasoning","summary":["公开摘要"],"content":secret,"encryptedContent":secret},
                {"id":"image","type":"userMessage","status":{"auth":secret},"content":[{"type":"image","url":{"auth":secret}}]}
            ]}],"requests":[{"id":1,"method":"item/commandExecution/requestApproval","params":{"command":"echo safe","cwd":"/workspace","_meta":{"auth":secret},"config":{"env":secret}}}]});
        let projected = view(&state);
        assert!(!projected.to_string().contains(secret));
        assert_eq!(
            projected["turns"][0]["messages"][0]["text"],
            "此内容请在电脑端查看"
        );
        assert_eq!(projected["turns"][0]["messages"][1]["text"], "公开摘要");
        assert_eq!(projected["requests"][0]["params"]["command"], "echo safe");
    }

    #[test]
    fn rich_items_keep_native_order_ids_and_only_explicit_tool_content() {
        let state = json!({"turns":[{"turnId":"turn","items":[
            {"id":"answer","type":"agentMessage","text":[{"type":"text","text":"one"},{"type":"output_text","text":"two"}]},
            {"id":"cmd","type":"commandExecution","command":"cargo test","cwd":"workspace","aggregatedOutput":"ok","exitCode":0},
            {"id":"tool","type":"mcpToolCall","tool":"lookup","arguments":{"query":"public"},"result":{"content":[{"type":"text","text":"result"}],"structuredContent":{"count":1},"_meta":{"auth":"PRIVATE_TOOL_CANARY"}},"config":"PRIVATE_TOOL_CANARY"},
            {"id":"plan","type":"plan","plan":[{"step":"test","status":"in_progress","_meta":"PRIVATE_TOOL_CANARY"}]},
            {"id":"diff","type":"fileChange","changes":[{"path":"a.rs","kind":{"type":"update","private":"PRIVATE_TOOL_CANARY"},"diff":"+ok"}]}
        ]}]});
        let projected = view(&state);
        let items = projected["turns"][0]["messages"].as_array().unwrap();
        assert_eq!(
            items
                .iter()
                .map(|v| v["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["answer", "cmd", "tool", "plan", "diff"]
        );
        assert_eq!(items[0]["text"], "one\ntwo");
        assert_eq!(items[1]["command"]["exitCode"], 0);
        assert_eq!(items[2]["tool"]["name"], "lookup");
        assert!(
            items[2]["tool"]["output"]
                .as_str()
                .unwrap()
                .contains("count")
        );
        assert_eq!(
            items[3]["plan"][0],
            json!({"step":"test","status":"in_progress"})
        );
        assert_eq!(
            items[4]["changes"][0],
            json!({"path":"a.rs","kind":"update","diff":"+ok"})
        );
        assert!(!projected.to_string().contains("PRIVATE_TOOL_CANARY"));
    }

    #[test]
    fn tool_arguments_filter_nested_private_configuration_in_objects_and_encoded_json() {
        let secret = "PRIVATE_ARGUMENTS_CANARY";
        let arguments = json!({"query":"public","_meta":{"auth":secret},"auth":secret,"env":{"TOKEN":secret},"config":{"token":secret},
            "nested":[{"query":"nested public","AUTH":secret,"config":{"token":secret},"values":[1,true,null]}],
            "encoded":json!({"query":"encoded public","_meta":{"auth":secret}}).to_string()});
        for kind in ["mcpToolCall", "dynamicToolCall"] {
            for input in [arguments.clone(), json!(arguments.to_string())] {
                let item = item_view(
                    &json!({"id":"tool","type":kind,"tool":"lookup","arguments":input}),
                    &json!("turn"),
                    0,
                );
                assert!(!item.to_string().contains(secret));
                let projected: Value =
                    serde_json::from_str(item["tool"]["input"].as_str().unwrap()).unwrap();
                assert_eq!(projected["query"], "public");
                assert_eq!(projected["nested"][0]["query"], "nested public");
                assert_eq!(projected["nested"][0]["values"], json!([1, true, null]));
                let encoded: Value =
                    serde_json::from_str(projected["encoded"].as_str().unwrap()).unwrap();
                assert_eq!(encoded, json!({"query":"encoded public"}));
            }
            let item = item_view(
                &json!({"id":"tool","type":kind,"arguments":"ordinary public query"}),
                &json!("turn"),
                0,
            );
            assert_eq!(item["tool"]["input"], "ordinary public query");
        }
    }

    #[test]
    fn view_exposes_current_composer_settings_and_preserves_rich_content() {
        let state = json!({"id":"t","latestModel":"old","latestReasoningEffort":"low",
        "latestThreadSettings":{"model":"new","effort":"high","permissions":":workspace","approvalPolicy":"on-request","approvalsReviewer":"user"},
        "turns":[{"turnId":"one","turnStartedAtMs":1000,"turnCompletedAtMs":4000,"items":[
            {"type":"userMessage","content":[{"type":"text","text":"查看附件"},{"type":"localImage","path":"C:\\private\\image.png"}]},
            {"type":"reasoning","summary":["第一步",{"text":"第二步"}]},
            {"type":"agentMessage","text":"**完成**\n\n```js\nconst x = 1;\n```"}
        ]}]});
        let result = view(&state);
        assert_eq!(result["model"], "new");
        assert_eq!(result["effort"], "high");
        assert_eq!(result["permissionMode"], "auto");
        assert_eq!(result["turns"][0]["completedAt"], 4000);
        assert_eq!(
            result["turns"][0]["messages"][0]["attachments"][0]["name"],
            "image.png"
        );
        assert_eq!(
            result["turns"][0]["messages"][0]["attachments"][0]["id"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(result["turns"][0]["messages"][1]["text"], "第一步\n第二步");
        assert!(
            result["turns"][0]["messages"][2]["text"]
                .as_str()
                .unwrap()
                .contains("```js")
        );
        assert!(!result.to_string().contains("private"));
        assert_eq!(
            permission_mode(
                &json!({"permissions":"company-policy","sandboxPolicy":{"type":"dangerFullAccess"},"approvalPolicy":"never"})
            ),
            "custom"
        );
        assert_eq!(
            permission_mode(
                &json!({"sandboxPolicy":{"type":"readOnly"},"approvalPolicy":"on-request"})
            ),
            "read-only"
        );
        assert_eq!(
            permission_mode(&json!({"permissions":":danger-full-access","approvalPolicy":"never"})),
            "full-access"
        );
        assert_eq!(
            permission_mode(
                &json!({"permissions":":workspace","approvalPolicy":"on-request","approvalsReviewer":"guardian_subagent"})
            ),
            "custom"
        );
    }

    #[test]
    fn patches_support_native_paths_and_json_pointers() {
        let mut state = json!({"items":["a"], "a/b":{"~key":1}});
        apply_patches(
            &mut state,
            &json!([
                {"op":"add","path":["items",1],"value":"b"},
                {"op":"replace","path":"/a~1b/~0key","value":2},
                {"op":"remove","path":["items",0]}
            ]),
        )
        .unwrap();
        assert_eq!(state, json!({"items":["b"], "a/b":{"~key":2}}));
        assert!(apply_patches(&mut state, &json!([{"op":"remove","path":"/items/9"}])).is_err());
        assert!(apply_patches(&mut state, &json!([{"op":"copy","path":[]}])).is_err());
    }

    #[test]
    fn images_survive_opening_fallback_and_steering_without_duplicate_messages() {
        let input = json!([{"type":"image","url":"data:image/gif;base64,R0lGODlh"}]);
        let mut state = json!({"turns":[{"params":{"input":input},"items":[]}]});
        let result = view(&state);
        assert_eq!(
            result["turns"][0]["messages"][0]["attachments"][0]["url"],
            input[0]["url"]
        );
        state["turns"][0]["items"] = json!([{"type":"userMessage","content":input},{"type":"steeringUserMessage","input":input}]);
        let result = view(&state);
        let messages = result["turns"][0]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["attachments"], messages[0]["attachments"]);
    }

    #[test]
    fn view_tracks_fast_and_cleared_service_tiers_from_desktop_patches() {
        let mut state = json!({"id":"t","latestThreadSettings":{"serviceTier":"priority"}});
        assert_eq!(view(&state)["serviceTier"], "priority");
        for tier in [json!("default"), Value::Null] {
            apply_patches(&mut state, &json!([{"op":"replace","path":["latestThreadSettings","serviceTier"],"value":tier}])).unwrap();
            assert_eq!(view(&state)["serviceTier"], tier);
        }
        assert_eq!(view(&json!({"id":"t"}))["serviceTier"], Value::Null);
    }

    #[test]
    fn canonical_history_retains_order_and_hides_verification_challenges() {
        let state = json!({"id":"t", "turnHistory":{"kind":"canonical","history":{
            "entitiesByKey":{"b":{"turnId":"two","items":[]},"a":{"turnId":"one","items":[{"type":"agentMessage","text":"完成"}]}},
            "islands":[{"entries":[{"value":"a"},{"value":"b"}]}]}},
            "requests":[{"id":1,"method":"item/tool/requestUserInput","params":{"openai/userVerification":"private-challenge"}}]});
        let view = view(&state);
        assert_eq!(view["turns"][0]["id"], "one");
        assert_eq!(view["turns"][0]["messages"][0]["text"], "完成");
        assert!(!view.to_string().contains("private-challenge"));
        assert_eq!(view["requests"][0]["supported"], false);
    }

    #[test]
    fn async_questions_disappear_only_after_accepted_answers() {
        let key = json!(["request_user_input_async", "question", 0]).to_string();
        let text = format!(
            "<send_user_message_question_reply>{}</send_user_message_question_reply>",
            json!([{"questionItemId":key,"answer":"yes"}])
        );
        let mut state = json!({"turns":[{"turnId":"turn","status":"inProgress","items":[
            {"id":"question","type":"agentMessage","questions":[{"title":"继续？","options":["是"]}]},
            {"type":"steeringUserMessage","status":"pending","input":[{"text":text}]}
        ]}]});
        assert_eq!(async_requests(&state).len(), 1);
        state["turns"][0]["items"][1]["status"] = json!("accepted");
        assert!(async_requests(&state).is_empty());
        state["turns"][0]["status"] = json!("failed");
        assert!(async_requests(&state).is_empty());
    }

    #[test]
    fn async_question_groups_fail_closed_without_leaking_or_renumbering_answered_items() {
        let unknown = "PRIVATE_ASYNC_CANARY";
        let mut state = json!({"turns":[{"turnId":"turn","status":"inProgress","items":[
            {"id":"question","type":"agentMessage","text":"公开消息","questions":[{"title":"确认？","options":["是"]},{"title":{"unknown":unknown}}]}
        ]}]});
        let request = async_requests(&state).remove(0);
        assert_eq!(request["supported"], false);
        assert!(request["params"].get("questions").is_none());
        assert!(!view(&state).to_string().contains(unknown));
        state["turns"][0]["items"][0]["questions"][1] =
            json!({"title":"第二题","options":["已知",{"private":unknown}]});
        assert_eq!(async_requests(&state)[0]["supported"], false);
        assert!(!view(&state).to_string().contains(unknown));
        let answered_key = json!(["request_user_input_async", "question", 1]).to_string();
        let reply = format!(
            "<send_user_message_question_reply>{}</send_user_message_question_reply>",
            json!([{"questionItemId":answered_key,"answer":"电脑端已处理"}])
        );
        state["turns"][0]["items"].as_array_mut().unwrap().push(
            json!({"type":"steeringUserMessage","status":"accepted","input":[{"text":reply}]}),
        );
        let request = async_requests(&state).remove(0);
        assert_eq!(request["supported"], true);
        assert_eq!(request["params"]["questions"].as_array().unwrap().len(), 1);
        assert_eq!(
            request["params"]["questions"][0]["id"],
            json!(["request_user_input_async", "question", 0]).to_string()
        );
    }
}
