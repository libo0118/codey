use std::{collections::HashMap, sync::Arc};

use serde_json::{Value, json};

const INITIAL_BODIES: usize = 20;
const HISTORY_BODIES: usize = 50;
const MESSAGE_BUDGET: usize = 256 * 1024;
const INITIAL_TOTAL_BUDGET: usize = 8 * 1024 * 1024;
const MAX_PATCHES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamVersion {
    Legacy,
    V2,
}

impl StreamVersion {
    pub fn from_query(query: Option<&str>) -> Result<Self, &'static str> {
        match query {
            None | Some("") => Ok(Self::Legacy),
            Some("stream=2") => Ok(Self::V2),
            _ => Err("实时连接版本无效"),
        }
    }
}

#[derive(Default)]
pub(crate) struct MessageWindow {
    // Retain recent/live context separately from conversation-only backfill.
    loaded: HashMap<(String, String), bool>,
    expand: bool,
    history_revision: u64,
}

fn is_dialogue(message: &Value) -> bool {
    message["role"] != "activity"
        && (!message["text"]
            .as_str()
            .expect("projected text")
            .trim()
            .is_empty()
            || !message["attachments"]
                .as_array()
                .expect("projected attachments")
                .is_empty())
}

impl MessageWindow {
    pub fn expand(&mut self) {
        self.expand = true;
    }

    pub fn view(&mut self, mut metadata: Value, turns: &[Arc<Value>]) -> Value {
        let mut messages = Vec::new();
        let mut turn_occurrences = HashMap::<String, usize>::new();
        for (turn_index, turn) in turns.iter().enumerate() {
            let occurrence = turn_occurrences.entry(turn["id"].to_string()).or_default();
            let turn_key = json!([turn["id"], *occurrence]).to_string();
            *occurrence += 1;
            let mut item_occurrences = HashMap::<String, usize>::new();
            for (message_index, message) in turn["messages"]
                .as_array()
                .expect("projected messages")
                .iter()
                .enumerate()
            {
                let occurrence = item_occurrences
                    .entry(message["id"].to_string())
                    .or_default();
                let key = (
                    turn_key.clone(),
                    json!([message["id"], *occurrence]).to_string(),
                );
                *occurrence += 1;
                messages.push((turn_index, message_index, message, key));
            }
        }
        // Keep every surviving loaded item, even when its former first item was
        // deleted or reordered. Appending messages never slides the window away.
        let anchor = messages
            .iter()
            .position(|(_, _, _, key)| self.loaded.contains_key(key));
        let initial = anchor.is_none() && !self.expand;
        let previous_recent = messages
            .iter()
            .position(|(_, _, _, key)| self.loaded.get(key) == Some(&true))
            .or_else(|| {
                messages
                    .iter()
                    .rposition(|(_, _, _, key)| self.loaded.contains_key(key))
                    .map(|index| index + 1)
            })
            .or_else(|| {
                messages
                    .iter()
                    .position(|(index, _, _, _)| turns[*index]["status"] == "inProgress")
            })
            .unwrap_or(messages.len());
        let mut start = anchor.unwrap_or(if initial {
            messages.len()
        } else {
            previous_recent
        });
        let mut count = if initial {
            INITIAL_BODIES
        } else if self.expand {
            HISTORY_BODIES
        } else {
            0
        };
        let mut bytes = 0;
        let mut total_bytes = 0;
        while start > 0 && count > 0 {
            let message = messages[start - 1].2;
            // Folded activities and empty messages do not consume the body quota.
            let counted = is_dialogue(message);
            let size = if initial || counted {
                message.to_string().len()
            } else {
                0
            };
            // A single large public message is visible intact, not silently lost.
            if (initial && total_bytes > 0 && total_bytes + size > INITIAL_TOTAL_BUDGET)
                || (counted && bytes > 0 && bytes + size > MESSAGE_BUDGET)
            {
                break;
            }
            start -= 1;
            total_bytes += size;
            if counted {
                bytes += size;
                count -= 1;
            }
        }
        let recent_start = if initial { start } else { previous_recent };
        // A requested Native backfill may arrive after the history command.
        let waiting_for_native =
            self.expand && count == HISTORY_BODIES && metadata["historyComplete"] != true;
        if self.expand && !waiting_for_native {
            self.history_revision += 1;
        }
        self.expand = waiting_for_native;
        self.loaded = messages
            .iter()
            .enumerate()
            .skip(start)
            .map(|(index, (_, _, _, key))| (key.clone(), index >= recent_start))
            .collect();
        let mut visible = vec![Vec::new(); turns.len()];
        for (position, (turn, _, message, _)) in messages.iter().enumerate().skip(start) {
            if position >= recent_start || is_dialogue(message) {
                visible[*turn].push(*message);
            }
        }
        metadata["turns"] = turns
            .iter()
            .enumerate()
            .filter_map(|(index, turn)| {
                let messages = &visible[index];
                // The composer still sees every active turn and the current
                // empty turn; old invisible completed-turn metadata is not history.
                if messages.is_empty() && turn["status"] != "inProgress" && index + 1 != turns.len()
                {
                    return None;
                }
                Some(Value::Object(
                    turn.as_object()
                        .expect("projected turn")
                        .iter()
                        .map(|(key, value)| {
                            (
                                key.clone(),
                                if key == "messages" {
                                    json!(messages)
                                } else {
                                    value.clone()
                                },
                            )
                        })
                        .collect(),
                ))
            })
            .collect();
        metadata["hasEarlierMessages"] = json!(
            messages[..start]
                .iter()
                .any(|(_, _, message, _)| is_dialogue(message))
        );
        metadata["historyRevision"] = json!(self.history_revision);
        metadata
    }
}

