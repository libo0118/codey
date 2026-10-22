use serde::Serialize;

use super::*;

const PORTABLE_HISTORY_BYTES: usize = 16 * 1024 * 1024;
const PORTABLE_HISTORY_ENTRIES: usize = 64;
const PORTABLE_HISTORY_ITEMS: usize = 16_384;
const PORTABLE_HISTORY_TTL: Duration = Duration::from_secs(15 * 60);
pub(crate) const PORTABLE_HISTORY_SWEEP_INTERVAL: Duration = Duration::from_secs(1);

// 与请求/保留响应预算采用同一工作集估算；这是预算值，不是进程 RSS 上限。
fn history_memory_bytes(payload_bytes: usize) -> Option<usize> {
    payload_bytes.checked_mul(REQUEST_MEMORY_BUDGET_MULTIPLIER)
}

#[derive(Default)]
struct HistoryBudget {
    bytes: usize,
    entries: usize,
}

// 在途请求与已完成检查点共享预算，取消或失败时由 Drop 释放。
struct HistoryReservation {
    budget: Arc<Mutex<HistoryBudget>>,
    bytes: usize,
}

impl HistoryReservation {
    fn acquire(budget: &Arc<Mutex<HistoryBudget>>, bytes: usize) -> Option<Self> {
        let mut state = budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.entries >= PORTABLE_HISTORY_ENTRIES
            || state.bytes.checked_add(history_memory_bytes(bytes)?)? > PORTABLE_HISTORY_BYTES
        {
            return None;
        }
        state.bytes += history_memory_bytes(bytes)?;
        state.entries += 1;
        Some(Self {
            budget: Arc::clone(budget),
            bytes,
        })
    }

    fn resize(&mut self, bytes: usize) -> Option<()> {
        let mut state = self
            .budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = state
            .bytes
            .checked_sub(history_memory_bytes(self.bytes)?)?
            .checked_add(history_memory_bytes(bytes)?)?;
        if total > PORTABLE_HISTORY_BYTES {
            return None;
        }
        state.bytes = total;
        self.bytes = bytes;
        Some(())
    }
}

impl Drop for HistoryReservation {
    fn drop(&mut self) {
        let mut state = self
            .budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.bytes -= history_memory_bytes(self.bytes).expect("reserved history size");
        state.entries -= 1;
    }
}

// 不实现 Debug，避免完整会话内容进入调试日志。
pub(crate) struct PortableHistoryCheckpoint {
    scope: [u8; 32],
    instructions: Option<[u8; 32]>,
    expires_at: Instant,
    input: Vec<Value>,
    reservation: HistoryReservation,
}

pub(crate) struct PortableHistoryEntry {
    checkpoint: PortableHistoryCheckpoint,
    output: Vec<Value>,
}

#[derive(Default)]
pub(crate) struct PortableHistoryCache {
    entries: VecDeque<Arc<PortableHistoryEntry>>,
    budget: Arc<Mutex<HistoryBudget>>,
    #[cfg(test)]
    materializations: Arc<std::sync::atomic::AtomicUsize>,
}

