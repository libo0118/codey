//! Fixed desktop-owner Goal actions; durable receipts never contain goal text.
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::AppState;

const STORE_ERROR: &str = "目标操作回执不可用，未重放操作，请在电脑端检查";
const MAX_RECEIPTS: usize = 4096;

pub(super) fn is_action(action: &str) -> bool {
    matches!(
        action,
        "goalRead" | "goalCreate" | "goalPause" | "goalResume" | "goalEdit"
    )
}

fn status(value: &str) -> Option<&str> {
    match value {
        "usage_limited" => Some("usageLimited"),
        "budget_limited" => Some("budgetLimited"),
        "active" | "paused" | "blocked" | "usageLimited" | "budgetLimited" | "complete" => {
            Some(value)
        }
        _ => None,
    }
}

pub(super) fn view(value: &Value) -> Result<Value, String> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let objective = value["objective"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 20_000)
        .ok_or("目标内容无效")?;
    let state = value["status"]
        .as_str()
        .and_then(status)
        .ok_or("目标状态无效")?;
    let mut result = json!({"objective":objective,"status":state});
    if let Some(id) = value.get("goalId").filter(|v| !v.is_null()) {
        let id = id
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 512)
            .ok_or("目标身份无效")?;
        result["goalId"] = json!(id);
    }
    for key in [
        "createdAt",
        "updatedAt",
        "tokenBudget",
        "tokensUsed",
        "timeUsedSeconds",
    ] {
        let number = match value.get(key).filter(|v| !v.is_null()) {
            Some(value) => Some(value.as_u64().ok_or("目标用量格式无效")?),
            None if key.ends_with("At") => value
                .get(format!("{key}Ms"))
                .filter(|v| !v.is_null())
                .map(|v| {
                    v.as_u64()
                        .filter(|n| *n <= 9_007_199_254_740_991)
                        .map(|n| n / 1000)
                        .ok_or("目标时间格式无效")
                })
                .transpose()?,
            None => None,
        };
        if let Some(number) = number {
            if number > 9_007_199_254_740_991 {
                return Err("目标数值过大".into());
            }
            result[key] = json!(number);
        }
    }
    Ok(result)
}

pub(super) fn metadata(state: &Value) -> (Value, &'static str) {
    let Some(goal) = state.get("threadGoal") else {
        return (Value::Null, "unavailable");
    };
    let goal = if goal.is_null() && state["completedThreadGoal"]["status"] == "complete" {
        &state["completedThreadGoal"]
    } else {
        goal
    };
    match view(goal) {
        Ok(goal) => (goal, "ready"),
        Err(_) => (Value::Null, "unavailable"),
    }
}

pub(super) fn validate(args: &Value) -> Result<(), String> {
    let fields = args.as_object().ok_or("目标请求无效")?;
    let action = args["action"]
        .as_str()
        .filter(|a| is_action(a))
        .ok_or("目标操作无效")?;
    super::desktop::validate_thread(args["threadId"].as_str().ok_or("缺少会话标识")?)?;
    super::desktop::validate_thread(args["requestId"].as_str().ok_or("缺少请求标识")?)?;
    let edits = matches!(action, "goalCreate" | "goalEdit");
    if fields.keys().any(|key| {
        !matches!(key.as_str(), "action" | "threadId" | "requestId")
            && !(action != "goalRead" && key == "expected")
            && !(edits && matches!(key.as_str(), "objective" | "tokenBudget"))
    }) {
        return Err("目标请求包含不支持的字段".into());
    }
    if action != "goalRead" {
        let expected = args.get("expected").ok_or("缺少预期目标身份")?;
        if expected.is_null() {
            if action != "goalCreate" {
                return Err("当前没有可操作的目标".into());
            }
        } else {
            let fields = expected.as_object().ok_or("目标身份无效")?;
            if fields.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "objective" | "status" | "goalId" | "createdAt" | "createdAtMs"
                )
            }) {
                return Err("目标身份字段无效".into());
            }
            let expected = view(expected)?;
            if expected.get("goalId").is_none() && expected.get("createdAt").is_none() {
                return Err("目标缺少稳定身份".into());
            }
        }
    }
    if edits {
        args["objective"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 20_000)
            .ok_or("目标内容为空或过长")?;
        if let Some(budget) = args.get("tokenBudget") {
            if !budget.is_null()
                && !budget
                    .as_u64()
                    .is_some_and(|n| n > 0 && n <= 9_007_199_254_740_991)
            {
                return Err("目标预算须为正整数或 null".into());
            }
        }
    }
    Ok(())
}

pub(super) fn unknown() -> Value {
    json!({"status":"failed","code":"goal_outcome_unknown","message":"目标操作结果未确认，请刷新查看，勿重复提交","availability":"unavailable","activation":"unknown"})
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    digest: [u8; 32],
    result: Option<Value>,
}

