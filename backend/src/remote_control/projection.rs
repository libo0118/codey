//! Cache encoded turns without retaining a second copy of desktop history.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use serde::Serialize;
use serde_json::{
    Value,
    value::{RawValue, to_raw_value},
};

use super::protocol;

mod stream;
pub(super) use stream::{Encoder, MessageWindow, PublicState, StreamVersion};

#[derive(Default)]
pub(super) struct Projection {
    turns: HashMap<String, Box<RawValue>>,
    compact_turns: HashMap<String, Arc<Value>>,
}

impl Projection {
    pub fn reset(&mut self) {
        self.turns.clear();
        self.compact_turns.clear();
    }

    // Invalidate before applying each batch. Structural changes fall back to a
    // full rebuild; known turn edits preserve all other cached encodings.
    pub fn invalidate(&mut self, state: &Value, patches: &Value) -> Result<bool, String> {
        let mut urgent = false;
        for patch in patches.as_array().ok_or("桌面增量格式无效")? {
            let path = protocol::patch_path(patch)?;
            urgent |= !matches!(
                path.first().map(String::as_str),
                Some("turns" | "turnHistory")
            ) || !path
                .iter()
                .any(|part| matches!(part.as_str(), "text" | "aggregatedOutput" | "summary"));
            if path.is_empty() {
                self.reset();
            } else if matches!(path[0].as_str(), "turns" | "turnHistory") {
                let mut target = state;
                let mut id = None;
                for (index, part) in path.iter().enumerate() {
                    target = if let Some(items) = target.as_array() {
                        part.parse::<usize>()
                            .ok()
                            .and_then(|index| items.get(index))
                            .unwrap_or(&Value::Null)
                    } else {
                        &target[part]
                    };
                    if let Some(turn) = target["turnId"].as_str() {
                        // Replacing/removing a whole turn can shift later paths
                        // within this batch, so discard the cache in that case.
                        if index + 1 < path.len() {
                            id = Some(turn);
                        }
                        break;
                    }
                }
                if let Some(id) = id {
                    self.turns.remove(id);
                    self.compact_turns.remove(id);
                } else {
                    self.reset();
                }
            }
        }
        Ok(urgent)
    }

    pub fn frame(&mut self, state: &Value) -> String {
        let turns = protocol::turns(state);
        let ids: HashSet<_> = turns
            .iter()
            .filter_map(|turn| turn["turnId"].as_str())
            .collect();
        if ids.len() != turns.len() {
            // Unknown or duplicate IDs cannot be cached safely.
            self.reset();
            return serde_json::json!({"type":"state","state":protocol::view(state)}).to_string();
        }
        self.turns.retain(|id, _| ids.contains(id.as_str()));
        for turn in &turns {
            let id = turn["turnId"].as_str().unwrap();
            self.turns
                .entry(id.to_string())
                .or_insert_with(|| to_raw_value(&protocol::turn_view(turn)).expect("JSON turn"));
        }
        #[derive(Serialize)]
        struct State<'a> {
            #[serde(flatten)]
            metadata: Value,
            turns: Vec<&'a RawValue>,
        }
        #[derive(Serialize)]
        struct Frame<'a> {
            r#type: &'static str,
            state: State<'a>,
        }
        serde_json::to_string(&Frame {
            r#type: "state",
            state: State {
                metadata: protocol::view_metadata(state),
                turns: turns
                    .iter()
                    .map(|turn| self.turns[turn["turnId"].as_str().unwrap()].as_ref())
                    .collect(),
            },
        })
        .expect("JSON state")
    }

    pub fn public_state(
        &mut self,
        state: &Value,
        version: StreamVersion,
        window: &mut MessageWindow,
    ) -> PublicState {
        if version == StreamVersion::Legacy {
            return PublicState::Legacy(self.frame(state));
        }
        let turns = protocol::turns(state);
        let ids: HashSet<_> = turns
            .iter()
            .filter_map(|turn| turn["turnId"].as_str())
            .collect();
        let cacheable = ids.len() == turns.len();
        self.compact_turns
            .retain(|id, _| cacheable && ids.contains(id.as_str()));
        let turns: Vec<_> = turns
            .iter()
            .map(|&turn| {
                if cacheable {
                    self.compact_turns
                        .entry(turn["turnId"].as_str().unwrap().to_string())
                        .or_insert_with(|| Arc::new(compact_turn(turn)))
                        .clone()
                } else {
                    Arc::new(compact_turn(turn))
                }
            })
            .collect();
        PublicState::V2(window.view(protocol::view_metadata(state), &turns))
    }
}