impl PortableHistoryCache {
    pub(crate) fn prune(&mut self) {
        let now = Instant::now();
        // 并发压缩可乱序完成，不能只清理队首。
        self.entries
            .retain(|entry| entry.checkpoint.expires_at > now);
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    // 锁内最多复制 64 个 Arc，历史校验、计量和正文复制均在锁外完成。
    pub(crate) fn snapshot(&mut self) -> PortableHistorySnapshot {
        self.prune();
        PortableHistorySnapshot {
            entries: self.entries.iter().cloned().collect(),
            budget: Arc::clone(&self.budget),
            #[cfg(test)]
            materializations: Arc::clone(&self.materializations),
        }
    }

    pub(crate) fn insert(&mut self, entry: PortableHistoryEntry) {
        self.prune();
        if Arc::ptr_eq(&self.budget, &entry.checkpoint.reservation.budget)
            && entry.checkpoint.expires_at > Instant::now()
        {
            self.entries.push_back(Arc::new(entry));
        }
    }

    #[cfg(test)]
    pub(crate) fn capture(
        &mut self,
        key: &str,
        route: &RouteTarget,
        model: &str,
        body: &Value,
    ) -> Option<PortableHistoryCheckpoint> {
        self.snapshot().capture(key, route, model, body)
    }

    #[cfg(test)]
    pub(crate) fn restore(
        &mut self,
        key: &str,
        route: &RouteTarget,
        model: &str,
        body: &Value,
    ) -> Option<Value> {
        self.snapshot()
            .restoration(key, route, model, body)?
            .materialize()
    }

    #[cfg(test)]
    pub(crate) fn remember(&mut self, checkpoint: PortableHistoryCheckpoint, response: &Value) {
        if let Some(entry) = checkpoint.finish(response) {
            self.insert(entry);
        }
    }
}

pub(crate) struct PortableHistorySnapshot {
    entries: Vec<Arc<PortableHistoryEntry>>,
    budget: Arc<Mutex<HistoryBudget>>,
    #[cfg(test)]
    pub(crate) materializations: Arc<std::sync::atomic::AtomicUsize>,
}

impl PortableHistorySnapshot {
    pub(crate) fn capture(
        &self,
        key: &str,
        route: &RouteTarget,
        model: &str,
        body: &Value,
    ) -> Option<PortableHistoryCheckpoint> {
        portable_request_size(body)?;
        let scope = portable_scope(key, route, model);
        let instructions = instructions_digest(body)?;
        let plan = self.plan(scope, instructions, body)?;
        let mut reservation =
            HistoryReservation::acquire(&self.budget, plan.normalization_budget(false)?)?;
        let mut history = json!({"input": plan.materialize(false)});
        normalize_native_responses_context(&mut history, true);
        let input = history.get_mut("input")?.as_array_mut()?;
        if input.iter().any(|item| item["type"] == "compaction") {
            return None;
        }
        validate_history(input.iter(), false)?;
        reservation.resize(serialized_size(input, PORTABLE_HISTORY_BYTES)?)?;
        Some(PortableHistoryCheckpoint {
            scope,
            instructions,
            expires_at: Instant::now() + PORTABLE_HISTORY_TTL,
            input: std::mem::take(input),
            reservation,
        })
    }
}

impl PortableHistoryCheckpoint {
    pub(crate) fn finish(mut self, response: &Value) -> Option<PortableHistoryEntry> {
        if self.expires_at <= Instant::now()
            || match response.get("status") {
                Some(status) => status != "completed",
                None => response["object"] != "response.compaction",
            }
            || response.get("error").is_some_and(|error| !error.is_null())
            || response
                .get("incomplete_details")
                .is_some_and(|details| !details.is_null())
        {
            return None;
        }
        // compact 旧接口没有 status，Responses 终态则必须由调用者先行确认。
        let output = response.get("output").and_then(Value::as_array)?;
        if output
            .iter()
            .filter(|item| item["type"] == "compaction")
            .count()
            != 1
            || !output.iter().any(is_opaque_compaction)
            || validate_history(output.iter(), true).is_none()
        {
            return None;
        }
        let output_bytes = serialized_size(output, PORTABLE_HISTORY_BYTES)?;
        let bytes = self.reservation.bytes.checked_add(output_bytes)?;
        self.reservation.resize(bytes)?;
        // 完整 output 字面窗口用于后续匹配，不先规范化或单独提取密文。
        Some(PortableHistoryEntry {
            checkpoint: self,
            output: output.clone(),
        })
    }
}

impl PortableHistorySnapshot {
    pub(crate) fn restoration<'a>(
        &'a self,
        key: &str,
        route: &RouteTarget,
        model: &str,
        body: &'a Value,
    ) -> Option<PortableHistoryRestore<'a>> {
        let body_bytes = portable_request_size(body)?;
        let instructions = instructions_digest(body)?;
        let plan = self.plan(portable_scope(key, route, model), instructions, body)?;
        let old_input_bytes = serialized_size(body.get("input")?, PORTABLE_HISTORY_BYTES)?;
        let input_bytes = plan.normalization_budget(true)?;
        let restored_bytes = body_bytes
            .checked_sub(old_input_bytes)?
            .checked_add(input_bytes)?;
        if restored_bytes > PORTABLE_HISTORY_BYTES {
            return None;
        }
        Some(PortableHistoryRestore {
            body,
            plan,
            body_bytes,
            restored_bytes,
            #[cfg(test)]
            materializations: Arc::clone(&self.materializations),
        })
    }

    fn plan<'a>(
        &'a self,
        scope: [u8; 32],
        instructions: Option<[u8; 32]>,
        body: &'a Value,
    ) -> Option<HistoryPlan<'a>> {
        if let Some(text) = body.get("input").and_then(Value::as_str) {
            return Some(HistoryPlan {
                items: Vec::new(),
                text: Some(text),
                trigger: None,
                expanded: false,
            });
        }
        let (mut items, trigger) = request_history(body)?;
        let expanded = items.iter().any(|item| is_opaque_compaction(item));
        if expanded {
            let now = Instant::now();
            let mut matches = self.entries.iter().rev().filter(|entry| {
                entry.checkpoint.expires_at > now
                    && entry.checkpoint.scope == scope
                    && entry.checkpoint.instructions == instructions
                    && items.len() >= entry.output.len()
                    && items
                        .iter()
                        .zip(&entry.output)
                        .all(|(actual, expected)| *actual == expected)
            });
            let entry = matches.next()?;
            if matches.any(|other| other.checkpoint.input != entry.checkpoint.input) {
                return None;
            }
            let tail = &items[entry.output.len()..];
            if entry.checkpoint.input.len().checked_add(tail.len())? > PORTABLE_HISTORY_ITEMS {
                return None;
            }
            items = entry
                .checkpoint
                .input
                .iter()
                .chain(tail.iter().copied())
                .collect();
        }
        validate_history(items.iter().copied(), false)?;
        Some(HistoryPlan {
            items,
            text: None,
            trigger,
            expanded,
        })
    }
}

