// Adapted from fraternity-z/CodeyX (AGPL-3.0-only).
use std::collections::BTreeMap;

use super::{SUBAGENT_REASONING_EFFORTS, SubagentRoleConfig, owned_subagent_role_ids};

pub(crate) const MAX_CUSTOM_SUBAGENT_ROLES: usize = 32;

pub(crate) fn validate_subagent_max_threads(limit: u32) -> Result<(), String> {
    if !(1..=64).contains(&limit) {
        return Err("最大子代理数量必须是 1 到 64 之间的整数".into());
    }
    Ok(())
}

pub(crate) fn valid_subagent_role_id(role: &str) -> bool {
    !role.is_empty()
        && role.len() <= 64
        && role
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !matches!(
            role,
            "explorer"
                | "worker"
                | "awaiter"
                | "reviewer"
                | "monitor"
                | "enabled"
                | "max_threads"
                | "max_concurrent_threads_per_session"
                | "max_depth"
                | "job_max_runtime_seconds"
                | "default_subagent_model"
                | "default_subagent_reasoning_effort"
                | "interrupt_message"
                | "con"
                | "prn"
                | "aux"
                | "nul"
                | "com1"
                | "com2"
                | "com3"
                | "com4"
                | "com5"
                | "com6"
                | "com7"
                | "com8"
                | "com9"
                | "lpt1"
                | "lpt2"
                | "lpt3"
                | "lpt4"
                | "lpt5"
                | "lpt6"
                | "lpt7"
                | "lpt8"
                | "lpt9"
        )
}

pub(crate) fn validate_subagent_roles(
    roles: &BTreeMap<String, SubagentRoleConfig>,
) -> Result<(), String> {
    if roles
        .keys()
        .filter(|role| !owned_subagent_role_ids().any(|owned| owned == role.as_str()))
        .count()
        > MAX_CUSTOM_SUBAGENT_ROLES
    {
        return Err("最多可配置 32 个自定义子代理".into());
    }
    for (role, selection) in roles {
        if !valid_subagent_role_id(role) {
            return Err(format!(
                "子代理标识无效：{role}；仅支持非保留的小写字母、数字或下划线，最多 64 字符"
            ));
        }
        let custom = !owned_subagent_role_ids().any(|owned| owned == role);
        for (label, text, max) in [
            ("名称", &selection.name, 80),
            ("用途说明", &selection.description, 1000),
            ("角色指令", &selection.developer_instructions, 4000),
        ] {
            if (custom && text.trim().is_empty()) || text.len() > max || text.contains('\0') {
                return Err(format!(
                    "子代理 {role} 的{label}不能为空且不能超过 {max} 字节"
                ));
            }
        }
        if selection.model.trim().is_empty()
            || selection.model.len() > 256
            || selection.model.contains('\0')
        {
            return Err(format!("子代理 {role} 的模型不能为空且不能超过 256 字节"));
        }
        if !SUBAGENT_REASONING_EFFORTS.contains(
            &selection
                .reasoning_effort
                .trim()
                .to_ascii_lowercase()
                .as_str(),
        ) {
            return Err(format!("子代理 {role} 的思考深度无效"));
        }
    }
    Ok(())
}

pub(crate) fn merge_subagent_role_updates(
    previous: &BTreeMap<String, SubagentRoleConfig>,
    incoming: &BTreeMap<String, SubagentRoleConfig>,
    deleted: &[String],
) -> Result<BTreeMap<String, SubagentRoleConfig>, String> {
    validate_subagent_roles(incoming)?;
    for role in deleted {
        if !valid_subagent_role_id(role) || owned_subagent_role_ids().any(|owned| owned == role) {
            return Err(format!("只能删除自定义子代理：{role}"));
        }
        if incoming.contains_key(role) {
            return Err(format!("子代理不能同时保存和删除：{role}"));
        }
    }
    let mut incoming = incoming.clone();
    super::normalize::migrate_legacy_subagent_roles(&mut incoming);
    incoming.retain(|role, _| {
        !super::LEGACY_SUBAGENT_ROLES
            .iter()
            .any(|(legacy, _)| *legacy == role)
    });
    let mut merged = previous.clone();
    merged.extend(incoming);
    for role in deleted {
        merged.remove(role);
    }
    validate_subagent_roles(&merged)?;
    Ok(merged)
}

#[cfg(test)]
mod tests;
