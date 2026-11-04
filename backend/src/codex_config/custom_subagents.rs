use super::*;
use std::io::Read;

pub(super) fn custom_agent_header(role: &str) -> String {
    format!("# Codey managed custom subagent: {role}\n")
}

pub(super) fn ensure_managed_runtime_agent_file(
    constraints_dir: &Path,
    path: &Path,
    role: &str,
) -> Result<()> {
    ensure_managed_runtime_path(constraints_dir, path)?;
    if SUBAGENT_ROLE_IDS.contains(&role) {
        return Ok(());
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let header = custom_agent_header(role);
    let mut existing = vec![0; header.len()];
    anyhow::ensure!(
        file.read_exact(&mut existing).is_ok() && existing == header.as_bytes(),
        "自定义子代理目标已存在且不是 Codey 受管文件，未覆盖：{}",
        path.display()
    );
    Ok(())
}

pub(crate) fn validate_custom_subagent_role_conflicts(
    roles: &BTreeMap<String, SubagentRoleConfig>,
) -> Result<()> {
    validate_custom_subagent_role_conflicts_at(roles, codex_home())
}

pub(super) fn validate_custom_subagent_role_conflicts_at(
    roles: &BTreeMap<String, SubagentRoleConfig>,
    home: &Path,
) -> Result<()> {
    if roles
        .keys()
        .all(|role| SUBAGENT_ROLE_IDS.contains(&role.as_str()))
    {
        return Ok(());
    }
    let bytes = read_optional(&home.join("config.toml"))?.unwrap_or_default();
    let document = parse_document(str::from_utf8(&bytes).context("Codex config.toml 不是 UTF-8")?)
        .map_err(|_| anyhow::anyhow!("无法读取原生角色配置，请先修复 Codex config.toml"))?;
    validate_custom_roles_against_document(roles, &document)?;
    validate_native_agent_files(home, roles)
}

pub(super) fn validate_native_agent_files(
    home: &Path,
    roles: &BTreeMap<String, SubagentRoleConfig>,
) -> Result<()> {
    let customs = roles
        .keys()
        .filter(|role| !SUBAGENT_ROLE_IDS.contains(&role.as_str()))
        .collect::<HashSet<_>>();
    if customs.is_empty() {
        return Ok(());
    }
    let directory = home.join("agents");
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        metadata.is_dir() && !metadata_is_link(&metadata),
        "原生子代理目录必须是普通目录"
    );
    for (index, entry) in fs::read_dir(&directory)?.enumerate() {
        anyhow::ensure!(index < 256, "原生子代理目录超过安全检查上限（256 项）");
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            metadata.is_file() && !metadata_is_link(&metadata),
            "原生子代理文件不能是链接或非普通文件：{}",
            path.display()
        );
        let bytes = crate::fs_util::read_bounded(&path, 256 * 1024)?;
        let document = str::from_utf8(&bytes)
            .ok()
            .and_then(|text| parse_document(text).ok())
            .with_context(|| format!("原生子代理文件无法解析：{}", path.display()))?;
        let name = document
            .get("name")
            .and_then(Item::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .with_context(|| format!("原生子代理文件缺少 name：{}", path.display()))?;
        anyhow::ensure!(
            !customs.iter().any(|role| role.eq_ignore_ascii_case(name)),
            "自定义子代理标识与原生角色重名：{name}"
        );
    }
    Ok(())
}

fn metadata_is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(super) fn validate_custom_roles_against_document(
    roles: &BTreeMap<String, SubagentRoleConfig>,
    document: &DocumentMut,
) -> Result<()> {
    if let Some(agents) = document.get("agents").and_then(Item::as_table_like) {
        for role in roles
            .keys()
            .filter(|role| !SUBAGENT_ROLE_IDS.contains(&role.as_str()))
        {
            anyhow::ensure!(
                !agents
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(role)),
                "自定义子代理标识与原生角色重名：{role}"
            );
        }
    }
    Ok(())
}

pub(super) fn ensure_managed_runtime_path(constraints_dir: &Path, path: &Path) -> Result<()> {
    anyhow::ensure!(
        path.starts_with(constraints_dir),
        "子代理路径超出 Codey 运行时目录"
    );
    for entry in path
        .ancestors()
        .take_while(|entry| entry.starts_with(constraints_dir))
    {
        let metadata = match fs::symlink_metadata(entry) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            !metadata_is_link(&metadata),
            "Codey 子代理路径不能是符号链接或重解析点：{}",
            entry.display()
        );
    }
    Ok(())
}