pub(crate) struct PortableHistoryRestore<'a> {
    body: &'a Value,
    plan: HistoryPlan<'a>,
    body_bytes: usize,
    restored_bytes: usize,
    #[cfg(test)]
    materializations: Arc<std::sync::atomic::AtomicUsize>,
}

impl PortableHistoryRestore<'_> {
    // 对纯文本历史按一字节一个 token 的上界计量，并预留协议项开销和摘要指令。
    pub(crate) fn input_token_bound(&self) -> Option<u64> {
        let bytes = self
            .restored_bytes
            .checked_add(self.plan.items.len().checked_mul(32)?)?
            .checked_add(4096)?;
        u64::try_from(bytes).ok()
    }

    pub(crate) fn request_budget_bytes(&self) -> Option<usize> {
        // 替换前仍持有原正文；grow_request_body_budget 再乘现有 JSON 工作集系数。
        self.body_bytes
            .checked_add(usize::try_from(self.input_token_bound()?).ok()?)
    }

    pub(crate) fn materialize(self) -> Option<Value> {
        #[cfg(test)]
        self.materializations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut restored = if self.plan.expanded {
            let mut object = serde_json::Map::new();
            for (field, value) in self.body.as_object()? {
                if field != "input" {
                    object.insert(field.clone(), value.clone());
                }
            }
            object.insert("input".into(), self.plan.materialize(true));
            Value::Object(object)
        } else {
            self.body.clone()
        };
        normalize_native_responses_context(&mut restored, true);
        let (items, _) = request_history(&restored)?;
        if items.iter().any(|item| item["type"] == "compaction") {
            return None;
        }
        validate_history(items.iter().copied(), false)?;
        serialized_size(&restored, PORTABLE_HISTORY_BYTES)?;
        Some(restored)
    }
}

struct HistoryPlan<'a> {
    items: Vec<&'a Value>,
    text: Option<&'a str>,
    trigger: Option<&'a Value>,
    expanded: bool,
}

impl HistoryPlan<'_> {
    // Codey 摘要解码后可能含需 JSON 转义的字符，提前预留上界再复制。
    fn normalization_budget(&self, include_trigger: bool) -> Option<usize> {
        let mut bytes = self.serialized_bytes(include_trigger)?;
        for item in &self.items {
            let extra = match item["type"].as_str() {
                Some("compaction") => item["encrypted_content"]
                    .as_str()?
                    .len()
                    .checked_mul(6)?
                    .checked_add(128)?,
                Some("reasoning") => {
                    serialized_size(*item, PORTABLE_HISTORY_BYTES)?.checked_add(128)?
                }
                _ => 0,
            };
            bytes = bytes.checked_add(extra)?;
            if bytes > PORTABLE_HISTORY_BYTES {
                return None;
            }
        }
        Some(bytes)
    }

    fn serialized_bytes(&self, include_trigger: bool) -> Option<usize> {
        if let Some(text) = self.text {
            #[derive(Serialize)]
            struct TextMessage<'a> {
                role: &'static str,
                content: &'a str,
            }
            return serialized_size(
                &[TextMessage {
                    role: "user",
                    content: text,
                }],
                PORTABLE_HISTORY_BYTES,
            );
        }
        let mut bytes = serialized_size(&self.items, PORTABLE_HISTORY_BYTES)?;
        if include_trigger && let Some(trigger) = self.trigger {
            bytes = bytes
                .checked_add(serialized_size(trigger, PORTABLE_HISTORY_BYTES)?)?
                .checked_add(usize::from(!self.items.is_empty()))?;
        }
        (bytes <= PORTABLE_HISTORY_BYTES).then_some(bytes)
    }

    fn materialize(&self, include_trigger: bool) -> Value {
        if let Some(text) = self.text {
            return json!([{"role":"user", "content":text}]);
        }
        let mut items: Vec<Value> = self.items.iter().map(|item| (*item).clone()).collect();
        if include_trigger && let Some(trigger) = self.trigger {
            items.push(trigger.clone());
        }
        Value::Array(items)
    }
}

