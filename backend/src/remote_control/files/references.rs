use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use serde_json::Value;

use super::super::protocol;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Reference {
    File {
        value: String,
        native: bool,
        markdown: bool,
    },
    InlineImage {
        url: Arc<String>,
        name: String,
    },
}

pub(super) type Scope = (String, String);

#[derive(Clone)]
pub(super) struct References {
    pub values: HashMap<String, Reference>,
    pub scopes: HashMap<Scope, HashMap<String, Reference>>,
    items: HashSet<Scope>,
    turns: HashSet<String>,
    complete_turns: HashSet<String>,
    complete: bool,
}

impl References {
    pub fn covers(&self, scope: &Scope) -> bool {
        self.items.contains(scope)
            || self.complete_turns.contains(&scope.0)
            || self.complete && !self.turns.contains(&scope.0)
    }
}

pub(super) fn collect(state: &Value) -> Result<References, String> {
    let mut collected = References {
        values: HashMap::new(),
        scopes: HashMap::new(),
        items: HashSet::new(),
        turns: HashSet::new(),
        complete_turns: HashSet::new(),
        complete: if state["turnHistory"]["kind"] == "canonical" {
            state["turnHistory"]["history"]["isComplete"] == true
        } else {
            // Native normalization defaults absent pagination to complete history.
            state["turnsPagination"].is_null()
                || state["turnsPagination"]["hasLoadedOldest"] == true
        },
    };
    for turn in protocol::turns(state) {
        let turn_id = &turn["turnId"];
        let scope_turn = turn_id.to_string();
        collected.turns.insert(scope_turn.clone());
        let items = &turn["items"];
        if turn["itemsPagination"]["hasLoadedOldest"] != false
            && (items.is_array() || items["isComplete"] == true || items["items"].is_array())
        {
            collected.complete_turns.insert(scope_turn.clone());
        }
        let public_turn_id =
            if turn_id.is_string() || turn_id.as_i64().is_some() || turn_id.as_u64().is_some() {
                turn_id
            } else {
                &Value::Null
            };
        let items = protocol::ordered_items(&turn["items"]);
        let has_user = items.iter().any(|item| {
            matches!(
                item["type"].as_str(),
                Some("userMessage" | "steeringUserMessage")
            )
        });
        for (index, item) in items.into_iter().enumerate() {
            let mut result = HashMap::new();
            let kind = item["type"].as_str().unwrap_or_default();
            let mut public = protocol::item_view(item, &turn["turnId"], index);
            if matches!(
                kind,
                "agentMessage" | "assistantMessage" | "userMessage" | "steeringUserMessage"
            ) {
                let text = match kind {
                    "agentMessage" | "assistantMessage" => protocol::text_content(&item["text"]),
                    "userMessage" => protocol::text_content(&item["content"]),
                    _ => protocol::text_content(&item["input"]),
                };
                add_markdown(&mut result, &text);
                let content = item
                    .get("content")
                    .or(item.get("input"))
                    .unwrap_or(&Value::Null);
                add_parts(
                    &mut result,
                    &turn["turnId"],
                    &item["id"],
                    index,
                    content,
                    matches!(kind, "userMessage" | "steeringUserMessage"),
                );
            }
            if matches!(
                kind,
                "mcpToolCall"
                    | "dynamicToolCall"
                    | "collabAgentToolCall"
                    | "subAgentActivity"
                    | "reasoning"
            ) {
                for text in public["tool"]["output"]
                    .as_str()
                    .into_iter()
                    .chain(
                        (kind == "reasoning")
                            .then(|| public["text"].as_str())
                            .flatten(),
                    )
                    .chain(
                        public["collaboration"]["agents"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|agent| agent["message"].as_str()),
                    )
                {
                    add_markdown(&mut result, text);
                }
            }
            add_inline_images(&mut result, public_turn_id, &mut public);
            collected.values.extend(result.clone());
            if collected.values.len() > 4096 {
                return Err("会话文件引用过多，请在电脑端下载".into());
            }
            let identity = item
                .get("id")
                .filter(|id| id.is_string() || id.is_number())
                .cloned()
                .unwrap_or_else(|| serde_json::json!(["index", index]));
            let scope = (scope_turn.clone(), identity.to_string());
            collected.items.insert(scope.clone());
            if !result.is_empty() {
                collected.scopes.insert(scope, result);
            }
        }
        let input_scope = (scope_turn, "params".into());
        collected.items.insert(input_scope.clone());
        if !has_user {
            let mut result = HashMap::new();
            let text = protocol::text_content(&turn["params"]["input"]);
            add_markdown(&mut result, &text);
            add_parts(
                &mut result,
                &turn["turnId"],
                &Value::Null,
                usize::MAX,
                &turn["params"]["input"],
                true,
            );
            add_inline_images(
                &mut result,
                public_turn_id,
                &mut protocol::item_view(
                    &serde_json::json!({"type":"userMessage","content":turn["params"]["input"]}),
                    &turn["turnId"],
                    usize::MAX,
                ),
            );
            collected.values.extend(result.clone());
            if !result.is_empty() {
                collected.scopes.insert(input_scope, result);
            }
        }
        if collected.values.len() > 4096 {
            return Err("会话文件引用过多，请在电脑端下载".into());
        }
    }
    Ok(collected)
}

