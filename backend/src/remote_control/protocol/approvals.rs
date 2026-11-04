//! Public approval summaries are also the capability check used by respond.
use serde_json::{Value, json};

use super::{changes_view, ordered_items, turns};

fn copy_strings(source: &Value, target: &mut Value, keys: &[&str]) {
    for key in keys {
        if let Some(value) = source[*key].as_str() {
            target[*key] = json!(value);
        }
    }
}

fn related<'a>(state: &'a Value, params: &Value, kind: &str) -> Option<&'a Value> {
    let id = params["itemId"].as_str()?;
    turns(state)
        .into_iter()
        .flat_map(|turn| ordered_items(&turn["items"]))
        .find(|item| item["id"] == id && item["type"] == kind)
}

fn permissions(value: &Value) -> Option<Value> {
    let fields = value.as_object()?;
    if fields.is_empty()
        || fields
            .keys()
            .any(|key| !matches!(key.as_str(), "network" | "fileSystem"))
    {
        return None;
    }
    let mut result = json!({});
    if let Some(network) = fields.get("network").filter(|v| !v.is_null()) {
        if network.as_object()?.keys().any(|key| key != "enabled") {
            return None;
        }
        result["network"] = json!({"enabled":network["enabled"].as_bool()?});
    }
    if let Some(files) = fields.get("fileSystem").filter(|v| !v.is_null()) {
        let fields = files.as_object()?;
        if fields.is_empty()
            || fields
                .keys()
                .any(|key| !matches!(key.as_str(), "read" | "write"))
        {
            return None;
        }
        let mut projected = json!({});
        for (key, paths) in fields {
            if paths.is_null() {
                continue;
            }
            let paths = paths.as_array()?;
            if paths.iter().any(|path| path.as_str().is_none()) {
                return None;
            }
            projected[key] = json!(paths.iter().filter_map(Value::as_str).collect::<Vec<_>>());
        }
        if projected.as_object()?.is_empty() {
            return None;
        }
        result["fileSystem"] = projected;
    }
    (!result.as_object()?.is_empty()).then_some(result)
}