fn portable_scope(key: &str, route: &RouteTarget, model: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    for value in [key, route.provider_id.as_str(), model] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    digest.update(route.context_config);
    digest.finalize().into()
}

fn instructions_digest(body: &Value) -> Option<Option<[u8; 32]>> {
    let Some(instructions) = body.get("instructions") else {
        return Some(None);
    };
    if !instructions.is_null() && !instructions.is_string() {
        return None;
    }
    let mut digest = Sha256::new();
    // 前置 body 大小检查已限制序列化量；hash writer 不分配内容副本。
    struct HashWriter<'a>(&'a mut Sha256);
    impl std::io::Write for HashWriter<'_> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.update(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(HashWriter(&mut digest), instructions).ok()?;
    Some(Some(digest.finalize().into()))
}

fn portable_request_size(body: &Value) -> Option<usize> {
    body.as_object()?;
    for name in ["previous_response_id", "conversation", "prompt"] {
        if body
            .get(name)
            .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        {
            return None;
        }
    }
    serialized_size(body, PORTABLE_HISTORY_BYTES)
}

fn request_history(body: &Value) -> Option<(Vec<&Value>, Option<&Value>)> {
    let input = body.get("input")?;
    let items = match input {
        Value::Array(items) => items.as_slice(),
        Value::Object(_) => std::slice::from_ref(input),
        Value::String(_) => return Some((Vec::new(), None)),
        _ => return None,
    };
    if items.len() > PORTABLE_HISTORY_ITEMS {
        return None;
    }
    let trigger = items
        .last()
        .filter(|item| item["type"] == "compaction_trigger");
    if trigger.is_some_and(|item| item.as_object().is_none_or(|object| object.len() != 1)) {
        return None;
    }
    let history = &items[..items.len() - usize::from(trigger.is_some())];
    Some((history.iter().collect(), trigger))
}

fn serialized_size(value: &(impl Serialize + ?Sized), limit: usize) -> Option<usize> {
    bounded_json_bytes(&value, limit).ok()
}

fn is_opaque_compaction(item: &Value) -> bool {
    item["type"] == "compaction"
        && item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.starts_with(CODEY_SUMMARY_PREFIX))
}

fn known_fields(item: &Value, fields: &[&str]) -> Option<()> {
    item.as_object()?
        .keys()
        .all(|field| fields.contains(&field.as_str()))
        .then_some(())
}

fn optional_text(item: &Value, name: &str) -> bool {
    item.get(name)
        .is_none_or(|value| value.is_null() || value.is_string())
}

fn completed_item(item: &Value) -> bool {
    item.get("status")
        .is_none_or(|status| status.is_null() || status == "completed")
}

fn text_parts(value: &Value, kinds: &[&str], allow_string: bool) -> Option<()> {
    if allow_string && value.is_string() {
        return Some(());
    }
    for part in value.as_array()? {
        known_fields(
            part,
            &["type", "text", "refusal", "annotations", "logprobs"],
        )?;
        let kind = part.get("type")?.as_str()?;
        if !kinds.contains(&kind) {
            return None;
        }
        let field = if kind == "refusal" { "refusal" } else { "text" };
        part.get(field)?.as_str()?;
        for name in ["annotations", "logprobs"] {
            if part.get(name).is_some_and(|value| {
                !value.is_null() && !value.as_array().is_some_and(Vec::is_empty)
            }) {
                return None;
            }
        }
    }
    Some(())
}

