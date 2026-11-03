//! Cache encoded turns without retaining a second copy of desktop history.
use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::{
    Value,
    value::{RawValue, to_raw_value},
};

use super::protocol;

#[derive(Default)]
pub(super) struct Projection {
    turns: HashMap<String, Box<RawValue>>,
}

impl Projection {
    pub fn reset(&mut self) {
        self.turns.clear();
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