fn add_markdown(result: &mut HashMap<String, Reference>, text: &str) {
    let mut end = text.len().min(protocol::MAX_PUBLIC_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    for value in markdown_references(&text[..end]) {
        result.insert(
            value.clone(),
            Reference::File {
                value,
                native: false,
                markdown: true,
            },
        );
    }
}

fn add_inline_images(result: &mut HashMap<String, Reference>, turn: &Value, public: &mut Value) {
    let message_id = public["id"].clone();
    for (slot, attachment) in public["attachments"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(url) = attachment["url"].as_str() else {
            continue;
        };
        if super::inline_image_metadata(url).is_err() {
            continue;
        }
        let id = super::inline_image_reference_id(turn, &message_id, slot, url);
        let Some(name) = attachment["name"].as_str().map(str::to_string) else {
            continue;
        };
        if let Value::String(url) = attachment["url"].take() {
            result.insert(
                id,
                Reference::InlineImage {
                    url: Arc::new(url),
                    name,
                },
            );
        }
    }
}

fn add_parts(
    result: &mut HashMap<String, Reference>,
    turn: &Value,
    item: &Value,
    index: usize,
    content: &Value,
    native: bool,
) {
    for (part_index, part) in content.as_array().into_iter().flatten().enumerate() {
        if !matches!(part["type"].as_str(), Some("file" | "localImage" | "image")) {
            continue;
        }
        if super::public_image_url(&part["url"]).is_some() {
            continue;
        }
        let Some(value) = part["path"].as_str().or(part["url"].as_str()) else {
            continue;
        };
        if value.len() > 4096 {
            continue;
        }
        let id = super::reference_id(turn, item, index, part_index, value);
        result.insert(
            id,
            Reference::File {
                value: value.to_string(),
                native,
                markdown: false,
            },
        );
    }
}

// Candidates only. Every candidate must independently pass the bound workspace boundary.
fn markdown_references(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        rest = &rest[start + 2..];
        let body = rest.trim_start();
        if let Some(body) = body.strip_prefix('<') {
            if let Some(end) = body.find('>')
                && body[end + 1..].trim_start().starts_with(')')
            {
                result.push(body[..end].to_string());
            }
        } else {
            let mut depth = 0usize;
            let mut end = None;
            let mut escaped = false;
            for (index, ch) in body.char_indices() {
                if escaped && matches!(ch, '(' | ')' | '\\') {
                    escaped = false;
                    continue;
                }
                escaped = ch == '\\';
                if ch == '(' {
                    depth += 1;
                }
                if ch == ')' {
                    if depth == 0 {
                        end = Some(index);
                        break;
                    }
                    depth -= 1;
                }
                if ch.is_whitespace() && depth == 0 {
                    end = Some(index);
                    break;
                }
            }
            if let Some(end) = end.filter(|end| *end > 0 && *end <= 4096) {
                result.push(body[..end].to_string());
            }
        }
        if result.len() > 4096 {
            break;
        }
    }
    // CommonMark reference-style links are still only candidates, not an authorization.
    for line in text.lines() {
        let line = line.trim_start();
        if !line.starts_with('[') {
            continue;
        }
        if let Some((_, destination)) = line.split_once("]:") {
            let destination = destination.trim_start();
            let value = if let Some(value) = destination.strip_prefix('<') {
                value.split_once('>').map(|(value, _)| value)
            } else {
                destination.split_whitespace().next()
            };
            if let Some(value) = value.filter(|s| !s.is_empty() && s.len() <= 4096) {
                result.push(value.to_string());
            }
        }
        if result.len() > 4096 {
            break;
        }
    }
    result
}

