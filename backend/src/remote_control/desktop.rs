//! Length-prefixed Codex desktop IPC; execution always stays with the desktop owner.
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

use super::protocol;

const MAX_FRAME: usize = 16 * 1024 * 1024;
// Native snapshots include full history and inline images, unlike the bounded public stream.
const MAX_RECEIVE_FRAME: usize = 64 * 1024 * 1024;
pub(super) const UNCERTAIN: &str = "桌面响应中断或超时，操作可能已提交；请先查看会话，不要重复发送";
pub(super) const OWNER_UNAVAILABLE: &str = "尚未取得桌面会话，请点击重新连接以在电脑中打开会话";

trait Transport: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Transport for T {}

#[derive(Clone)]
pub(super) struct NativeProvenance {
    pub owner: String,
    pub revision: Option<u64>,
}

pub(super) struct Desktop {
    stream: Box<dyn Transport>,
    buffer: Vec<u8>,
    outgoing: Vec<u8>,
    written: usize,
    client: Option<String>,
    owner: Option<String>,
    pub thread: String,
    pub state: Value,
    revision: Option<Value>,
    projection: super::projection::Projection,
    pub urgent: bool,
}

impl Desktop {
    async fn connect() -> Result<Self, String> {
        #[cfg(windows)]
        let stream = {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            loop {
                match tokio::net::windows::named_pipe::ClientOptions::new()
                    .open(r"\\.\pipe\codex-ipc")
                {
                    Ok(stream) => break stream,
                    Err(error)
                        if error.raw_os_error() == Some(231)
                            && tokio::time::Instant::now() < deadline =>
                    {
                        tokio::time::sleep(Duration::from_millis(50)).await
                    }
                    Err(_) => {
                        return Err("无法连接 Codex 桌面，请确认 Codey 和 Codex 正在运行".into());
                    }
                }
            }
        };
        #[cfg(unix)]
        let stream =
            tokio::net::UnixStream::connect(crate::codex_config::codex_home().join("ipc/ipc.sock"))
                .await
                .map_err(|_| "无法连接 Codex 桌面，请确认 Codey 和 Codex 正在运行")?;
        Self::initialize(Box::new(stream)).await
    }