fn journal(
    path: &Path,
    update: impl FnOnce(&mut BTreeMap<String, Receipt>) -> Result<Option<Value>, String>,
) -> Result<Option<Value>, String> {
    let parent = path.parent().ok_or(STORE_ERROR)?;
    std::fs::create_dir_all(parent).map_err(|_| STORE_ERROR)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))
        .map_err(|_| STORE_ERROR)?;
    fs2::FileExt::lock_exclusive(&lock).map_err(|_| STORE_ERROR)?;
    let mut receipts: BTreeMap<String, Receipt> = match std::fs::read(path) {
        Ok(bytes) if bytes.len() <= 4 * 1024 * 1024 => {
            serde_json::from_slice(&bytes).map_err(|_| STORE_ERROR)?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        _ => return Err(STORE_ERROR.into()),
    };
    let result = update(&mut receipts)?;
    let bytes = serde_json::to_vec(&receipts).map_err(|_| STORE_ERROR)?;
    crate::fs_util::atomic_write_private_with_parent(path, &bytes).map_err(|_| STORE_ERROR)?;
    Ok(result)
}

fn reserve(path: &Path, id: &str, digest: [u8; 32]) -> Result<Option<Value>, String> {
    journal(path, |receipts| {
        if let Some(receipt) = receipts.get(id) {
            if receipt.digest != digest {
                return Err("同一操作标识不能用于不同内容".into());
            }
            let mut result = receipt.result.clone().unwrap_or_else(unknown);
            result["replayed"] = json!(true);
            return Ok(Some(result));
        }
        if receipts.len() >= MAX_RECEIPTS {
            return Err("目标操作回执已满，请在电脑端检查".into());
        }
        receipts.insert(
            id.to_string(),
            Receipt {
                digest,
                result: None,
            },
        );
        Ok(None)
    })
}

fn finish(path: &Path, id: &str, digest: [u8; 32], value: Value) -> Result<Option<Value>, String> {
    journal(path, |receipts| {
        let receipt = receipts
            .get_mut(id)
            .filter(|r| r.digest == digest)
            .ok_or(STORE_ERROR)?;
        let mut result = json!({});
        for key in ["status", "code", "message", "availability", "activation"] {
            if let Some(value) = value.get(key) {
                result[key] = value.clone();
            }
        }
        receipt.result = Some(result);
        Ok(None)
    })
}

pub(super) async fn once(
    path: PathBuf,
    id: String,
    digest: [u8; 32],
    operation: impl std::future::Future<Output = Result<Value, String>>,
) -> Result<Value, String> {
    let (saved_path, saved_id) = (path.clone(), id.clone());
    if let Some(result) =
        tokio::task::spawn_blocking(move || reserve(&saved_path, &saved_id, digest))
            .await
            .map_err(|_| STORE_ERROR)??
    {
        return Ok(result);
    }
    let result = operation.await.unwrap_or_else(|message| json!({"status":"failed","message":message,"availability":"unavailable","activation":"not-requested"}));
    let saved = result.clone();
    if !matches!(
        tokio::task::spawn_blocking(move || finish(&path, &id, digest, saved)).await,
        Ok(Ok(_))
    ) {
        return Ok(unknown());
    }
    Ok(result)
}

fn require_idle(state: &Value) -> Result<(), String> {
    let history = if state["turnHistory"]["kind"] == "canonical" {
        &state["turnHistory"]["history"]
    } else {
        &state["turns"]
    };
    if !history.is_array()
        && !history["items"].is_array()
        && !(history["entitiesByKey"].is_object() && history["islands"].is_array())
    {
        return Err("尚未确认当前回复状态，请重新连接".into());
    }
    if super::protocol::turns(state)
        .iter()
        .any(|turn| turn["status"] == "inProgress")
    {
        return Err("当前回复仍在执行，请等待完成后新建或继续目标".into());
    }
    Ok(())
}

fn same_identity(left: &Value, right: &Value) -> bool {
    !left.is_null()
        && left["objective"] == right["objective"]
        && left["status"] == right["status"]
        && if left.get("goalId").is_some() {
            left["goalId"] == right["goalId"]
        } else {
            left.get("createdAt").is_some() && left["createdAt"] == right["createdAt"]
        }
}

