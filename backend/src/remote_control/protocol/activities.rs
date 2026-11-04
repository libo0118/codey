use serde_json::{Value, json};

pub(super) fn collaboration(item: &Value) -> Option<Value> {
    match item["type"].as_str()? {
        "collabAgentToolCall" => {
            let action = item["tool"].as_str()?;
            if !matches!(
                action,
                "spawnAgent"
                    | "sendInput"
                    | "resumeAgent"
                    | "wait"
                    | "closeAgent"
                    | "sendMessage"
                    | "followupTask"
                    | "interruptAgent"
                    | "listAgents"
            ) {
                return None;
            }
            let mut ids: Vec<_> = item["receiverThreadIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if let Some(states) = item["agentsStates"].as_object() {
                for id in states.keys() {
                    if !ids.contains(&id.as_str()) {
                        ids.push(id);
                    }
                }
            }
            let agents: Vec<_> = ids
                .into_iter()
                .map(|id| {
                    let mut agent = json!({"id":id});
                    // Hydrated thread titles/previews can be derived from hidden prompts.
                    if let Some(name) = item["receiverThreads"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|receiver| receiver["threadId"].as_str() == Some(id))
                        .and_then(|receiver| receiver["thread"]["agentNickname"].as_str())
                        .filter(|name| !name.trim().is_empty())
                    {
                        agent["name"] = json!(name);
                    }
                    let state = &item["agentsStates"][id];
                    if let Some(status) = state["status"].as_str().filter(|status| {
                        matches!(
                            *status,
                            "pendingInit"
                                | "running"
                                | "interrupted"
                                | "completed"
                                | "errored"
                                | "shutdown"
                                | "notFound"
                        )
                    }) {
                        agent["status"] = json!(status);
                        if let Some(message) = state["message"].as_str() {
                            agent["message"] = json!(message);
                        }
                    }
                    agent
                })
                .collect();
            Some(json!({"action":action,"agents":agents}))
        }
        "subAgentActivity" => {
            let action = item["kind"].as_str()?;
            let status = match action {
                "started" => Some("running"),
                "completed" => Some("completed"),
                "interrupted" => Some("interrupted"),
                "interacted" => None,
                _ => return None,
            };
            let mut agent = json!({"id":item["agentThreadId"].as_str()?});
            if let Some(name) = item["agentPath"]
                .as_str()
                .and_then(|path| path.rsplit('/').next())
                .filter(|name| !name.trim().is_empty())
            {
                agent["name"] = json!(name);
            }
            if let Some(status) = status {
                agent["status"] = json!(status);
            }
            Some(json!({"action":action,"agents":[agent]}))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
