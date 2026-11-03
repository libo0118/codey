// Adapted from fraternity-z/CodeyX (AGPL-3.0-only).
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

mod database;
mod files;
mod retention;
#[cfg(test)]
mod tests;

pub(crate) use retention::{SavePolicyRequest, automatic_cleanup, save_policy};

const PENDING: &str = ".codey-session-cleanup-v1.json";
const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;
static OPERATION: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionEntry {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub updated_at: i64,
    pub archived: bool,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Inventory {
    sessions: Vec<SessionEntry>,
    revision: String,
    pending_count: usize,
    legacy_pending_ids: Vec<String>,
    cleanup_policy: retention::CleanupPolicy,
    cleanup_policy_revision: String,
}

struct SessionScan {
    sessions: Vec<SessionEntry>,
    revision: String,
    pending_count: usize,
    legacy_pending_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DeleteRequest {
    ids: Vec<String>,
    revision: String,
    confirmed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CleanupResult {
    selected_count: usize,
    scheduled_count: usize,
    cleared_legacy_count: usize,
    restart_required: bool,
    pub(crate) errors: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    version: u32,
    ids: BTreeSet<String>,
    #[serde(default)]
    automatic: BTreeMap<String, AutomaticCleanup>,
    #[serde(default)]
    legacy_ids: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AutomaticCleanup {
    updated_at: i64,
    policy_revision: String,
    #[serde(default)]
    phase: AutomaticCleanupPhase,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AutomaticCleanupPhase {
    #[default]
    Queued,
    Started,
}

impl Pending {
    fn executable_count(&self) -> usize {
        self.ids.len() + self.automatic.len()
    }

    fn count(&self) -> usize {
        self.executable_count() + self.legacy_ids.len()
    }
}

fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}

fn read_pending(home: &Path) -> Result<Pending> {
    let path = home.join(PENDING);
    if !files::exists(&path)? {
        return Ok(Pending {
            version: 2,
            ..Pending::default()
        });
    }
    files::checked(home, &path)?;
    let value: serde_json::Value =
        serde_json::from_slice(&crate::fs_util::read_bounded(&path, MAX_PENDING_BYTES)?)
            .context("会话清理任务文件损坏，已停止清理")?;
    let pending: Pending = match value["version"].as_u64() {
        Some(1) => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct LegacyPending {
                version: u32,
                ids: BTreeSet<String>,
            }
            let legacy: LegacyPending =
                serde_json::from_value(value).context("会话清理任务格式不受支持")?;
            ensure!(legacy.version == 1, "会话清理任务格式不受支持");
            // v1 不记录来源，不能将旧自动任务升级为已确认的手动删除。
            Pending {
                version: 2,
                legacy_ids: legacy.ids,
                ..Pending::default()
            }
        }
        Some(2) => serde_json::from_value(value).context("会话清理任务格式不受支持")?,
        _ => anyhow::bail!("会话清理任务格式不受支持，任务与会话数据已保留"),
    };
    ensure!(
        pending.version == 2
            && pending.count() <= 100_000
            && pending
                .ids
                .iter()
                .chain(pending.legacy_ids.iter())
                .all(|id| valid_id(id))
            && pending.ids.is_disjoint(&pending.legacy_ids)
            && pending.automatic.iter().all(|(id, intent)| valid_id(id)
                && intent.updated_at > 0
                && intent.policy_revision.len() == 64
                && intent
                    .policy_revision
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                && !pending.ids.contains(id)
                && !pending.legacy_ids.contains(id)),
        "会话清理任务格式不受支持"
    );
    Ok(pending)
}

fn scan(home: &Path) -> Result<SessionScan> {
    let mut entries = BTreeMap::new();
    files::inventory(home, &mut entries)?;
    database::inventory(home, &mut entries)?;
    let pending = read_pending(home)?;
    // 正常写入不改变清理范围；会话范围或完整待清理状态变化需要重新确认。
    let scope = entries
        .iter()
        .map(|(id, entry)| (id, entry.archived))
        .collect::<Vec<_>>();
    let revision = crate::fs_util::sha256_hex(&serde_json::to_vec(&(&scope, &pending))?);
    let mut sessions = entries.into_values().collect::<Vec<_>>();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
    Ok(SessionScan {
        sessions,
        revision,
        pending_count: pending.executable_count(),
        legacy_pending_ids: pending.legacy_ids.into_iter().collect(),
    })
}

pub(crate) fn list(home: &Path) -> Result<Inventory> {
    let _guard = OPERATION
        .lock()
        .map_err(|_| anyhow::anyhow!("会话管理锁不可用"))?;
    let scanned = scan(home)?;
    let cleanup_policy = retention::read_policy(home)?;
    let cleanup_policy_revision = retention::policy_revision(&cleanup_policy)?;
    Ok(Inventory {
        sessions: scanned.sessions,
        revision: scanned.revision,
        pending_count: scanned.pending_count,
        legacy_pending_ids: scanned.legacy_pending_ids,
        cleanup_policy,
        cleanup_policy_revision,
    })
}

fn lock(home: &Path) -> Result<fs::File> {
    let path = home.join(".codey-session-cleanup.lock");
    if files::exists(&path)? {
        files::checked(home, &path)?;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    file.try_lock_exclusive()
        .context("其他会话清理操作正在执行，请稍后刷新")?;
    Ok(file)
}

pub(crate) fn remove(home: &Path, request: DeleteRequest) -> Result<CleanupResult> {
    ensure!(request.confirmed, "请先确认永久清理会话");
    ensure!(
        !request.ids.is_empty() && request.ids.len() <= 100_000,
        "请选择要清理的会话"
    );
    ensure!(request.ids.iter().all(|id| valid_id(id)), "会话标识无效");
    let _guard = OPERATION
        .lock()
        .map_err(|_| anyhow::anyhow!("会话管理锁不可用"))?;
    let _file = lock(home)?;
    let inventory = scan(home)?;
    ensure!(
        request.revision == inventory.revision,
        "会话列表已变化，请刷新并重新确认清理范围"
    );
    let known = inventory
        .sessions
        .iter()
        .map(|entry| &entry.id)
        .collect::<BTreeSet<_>>();
    let legacy = inventory.legacy_pending_ids.iter().collect::<BTreeSet<_>>();
    ensure!(
        request
            .ids
            .iter()
            .all(|id| known.contains(id) || legacy.contains(id)),
        "所选会话已变化，请刷新列表"
    );
    let ids = request.ids.into_iter().collect::<BTreeSet<_>>();
    let (scheduled, cleared_legacy) = ids.into_iter().partition(|id| known.contains(id));
    remove_selected(home, &scheduled, &cleared_legacy)
}

fn remove_selected(
    home: &Path,
    ids: &BTreeSet<String>,
    cleared_legacy: &BTreeSet<String>,
) -> Result<CleanupResult> {
    schedule(home, ids, BTreeMap::new(), cleared_legacy)
}

fn schedule(
    home: &Path,
    ids: &BTreeSet<String>,
    automatic: BTreeMap<String, AutomaticCleanup>,
    cleared_legacy: &BTreeSet<String>,
) -> Result<CleanupResult> {
    // 先固定删除集合；重启时绝不把新建会话追加到“全部”中。
    let mut pending = read_pending(home)?;
    let selected_count = ids.len() + automatic.len() + cleared_legacy.len();
    let mut scheduled_count = ids.len();
    for id in cleared_legacy {
        pending.legacy_ids.remove(id);
    }
    pending.ids.extend(ids.iter().cloned());
    for id in ids {
        pending.automatic.remove(id);
        pending.legacy_ids.remove(id);
    }
    for (id, intent) in automatic {
        if !pending.ids.contains(&id) && !pending.legacy_ids.contains(&id) {
            pending.automatic.entry(id).or_insert(intent);
            scheduled_count += 1;
        }
    }
    ensure!(pending.count() <= 100_000, "超过待清理会话数量上限");
    if scheduled_count > 0 || !cleared_legacy.is_empty() {
        write_pending(home, &pending)?;
    }
    // 活跃 Codex 仍可能写入这些文件；停止旧进程后的启动维护才执行删除。
    Ok(CleanupResult {
        selected_count,
        scheduled_count,
        cleared_legacy_count: cleared_legacy.len(),
        restart_required: pending.executable_count() > 0,
        errors: Vec::new(),
    })
}

fn write_pending(home: &Path, pending: &Pending) -> Result<()> {
    let pending_path = home.join(PENDING);
    if files::exists(&pending_path)? {
        files::checked(home, &pending_path)?;
    }
    let bytes = serde_json::to_vec(pending)?;
    ensure!(
        bytes.len() <= MAX_PENDING_BYTES,
        "会话清理任务超过安全大小上限"
    );
    crate::fs_util::atomic_write_private(&pending_path, &bytes)
}

/// 仅由已停止旧 Codex 写入进程的启动维护调用。
pub(crate) fn finish_pending(home: &Path) -> Result<()> {
    finish_pending_at(home, chrono::Utc::now().timestamp_millis())
}

fn finish_pending_at(home: &Path, now_ms: i64) -> Result<()> {
    finish_pending_with(home, now_ms, write_pending, files::clean)
}

fn finish_pending_with(
    home: &Path,
    now_ms: i64,
    mut save_pending: impl FnMut(&Path, &Pending) -> Result<()>,
    clean_files: impl FnOnce(&Path, &BTreeSet<String>, bool) -> Result<()>,
) -> Result<()> {
    let _guard = OPERATION
        .lock()
        .map_err(|_| anyhow::anyhow!("会话管理锁不可用"))?;
    if !files::exists(&home.join(PENDING))? {
        return Ok(());
    }
    let _file = lock(home)?;
    let mut pending = read_pending(home)?;
    if !pending.automatic.is_empty() {
        let policy = retention::read_policy(home)?;
        let revision = retention::policy_revision(&policy)?;
        let sessions = scan(home)?
            .sessions
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect::<BTreeMap<_, _>>();
        pending.automatic.retain(|id, intent| {
            let unchanged = sessions
                .get(id)
                .is_none_or(|entry| entry.updated_at <= intent.updated_at);
            // 已开始的删除须完成残留清理，但任何新活动都会撤销自动意图。
            let authorized = unchanged
                && (intent.phase == AutomaticCleanupPhase::Started
                    || (intent.policy_revision == revision
                        && sessions.get(id).is_some_and(|entry| {
                            retention::eligible(&policy, entry.updated_at, now_ms)
                        })));
            if authorized {
                intent.phase = AutomaticCleanupPhase::Started;
            }
            authorized
        });
        // 删除前固定开始状态与取消项；保存失败不得触碰会话数据。
        save_pending(home, &pending)?;
    }
    let mut ids = pending.ids.clone();
    ids.extend(pending.automatic.keys().cloned());
    if !ids.is_empty() {
        // 文件失败时保留数据库元数据，避免丢失后续重试的活动依据。
        clean_files(home, &ids, true).context("会话清理尚未完成")?;
        database::clean(home, &ids, true).context("会话清理尚未完成")?;
    }
    // 验证所有可枚举存储都不再包含目标，然后才移除重试任务。
    let remaining = scan(home)?;
    ensure!(
        !remaining
            .sessions
            .iter()
            .any(|entry| ids.contains(&entry.id)),
        "仍存在待清理会话，已保留清理任务"
    );
    if !pending.legacy_ids.is_empty() {
        if !ids.is_empty() {
            pending.ids.clear();
            pending.automatic.clear();
            save_pending(home, &pending)?;
        }
        anyhow::bail!(
            "旧版清理任务无法确认手动或自动来源，请在本机会话管理刷新并重新确认；未确认数据已保留"
        );
    }
    fs::remove_file(home.join(PENDING)).context("移除已完成的会话清理任务失败")?;
    Ok(())
}

fn entry<'a>(
    entries: &'a mut BTreeMap<String, SessionEntry>,
    id: &str,
) -> Result<&'a mut SessionEntry> {
    ensure!(valid_id(id), "发现不受支持的本地会话标识，已停止扫描");
    Ok(entries
        .entry(id.to_owned())
        .or_insert_with(|| SessionEntry {
            id: id.to_owned(),
            title: "未命名会话".into(),
            ..SessionEntry::default()
        }))
}