pub(super) fn params(state: &Value, request: &Value) -> Option<Value> {
    if !(request["id"].is_string() || request["id"].is_i64() || request["id"].is_u64())
        || request["params"]
            .to_string()
            .contains("openai/userVerification")
    {
        return None;
    }
    let source = &request["params"];
    source.as_object()?;
    if ["reason", "cwd", "grantRoot", "command", "planContent"]
        .iter()
        .any(|key| {
            source
                .get(*key)
                .is_some_and(|value| !value.is_null() && !value.is_string())
        })
    {
        return None;
    }
    let mut result = json!({});
    copy_strings(source, &mut result, &["reason"]);
    match request["method"].as_str()? {
        "item/commandExecution/requestApproval" => {
            let item = related(state, source, "commandExecution");
            let command = source["command"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| item?["command"].as_str().filter(|s| !s.trim().is_empty()))?;
            let cwd = source
                .get("cwd")
                .filter(|value| !value.is_null())
                .or_else(|| item?.get("cwd").filter(|value| !value.is_null()))?
                .as_str()
                .filter(|s| !s.trim().is_empty())?;
            result["command"] = json!(command);
            result["cwd"] = json!(cwd);
            if let Some(context) = source
                .get("networkApprovalContext")
                .filter(|v| !v.is_null())
            {
                let host = context["host"].as_str()?;
                let protocol = context["protocol"].as_str()?;
                result["networkApprovalContext"] = json!({"host":host,"protocol":protocol});
            }
            if let Some(decisions) = source.get("availableDecisions").filter(|v| !v.is_null()) {
                let decisions = decisions.as_array()?;
                let known: Vec<_> = decisions
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| matches!(*s, "accept" | "decline" | "cancel"))
                    .collect();
                if known.is_empty() {
                    return None;
                }
                result["availableDecisions"] = json!(known);
            }
        }
        "item/fileChange/requestApproval" => {
            let changes = source
                .get("changes")
                .filter(|v| v.as_array().is_some_and(|values| !values.is_empty()))
                .or_else(|| related(state, source, "fileChange")?.get("changes"))?;
            let projected = changes_view(changes);
            if projected.is_empty() || projected.len() != changes.as_array()?.len() {
                return None;
            }
            result["changes"] = json!(projected);
            copy_strings(source, &mut result, &["grantRoot"]);
            if let Some(decisions) = source.get("availableDecisions").filter(|v| !v.is_null()) {
                let known: Vec<_> = decisions
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| matches!(*s, "accept" | "decline" | "cancel"))
                    .collect();
                if known.is_empty() {
                    return None;
                }
                result["availableDecisions"] = json!(known);
            }
        }
        "item/permissions/requestApproval" => {
            result["permissions"] = permissions(&source["permissions"])?
        }
        "item/tool/requestUserInput" | "tool/requestUserInput" => {
            let questions = source["questions"].as_array()?;
            if questions.is_empty() || questions.len() > 100 {
                return None;
            }
            let mut projected = Vec::new();
            let mut ids = std::collections::HashSet::new();
            for question in questions {
                let id = question["id"].as_str().filter(|s| !s.trim().is_empty())?;
                if !ids.insert(id) {
                    return None;
                }
                let text = question["question"].as_str().filter(|s| !s.is_empty())?;
                if question["isSecret"] == true {
                    return None;
                }
                let mut q = json!({"id":id,"question":text});
                copy_strings(question, &mut q, &["header"]);
                if let Some(options) = question.get("options").filter(|v| !v.is_null()) {
                    let mut choices = Vec::new();
                    for option in options.as_array()? {
                        let mut choice = json!({"label":option["label"].as_str()?});
                        copy_strings(option, &mut choice, &["description"]);
                        choices.push(choice);
                    }
                    q["options"] = json!(choices);
                }
                projected.push(q);
            }
            result["questions"] = json!(projected);
        }
        "item/plan/requestImplementation" => {
            result["planContent"] = json!(
                source["planContent"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())?
            );
            copy_strings(source, &mut result, &["turnId"]);
        }
        _ => return None,
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_approvals_without_command_scoped_cwd_are_unsupported() {
        let mut state = json!({"cwd":"/thread","turns":[{"turnId":"turn","items":[
            {"id":"command","type":"commandExecution","command":"pwd"},
            {"id":"other","type":"commandExecution","command":"ls","cwd":"/unrelated"}
        ]}]});
        for source in [json!({"itemId":"command"}), json!({"command":"pwd"})] {
            let request =
                json!({"id":1,"method":"item/commandExecution/requestApproval","params":source});
            state["requests"] = json!([request]);
            assert!(params(&state, &request).is_none());
            assert!(!super::super::request_supported(&state, &request));
            assert_eq!(
                super::super::view_metadata(&state)["requests"][0]["supported"],
                false
            );
        }
    }

    #[test]
    fn command_approvals_keep_explicit_and_same_item_command_scope() {
        let mut state = json!({"cwd":"/thread","turns":[{"turnId":"turn","items":[
            {"id":"command","type":"commandExecution","command":"pwd","cwd":"/item"},
            {"id":"other","type":"commandExecution","command":"ls","cwd":"/unrelated"}
        ]}]});
        for (source, expected) in [
            (
                json!({"itemId":"command"}),
                json!({"command":"pwd","cwd":"/item"}),
            ),
            (
                json!({"itemId":"command","command":"echo explicit","cwd":"/request"}),
                json!({"command":"echo explicit","cwd":"/request"}),
            ),
        ] {
            let request =
                json!({"id":1,"method":"item/commandExecution/requestApproval","params":source});
            state["requests"] = json!([request]);
            assert_eq!(params(&state, &request), Some(expected.clone()));
            assert!(super::super::request_supported(&state, &request));
            let projected = super::super::view_metadata(&state);
            assert_eq!(projected["requests"][0]["supported"], true);
            assert_eq!(projected["requests"][0]["params"], expected);
        }
    }

    #[test]
    fn unsupported_permission_shapes_and_empty_approval_summaries_cannot_be_approved() {
        for (method, source) in [
            (
                "item/commandExecution/requestApproval",
                json!({"config":{"command":"secret"}}),
            ),
            (
                "item/fileChange/requestApproval",
                json!({"changes":[{"private":"secret"}]}),
            ),
            (
                "item/permissions/requestApproval",
                json!({"permissions":{}}),
            ),
            (
                "item/permissions/requestApproval",
                json!({"permissions":{"fileSystem":{"unknown":"secret"}}}),
            ),
            (
                "item/tool/requestUserInput",
                json!({"questions":[{"id":"q","question":{"auth":"secret"}}]}),
            ),
        ] {
            assert!(params(&json!({}), &json!({"id":1,"method":method,"params":source})).is_none());
        }
        let projected = params(&json!({}), &json!({"id":1,"method":"item/permissions/requestApproval","params":{"permissions":{"network":{"enabled":true},"fileSystem":{"write":["workspace"]}},"auth":"secret"}})).unwrap();
        assert_eq!(projected["permissions"]["network"]["enabled"], true);
        assert!(!projected.to_string().contains("secret"));
    }
}
