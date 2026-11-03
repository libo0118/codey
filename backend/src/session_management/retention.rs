use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    AutomaticCleanup, AutomaticCleanupPhase, CleanupResult, OPERATION, files, lock, scan, schedule,
};

const POLICY: &str = ".codey-session-retention-v1.json";
const DAY_MS: i64 = 86_400_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CleanupPolicy {
    enabled: bool,
    retention_days: u32,
}

impl Default for CleanupPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            retention_days: 30,
        }
    }
}

impl CleanupPolicy {
    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=36_500).contains(&self.retention_days),
            "会话保留天数必须为 1 到 36500 的整数"
        );
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SavePolicyRequest {
    policy: CleanupPolicy,
    revision: String,
    confirmed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavePolicyResult {
    policy: CleanupPolicy,
    revision: String,
    cleanup: Option<CleanupResult>,
    cleanup_error: Option<String>,
}

pub(super) fn read_policy(home: &Path) -> Result<CleanupPolicy> {
    let path = home.join(POLICY);
    if !files::exists(&path)? {
        return Ok(CleanupPolicy::default());
    }
    files::checked(home, &path)?;
    let policy: CleanupPolicy =
        serde_json::from_slice(&crate::fs_util::read_bounded(&path, 16 * 1024)?)
            .context("会话自动清理设置损坏，已停止自动清理")?;
    policy.validate()?;
    Ok(policy)
}

pub(super) fn policy_revision(policy: &CleanupPolicy) -> Result<String> {
    Ok(crate::fs_util::sha256_hex(&serde_json::to_vec(policy)?))
}

pub(crate) fn save_policy(
    home: &Path,
    request: SavePolicyRequest,
    now_ms: i64,
) -> Result<SavePolicyResult> {
    request.policy.validate()?;
    ensure!(
        !request.policy.enabled || request.confirmed,
        "请先确认自动永久清理超过保留期限的会话"
    );
    let _guard = OPERATION
        .lock()
        .map_err(|_| anyhow::anyhow!("会话管理锁不可用"))?;
    fs::create_dir_all(home).context("创建会话自动清理设置目录失败")?;
    let _file = lock(home)?;
    let previous = read_policy(home)?;
    ensure!(
        request.revision == policy_revision(&previous)?,
        "会话自动清理设置已变化，请刷新后重新保存"
    );
    let path = home.join(POLICY);
    if files::exists(&path)? {
        files::checked(home, &path)?;
    }
    crate::fs_util::atomic_write_private(&path, &serde_json::to_vec(&request.policy)?)
        .context("保存会话自动清理设置失败")?;
    let revision = policy_revision(&request.policy)?;
    // 设置已落盘；清理失败必须与保存失败区分，避免重复提交副作用。
    let (cleanup, cleanup_error) = match cleanup_expired(home, &request.policy, now_ms) {
        Ok(result) => (result, None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    Ok(SavePolicyResult {
        policy: request.policy,
        revision,
        cleanup,
        cleanup_error,
    })
}

pub(crate) fn automatic_cleanup(home: &Path, now_ms: i64) -> Result<Option<CleanupResult>> {
    let _guard = OPERATION
        .lock()
        .map_err(|_| anyhow::anyhow!("会话管理锁不可用"))?;
    if !read_policy(home)?.enabled {
        return Ok(None);
    }
    let _file = lock(home)?;
    cleanup_expired(home, &read_policy(home)?, now_ms)
}

fn cleanup_expired(
    home: &Path,
    policy: &CleanupPolicy,
    now_ms: i64,
) -> Result<Option<CleanupResult>> {
    if !policy.enabled {
        return Ok(None);
    }
    let revision = policy_revision(policy)?;
    let automatic = scan(home)?
        .sessions
        .into_iter()
        .filter(|entry| eligible(policy, entry.updated_at, now_ms))
        .map(|entry| {
            (
                entry.id,
                AutomaticCleanup {
                    updated_at: entry.updated_at,
                    policy_revision: revision.clone(),
                    phase: AutomaticCleanupPhase::Queued,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    if automatic.is_empty() {
        return Ok(None);
    }
    ensure!(automatic.len() <= 100_000, "超过单次会话自动清理数量上限");
    schedule(home, &BTreeSet::new(), automatic, &BTreeSet::new()).map(Some)
}

pub(super) fn eligible(policy: &CleanupPolicy, updated_at: i64, now_ms: i64) -> bool {
    policy.enabled
        && updated_at > 0
        && updated_at < now_ms.saturating_sub(i64::from(policy.retention_days) * DAY_MS)
}

#[cfg(test)]
mod tests;