pub(super) fn local_path(value: &str) -> Result<PathBuf, String> {
    parse_path(value, true)
}

pub(super) fn native_path(value: &str) -> Result<PathBuf, String> {
    parse_path(value, false)
}

fn parse_path(value: &str, markdown: bool) -> Result<PathBuf, String> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("文件引用无效".into());
    }
    let decoded;
    let value = if value.starts_with("file:") {
        let raw = percent_encoding::percent_decode_str(value)
            .decode_utf8()
            .map_err(|_| "文件引用编码无效")?;
        if raw.split(['/', '\\']).any(|part| part == "..") {
            return Err("文件引用不能包含路径穿越".into());
        }
        let url = reqwest::Url::parse(value).map_err(|_| "本地文件引用无效")?;
        if url.host_str().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("不支持网络文件地址".into());
        }
        decoded = url
            .to_file_path()
            .map_err(|_| "此文件路径没有本机映射，不支持下载")?
            .to_string_lossy()
            .into_owned();
        &decoded
    } else {
        decoded = if markdown {
            percent_encoding::percent_decode_str(value)
                .decode_utf8()
                .map_err(|_| "文件引用编码无效")?
                .into_owned()
        } else {
            value.to_string()
        };
        &decoded
    };
    let value = value
        .strip_prefix(r"\\?\")
        .filter(|path| {
            path.as_bytes().get(1) == Some(&b':')
                && path.as_bytes()[0].is_ascii_alphabetic()
                && path
                    .get(2..)
                    .is_some_and(|tail| tail.starts_with(['/', '\\']))
        })
        .unwrap_or(value);
    if value.starts_with(['\\']) || value.starts_with("//") || value.chars().any(char::is_control) {
        return Err("不支持网络或设备文件路径".into());
    }
    let drive = value.as_bytes().get(1) == Some(&b':') && value.as_bytes()[0].is_ascii_alphabetic();
    if (drive && !cfg!(windows))
        || (!drive && value.contains(':'))
        || (drive && value[2..].contains(':'))
    {
        return Err("此文件路径没有本机映射，不支持下载".into());
    }
    let value = if markdown {
        value.replace("\\(", "(").replace("\\)", ")")
    } else {
        value.to_string()
    };
    #[cfg(windows)]
    if value.starts_with('/') || (drive && !value[2..].starts_with(['/', '\\'])) {
        return Err("此文件路径没有本机映射，不支持下载".into());
    }
    let path = PathBuf::from(value);
    for component in path.components() {
        match component {
            Component::ParentDir => return Err("文件引用不能包含路径穿越".into()),
            Component::Normal(name) => {
                #[cfg(windows)]
                {
                    let name = name.to_string_lossy();
                    let base = name
                        .split('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase();
                    if name.ends_with(['.', ' '])
                        || matches!(
                            base.as_str(),
                            "CON"
                                | "PRN"
                                | "AUX"
                                | "NUL"
                                | "COM1"
                                | "COM2"
                                | "COM3"
                                | "COM4"
                                | "COM5"
                                | "COM6"
                                | "COM7"
                                | "COM8"
                                | "COM9"
                                | "LPT1"
                                | "LPT2"
                                | "LPT3"
                                | "LPT4"
                                | "LPT5"
                                | "LPT6"
                                | "LPT7"
                                | "LPT8"
                                | "LPT9"
                        )
                    {
                        return Err("不支持设备文件路径".into());
                    }
                }
                let _ = name;
            }
            _ => {}
        }
    }
    Ok(path)
}

fn comparable(path: &Path) -> String {
    let normalized: PathBuf = path.components().collect();
    let path = normalized.to_string_lossy().replace('\\', "/");
    let path = path
        .strip_prefix("//?/")
        .unwrap_or(&path)
        .trim_end_matches('/');
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_string()
    }
}

pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    comparable(left) == comparable(right)
}

pub(super) fn within(path: &Path, root: &Path) -> bool {
    let path = comparable(path);
    let root = comparable(root);
    path == root || path.starts_with(&format!("{root}/"))
}

pub(super) fn forbidden_file(path: &Path) -> bool {
    let value = comparable(path);
    if value
        .split('/')
        .any(|part| matches!(part, ".codex" | ".ssh" | ".gnupg"))
    {
        return true;
    }
    !cfg!(windows)
        && [
            "/etc", "/usr", "/bin", "/sbin", "/proc", "/sys", "/dev", "/run",
        ]
        .iter()
        .any(|system| value == *system || value.starts_with(&format!("{system}/")))
}

pub(super) fn forbidden_workspace(root: &Path) -> bool {
    let value = comparable(root);
    if value.is_empty()
        || root.parent().is_none()
        || root.components().count() <= if cfg!(windows) { 2 } else { 1 }
        || forbidden_file(root)
    {
        return true;
    }
    let parts: Vec<_> = value.split('/').filter(|s| !s.is_empty()).collect();
    if cfg!(windows) {
        if parts.len() == 2 && parts[1] == "users" || parts.len() == 3 && parts[1] == "users" {
            return true;
        }
    } else {
        if matches!(
            value.as_str(),
            "/home" | "/Users" | "/tmp" | "/var" | "/opt"
        ) || (parts.len() == 2 && matches!(parts[0], "home" | "Users"))
        {
            return true;
        }
        if value == "/root" || value.starts_with("/root/") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_links_are_candidates_and_network_traversal_or_unmapped_paths_never_reach_fs() {
        let text = "[文档](outputs/a.docx) [空格](<outputs/a b.docx>) [绝对](C:\\work\\a.docx) [引用][x]\n[x]: outputs/b.pdf";
        assert_eq!(
            markdown_references(text),
            [
                "outputs/a.docx",
                "outputs/a b.docx",
                "C:\\work\\a.docx",
                "outputs/b.pdf"
            ]
        );
        for value in [
            "https://evil.invalid/file",
            "//evil.invalid/share",
            "\\\\evil.invalid\\share",
            "../private",
            "a/%2e%2e/private",
            "file://evil.invalid/share",
            "sandbox:/mnt/data/a.docx",
            "ssh://host/a",
        ] {
            assert!(local_path(value).is_err(), "{value}");
        }
        assert!(forbidden_workspace(Path::new(if cfg!(windows) {
            "C:/"
        } else {
            "/"
        })));
        assert!(forbidden_workspace(Path::new(if cfg!(windows) {
            "C:/Users/user"
        } else {
            "/home/user"
        })));
        assert!(
            local_path("outputs/a%20b.docx")
                .unwrap()
                .ends_with("a b.docx")
        );
    }
}
