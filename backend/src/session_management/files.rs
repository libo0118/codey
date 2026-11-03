use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::{SessionEntry, entry, valid_id};

pub(super) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("检查会话存储失败：{}", path.display())),
    }
}

pub(super) fn checked(home: &Path, path: &Path) -> Result<()> {
    let root = home.canonicalize().context("无法定位 Codex 数据目录")?;
    let relative = path
        .strip_prefix(home)
        .context("会话存储不在 Codex 数据目录内")?;
    let mut current = home.to_path_buf();
    for component in relative.components() {
        ensure!(
            matches!(component, std::path::Component::Normal(_)),
            "会话存储路径无效"
        );
        current.push(component);
        let metadata = fs::symlink_metadata(&current)?;
        let link = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let link = {
            use std::os::windows::fs::MetadataExt;
            link || metadata.file_attributes() & 0x400 != 0
        };
        ensure!(!link, "会话存储包含链接，已拒绝操作：{}", current.display());
    }
    ensure!(
        path.canonicalize()?.starts_with(root),
        "会话存储越出 Codex 数据目录"
    );
    Ok(())
}

fn walk(home: &Path, directory: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    if !exists(directory)? {
        return Ok(result);
    }
    let mut stack = vec![directory.to_path_buf()];
    while let Some(path) = stack.pop() {
        checked(home, &path)?;
        if path.is_dir() {
            for item in fs::read_dir(&path)? {
                stack.push(item?.path());
            }
        } else {
            result.push(path);
        }
    }
    result.sort();
    Ok(result)
}

fn filename_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let candidate = stem.get(stem.len().checked_sub(36)?..)?;
    if !valid_id(candidate) {
        return None;
    }
    // A "_<uuid>" suffix is not the session ID when a valid ID precedes it.
    let original = stem
        .strip_suffix(candidate)
        .and_then(|prefix| prefix.strip_suffix('_'))
        .and_then(|prefix| prefix.get(prefix.len().checked_sub(36)?..))
        .filter(|id| valid_id(id));
    Some(original.unwrap_or(candidate).to_owned())
}

fn rollout_metadata(path: &Path) -> Result<Value> {
    let mut first = String::new();
    BufReader::new(fs::File::open(path)?.take(1024 * 1024)).read_line(&mut first)?;
    let value: Value = serde_json::from_str(&first).context("无法识别会话文件元数据")?;
    ensure!(value["type"] == "session_meta", "会话文件缺少元数据");
    Ok(value["payload"].clone())
}

fn rollout_identity(path: &Path) -> Result<(String, Value)> {
    let filename = filename_id(path);
    let metadata = match rollout_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) if filename.is_some() => Value::Null,
        Err(error) => return Err(error),
    };
    let payload = metadata["id"].as_str();
    if let (Some(filename), Some(payload)) = (&filename, payload) {
        ensure!(
            filename == payload,
            "会话文件名与元数据标识不一致，已停止操作"
        );
    }
    let id = payload
        .map(str::to_owned)
        .or(filename)
        .context("会话缺少标识")?;
    Ok((id, metadata))
}

pub(super) fn inventory(home: &Path, entries: &mut BTreeMap<String, SessionEntry>) -> Result<()> {
    if !exists(home)? {
        return Ok(());
    }
    for folder in ["sessions", "archived_sessions"] {
        for path in walk(home, &home.join(folder))? {
            if path.extension().and_then(|v| v.to_str()) != Some("jsonl") {
                continue;
            }
            let (id, metadata) = rollout_identity(&path)
                .with_context(|| format!("读取会话 {} 失败", path.display()))?;
            let value = entry(entries, &id)?;
            value.cwd = metadata["cwd"].as_str().unwrap_or_default().into();
            value.archived |= folder == "archived_sessions";
            let file = fs::metadata(&path)?;
            value.bytes += file.len();
            value.updated_at = value.updated_at.max(
                file.modified()?
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            );
        }
    }
    let mut indexes = vec![home.join("session_index.jsonl"), home.join("history.jsonl")];
    indexes.extend(
        walk(home, &home.join("backups_state/provider-sync"))?
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .is_some_and(|value| value == "session_index.jsonl")
            }),
    );
    for index in indexes.into_iter().filter(|path| path.exists()) {
        checked(home, &index)?;
        for line in BufReader::new(fs::File::open(index)?).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(&line).context("会话索引损坏，请先修复后再清理")?;
            if let Some(id) = value["id"]
                .as_str()
                .or(value["session_id"].as_str())
                .filter(|id| valid_id(id))
            {
                let current = entry(entries, id)?;
                if let Some(title) = value["thread_name"].as_str() {
                    current.title = title.into();
                }
            }
        }
    }
    for path in walk(home, &home.join("shell_snapshots"))? {
        if let Some(id) = filename_id(&path) {
            entry(entries, &id)?;
        }
    }
    Ok(())
}

fn matches_id(value: &str, ids: &BTreeSet<String>) -> bool {
    ids.contains(value)
        || value
            .strip_prefix("local:")
            .is_some_and(|id| ids.contains(id))
}

fn refers_to_selected(value: &Value, ids: &BTreeSet<String>) -> bool {
    match value {
        Value::String(value) => matches_id(value, ids),
        Value::Object(object) => ["id", "thread_id", "session_id", "threadId", "sessionId"]
            .iter()
            .any(|key| {
                object
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|id| matches_id(id, ids))
            }),
        _ => false,
    }
}

fn prune(value: &mut Value, ids: &BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            object.retain(|key, value| !matches_id(key, ids) && !refers_to_selected(value, ids));
            for value in object.values_mut() {
                prune(value, ids);
            }
        }
        Value::Array(items) => {
            items.retain(|value| !refers_to_selected(value, ids));
            for value in items {
                prune(value, ids);
            }
        }
        _ => {}
    }
}