pub(super) async fn perform(
    state: &Arc<AppState>,
    args: &Value,
    authorize: impl Fn() -> Result<(), String>,
) -> Result<Value, String> {
    validate(args)?;
    let thread = args["threadId"].as_str().ok_or("缺少会话标识")?;
    let action = args["action"].as_str().ok_or("目标操作无效")?;
    let activating = matches!(action, "goalCreate" | "goalResume");
    let desktop = super::desktop::Desktop::follow(thread).await?;
    if activating {
        require_idle(&desktop.state)?;
        let runtime = state
            .runtime
            .lock()
            .await
            .clone()
            .ok_or("Codex 桌面尚未启动")?;
        super::server::require_model_request_gate(&runtime.renderer_websocket_url().await).await?;
    }
    authorize()?;
    let mut result = match super::create::desktop_request(state, args.clone()).await {
        Ok(result) => result,
        Err(error) if action == "goalRead" => return Err(error),
        Err(_) => return Ok(unknown()),
    };
    if !activating || result["status"] != "ok" {
        return Ok(result);
    }
    // A confirmed active goal is not a confirmed wakeup. Never replay either write.
    let activation = async {
        authorize()?;
        let mut desktop = super::desktop::Desktop::follow(thread).await?;
        require_idle(&desktop.state)?;
        let current = super::create::desktop_request(state, json!({"action":"goalRead","threadId":thread})).await?;
        if current["status"] != "ok" || !same_identity(&current["goal"], &result["goal"]) { return Err("目标已变化".into()); }
        desktop.action("send", &json!({"requestId":args["requestId"],"text":if action == "goalCreate" { "目标已创建。请立即开始执行当前目标，不要重复创建目标。" } else { "目标已恢复。请继续执行当前目标。" }}), async {
            let runtime = state.runtime.lock().await.clone().ok_or("Codex 桌面尚未启动")?;
            super::server::require_model_request_gate(&runtime.renderer_websocket_url().await).await?;
            authorize()
        }, async { Err("目标激活不支持附件".into()) }).await?;
        Ok::<_, String>(())
    }.await;
    if activation.is_err() {
        let mut value = unknown();
        value["goal"] = result["goal"].clone();
        return Ok(value);
    }
    result["activation"] = json!("accepted");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_is_allowlisted_and_never_treats_missing_or_invalid_as_none() {
        let goal = json!({"objective":"safe","status":"usage_limited","createdAtMs":12345,"tokensUsed":7,"config":"SECRET"});
        assert_eq!(
            view(&goal).unwrap(),
            json!({"objective":"safe","status":"usageLimited","createdAt":12,"tokensUsed":7})
        );
        assert_eq!(metadata(&json!({})).1, "unavailable");
        assert_eq!(
            metadata(&json!({"threadGoal":null})),
            (Value::Null, "ready")
        );
        assert_eq!(
            metadata(&json!({"threadGoal":{"objective":"safe","status":"future"}})).1,
            "unavailable"
        );
        assert_eq!(metadata(&json!({"threadGoal":null,"completedThreadGoal":{"objective":"done","status":"complete"}})).0["status"],"complete");
    }

    #[test]
    fn boundary_requires_stable_identity_known_fields_and_valid_budget() {
        let args = json!({"action":"goalCreate","threadId":uuid::Uuid::new_v4().to_string(),"requestId":uuid::Uuid::new_v4().to_string(),"expected":null,"objective":"task"});
        validate(&args).unwrap();
        for patch in [
            json!({"expected":{}}),
            json!({"tokenBudget":0}),
            json!({"tokenBudget":1.5}),
            json!({"rawRpc":{}}),
            json!({"objective":" "}),
        ] {
            let mut bad = args.clone();
            for (key, value) in patch.as_object().unwrap() {
                bad[key] = value.clone();
            }
            assert!(validate(&bad).is_err());
        }
        for budget in [Value::Null, json!(1)] {
            let mut good = args.clone();
            good["tokenBudget"] = budget;
            validate(&good).unwrap();
        }
        assert!(require_idle(&json!({"turns":[{"status":"inProgress"}]})).is_err());
        assert!(require_idle(&json!({})).is_err());
        assert!(require_idle(&json!({"turns":[]})).is_ok());
        assert!(require_idle(&json!({"turnHistory":{"kind":"canonical","history":{"entitiesByKey":{"a":{"status":"inProgress"}},"islands":[{"entries":[{"value":"a"}]}]}}})).is_err());
    }

    #[tokio::test]
    async fn receipts_survive_restart_pending_unknown_and_changed_payload_without_goal_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goals.json");
        let value=once(path.clone(),"a".into(),[1;32],async { Ok(json!({"status":"ok","availability":"ready","goal":{"objective":"SECRET"},"activation":"accepted"})) }).await.unwrap();
        assert_eq!(value["activation"], "accepted");
        assert!(!std::fs::read_to_string(&path).unwrap().contains("SECRET"));
        let duplicate = once(path.clone(), "a".into(), [1; 32], async {
            panic!("replayed")
        })
        .await
        .unwrap();
        assert_eq!(duplicate["activation"], "accepted");
        assert_eq!(duplicate["replayed"], true);
        assert!(
            once(path.clone(), "a".into(), [2; 32], async {
                panic!("changed payload")
            })
            .await
            .is_err()
        );
        reserve(&path, "pending", [3; 32]).unwrap();
        assert_eq!(
            once(path.clone(), "pending".into(), [3; 32], async {
                panic!("unknown replay")
            })
            .await
            .unwrap()["activation"],
            "unknown"
        );
        std::fs::write(&path, "corrupt").unwrap();
        assert!(
            once(path, "b".into(), [4; 32], async {
                panic!("corrupt journal")
            })
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn cancelled_operations_leave_a_durable_unknown_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goals.json");
        let saved = path.clone();
        let (started, wait) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            once(saved, "cancelled".into(), [1; 32], async {
                started.send(()).unwrap();
                std::future::pending().await
            })
            .await
        });
        wait.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let result = once(path, "cancelled".into(), [1; 32], async {
            panic!("cancelled write replayed")
        })
        .await
        .unwrap();
        assert_eq!(result["activation"], "unknown");
    }
}