pub(crate) struct PreparedFrame {
    pub text: String,
    state: Arc<PublicState>,
    revision: u64,
}

#[derive(PartialEq)]
pub(crate) enum PublicState {
    Legacy(String),
    V2(Value),
}

pub(crate) struct Encoder {
    version: StreamVersion,
    delivered: Option<Arc<PublicState>>,
    revision: u64,
}

impl Encoder {
    pub fn new(version: StreamVersion) -> Self {
        Self {
            version,
            delivered: None,
            revision: 0,
        }
    }

    pub fn prepare(&self, state: Arc<PublicState>) -> Result<Option<PreparedFrame>, String> {
        if self.delivered.as_deref() == Some(state.as_ref()) {
            return Ok(None);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or("实时连接版本已耗尽，请重新连接")?;
        let text = match (self.version, state.as_ref(), self.delivered.as_deref()) {
            (StreamVersion::Legacy, PublicState::Legacy(text), _) => text.clone(),
            (StreamVersion::V2, PublicState::V2(state), None) => {
                json!({"type":"state","streamVersion":2,"revision":revision,"state":state})
                    .to_string()
            }
            (StreamVersion::V2, PublicState::V2(state), Some(PublicState::V2(before))) => {
                let mut patches = Vec::new();
                diff(before, state, "", &mut patches);
                if patches.len() >= MAX_PATCHES {
                    patches = vec![json!({"op":"replace","path":"","value":state})];
                }
                json!({"type":"delta","baseRevision":self.revision,"revision":revision,"patches":patches}).to_string()
            }
            _ => return Err("实时公开快照格式不匹配".into()),
        };
        Ok(Some(PreparedFrame {
            text,
            state,
            revision,
        }))
    }

    // Only the writer that has successfully flushed the WebSocket may advance
    // this baseline; preparing or replacing a pending state cannot advance it.
    pub fn delivered(&mut self, frame: PreparedFrame) {
        self.delivered = Some(frame.state);
        self.revision = frame.revision;
    }
}

fn diff(before: &Value, after: &Value, path: &str, patches: &mut Vec<Value>) {
    if patches.len() >= MAX_PATCHES || before == after {
        return;
    }
    match (before, after) {
        (Value::Object(before), Value::Object(after)) => {
            for key in before.keys().filter(|key| !after.contains_key(*key)) {
                if patches.len() >= MAX_PATCHES {
                    return;
                }
                patches.push(json!({"op":"remove","path":pointer(path,key)}));
            }
            for (key, value) in after {
                if patches.len() >= MAX_PATCHES {
                    return;
                }
                let path = pointer(path, key);
                match before.get(key) {
                    Some(before) => diff(before, value, &path, patches),
                    None => patches.push(json!({"op":"add","path":path,"value":value})),
                }
            }
        }
        (Value::Array(before), Value::Array(after)) => {
            for (index, (before, after)) in before.iter().zip(after).enumerate() {
                if patches.len() >= MAX_PATCHES {
                    return;
                }
                diff(before, after, &format!("{path}/{index}"), patches);
            }
            for index in (after.len()..before.len()).rev() {
                if patches.len() >= MAX_PATCHES {
                    return;
                }
                patches.push(json!({"op":"remove","path":format!("{path}/{index}")}));
            }
            for (index, value) in after.iter().enumerate().skip(before.len()) {
                if patches.len() >= MAX_PATCHES {
                    return;
                }
                patches.push(json!({"op":"add","path":format!("{path}/{index}"),"value":value}));
            }
        }
        _ => patches.push(json!({"op":"replace","path":path,"value":after})),
    }
}

fn pointer(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

#[cfg(test)]
mod tests;