    async fn initialize(stream: Box<dyn Transport>) -> Result<Self, String> {
        let mut desktop = Self {
            stream,
            buffer: Vec::new(),
            outgoing: Vec::new(),
            written: 0,
            client: None,
            owner: None,
            thread: String::new(),
            state: Value::Null,
            revision: None,
            projection: Default::default(),
            urgent: true,
        };
        let result = desktop
            .request(
                "initialize",
                0,
                json!({"clientType":"codey-remote-control"}),
                false,
            )
            .await?;
        desktop.client = Some(
            result["result"]["clientId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("Codex 桌面初始化响应不兼容")?
                .to_string(),
        );
        Ok(desktop)
    }

    pub async fn follow(thread: &str) -> Result<Self, String> {
        validate_thread(thread)?;
        Self::subscribe(Self::connect().await?, thread).await
    }

    async fn subscribe(mut desktop: Self, thread: &str) -> Result<Self, String> {
        desktop.thread = thread.to_string();
        desktop
            .send(&json!({"type":"broadcast", "sourceClientId":desktop.client,
            "method":"thread-stream-following-changed", "version":1,
            "params":{"hostId":"local", "conversationId":thread, "following":true}}))
            .await?;
        tokio::time::timeout(Duration::from_secs(8), async {
            while !desktop.update().await? {}
            Ok::<_, String>(())
        })
        .await
        .map_err(|_| OWNER_UNAVAILABLE)??;
        Ok(desktop)
    }

    async fn send(&mut self, message: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(message).map_err(|_| "桌面消息编码失败")?;
        if body.len() > MAX_FRAME {
            return Err("桌面消息过大".into());
        }
        self.outgoing
            .extend_from_slice(&(body.len() as u32).to_le_bytes());
        self.outgoing.extend_from_slice(&body);
        tokio::time::timeout(Duration::from_secs(5), self.flush_outgoing())
            .await
            .map_err(|_| UNCERTAIN)?
    }

    // Persist the write cursor too: a heartbeat may cancel discovery replies.
    async fn flush_outgoing(&mut self) -> Result<(), String> {
        while self.written < self.outgoing.len() {
            let count = self
                .stream
                .write(&self.outgoing[self.written..])
                .await
                .map_err(|_| UNCERTAIN)?;
            if count == 0 {
                return Err(UNCERTAIN.into());
            }
            self.written += count;
        }
        self.outgoing.clear();
        self.written = 0;
        self.stream.flush().await.map_err(|_| UNCERTAIN.into())
    }

    // The buffer belongs to the connection so cancellation for stream heartbeats
    // never loses a partially received frame or its length prefix.
    async fn next(&mut self) -> Result<Value, String> {
        self.flush_outgoing().await?;
        loop {
            if self.buffer.len() >= 4 {
                let length = u32::from_le_bytes(self.buffer[..4].try_into().unwrap()) as usize;
                if length == 0 || length > MAX_RECEIVE_FRAME {
                    return Err("Codex 桌面 IPC 帧长度无效或超过 64 MiB".into());
                }
                if self.buffer.len() >= length + 4 {
                    let value: Value = serde_json::from_slice(&self.buffer[4..length + 4])
                        .map_err(|_| "Codex 桌面 IPC 数据无效")?;
                    self.buffer.drain(..length + 4);
                    if value["type"] == "client-discovery-request" {
                        self.send(&json!({"type":"client-discovery-response", "requestId":value["requestId"], "response":{"canHandle":false}})).await?;
                        continue;
                    }
                    return Ok(value);
                }
            }
            let mut chunk = [0u8; 8192];
            let length = self
                .stream
                .read(&mut chunk)
                .await
                .map_err(|_| "Codex 桌面连接已断开")?;
            if length == 0 {
                return Err("Codex 桌面连接已断开".into());
            }
            self.buffer.extend_from_slice(&chunk[..length]);
        }
    }

    async fn request(
        &mut self,
        method: &str,
        version: u8,
        params: Value,
        mutation: bool,
    ) -> Result<Value, String> {
        let id = Uuid::new_v4().to_string();
        let mut message = json!({"type":"request", "requestId":id, "sourceClientId":self.client,
            "method":method, "version":version, "params":params, "timeoutMs":30_000});
        if let Some(owner) = &self.owner {
            message["targetClientId"] = json!(owner);
        }
        self.send(&message).await?;
        let result = tokio::time::timeout(Duration::from_secs(32), async {
            loop {
                let response = self.next().await?;
                if response["type"] == "response" && response["requestId"] == id {
                    if response["resultType"] != "success" {
                        return Err("Codex 桌面未接受操作，请刷新会话检查状态或在电脑端处理".into());
                    }
                    return Ok(response);
                }
                self.apply_event(response)?;
            }
        })
        .await;
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) if !mutation => Err(error),
            Err(_) if !mutation => Err("Codex 桌面读取超时".into()),
            _ => Err(UNCERTAIN.into()),
        }
    }

    pub async fn update(&mut self) -> Result<bool, String> {
        let event = self.next().await?;
        self.apply_event(event)
    }

    pub fn public_state(
        &mut self,
        version: super::projection::StreamVersion,
        window: &mut super::projection::MessageWindow,
    ) -> super::projection::PublicState {
        self.projection.public_state(&self.state, version, window)
    }

    pub fn provenance(&self) -> Result<NativeProvenance, String> {
        Ok(NativeProvenance {
            owner: self.owner.clone().ok_or("尚未取得桌面会话所属客户端")?,
            revision: self
                .revision
                .as_ref()
                .and_then(Value::as_u64)
                .filter(|revision| *revision <= 9_007_199_254_740_991),
        })
    }

    #[cfg(test)]
    pub(super) async fn follow_transport(
        stream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
        thread: &str,
    ) -> Result<Self, String> {
        validate_thread(thread)?;
        Self::subscribe(Self::initialize(Box::new(stream)).await?, thread).await
    }

    fn apply_event(&mut self, event: Value) -> Result<bool, String> {
        if event["type"] != "broadcast" {
            return Ok(false);
        }
        if event["method"] == "client-status-changed"
            && event["params"]["status"] == "disconnected"
            && self.owner.as_deref() == event["params"]["clientId"].as_str()
        {
            return Err("会话所属的 Codex 窗口已断开，请重新连接".into());
        }
        if event["method"] != "thread-stream-state-changed"
            || event["params"]["conversationId"] != self.thread
            || event["params"]["hostId"] != "local"
        {
            return Ok(false);
        }
        if let Some(targets) = event["targetClientIds"].as_array()
            && !targets
                .iter()
                .any(|id| id.as_str() == self.client.as_deref())
        {
            return Ok(false);
        }
        if event["version"] != 11 {
            return Err("当前 Codex 桌面会话协议不兼容，请更新 Codey".into());
        }
        let change = &event["params"]["change"];
        let source = event["sourceClientId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("桌面会话缺少所属客户端")?;
        if self.owner.as_deref().is_some_and(|owner| owner != source) {
            return Ok(false);
        }
        match change["type"].as_str() {
            Some("snapshot") => {
                if change["conversationState"]["id"] != self.thread {
                    return Err("桌面快照会话不匹配".into());
                }
                self.state = change["conversationState"].clone();
                self.projection.reset();
                self.urgent = true;
                self.owner = Some(source.to_string());
            }
            Some("patches") if self.owner.is_some() => {
                if self.revision.as_ref() != change.get("baseRevision") || self.revision.is_none() {
                    return Err("会话增量已失步，请重新连接获取完整快照".into());
                }
                self.urgent = self
                    .projection
                    .invalidate(&self.state, &change["patches"])?;
                protocol::apply_patches(&mut self.state, &change["patches"])?;
                if self.state["id"] != self.thread {
                    return Err("桌面增量会话不匹配".into());
                }
            }
            _ => return Ok(false),
        }
        self.revision = Some(
            change
                .get("revision")
                .filter(|v| !v.is_null())
                .ok_or("桌面快照缺少版本")?
                .clone(),
        );
        Ok(true)
    }

    pub async fn action(
        &mut self,
        action: &str,
        args: &Value,
        request_gate: impl std::future::Future<Output = Result<(), String>>,
        prepare_files: impl std::future::Future<Output = Result<(Vec<Value>, Vec<Value>), String>>,
    ) -> Result<Value, String> {
        let (method, version, mut params) = action_params(&self.state, action, args)?;
        // 问题与计划的答复也可能开始新一轮；按实际 RPC 而不是 UI 操作名校验。
        if matches!(
            method,
            "thread-follower-start-turn"
                | "thread-follower-steer-turn"
                | "thread-follower-update-thread-settings"
        ) {
            request_gate.await?;
        }
        if matches!(action, "send" | "steer")
            && args["files"]
                .as_array()
                .is_some_and(|files| !files.is_empty())
        {
            let (input, attachments) = match prepare_files.await {
                Ok(prepared) => prepared,
                Err(message) => return Ok(super::create::preparation_failure(message)),
            };
            if action == "send" {
                params["turnStart"]["request"]["input"] = json!(input);
                params["turnStart"]["context"]["attachments"] = json!(attachments);
            } else {
                params["input"] = json!(input);
                params["restoreMessage"]["request"]["input"] = json!(input);
                params["restoreMessage"]["context"]["attachments"] = json!(attachments);
                params["attachments"] = json!(attachments);
            }
        }
        params["conversationId"] = json!(self.thread);
        let result = self
            .request(method, version, params, action != "history")
            .await?;
        let result = &result["result"];
        if result["ok"] == false || result["applied"] == false {
            return Err(UNCERTAIN.into());
        }
        let status = match result.get("status") {
            None | Some(Value::Null) => "ok", // Native RPCs acknowledge with ok/applied/revision or a result.
            Some(status) => match status.as_str() {
                Some("ok") => "ok",
                Some("accepted") => "accepted",
                _ => return Err(UNCERTAIN.into()),
            },
        };
        Ok(json!({"status":status}))
    }
}

pub(super) fn validate_thread(thread: &str) -> Result<(), String> {
    Uuid::parse_str(thread)
        .map(|_| ())
        .map_err(|_| "会话标识无效".into())
}

fn question_answers<'a>(
    questions: &'a [Value],
    answers: &'a Value,
) -> Result<Vec<(&'a str, &'a str)>, String> {
    let answers = answers.as_object().ok_or("请填写问题答案")?;
    let ids = questions
        .iter()
        .map(|question| question["id"].as_str().filter(|id| !id.trim().is_empty()))
        .collect::<Option<std::collections::HashSet<_>>>()
        .ok_or("问题标识无效")?;
    if questions.is_empty()
        || ids.len() != questions.len()
        || answers.len() != ids.len()
        || answers.keys().any(|id| !ids.contains(id.as_str()))
    {
        return Err("请准确回答所有问题".into());
    }
    questions
        .iter()
        .map(|question| {
            let id = question["id"].as_str().ok_or("问题标识无效")?;
            let answer = answers[id]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 20_000)
                .ok_or("问题答案为空或过长")?;
            Ok((id, answer))
        })
        .collect()
}