fn compact_turn(turn: &Value) -> Value {
    let mut result = protocol::turn_view(turn);
    let turn_id = result["id"].clone();
    for message in result["messages"]
        .as_array_mut()
        .expect("projected messages")
    {
        // Structured fields already contain this same public output; unknown
        // schemas keep their safe placeholder text instead of being blanked.
        let redundant = matches!(
            message["kind"].as_str(),
            Some("commandExecution" | "mcpToolCall" | "dynamicToolCall")
        ) || (message["kind"] == "fileChange"
            && message["changes"]
                .as_array()
                .expect("projected changes")
                .iter()
                .map(|change| {
                    format!(
                        "{}\n{}",
                        change["path"].as_str().expect("projected path"),
                        change["diff"].as_str().expect("projected diff")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
                .starts_with(message["text"].as_str().expect("projected text")));
        if redundant {
            message["text"] = Value::String(String::new());
        }
        let message_id = message["id"].clone();
        for (slot, attachment) in message["attachments"]
            .as_array_mut()
            .expect("projected attachments")
            .iter_mut()
            .enumerate()
        {
            if let Some(url) = attachment["url"]
                .as_str()
                .filter(|url| url.starts_with("data:image/"))
            {
                let id = super::files::inline_image_reference_id(&turn_id, &message_id, slot, url);
                let attachment = attachment.as_object_mut().expect("projected attachment");
                attachment.remove("url");
                attachment.insert("id".into(), Value::String(id));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(projection: &mut Projection, state: &Value) {
        assert_eq!(
            serde_json::from_str::<Value>(&projection.frame(state)).unwrap()["state"],
            protocol::view(state)
        );
    }

    #[test]
    fn cached_frames_use_the_same_safe_rich_projection_after_patches() {
        let mut state = json!({"id":"t","turns":[{"turnId":"a","items":[
            {"id":"tool","type":"mcpToolCall","tool":"test","result":{"content":[{"type":"text","text":"before"}],"_meta":{"config":"CACHE_SECRET_CANARY"}}},
            {"id":"new","type":"newPrivateItem","auth":"CACHE_SECRET_CANARY"}
        ]}],"requests":[{"id":1,"method":"item/commandExecution/requestApproval","params":{"command":"safe","env":"CACHE_SECRET_CANARY"}}]});
        let mut projection = Projection::default();
        check(&mut projection, &state);
        assert!(!projection.frame(&state).contains("CACHE_SECRET_CANARY"));
        let patches = json!([{"op":"replace","path":"/turns/0/items/0/result/content/0/text","value":"after"}]);
        projection.invalidate(&state, &patches).unwrap();
        protocol::apply_patches(&mut state, &patches).unwrap();
        check(&mut projection, &state);
        let frame = projection.frame(&state);
        assert!(frame.contains("after"));
        assert!(!frame.contains("CACHE_SECRET_CANARY"));
    }

    #[test]
    fn collaboration_patches_keep_native_identity_order_and_cached_turns() {
        let mut state = json!({"turns":[{"turnId":"a","items":[
            {"id":"start","type":"subAgentActivity","kind":"started","agentThreadId":"child","agentPath":"/root/mock_review"},
            {"id":"middle","type":"agentMessage","text":"公开进度"},
            {"id":"wait","type":"collabAgentToolCall","tool":"wait","status":"inProgress","receiverThreadIds":["child"],"agentsStates":{"child":{"status":"running","message":null}},
                "receiverThreads":[{"threadId":"child","thread":{"agentNickname":"Mock","title":"CACHE_COLLAB_CANARY"}}],"prompt":"CACHE_COLLAB_CANARY"}
        ]},{"turnId":"b","items":[]}]});
        let mut projection = Projection::default();
        check(&mut projection, &state);
        for patches in [
            json!([{"op":"replace","path":"/turns/0/items/2/agentsStates/child","value":{"status":"completed","message":"公开结果","_meta":"CACHE_COLLAB_CANARY"}},{"op":"replace","path":"/turns/0/items/2/status","value":"completed"}]),
            json!([{"op":"replace","path":"/turns/0/items/2/receiverThreads/0/thread/agentNickname","value":"Mock Reviewer"}]),
            json!([{"op":"add","path":"/turns/0/items/-","value":{"id":"finish","type":"subAgentActivity","kind":"completed","agentThreadId":"child","agentPath":"/root/mock_review"}}]),
        ] {
            assert!(projection.invalidate(&state, &patches).unwrap());
            assert!(!projection.turns.contains_key("a"));
            assert!(projection.turns.contains_key("b"));
            protocol::apply_patches(&mut state, &patches).unwrap();
            check(&mut projection, &state);
        }
        let frame: Value = serde_json::from_str(&projection.frame(&state)).unwrap();
        let items = frame["state"]["turns"][0]["messages"].as_array().unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| item["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["start", "middle", "wait", "finish"]
        );
        assert_eq!(
            items[2]["collaboration"]["agents"][0]["message"],
            "公开结果"
        );
        assert_eq!(
            items[2]["collaboration"]["agents"][0]["name"],
            "Mock Reviewer"
        );
        assert!(!frame.to_string().contains("CACHE_COLLAB_CANARY"));
    }

    #[test]
    fn cached_projection_tracks_text_deletion_reordering_and_approvals() {
        let mut state = json!({"turns":[{"turnId":"a","items":[{"type":"agentMessage","text":"history"}]},{"turnId":"b","status":"inProgress","items":[{"id":"item","type":"agentMessage","text":"live"}]}],"requests":[]});
        let mut projection = Projection::default();
        check(&mut projection, &state);
        for (patches, urgent) in [
            (
                json!([{"op":"replace","path":"/turns/1/items/0/text","value":"updated"}]),
                false,
            ),
            (
                json!([{"op":"add","path":["turns",1,"items",0,"questions"],"value":[{"title":"Continue?"}]}]),
                true,
            ),
            (
                json!([{"op":"replace","path":["turns",1,"status"],"value":"completed"}]),
                true,
            ),
            (json!([{"op":"remove","path":"/turns/0"}]), true),
            (
                json!([{"op":"add","path":"/requests/0","value":{"id":1,"method":"item/commandExecution/requestApproval","params":{}}}]),
                true,
            ),
            (json!([{"op":"replace","path":"/turns","value":[]}]), true),
        ] {
            assert_eq!(projection.invalidate(&state, &patches).unwrap(), urgent);
            protocol::apply_patches(&mut state, &patches).unwrap();
            check(&mut projection, &state);
        }
        assert!(projection.turns.is_empty());
    }

    #[test]
    fn unchanged_history_encoding_is_reused_and_snapshots_reset_it() {
        let mut state = json!({"turnHistory":{"kind":"canonical","history":{"entitiesByKey":{"a":{"turnId":"a","items":[]},"b":{"turnId":"b","items":[]}},"islands":[{"entries":[{"value":"a"},{"value":"b"}]}]}}});
        let mut projection = Projection::default();
        check(&mut projection, &state);
        let patches = json!([{"op":"add","path":"/turnHistory/history/entitiesByKey/b/status","value":"completed"}]);
        projection.invalidate(&state, &patches).unwrap();
        assert!(projection.turns.contains_key("a"));
        assert!(!projection.turns.contains_key("b"));
        protocol::apply_patches(&mut state, &patches).unwrap();
        check(&mut projection, &state);
        let patches = json!([{"op":"replace","path":"/turnHistory/history/islands/0/entries","value":[{"value":"b"},{"value":"a"}]}]);
        projection.invalidate(&state, &patches).unwrap();
        protocol::apply_patches(&mut state, &patches).unwrap();
        check(&mut projection, &state);
        projection.reset();
        check(&mut projection, &json!({"turns":[{"items":[]}]}));
    }

    #[test]
    fn structural_edits_in_one_batch_cannot_reuse_a_shifted_turn() {
        let mut state = json!({"turns":[{"turnId":"a","items":[]},{"turnId":"b","items":[{"type":"agentMessage","text":"old"}]}]});
        let mut projection = Projection::default();
        check(&mut projection, &state);
        let patches = json!([{"op":"remove","path":"/turns/0"},{"op":"replace","path":"/turns/0/items/0/text","value":"new"}]);
        projection.invalidate(&state, &patches).unwrap();
        protocol::apply_patches(&mut state, &patches).unwrap();
        check(&mut projection, &state);
        let patches = json!([{"op":"add","path":"/requests","value":[{"id":1,"method":"item/tool/requestUserInput","params":{"text":"approval"}}]}]);
        assert!(projection.invalidate(&state, &patches).unwrap());
    }
}
