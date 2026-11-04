// Adapted from fraternity-z/CodeyX (AGPL-3.0-only).
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{CodeyConfig, ProviderProfile};

pub(crate) mod webdav;

pub(crate) const MAX_BACKUP_BYTES: usize = 4 * 1024 * 1024;
const FORMAT: &str = "codey-config";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Backup {
    format: String,
    version: u32,
    created_at: String,
    config: CodeyConfig,
}

pub(crate) fn export(config: CodeyConfig) -> Result<String, String> {
    let content = serde_json::to_string_pretty(&Backup {
        format: FORMAT.into(),
        version: VERSION,
        created_at: chrono::Utc::now().to_rfc3339(),
        config,
    })
    .map_err(|_| "生成配置备份失败")?;
    check_size(content.len())?;
    Ok(content)
}

pub(crate) fn check_size(size: usize) -> Result<(), String> {
    if size == 0 || size > MAX_BACKUP_BYTES {
        return Err("备份文件必须为非空 JSON，且不超过 4 MiB".into());
    }
    Ok(())
}

pub(crate) fn save_recovery(config_path: &Path, config: CodeyConfig) -> Result<PathBuf, String> {
    let path = config_path.with_file_name(format!("before-restore-{}.json", uuid::Uuid::new_v4()));
    let content = export(config)?;
    crate::fs_util::atomic_write_private_with_parent(&path, content.as_bytes())
        .map_err(|_| "无法保存恢复前备份，原配置未更改")?;
    Ok(path)
}

pub(crate) fn parse(content: &str) -> Result<Backup, String> {
    check_size(content.len())?;
    let value: Value = serde_json::from_str(content.trim_start_matches('\u{feff}'))
        .map_err(|_| "备份不是有效的 JSON 文件")?;
    let config = value
        .get("config")
        .and_then(Value::as_object)
        .ok_or("缺少备份配置")?;
    for key in [
        "profiles",
        "activeProfileId",
        "localRouterEnabled",
        "subagentRoles",
        "webhook",
    ] {
        if !config.contains_key(key) {
            return Err("备份配置不完整，请选择 Codey 导出的 JSON 文件".into());
        }
    }
    let backup: Backup =
        serde_json::from_value(value).map_err(|_| "备份字段类型无效，请检查备份文件")?;
    if backup.format != FORMAT || backup.version != VERSION {
        return Err("不支持的备份格式或版本，请使用兼容版本的 Codey".into());
    }
    chrono::DateTime::parse_from_rfc3339(&backup.created_at).map_err(|_| "备份时间无效")?;
    validate(&backup.config)?;
    Ok(backup)
}

fn validate(config: &CodeyConfig) -> Result<(), String> {
    // 配置校验器可能引用输入字段；备份错误只返回阶段，避免回显凭据。
    crate::config::validate_provider_profiles(&config.profiles)
        .map_err(|_| "备份中的线路配置无效")?;
    if !crate::config::SUBAGENT_REASONING_EFFORTS
        .contains(&config.subagent_reasoning_effort.as_str())
        || crate::config::validate_subagent_roles(&config.subagent_roles).is_err()
    {
        return Err("备份中的子代理配置无效".into());
    }
    config
        .webhook
        .validate()
        .map_err(|_| "备份中的通知配置无效")?;
    config
        .prompt_optimization
        .validate()
        .map_err(|_| "备份中的提示词优化配置无效")?;
    for policies in config.model_context_by_provider.values() {
        for policy in policies.values() {
            policy
                .validate()
                .map_err(|_| "备份中的模型上下文配置无效")?;
        }
    }
    for models in config
        .model_reasoning_efforts_by_provider
        .values()
        .chain(config.upstream_model_reasoning_efforts_by_provider.values())
    {
        for efforts in models.values() {
            let mut levels = BTreeSet::new();
            if efforts.is_empty()
                || efforts.len() > crate::config::MAX_MODEL_REASONING_EFFORTS
                || efforts.iter().any(|effort| {
                    !crate::config::MODEL_REASONING_EFFORT_LEVELS.contains(&effort.level.as_str())
                        || effort.value.trim().is_empty()
                        || effort.value.len()
                            > crate::config::MAX_MODEL_REASONING_EFFORT_VALUE_BYTES
                        || !levels.insert(&effort.level)
                })
            {
                return Err("备份中的模型思考强度配置无效".into());
            }
        }
    }
    Ok(())
}