fn validate_history<'a>(
    items: impl IntoIterator<Item = &'a Value>,
    allow_opaque: bool,
) -> Option<()> {
    let mut seen = HashSet::new();
    let mut pending = HashMap::new();
    for (index, item) in items.into_iter().enumerate() {
        if index >= PORTABLE_HISTORY_ITEMS || !completed_item(item) || !optional_text(item, "id") {
            return None;
        }
        let kind = match item.get("type") {
            None => "message",
            Some(value) => value.as_str()?,
        };
        match kind {
            "message" => {
                known_fields(item, &["type", "id", "role", "content", "status", "phase"])?;
                if !matches!(
                    item.get("role")?.as_str()?,
                    "system" | "developer" | "user" | "assistant"
                ) || !optional_text(item, "phase")
                {
                    return None;
                }
                text_parts(
                    item.get("content")?,
                    &["input_text", "output_text", "text", "refusal"],
                    true,
                )?;
            }
            "function_call" | "custom_tool_call" => {
                let payload = if kind == "function_call" {
                    "arguments"
                } else {
                    "input"
                };
                known_fields(item, &["type", "id", "call_id", "name", payload, "status"])?;
                let id = item.get("call_id")?.as_str()?;
                if id.trim().is_empty()
                    || item.get("name")?.as_str()?.trim().is_empty()
                    || !seen.insert(id)
                {
                    return None;
                }
                item.get(payload)?.as_str()?;
                pending.insert(id, kind);
            }
            "function_call_output" | "custom_tool_call_output" => {
                known_fields(item, &["type", "id", "call_id", "output", "status"])?;
                let id = item.get("call_id")?.as_str()?;
                let expected = if kind == "function_call_output" {
                    "function_call"
                } else {
                    "custom_tool_call"
                };
                if id.trim().is_empty() || pending.remove(id) != Some(expected) {
                    return None;
                }
                text_parts(
                    item.get("output")?,
                    &["input_text", "output_text", "text"],
                    true,
                )?;
            }
            "reasoning" => {
                known_fields(
                    item,
                    &[
                        "type",
                        "id",
                        "encrypted_content",
                        "content",
                        "summary",
                        "status",
                    ],
                )?;
                if !optional_text(item, "encrypted_content") {
                    return None;
                }
                if let Some(content) = item.get("content") {
                    text_parts(content, &["reasoning_text", "text"], false)?;
                }
                if let Some(summary) = item.get("summary") {
                    text_parts(summary, &["summary_text", "text"], false)?;
                }
            }
            "compaction" => {
                known_fields(item, &["type", "id", "encrypted_content"])?;
                let content = item.get("encrypted_content")?.as_str()?;
                if content.trim().is_empty()
                    || (!allow_opaque && !content.starts_with(CODEY_SUMMARY_PREFIX))
                {
                    return None;
                }
            }
            _ => return None,
        }
    }
    pending.is_empty().then_some(())
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::*;
    use crate::local_router::tests::router_config;

    fn route() -> RouteTarget {
        let (config, provider, _) = router_config("http://127.0.0.1:9/v1".into());
        RouterSnapshot::from_config(&config).routes[&provider]
            .as_ref()
            .clone()
    }

    fn full_history() -> Value {
        json!({"model":"model", "instructions":"keep current rules", "input":[
            {"role":"system", "content":"system rule"},
            {"role":"developer", "content":[{"type":"input_text", "text":"developer rule"}]},
            {"role":"user", "content":"run both tools"},
            {"type":"function_call", "call_id":"call1", "name":"read", "arguments":"{\"path\":\"src\"}"},
            {"type":"function_call_output", "call_id":"call1", "output":"complete result\nwith evidence"},
            {"type":"custom_tool_call", "call_id":"call2", "name":"patch", "input":"complete patch"},
            {"type":"custom_tool_call_output", "call_id":"call2", "output":[{"type":"input_text", "text":"all details preserved"}]},
            {"role":"assistant", "content":"finished"}
        ]})
    }

    fn compact(ciphertext: &str) -> Value {
        json!({"status":"completed", "output":[
            {"type":"compaction", "id":"cmp_exact", "encrypted_content":ciphertext},
            {"type":"message", "role":"user", "content":"retained user turn"}
        ]})
    }

    fn continuation(source: &Value, response: &Value) -> Value {
        let mut body = source.clone();
        body["input"] = response["output"].clone();
        body["input"].as_array_mut().unwrap().extend([
            json!({"role":"user", "content":"continue"}),
            json!({"type":"compaction_trigger"}),
        ]);
        body
    }

    fn remember(
        cache: &mut PortableHistoryCache,
        route: &RouteTarget,
        source: &Value,
        response: &Value,
    ) {
        let checkpoint = cache.capture("thread:one", route, "model", source).unwrap();
        cache.remember(checkpoint, response);
    }

    #[test]
    fn restores_complete_tool_history_without_retained_output_duplicates() {
        let mut cache = PortableHistoryCache::default();
        let route = route();
        let source = full_history();
        let original = source.clone();
        let response = compact("opaque");
        remember(&mut cache, &route, &source, &response);
        let next = continuation(&source, &response);
        let restored = cache.restore("thread:one", &route, "model", &next).unwrap();
        let mut expected = source.clone();
        expected["input"]
            .as_array_mut()
            .unwrap()
            .extend(next["input"].as_array().unwrap()[2..].iter().cloned());
        assert_eq!(restored, expected);
        assert_eq!(source, original);
        assert_eq!(next["input"][0]["encrypted_content"], "opaque");
    }

    #[test]
    fn expands_repeated_compactions_and_discards_only_the_legal_trigger() {
        let mut cache = PortableHistoryCache::default();
        let route = route();
        let source = full_history();
        let first = compact("first");
        remember(&mut cache, &route, &source, &first);
        let next = continuation(&source, &first);
        let second = compact("second");
        remember(&mut cache, &route, &next, &second);
        let restored = cache
            .restore("thread:one", &route, "model", &continuation(&next, &second))
            .unwrap();
        assert_eq!(
            restored["input"].as_array().unwrap().len(),
            source["input"].as_array().unwrap().len() + 3
        );
        assert_eq!(restored["input"][8]["content"], "continue");
        assert_eq!(restored["input"][9]["content"], "continue");
        assert_eq!(restored["input"][10]["type"], "compaction_trigger");
        assert!(!restored.to_string().contains("encrypted_content"));
    }

    #[test]
    fn rejects_different_scope_account_model_config_or_instructions() {
        let mut cache = PortableHistoryCache::default();
        let route = route();
        let source = full_history();
        let response = compact("same ciphertext");
        remember(&mut cache, &route, &source, &response);
        let next = continuation(&source, &response);
        assert!(
            cache
                .restore("thread:other", &route, "model", &next)
                .is_none()
        );
        assert!(
            cache
                .restore("thread:one", &route, "other", &next)
                .is_none()
        );
        let mut changed = route.clone();
        changed.provider_id.push_str("-other");
        assert!(
            cache
                .restore("thread:one", &changed, "model", &next)
                .is_none()
        );
        changed = route.clone();
        changed.context_config[0] ^= 1;
        assert!(
            cache
                .restore("thread:one", &changed, "model", &next)
                .is_none()
        );
        for instructions in [json!("updated rules"), Value::Null] {
            let mut changed = next.clone();
            changed["instructions"] = instructions;
            assert!(
                cache
                    .restore("thread:one", &route, "model", &changed)
                    .is_none()
            );
        }
        let mut changed = next.clone();
        changed.as_object_mut().unwrap().remove("instructions");
        assert!(
            cache
                .restore("thread:one", &route, "model", &changed)
                .is_none()
        );
    }

    #[test]
    fn rejects_any_change_or_missing_part_of_the_complete_output_prefix() {
        let mut cache = PortableHistoryCache::default();
        let route = route();
        let source = full_history();
        let response = compact("opaque");
        remember(&mut cache, &route, &source, &response);
        let next = continuation(&source, &response);
        let mut candidates = Vec::new();
        for index in 0..2 {
            let mut changed = next.clone();
            changed["input"][index]["id"] = json!("different");
            candidates.push(changed);
            let mut missing = next.clone();
            missing["input"].as_array_mut().unwrap().remove(index);
            // 缺少 compaction 的纯明文无需恢复；仍含密文才必须拒绝。
            if index == 1 {
                candidates.push(missing);
            }
        }
        let mut shifted = next.clone();
        shifted["input"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"role":"user","content":"prefix"}));
        candidates.push(shifted);
        let mut changed = next.clone();
        changed["input"][1]["content"] = json!("changed retained user turn");
        candidates.push(changed);
        for candidate in candidates {
            assert!(
                cache
                    .restore("thread:one", &route, "model", &candidate)
                    .is_none()
            );
        }
        let mut other_source = source.clone();
        other_source["input"][0]["content"] = json!("different original history");
        remember(&mut cache, &route, &other_source, &response);
        assert!(
            cache
                .restore("thread:one", &route, "model", &next)
                .is_none()
        );
    }

    #[test]
    fn expires_entries_and_counts_in_flight_checkpoints() {
        let mut cache = PortableHistoryCache::default();
        let route = route();
        let source = full_history();
        let response = compact("opaque");
        remember(&mut cache, &route, &source, &response);
        Arc::get_mut(&mut cache.entries[0])
            .unwrap()
            .checkpoint
            .expires_at = Instant::now() - Duration::from_secs(1);
        assert!(
            cache
                .restore(
                    "thread:one",
                    &route,
                    "model",
                    &continuation(&source, &response)
                )
                .is_none()
        );
        assert_eq!(cache.budget.lock().unwrap().bytes, 0);
        let mut pending = Vec::new();
        for _ in 0..PORTABLE_HISTORY_ENTRIES {
            pending.push(
                cache
                    .capture("thread:one", &route, "model", &source)
                    .unwrap(),
            );
        }
        assert!(
            cache
                .capture("thread:one", &route, "model", &source)
                .is_none()
        );
        assert_eq!(
            cache.budget.lock().unwrap().entries,
            PORTABLE_HISTORY_ENTRIES
        );
        pending.pop();
        assert!(
            cache
                .capture("thread:one", &route, "model", &source)
                .is_some()
        );
        drop(pending);
        assert_eq!(cache.budget.lock().unwrap().bytes, 0);
    }

    #[test]
    fn bounds_input_output_total_bytes_and_restore_before_cloning() {
        let mut cache = PortableHistoryCache::default();
        let route = route();
        let oversized =
            json!({"input":[{"role":"user", "content":"x".repeat(PORTABLE_HISTORY_BYTES)}]});
        assert!(
            cache
                .capture("thread:one", &route, "model", &oversized)
                .is_none()
        );
        assert!(
            cache
                .restore("thread:one", &route, "model", &oversized)
                .is_none()
        );
        let large =
            json!({"input":[{"role":"user", "content":"x".repeat(PORTABLE_HISTORY_BYTES / 8)}]});
        let first = cache
            .capture("thread:one", &route, "model", &large)
            .unwrap();
        assert!(
            cache
                .capture("thread:one", &route, "model", &large)
                .is_none()
        );
        let huge_output = compact(&"y".repeat(PORTABLE_HISTORY_BYTES / 2));
        cache.remember(first, &huge_output);
        assert!(cache.entries.is_empty());
        assert_eq!(cache.budget.lock().unwrap().bytes, 0);
        let output = compact("small");
        remember(&mut cache, &route, &large, &output);
        let mut next = continuation(&large, &output);
        next["input"][2]["content"] = json!("z".repeat(PORTABLE_HISTORY_BYTES));
        assert!(
            cache
                .restore("thread:one", &route, "model", &next)
                .is_none()
        );
    }

    #[test]
    fn rejects_references_unknown_items_and_invalid_tool_pairs() {
        let route = route();
        let mut cache = PortableHistoryCache::default();
        let source = full_history();
        for (field, value) in [
            ("previous_response_id", json!("response-id")),
            ("conversation", json!({"id":"conversation"})),
            ("prompt", json!({"id":"prompt"})),
        ] {
            let mut body = source.clone();
            body[field] = value;
            assert!(
                cache
                    .capture("thread:one", &route, "model", &body)
                    .is_none()
            );
            assert!(
                cache
                    .restore("thread:one", &route, "model", &body)
                    .is_none()
            );
        }
        let invalid = [
            json!({"type":"item_reference","id":"item"}),
            json!({"type":"unknown","payload":"unreadable"}),
            json!({"role":"user","content":[{"type":"encrypted_content","encrypted_content":"secret"}]}),
            json!({"type":"function_call_output","call_id":"orphan","output":"important result"}),
            json!({"type":"function_call","call_id":"unfinished","name":"read","arguments":"{}"}),
            json!({"type":"compaction_trigger", "extra":true}),
            json!({"role":"user","content":[{"type":"input_file","file_id":"file-other-account"}]}),
        ];
        for item in invalid {
            let body = json!({"input":[item]});
            assert!(
                cache
                    .capture("thread:one", &route, "model", &body)
                    .is_none()
            );
            assert!(
                cache
                    .restore("thread:one", &route, "model", &body)
                    .is_none()
            );
        }
        for duplicate in [source["input"][3].clone(), source["input"][4].clone()] {
            let mut body = source.clone();
            body["input"].as_array_mut().unwrap().push(duplicate);
            assert!(
                cache
                    .capture("thread:one", &route, "model", &body)
                    .is_none()
            );
        }
        let mut wrong_kind = source.clone();
        wrong_kind["input"][4]["type"] = json!("custom_tool_call_output");
        assert!(
            cache
                .capture("thread:one", &route, "model", &wrong_kind)
                .is_none()
        );
        let mut wrong_id = source.clone();
        wrong_id["input"][4]["call_id"] = json!(" call1 ");
        assert!(
            cache
                .capture("thread:one", &route, "model", &wrong_id)
                .is_none()
        );
    }

    #[test]
    fn normalizes_codey_summaries_and_visible_reasoning_without_mutating_source() {
        let route = route();
        let mut cache = PortableHistoryCache::default();
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode("portable summary\nwith evidence");
        let body = json!({"input":[
            {"type":"compaction","encrypted_content":format!("{CODEY_SUMMARY_PREFIX}{encoded}")},
            {"type":"reasoning","summary":[{"type":"summary_text","text":"visible reasoning"}],"encrypted_content":"account-bound"},
            {"type":"reasoning","summary":[],"encrypted_content":"opaque-reasoning"},
            {"role":"user","content":"continue"}
        ]});
        let original = body.clone();
        let restored = cache.restore("thread:one", &route, "model", &body).unwrap();
        assert_eq!(body, original);
        assert_eq!(restored["input"].as_array().unwrap().len(), 3);
        assert_eq!(
            restored["input"][0]["content"][0]["text"],
            "portable summary\nwith evidence"
        );
        assert_eq!(
            restored["input"][1]["summary"][0]["text"],
            "visible reasoning"
        );
        assert!(!restored.to_string().contains("encrypted_content"));
        let checkpoint = cache.capture("thread:one", &route, "model", &body).unwrap();
        assert_eq!(checkpoint.input, *restored["input"].as_array().unwrap());
        for encoded in ["bad!", ""] {
            let invalid = json!({"input":[{"type":"compaction","encrypted_content":format!("{CODEY_SUMMARY_PREFIX}{encoded}")} ]});
            assert!(
                cache
                    .capture("thread:one", &route, "model", &invalid)
                    .is_none()
            );
            assert!(
                cache
                    .restore("thread:one", &route, "model", &invalid)
                    .is_none()
            );
        }
    }

    #[test]
    fn failed_incomplete_unrecognized_or_unpaired_output_is_never_remembered() {
        let route = route();
        let mut cache = PortableHistoryCache::default();
        let source = full_history();
        let mut bad_responses = Vec::new();
        for status in ["failed", "incomplete", "in_progress", "cancelled"] {
            let mut response = compact("opaque");
            response["status"] = json!(status);
            bad_responses.push(response);
        }
        for (field, value) in [
            ("error", json!({"message":"failed"})),
            ("incomplete_details", json!({"reason":"budget"})),
        ] {
            let mut response = compact("opaque");
            response[field] = value;
            bad_responses.push(response);
        }
        let mut missing_status = compact("opaque");
        missing_status.as_object_mut().unwrap().remove("status");
        bad_responses.push(missing_status);
        for item in [
            json!({"type":"unknown"}),
            json!({"type":"function_call_output","call_id":"orphan","output":"result"}),
            json!({"type":"message","role":"assistant","content":"partial","status":"in_progress"}),
        ] {
            let mut response = compact("opaque");
            response["output"].as_array_mut().unwrap().push(item);
            bad_responses.push(response);
        }
        for response in bad_responses {
            remember(&mut cache, &route, &source, &response);
            assert!(cache.entries.is_empty());
            assert_eq!(cache.budget.lock().unwrap().bytes, 0);
        }
        let mut legacy = compact("complete legacy");
        legacy.as_object_mut().unwrap().remove("status");
        legacy["object"] = json!("response.compaction");
        remember(&mut cache, &route, &source, &legacy);
        assert_eq!(cache.entries.len(), 1);
    }

    #[tokio::test]
    async fn idle_router_prunes_expired_history_and_stop_releases_cache() {
        let (config, provider, model) = router_config("http://127.0.0.1:9/v1".into());
        let router = LocalRouter::start(&config).await.unwrap();
        let cache = router.portable_history_cache.upgrade().unwrap();
        let route = Arc::clone(&router.snapshot.read().unwrap().routes[&provider]);
        let response = compact("expires while idle");
        let budget;
        let entry;
        {
            let mut cache = cache.lock().unwrap();
            let mut checkpoint = cache
                .capture("thread:one", &route, &model, &full_history())
                .unwrap();
            checkpoint.expires_at = Instant::now() + Duration::from_millis(50);
            cache.remember(checkpoint, &response);
            budget = Arc::clone(&cache.budget);
            entry = Arc::downgrade(&cache.entries[0]);
            assert!(budget.lock().unwrap().bytes > 0);
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while entry.upgrade().is_some() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("idle sweeper must release expired history without requests");
        assert_eq!(budget.lock().unwrap().bytes, 0);
        {
            let mut cache = cache.lock().unwrap();
            let checkpoint = cache
                .capture("thread:one", &route, &model, &full_history())
                .unwrap();
            cache.remember(checkpoint, &response);
            assert!(!cache.entries.is_empty());
        }
        router.stop().await.unwrap();
        assert!(cache.lock().unwrap().entries.is_empty());
        assert_eq!(budget.lock().unwrap().bytes, 0);
        drop(cache);
        assert!(router.portable_history_cache.upgrade().is_none());
    }
}