pub(super) fn action_params(
    state: &Value,
    action: &str,
    args: &Value,
) -> Result<(&'static str, u8, Value), String> {
    let active = protocol::turns(state)
        .into_iter()
        .rev()
        .find(|turn| turn["status"] == "inProgress");
    match action {
        "send" | "steer" => {
            let input = super::images::message_input(args)?;
            let id = args["requestId"].as_str().ok_or("消息缺少请求标识")?;
            validate_thread(id)?;
            if action == "send" && active.is_some() {
                return Err("任务正在执行，请使用补充指令或先停止任务".into());
            }
            if action == "steer" && active.is_none() {
                return Err("当前没有可补充指令的任务，请直接发送".into());
            }
            let request = json!({"threadId":state["id"], "clientUserMessageId":id,
                "input":input});
            let context =
                json!({"inheritThreadSettings":true, "attachments":[], "commentAttachments":[]});
            if action == "send" {
                Ok((
                    "thread-follower-start-turn",
                    2,
                    json!({"turnStart":{"request":request,"context":context}}),
                ))
            } else {
                Ok((
                    "thread-follower-steer-turn",
                    1,
                    json!({"input":request["input"],"clientUserMessageId":id,"restoreMessage":{"request":request,"context":context},"attachments":[]}),
                ))
            }
        }
        "interrupt" => {
            let turn = active
                .and_then(|t| t["turnId"].as_str())
                .ok_or("当前没有可停止的任务")?;
            if args["turnId"].as_str() != Some(turn) {
                return Err("执行轮次已变化，请刷新后再停止".into());
            }
            Ok((
                "thread-follower-interrupt-turn",
                4,
                json!({"mode":"user-stop", "expectedTurnId":turn}),
            ))
        }
        "settings" => {
            let mut settings = json!({});
            if let Some(model) = args.get("model") {
                let model = model
                    .as_str()
                    .filter(|s| !s.trim().is_empty() && s.len() <= 512)
                    .ok_or("模型名称无效")?;
                settings["model"] = json!(model);
            }
            if let Some(effort) = args.get("effort") {
                let effort = effort
                    .as_str()
                    .filter(|s| {
                        matches!(
                            *s,
                            "none"
                                | "minimal"
                                | "low"
                                | "medium"
                                | "high"
                                | "xhigh"
                                | "max"
                                | "ultra"
                        )
                    })
                    .ok_or("思考强度无效")?;
                settings["effort"] = json!(effort);
            }
            if let Some(tier) = args.get("serviceTier") {
                if !tier.is_null() && !matches!(tier.as_str(), Some("priority" | "default")) {
                    return Err("速度模式无效".into());
                }
                settings["serviceTier"] = tier.clone();
            }
            if let Some(mode) = args.get("permissionMode") {
                // Same built-in permission profiles as the desktop composer.
                // Apply only an explicit selection, leaving custom policies alone.
                let (profile, approval) = match mode.as_str() {
                    Some("read-only") => (":read-only", json!("on-request")),
                    Some("auto") => (
                        ":workspace",
                        if state["workspaceKind"] == "projectless" {
                            protocol::projectless_approval_policy()
                        } else {
                            json!("on-request")
                        },
                    ),
                    Some("full-access") => (":danger-full-access", json!("never")),
                    _ => return Err("权限模式无效".into()),
                };
                settings["approvalPolicy"] = approval;
                settings["approvalsReviewer"] = json!("user");
                settings["permissions"] = json!(profile);
            }
            if settings.as_object().is_none_or(|value| value.is_empty()) {
                return Err("请选择要修改的会话设置".into());
            }
            Ok((
                "thread-follower-update-thread-settings",
                2,
                json!({"threadSettings":settings}),
            ))
        }
        "history" => Ok(("thread-follower-load-complete-history", 1, json!({}))),
        "respond" => {
            if let Some(request) = protocol::async_requests(state)
                .into_iter()
                .find(|r| r["id"] == args["approvalId"])
            {
                if request["supported"] != true {
                    return Err("此类请求请在电脑端处理".into());
                }
                let questions = request["params"]["questions"]
                    .as_array()
                    .ok_or("问题格式无效")?;
                let mut replies = Vec::new();
                for (question, (id, answer)) in questions
                    .iter()
                    .zip(question_answers(questions, &args["answers"])?)
                {
                    replies.push(json!({"questionItemId":id,"question":question["question"],"answer":answer}));
                }
                let text = format!(
                    "<send_user_message_question_reply>\n{}\n</send_user_message_question_reply>",
                    json!(replies)
                );
                return action_params(
                    state,
                    if active.is_some() { "steer" } else { "send" },
                    &json!({"text":text,"requestId":args["requestId"]}),
                );
            }
            let request = state["requests"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["id"] == args["approvalId"] && !r["id"].is_null())
                .ok_or("该请求已处理或已过期，请刷新会话")?;
            if !protocol::request_supported(state, request) {
                return Err("此类请求请在电脑端处理".into());
            }
            if request["method"] == "item/plan/requestImplementation" {
                let (text, mode) = match args["decision"].as_str() {
                    Some("implement") => (
                        format!(
                            "Implement the following plan:\n\n{}",
                            request["params"]["planContent"]
                                .as_str()
                                .filter(|s| !s.trim().is_empty())
                                .ok_or("计划内容尚未加载")?
                        ),
                        "default",
                    ),
                    Some("revise") => (
                        args["text"]
                            .as_str()
                            .filter(|s| !s.trim().is_empty())
                            .ok_or("请填写计划修改意见")?
                            .to_string(),
                        "plan",
                    ),
                    _ => return Err("请选择执行计划或修改计划".into()),
                };
                let model = state["latestModel"]
                    .as_str()
                    .or(state["latestCollaborationMode"]["settings"]["model"].as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or("尚未取得桌面模型设置，请重新连接")?;
                let (method, version, mut payload) = action_params(
                    state,
                    "send",
                    &json!({"text":text,"requestId":args["requestId"]}),
                )?;
                payload["turnStart"]["request"]["collaborationMode"] = json!({"mode":mode,"settings":{"model":model,"reasoning_effort":state["latestReasoningEffort"].as_str().or(state["latestThreadSettings"]["effort"].as_str()),"developer_instructions":null}});
                return Ok((method, version, payload));
            }
            let mut payload = json!({"requestId":request["id"]});
            let method = match request["method"].as_str().unwrap_or_default() {
                "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                    let decision = args["decision"]
                        .as_str()
                        .filter(|s| matches!(*s, "accept" | "decline" | "cancel"))
                        .ok_or("审批决定无效")?;
                    if let Some(available) = request["params"]["availableDecisions"].as_array()
                        && !available.iter().any(|v| v == decision)
                    {
                        return Err("当前审批不支持该决定".into());
                    }
                    payload["decision"] = json!(decision);
                    if request["method"] == "item/fileChange/requestApproval" {
                        "thread-follower-file-approval-decision"
                    } else {
                        "thread-follower-command-approval-decision"
                    }
                }
                "item/permissions/requestApproval" => {
                    let decision = args["decision"]
                        .as_str()
                        .filter(|s| matches!(*s, "accept" | "decline"))
                        .ok_or("审批决定无效")?;
                    payload["response"] = json!({"permissions":if decision == "accept" { request["params"]["permissions"].clone() } else { json!({}) }, "scope":"turn"});
                    "thread-follower-permissions-request-approval-response"
                }
                _ => {
                    let questions = request["params"]["questions"]
                        .as_array()
                        .ok_or("请求问题无效")?;
                    let mut response = serde_json::Map::new();
                    for (id, text) in question_answers(questions, &args["answers"])? {
                        response.insert(id.into(), json!({"answers":[text]}));
                    }
                    payload["response"] = json!({"answers":response});
                    "thread-follower-submit-user-input"
                }
            };
            Ok((method, 1, payload))
        }
        _ => Err("不支持的远程会话操作".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn framed_transport_handles_fragments_and_discovery() {
        let (client, mut server) = tokio::io::duplex(1024);
        let fixture = tokio::spawn(async move {
            let length = server.read_u32_le().await.unwrap();
            let mut bytes = vec![0; length as usize];
            server.read_exact(&mut bytes).await.unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request["method"], "initialize");
            let reply = json!({"type":"response","requestId":request["requestId"],"resultType":"success","result":{"clientId":"mobile"}}).to_string().into_bytes();
            server
                .write_all(&(reply.len() as u32).to_le_bytes())
                .await
                .unwrap();
            for part in reply.chunks(3) {
                server.write_all(part).await.unwrap();
            }
        });
        let desktop = Desktop::initialize(Box::new(client)).await.unwrap();
        assert_eq!(desktop.client.as_deref(), Some("mobile"));
        fixture.await.unwrap();
    }

    #[tokio::test]
    async fn invalid_frame_and_stale_revisions_are_rejected() {
        let (client, mut server) = tokio::io::duplex(64);
        let mut desktop = Desktop {
            stream: Box::new(client),
            buffer: Vec::new(),
            outgoing: Vec::new(),
            written: 0,
            client: Some("mobile".into()),
            owner: None,
            thread: "t".into(),
            state: Value::Null,
            revision: None,
            projection: Default::default(),
            urgent: true,
        };
        server.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        assert!(desktop.next().await.unwrap_err().contains("帧长度"));
        let event = json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"owner","params":{"hostId":"local","conversationId":"t","change":{"type":"snapshot","revision":1,"conversationState":{"id":"t"}}}});
        assert!(desktop.apply_event(event).unwrap());
        let patch = json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"owner","params":{"hostId":"local","conversationId":"t","change":{"type":"patches","revision":3,"baseRevision":2,"patches":[]}}});
        assert!(desktop.apply_event(patch).unwrap_err().contains("失步"));
    }

    #[tokio::test]
    async fn native_provenance_accepts_only_safe_numeric_revisions() {
        let (mut desktop, _server) = action_fixture(json!({"id":"t"}));
        assert_eq!(desktop.provenance().unwrap().revision, None);
        for revision in [
            json!("2"),
            json!(-1),
            json!(1.5),
            json!(9_007_199_254_740_992u64),
        ] {
            let event = json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"owner","params":{"hostId":"local","conversationId":"t","change":{"type":"snapshot","revision":revision,"conversationState":{"id":"t"}}}});
            assert!(desktop.apply_event(event).unwrap());
            assert_eq!(desktop.provenance().unwrap().revision, None);
        }
        let event = json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"owner","params":{"hostId":"local","conversationId":"t","change":{"type":"snapshot","revision":42,"conversationState":{"id":"t"}}}});
        assert!(desktop.apply_event(event).unwrap());
        let provenance = desktop.provenance().unwrap();
        assert_eq!(provenance.owner, "owner");
        assert_eq!(provenance.revision, Some(42));
    }

    #[test]
    fn settings_are_partial_and_permissions_use_native_profiles() {
        let state = json!({"id":"t","cwd":"E:/code/codey","latestThreadSettings":{"approvalPolicy":"custom","model":"keep"}});
        let (_, version, model) = action_params(
            &state,
            "settings",
            &json!({"model":"route/model","effort":"high"}),
        )
        .unwrap();
        assert_eq!(version, 2);
        assert_eq!(
            model,
            json!({"threadSettings":{"model":"route/model","effort":"high"}})
        );
        let (_, _, effort) = action_params(&state, "settings", &json!({"effort":"low"})).unwrap();
        assert_eq!(effort, json!({"threadSettings":{"effort":"low"}}));
        for tier in [json!("priority"), json!("default"), Value::Null] {
            let (_, _, payload) =
                action_params(&state, "settings", &json!({"serviceTier":tier})).unwrap();
            assert_eq!(payload, json!({"threadSettings":{"serviceTier":tier}}));
        }
        for (mode, profile, approval) in [
            ("read-only", ":read-only", "on-request"),
            ("auto", ":workspace", "on-request"),
            ("full-access", ":danger-full-access", "never"),
        ] {
            let (method, _, payload) =
                action_params(&state, "settings", &json!({"permissionMode":mode})).unwrap();
            let settings = &payload["threadSettings"];
            assert_eq!(method, "thread-follower-update-thread-settings");
            assert_eq!(settings["permissions"], profile);
            assert_eq!(settings["approvalPolicy"], approval);
            assert_eq!(settings["approvalsReviewer"], "user");
            assert!(
                settings.get("sandboxPolicy").is_none(),
                "Native rejects permissions combined with sandboxPolicy"
            );
            assert!(settings.get("activePermissionProfile").is_none());
            assert!(settings.get("model").is_none());
            assert!(payload.get("activeTurnId").is_none());
        }
        for args in [
            json!({}),
            json!({"model":""}),
            json!({"effort":"invalid"}),
            json!({"permissionMode":"custom"}),
            json!({"model":null}),
            json!({"permissionMode":true}),
            json!({"serviceTier":"fast"}),
            json!({"serviceTier":""}),
            json!({"serviceTier":true}),
            json!({"serviceTier":1}),
        ] {
            assert!(action_params(&state, "settings", &args).is_err());
        }
        let (_, _, settings) = action_params(
            &json!({"workspaceKind":"projectless"}),
            "settings",
            &json!({"permissionMode":"auto"}),
        )
        .unwrap();
        assert_eq!(
            settings["threadSettings"]["approvalPolicy"],
            protocol::projectless_approval_policy()
        );
        assert!(settings["threadSettings"].get("sandboxPolicy").is_none());
    }

    #[test]
    fn actions_keep_desktop_settings_and_validate_live_turns_and_approvals() {
        let state = json!({"id":"t","cwd":"workspace","turns":[],"requests":[{"id":7,"method":"item/commandExecution/requestApproval","params":{"command":"echo safe","cwd":"workspace","availableDecisions":["decline"]}}]});
        let (_, version, payload) = action_params(
            &state,
            "send",
            &json!({"text":"执行测试","requestId":Uuid::new_v4().to_string()}),
        )
        .unwrap();
        assert_eq!(version, 2);
        assert_eq!(
            payload["turnStart"]["context"]["inheritThreadSettings"],
            true
        );
        assert!(
            payload["turnStart"]["request"]
                .get("approvalPolicy")
                .is_none()
        );
        assert!(action_params(&state, "interrupt", &json!({"turnId":"stale"})).is_err());
        assert!(
            action_params(
                &state,
                "respond",
                &json!({"approvalId":7,"decision":"accept"})
            )
            .is_err()
        );
        let unknown = json!({"id":"t","requests":[{"id":9,"method":"item/permissions/requestApproval","params":{"permissions":{"futureScope":{"private":"secret"}}}}]});
        assert_eq!(protocol::view(&unknown)["requests"][0]["supported"], false);
        assert!(
            action_params(
                &unknown,
                "respond",
                &json!({"approvalId":9,"decision":"accept"})
            )
            .is_err()
        );
        assert!(
            action_params(
                &state,
                "respond",
                &json!({"approvalId":7,"decision":"decline"})
            )
            .is_ok()
        );
        assert!(
            action_params(
                &state,
                "respond",
                &json!({"approvalId":8,"decision":"decline"})
            )
            .is_err()
        );
    }

    #[test]
    fn sends_and_steers_image_only_input_with_restore_data_within_frame_budget() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let mut bytes = vec![0; 4 * 1024 * 1024];
        bytes[..3].copy_from_slice(&[0xff, 0xd8, 0xff]);
        let url = format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes));
        let args = json!({"text":"","images":[{"url":url}],"requestId":Uuid::new_v4().to_string()});
        let (_, _, send) = action_params(&json!({"id":"t"}), "send", &args).unwrap();
        let input = &send["turnStart"]["request"]["input"];
        assert_eq!(input, &json!([{"type":"image","url":url}]));
        let active = json!({"id":"t","turns":[{"status":"inProgress"}]});
        let (_, _, steer) = action_params(&active, "steer", &args).unwrap();
        assert_eq!(&steer["input"], input);
        assert_eq!(&steer["restoreMessage"]["request"]["input"], input);
        assert!(steer.to_string().len() + 1024 < MAX_FRAME);
        assert!(action_params(&active, "send", &args).is_err());
        assert!(action_params(&json!({}), "steer", &args).is_err());
    }

    async fn read_frame(stream: &mut tokio::io::DuplexStream) -> Value {
        let size = stream.read_u32_le().await.unwrap() as usize;
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn write_frame(stream: &mut tokio::io::DuplexStream, value: Value) {
        let bytes = value.to_string().into_bytes();
        stream.write_u32_le(bytes.len() as u32).await.unwrap();
        stream.write_all(&bytes).await.unwrap();
    }

    fn action_fixture(state: Value) -> (Desktop, tokio::io::DuplexStream) {
        let (client, server) = tokio::io::duplex(8192);
        (
            Desktop {
                stream: Box::new(client),
                buffer: Vec::new(),
                outgoing: Vec::new(),
                written: 0,
                client: Some("mobile".into()),
                owner: Some("owner".into()),
                thread: "t".into(),
                state,
                revision: None,
                projection: Default::default(),
                urgent: true,
            },
            server,
        )
    }

    #[tokio::test]
    async fn native_file_metadata_reaches_start_and_steer_after_preparation() {
        let args = json!({"text":"read this","files":[{"name":"report.pdf","url":"data:application/octet-stream;base64,YQ=="}],"requestId":Uuid::new_v4().to_string()});
        let native = json!({"files":[{"label":"report.pdf","path":"/managed/report.pdf","fsPath":"/managed/report.pdf"}]});
        let mut input = super::super::images::message_input(&args).unwrap();
        let attachments = super::super::images::attach_files(&mut input, &args, &native).unwrap();
        for action in ["send", "steer"] {
            let state = if action == "send" {
                json!({"id":"t"})
            } else {
                json!({"id":"t","turns":[{"status":"inProgress"}]})
            };
            let (mut desktop, mut server) = action_fixture(state);
            let expected_input = json!(input);
            let expected_attachments = json!(attachments);
            let response = tokio::spawn(async move {
                let frame = read_frame(&mut server).await;
                let params = &frame["params"];
                if action == "send" {
                    assert_eq!(frame["method"], "thread-follower-start-turn");
                    assert_eq!(params["turnStart"]["request"]["input"], expected_input);
                    assert_eq!(
                        params["turnStart"]["context"]["attachments"],
                        expected_attachments
                    );
                } else {
                    assert_eq!(frame["method"], "thread-follower-steer-turn");
                    assert_eq!(params["input"], expected_input);
                    assert_eq!(params["restoreMessage"]["request"]["input"], expected_input);
                    assert_eq!(params["attachments"], expected_attachments);
                    assert_eq!(
                        params["restoreMessage"]["context"]["attachments"],
                        expected_attachments
                    );
                }
                write_frame(&mut server, json!({"type":"response","requestId":frame["requestId"],"resultType":"success","result":{"status":"accepted"}})).await;
            });
            let result = desktop
                .action(action, &args, async { Ok(()) }, async {
                    Ok((input.clone(), attachments.clone()))
                })
                .await
                .unwrap();
            assert_eq!(result["status"], "accepted");
            response.await.unwrap();
        }
    }

    #[tokio::test]
    async fn failed_gate_or_revoked_preparation_never_sends_a_native_turn() {
        let args = json!({"text":"","files":[{"name":"a.txt","url":"data:application/octet-stream;base64,YQ=="}],"requestId":Uuid::new_v4().to_string()});
        for gate_fails in [true, false] {
            let (mut desktop, mut server) = action_fixture(json!({"id":"t"}));
            let result = desktop
                .action(
                    "send",
                    &args,
                    async {
                        if gate_fails {
                            Err("gate unavailable".into())
                        } else {
                            Ok(())
                        }
                    },
                    async {
                        if gate_fails {
                            panic!("preparation ran before the gate")
                        } else {
                            Err("登录会话已撤销".into())
                        }
                    },
                )
                .await;
            if gate_fails {
                assert_eq!(result.unwrap_err(), "gate unavailable");
            } else {
                assert_eq!(
                    result.unwrap(),
                    json!({"status":"failed","submission":"not-started","message":"文件准备失败，消息未发送：登录会话已撤销"})
                );
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(10), server.read_u8())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn large_native_snapshot_preserves_framing_and_outgoing_limit() {
        let (mut desktop, mut server) = action_fixture(Value::Null);
        // The reported Linux snapshot is 22,197,762 bytes, above the old 16 MiB limit.
        let text_len = 22_197_762;
        let fixture = tokio::spawn(async move {
            write_frame(&mut server, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"owner","params":{"hostId":"local","conversationId":"t","change":{"type":"snapshot","revision":1,"conversationState":{"id":"t","title":"x".repeat(text_len),"turns":[]}}}})).await;
            write_frame(&mut server, json!({"type":"response","requestId":"next"})).await;
        });
        let snapshot = desktop.next().await.unwrap();
        assert!(desktop.apply_event(snapshot).unwrap());
        assert_eq!(desktop.state["title"].as_str().unwrap().len(), text_len);
        assert_eq!(desktop.next().await.unwrap()["requestId"], "next");
        fixture.await.unwrap();
        assert_eq!(
            desktop
                .send(&json!({"text":"x".repeat(MAX_FRAME)}))
                .await
                .unwrap_err(),
            "桌面消息过大"
        );
    }

    #[tokio::test]
    async fn native_frame_limit_rejects_invalid_headers_before_reading_payload() {
        for size in [0, (MAX_RECEIVE_FRAME + 1) as u32, u32::MAX] {
            let (mut desktop, mut server) = action_fixture(Value::Null);
            server.write_u32_le(size).await.unwrap();
            let error = tokio::time::timeout(Duration::from_millis(100), desktop.next())
                .await
                .expect("invalid frame must not wait for its body")
                .unwrap_err();
            assert!(error.contains("帧长度"));
        }
    }

    #[tokio::test]
    async fn unverified_model_actions_send_no_desktop_request_including_replies() {
        let active = json!({"id":"t","turns":[{"turnId":"turn","status":"inProgress","items":[{"id":"q","type":"agentMessage","questions":[{"title":"如何连接？","options":["外网"]}]}]}]});
        let question = protocol::async_requests(&active).remove(0);
        let question_id = question["params"]["questions"][0]["id"].as_str().unwrap();
        let message = json!({"text":"test","requestId":Uuid::new_v4().to_string()});
        for (state, action, args) in [
            (json!({"id":"t"}), "send", message.clone()),
            (active.clone(), "steer", message),
            (
                json!({"id":"t"}),
                "settings",
                json!({"model":"route/model"}),
            ),
            (
                active,
                "respond",
                json!({"approvalId":question["id"],"requestId":Uuid::new_v4().to_string(),"answers":{question_id:"外网"}}),
            ),
            (
                json!({"id":"t","latestModel":"route/model","requests":[{"id":5,"method":"item/plan/requestImplementation","params":{"planContent":"运行测试"}}]}),
                "respond",
                json!({"approvalId":5,"requestId":Uuid::new_v4().to_string(),"decision":"implement"}),
            ),
        ] {
            let (mut desktop, mut server) = action_fixture(state);
            let error = desktop
                .action(
                    action,
                    &args,
                    async { Err("gate unavailable".into()) },
                    async { panic!("attachments must not be prepared") },
                )
                .await
                .unwrap_err();
            assert_eq!(error, "gate unavailable", "{action}");
            assert!(
                tokio::time::timeout(Duration::from_millis(10), server.read_u8())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn history_stop_and_scoped_approval_do_not_poll_the_model_gate() {
        for (state, action, args, expected_method) in [
            (
                json!({"id":"t"}),
                "history",
                json!({}),
                "thread-follower-load-complete-history",
            ),
            (
                json!({"id":"t","turns":[{"turnId":"turn","status":"inProgress"}]}),
                "interrupt",
                json!({"turnId":"turn"}),
                "thread-follower-interrupt-turn",
            ),
            (
                json!({"id":"t","requests":[{"id":7,"method":"item/commandExecution/requestApproval","params":{"command":"echo safe","cwd":"/workspace","availableDecisions":["decline"]}}]}),
                "respond",
                json!({"approvalId":7,"decision":"decline"}),
                "thread-follower-command-approval-decision",
            ),
        ] {
            let (mut desktop, mut server) = action_fixture(state);
            let response = tokio::spawn(async move {
                let request = read_frame(&mut server).await;
                assert_eq!(request["method"], expected_method);
                write_frame(&mut server, json!({"type":"response","requestId":request["requestId"],"resultType":"success","result":{"status":"accepted"}})).await;
            });
            let result = desktop
                .action(
                    action,
                    &args,
                    async { panic!("the model gate must not run") },
                    async { panic!("attachments must not be prepared") },
                )
                .await
                .unwrap();
            assert_eq!(result, json!({"status":"accepted"}));
            response.await.unwrap();
        }
    }

    #[tokio::test]
    async fn follows_desktop_owner_and_sends_to_original_workspace() {
        let (client, mut server) = tokio::io::duplex(8192);
        let thread = Uuid::new_v4().to_string();
        let thread_copy = thread.clone();
        let fixture = tokio::spawn(async move {
            let initialize = read_frame(&mut server).await;
            write_frame(&mut server,json!({"type":"response","requestId":initialize["requestId"],"resultType":"success","result":{"clientId":"mobile"}})).await;
            let following = read_frame(&mut server).await;
            assert_eq!(following["params"]["conversationId"], thread_copy);
            assert_eq!(following["params"]["following"], true);
            write_frame(&mut server,json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"desktop-owner","params":{"hostId":"local","conversationId":thread_copy,"change":{"type":"snapshot","revision":1,"conversationState":{"id":thread_copy,"turns":[]}}}})).await;
            write_frame(
                &mut server,
                json!({"type":"client-discovery-request","requestId":"discovery"}),
            )
            .await;
            let action = read_frame(&mut server).await;
            assert_eq!(action["targetClientId"], "desktop-owner");
            assert_eq!(action["method"], "thread-follower-start-turn");
            assert_eq!(action["params"]["conversationId"], thread_copy);
            assert_eq!(
                action["params"]["turnStart"]["request"]["input"][1]["type"],
                "image"
            );
            assert_eq!(
                action["params"]["turnStart"]["context"]["inheritThreadSettings"],
                true
            );
            let discovery = read_frame(&mut server).await;
            assert_eq!(discovery["response"]["canHandle"], false);
            write_frame(&mut server,json!({"type":"response","requestId":action["requestId"],"resultType":"success","result":{"status":"accepted","message":"PRIVATE_ACK_CANARY","config":"PRIVATE_ACK_CANARY"}})).await;
        });
        let mut desktop = Desktop::subscribe(
            Desktop::initialize(Box::new(client)).await.unwrap(),
            &thread,
        )
        .await
        .unwrap();
        let ack = desktop.action(
                    "send",
                    &json!({"text":"test","images":[{"url":"data:image/gif;base64,R0lGODlh"}],"requestId":Uuid::new_v4().to_string()}),
                    async { Ok(()) },
                    async { panic!("attachments must not be prepared") },
                )
                .await
                .unwrap();
        assert_eq!(ack["status"], "accepted");
        assert!(!ack.to_string().contains("PRIVATE_ACK_CANARY"));
        fixture.await.unwrap();
    }

    #[tokio::test]
    async fn acknowledgements_allow_only_confirmed_public_statuses_and_never_native_messages() {
        let cases = vec![
            (json!({"status":"accepted"}), Some("accepted")),
            (json!({"status":"ok"}), Some("ok")),
            (json!({"ok":true}), Some("ok")),
            (json!({"applied":true}), Some("ok")),
            (json!({"revision":3}), Some("ok")),
            (
                json!({"result":{"turn":{"status":"inProgress","config":"PRIVATE_ACK_CANARY"}}}),
                Some("ok"),
            ),
            (json!({"status":"PRIVATE_ACK_CANARY"}), None),
            (json!({"status":"not-ready"}), None),
            (json!({"status":"failed"}), None),
            (json!({"status":{"auth":"PRIVATE_ACK_CANARY"}}), None),
            (json!({"status":true}), None),
            (json!({"ok":false}), None),
            (json!({"applied":false}), None),
        ];
        let (client, mut server) = tokio::io::duplex(4096);
        let responses = cases.clone();
        let fixture = tokio::spawn(async move {
            let initialize = read_frame(&mut server).await;
            write_frame(&mut server, json!({"type":"response","requestId":initialize["requestId"],"resultType":"success","result":{"clientId":"mobile"}})).await;
            for (mut result, _) in responses {
                let request = read_frame(&mut server).await;
                assert_eq!(request["method"], "thread-follower-load-complete-history");
                result["message"] = json!("PRIVATE_ACK_CANARY");
                write_frame(&mut server, json!({"type":"response","requestId":request["requestId"],"resultType":"success","result":result})).await;
            }
        });
        let mut desktop = Desktop::initialize(Box::new(client)).await.unwrap();
        for (_, status) in cases {
            let result = desktop
                .action("history", &json!({}), async { Ok(()) }, async {
                    panic!("attachments must not be prepared")
                })
                .await;
            match status {
                Some(status) => assert_eq!(result.unwrap(), json!({"status":status})),
                None => assert_eq!(result.unwrap_err(), UNCERTAIN),
            }
        }
        fixture.await.unwrap();
    }

    #[test]
    fn command_approval_scope_is_visible_and_required_for_the_same_respond_capability() {
        let mut state = json!({"id":"t","cwd":"/workspace","turns":[{"turnId":"turn","items":[{"id":"i","type":"commandExecution","command":"rm report","cwd":"/important"},{"id":"other","type":"commandExecution","command":"unrelated","cwd":"/wrong"}]}],"requests":[{"id":7,"method":"item/commandExecution/requestApproval","params":{"itemId":"i"}}]});
        let args = json!({"approvalId":7,"decision":"accept"});
        let view = protocol::view(&state);
        assert_eq!(view["requests"][0]["supported"], true);
        assert_eq!(view["requests"][0]["params"]["command"], "rm report");
        assert_eq!(view["requests"][0]["params"]["cwd"], "/important");
        let (method, _, payload) = action_params(&state, "respond", &args).unwrap();
        assert_eq!(method, "thread-follower-command-approval-decision");
        assert_eq!(payload, json!({"requestId":7,"decision":"accept"}));
        state["requests"][0]["params"]["cwd"] = json!("/explicit");
        assert_eq!(
            protocol::view(&state)["requests"][0]["params"]["cwd"],
            "/explicit"
        );
        state["requests"][0]["params"]["cwd"] = Value::Null;
        state["turns"][0]["items"][0]["cwd"] = Value::Null;
        assert_eq!(protocol::view(&state)["requests"][0]["supported"], false);
        assert!(action_params(&state, "respond", &args).is_err());
        for invalid in [
            Value::Null,
            json!(""),
            json!(" "),
            json!({"env":"PRIVATE_SCOPE_CANARY"}),
        ] {
            state["cwd"] = invalid;
            assert_eq!(protocol::view(&state)["requests"][0]["supported"], false);
            assert!(action_params(&state, "respond", &args).is_err());
        }
        state["cwd"] = json!("/workspace");
        state["turns"][0]["items"][0]["cwd"] = json!({"config":"PRIVATE_SCOPE_CANARY"});
        assert_eq!(protocol::view(&state)["requests"][0]["supported"], false);
        assert!(action_params(&state, "respond", &args).is_err());
    }

    #[test]
    fn synchronous_questions_require_unique_valid_ids_and_exact_string_answers() {
        for method in ["item/tool/requestUserInput", "tool/requestUserInput"] {
            let mut state = json!({"id":"t","requests":[{"id":7,"method":method,"params":{"questions":[{"id":"q","question":"first"},{"id":"q","question":"second"}]}}]});
            let duplicate = json!({"approvalId":7,"answers":{"q":"yes","extra":"x"}});
            assert_eq!(protocol::view(&state)["requests"][0]["supported"], false);
            assert!(action_params(&state, "respond", &duplicate).is_err());
            for invalid in [
                Value::Null,
                json!(""),
                json!("  "),
                json!(7),
                json!({"private":"PRIVATE_QUESTION_CANARY"}),
            ] {
                state["requests"][0]["params"]["questions"][1]["id"] = invalid;
                assert_eq!(protocol::view(&state)["requests"][0]["supported"], false);
                assert!(action_params(&state, "respond", &duplicate).is_err());
            }
            state["requests"][0]["params"]["questions"][1]["id"] = json!("other");
            assert_eq!(protocol::view(&state)["requests"][0]["supported"], true);
            for answers in [
                json!({"q":"yes","extra":"x"}),
                json!({"q":"yes"}),
                json!({"q":"yes","other":"ok","extra":"x"}),
                json!({"q":"yes","other":true}),
                json!({"q":"yes","other":7}),
                json!({"q":"yes","other":null}),
                json!({"q":"yes","other":["x"]}),
                json!({"q":"yes","other":{"answers":["x"]}}),
                json!({"q":"yes","other":" "}),
            ] {
                assert!(
                    action_params(
                        &state,
                        "respond",
                        &json!({"approvalId":7,"answers":answers})
                    )
                    .is_err()
                );
            }
            let (method, _, payload) = action_params(
                &state,
                "respond",
                &json!({"approvalId":7,"answers":{"q":"yes","other":"ok"}}),
            )
            .unwrap();
            assert_eq!(method, "thread-follower-submit-user-input");
            assert_eq!(
                payload["response"]["answers"],
                json!({"q":{"answers":["yes"]},"other":{"answers":["ok"]}})
            );
        }
    }

    #[test]
    fn asynchronous_answers_keep_generated_question_ids_and_reject_extra_or_non_string_answers() {
        let state = json!({"id":"t","turns":[{"turnId":"turn","status":"inProgress","items":[{"id":"item","type":"agentMessage","questions":[{"title":"first"},{"title":"second"}]}]}]});
        let request = protocol::async_requests(&state).remove(0);
        let first = json!(["request_user_input_async", "item", 0]).to_string();
        let second = json!(["request_user_input_async", "item", 1]).to_string();
        assert_eq!(request["params"]["questions"][0]["id"], first);
        assert_eq!(request["params"]["questions"][1]["id"], second);
        for answers in [
            json!({first.clone():"yes","extra":"x"}),
            json!({first.clone():"yes",second.clone():7}),
            json!({first.clone():"yes",second.clone():"ok","extra":"x"}),
        ] {
            assert!(action_params(&state, "respond", &json!({"approvalId":request["id"],"requestId":Uuid::new_v4().to_string(),"answers":answers})).is_err());
        }
        let (method, _, payload) = action_params(&state, "respond", &json!({"approvalId":request["id"],"requestId":Uuid::new_v4().to_string(),"answers":{first.clone():"yes",second.clone():"ok"}})).unwrap();
        assert_eq!(method, "thread-follower-steer-turn");
        let text = payload["input"][0]["text"].as_str().unwrap();
        let replies: Value = serde_json::from_str(
            text.trim()
                .strip_prefix("<send_user_message_question_reply>")
                .unwrap()
                .strip_suffix("</send_user_message_question_reply>")
                .unwrap()
                .trim(),
        )
        .unwrap();
        assert_eq!(
            replies,
            json!([{"questionItemId":first,"question":"first","answer":"yes"},{"questionItemId":second,"question":"second","answer":"ok"}])
        );
    }

    #[test]
    fn answers_async_questions_and_plan_using_current_desktop_state() {
        let state = json!({"id":"t","latestModel":"test-model","turns":[{"turnId":"turn","status":"inProgress","items":[{"id":"q","type":"agentMessage","questions":[{"title":"如何连接？","options":["外网"]}]}]}]});
        let request = protocol::async_requests(&state).remove(0);
        let question = request["params"]["questions"][0]["id"].as_str().unwrap();
        let (method,_,payload) = action_params(&state,"respond",&json!({"approvalId":request["id"],"requestId":Uuid::new_v4().to_string(),"answers":{question:"外网"}})).unwrap();
        assert_eq!(method, "thread-follower-steer-turn");
        assert!(
            payload["input"][0]["text"]
                .as_str()
                .unwrap()
                .contains("send_user_message_question_reply")
        );
        let state = json!({"id":"t","latestModel":"test-model","requests":[{"id":5,"method":"item/plan/requestImplementation","params":{"planContent":"运行测试"}}]});
        let (_, _, payload) = action_params(
            &state,
            "respond",
            &json!({"approvalId":5,"requestId":Uuid::new_v4().to_string(),"decision":"implement"}),
        )
        .unwrap();
        assert_eq!(
            payload["turnStart"]["request"]["collaborationMode"]["mode"],
            "default"
        );
        assert_eq!(
            payload["turnStart"]["request"]["collaborationMode"]["settings"]["model"],
            "test-model"
        );
    }

    #[test]
    fn mixed_async_question_schemas_cannot_be_answered_as_a_visible_subset() {
        for bad in [
            json!({"title":{"private":"ASYNC_RESPOND_CANARY"}}),
            json!({"title":"第二题","options":["合法",{"private":"ASYNC_RESPOND_CANARY"}]}),
        ] {
            let state = json!({"id":"t","turns":[{"turnId":"turn","status":"inProgress","items":[{"id":"q","type":"agentMessage","questions":[{"title":"确认？"},bad]}]}]});
            let request = protocol::async_requests(&state).remove(0);
            assert_eq!(request["supported"], false);
            assert!(
                !protocol::view(&state)
                    .to_string()
                    .contains("ASYNC_RESPOND_CANARY")
            );
            let key = json!(["request_user_input_async", "q", 0]).to_string();
            let result = action_params(
                &state,
                "respond",
                &json!({"approvalId":request["id"],"requestId":Uuid::new_v4().to_string(),"answers":{key:"yes"}}),
            );
            assert!(result.err().unwrap().contains("电脑端"));
        }
    }
}