fn rewrite_json(home: &Path, path: &Path, ids: &BTreeSet<String>) -> Result<()> {
    checked(home, path)?;
    ensure!(
        fs::metadata(path)?.len() <= 32 * 1024 * 1024,
        "界面会话缓存过大，已停止清理"
    );
    let original = fs::read(path)?;
    let mut value: Value = serde_json::from_slice(&original).context("界面会话缓存格式不受支持")?;
    prune(&mut value, ids);
    let next = serde_json::to_vec(&value)?;
    if next != original {
        crate::fs_util::atomic_write_private(path, &next)?;
    }
    Ok(())
}

fn filter_jsonl(
    home: &Path,
    path: &Path,
    ids: &BTreeSet<String>,
    persist_index: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    if !exists(path)? {
        return Ok(());
    }
    checked(home, path)?;
    let before = fs::metadata(path)?;
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().context("会话索引目录无效")?)?;
    let mut line = Vec::new();
    let mut changed = false;
    loop {
        line.clear();
        let count = Read::by_ref(&mut reader)
            .take(16 * 1024 * 1024)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            break;
        }
        ensure!(count < 16 * 1024 * 1024, "会话索引单条记录过大");
        if line.iter().all(u8::is_ascii_whitespace) {
            temporary.write_all(&line)?;
            continue;
        }
        let value: Value =
            serde_json::from_slice(&line).context("会话索引或输入历史格式不受支持")?;
        if refers_to_selected(&value, ids) {
            changed = true;
        } else {
            temporary.write_all(&line)?;
        }
    }
    if changed {
        // Windows 替换前释放读写句柄，避免自身句柄阻止原子改名。
        drop(reader);
        let after = fs::metadata(path)?;
        ensure!(
            before.len() == after.len() && before.modified()? == after.modified()?,
            "会话索引正在写入，将在重启时继续清理"
        );
        temporary.as_file().sync_all()?;
        checked(home, path)?;
        let temporary = temporary.into_temp_path();
        persist_index(&temporary, path)
            .with_context(|| format!("原子替换会话索引失败：{}", path.display()))?;
    }
    Ok(())
}

pub(super) fn clean(home: &Path, ids: &BTreeSet<String>, stopped: bool) -> Result<()> {
    clean_with(home, ids, stopped, crate::fs_util::persist_temp_file)
}

pub(super) fn clean_with(
    home: &Path,
    ids: &BTreeSet<String>,
    stopped: bool,
    mut persist_index: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    for folder in ["sessions", "archived_sessions", "shell_snapshots"] {
        for path in walk(home, &home.join(folder))? {
            let id = if folder != "shell_snapshots"
                && path.extension().and_then(|v| v.to_str()) == Some("jsonl")
            {
                Some(rollout_identity(&path)?.0)
            } else {
                filename_id(&path)
            };
            let selected = id.is_some_and(|id| ids.contains(&id));
            if selected {
                checked(home, &path)?;
                fs::remove_file(&path)
                    .with_context(|| format!("删除会话文件失败：{}", path.display()))?;
            }
        }
    }
    for name in ["session_index.jsonl", "history.jsonl"] {
        filter_jsonl(home, &home.join(name), ids, &mut persist_index)?;
    }
    for path in walk(home, &home.join("backups_state/provider-sync"))? {
        if path.file_name().is_some_and(|v| v == "session_index.jsonl") {
            filter_jsonl(home, &path, ids, &mut persist_index)?;
        }
    }
    // 兼容旧版单轮删除留下的记录，整段会话清理时一并移除。
    for path in walk(home, &home.join(".codey-message-delete-tombstones-v1"))? {
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let value: Value = serde_json::from_slice(&fs::read(&path)?)?;
        if refers_to_selected(&value, ids) {
            checked(home, &path)?;
            fs::remove_file(path)?;
        }
    }
    if stopped {
        for name in [".codex-global-state.json", ".codex-global-state.json.bak"] {
            let path = home.join(name);
            if exists(&path)? {
                rewrite_json(home, &path, ids)?;
            }
        }
        // 这些文件是全局状态原子写入遗留的临时副本，不能再恢复已删除会话。
        for item in fs::read_dir(home)? {
            let path = item?.path();
            if path
                .file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|v| v.starts_with("..codex-global-state.json.tmp-"))
            {
                checked(home, &path)?;
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_identity_handles_session_suffixes_and_imported_names() {
        let id = "11111111-1111-4111-8111-111111111111";
        let random = "22222222-2222-4222-8222-222222222222";
        for name in [
            format!("rollout-2026-10-18-{id}.jsonl"),
            format!("rollout-2026-10-18T16-20-49-{id}.jsonl"),
            format!("rollout-123-{random}-{id}.jsonl"),
            format!("rollout-2026-10-18T16-20-49-{id}_{random}.jsonl"),
            format!("rollout-{random}_copy-{id}.jsonl"),
            format!("{}_{id}.jsonl", "x".repeat(36)),
            format!("é{}_{id}.jsonl", "x".repeat(35)),
            format!("导入-{id}.jsonl"),
            format!("导入_{id}.jsonl"),
            format!("{id}.ps1"),
            id.to_owned(),
        ] {
            assert_eq!(filename_id(Path::new(&name)).as_deref(), Some(id), "{name}");
        }
        for name in [
            "short.jsonl".to_owned(),
            format!("{id}-invalid.jsonl"),
            format!("{id}_invalid.jsonl"),
            format!("{id}_{random}-invalid.jsonl"),
            format!("é{}.jsonl", "x".repeat(35)),
            format!("{}.jsonl", "x".repeat(36)),
        ] {
            assert_eq!(filename_id(Path::new(&name)), None, "{name}");
        }
    }
}