fn managed(profile: &ProviderProfile) -> bool {
    profile.official_account
        || profile.official_account_id.is_some()
        || profile.auth_mode == crate::config::AUTH_MODE_OFFICIAL_ACCOUNT
        || profile.plugin_owner_id.is_some()
        || profile.plugin_route_spec.is_some()
}

fn preserve_entry<T: Clone>(
    next: &mut BTreeMap<String, T>,
    previous: &BTreeMap<String, T>,
    key: &str,
) {
    next.remove(key);
    if let Some(value) = previous.get(key) {
        next.insert(key.to_string(), value.clone());
    }
}

impl Backup {
    pub(crate) fn summary(&self, revision: u64) -> Value {
        json!({ "createdAt": self.created_at, "routeCount": self.config.profiles.iter().filter(|p| !managed(p)).count(),
            "channelCount": self.config.webhook.channels.len(), "settingsRevision": revision,
            "containsSecrets": true })
    }

    pub(crate) fn prepare(self, previous: &CodeyConfig) -> Result<CodeyConfig, String> {
        let mut next = self.config;
        // 账号、插件和系统密钥库由各自存储管理，JSON 不能授予其本机身份。
        next.profiles.retain(|profile| !managed(profile));
        if previous.profiles.iter().any(managed) {
            next.profiles
                .retain(|profile| !profile.is_unconfigured_default());
        }
        for profile in previous.profiles.iter().filter(|profile| managed(profile)) {
            if next.profiles.iter().any(|incoming| {
                incoming.id == profile.id || incoming.provider_id() == profile.provider_id()
            }) {
                return Err("备份线路与本机账号或插件线路冲突，请先调整线路标识".into());
            }
            let key = profile.provider_id();
            preserve_entry(
                &mut next.selected_models_by_provider,
                &previous.selected_models_by_provider,
                key,
            );
            preserve_entry(
                &mut next.model_context_by_provider,
                &previous.model_context_by_provider,
                key,
            );
            preserve_entry(
                &mut next.model_reasoning_efforts_by_provider,
                &previous.model_reasoning_efforts_by_provider,
                key,
            );
            preserve_entry(
                &mut next.upstream_model_reasoning_efforts_by_provider,
                &previous.upstream_model_reasoning_efforts_by_provider,
                key,
            );
            preserve_entry(
                &mut next.manual_third_party_models_by_provider,
                &previous.manual_third_party_models_by_provider,
                key,
            );
            preserve_entry(
                &mut next.declared_official_models_by_provider,
                &previous.declared_official_models_by_provider,
                key,
            );
            preserve_entry(
                &mut next.upstream_models_by_provider,
                &previous.upstream_models_by_provider,
                key,
            );
            next.profiles.push(profile.clone());
        }
        next.model_alias_history
            .extend(previous.model_alias_history.clone());
        next.codex_app_path.clone_from(&previous.codex_app_path);
        next.user_scripts.clone_from(&previous.user_scripts);
        next.codex_launch_proxy
            .clone_from(&previous.codex_launch_proxy);
        next.gpu_launch_mode = previous.gpu_launch_mode;
        next.official_account_available_this_launch =
            previous.official_account_available_this_launch;
        next.official_account_status_this_launch = previous.official_account_status_this_launch;
        next.settings_revision = previous.settings_revision;
        let next = next.normalize();
        validate(&next)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests;
