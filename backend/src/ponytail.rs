// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
//! Ponytail 官方插件与配置适配；安装始终交给 Codex CLI。
use anyhow::{Context, Result, ensure};
use codey_runtime_core::config_manager::ConfigManager;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use toml_edit::{DocumentMut, Item};

const PLUGIN: &str = "ponytail@ponytail";
const SOURCE: &str = "DietrichGebert/ponytail";
const MAX_CONFIG: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    Lite,
    Full,
    Ultra,
    Off,
}

fn normalized_mode(value: &str) -> Option<Mode> {
    serde_json::from_value(json!(value.trim().to_ascii_lowercase())).ok()
}

pub(crate) fn config_path(
    home: &Path,
    windows: bool,
    xdg: Option<&Path>,
    appdata: Option<&Path>,
) -> Result<PathBuf> {
    let base = xdg
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            if windows {
                appdata
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| home.join("AppData/Roaming"))
            } else {
                home.join(".config")
            }
        });
    ensure!(
        base.is_absolute(),
        "Ponytail 配置目录必须是绝对路径，请检查 XDG_CONFIG_HOME / APPDATA"
    );
    Ok(base.join("ponytail/config.json"))
}

pub(crate) fn user_config_path() -> Result<PathBuf> {
    let dirs = directories::BaseDirs::new().context("无法确定当前用户目录")?;
    config_path(
        dirs.home_dir(),
        cfg!(windows),
        std::env::var_os("XDG_CONFIG_HOME")
            .as_deref()
            .map(Path::new),
        std::env::var_os("APPDATA").as_deref().map(Path::new),
    )
}

fn config_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => anyhow::bail!("无法读取 Ponytail 配置文件"),
        Ok(_) => crate::fs_util::read_bounded(path, MAX_CONFIG)
            .map(Some)
            .map_err(|_| anyhow::anyhow!("Ponytail 配置必须是普通文件且不超过 256 KiB")),
    }
}

fn document(bytes: Option<&[u8]>) -> Result<Value> {
    let Some(bytes) = bytes else {
        return Ok(json!({}));
    };
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("Ponytail 配置不是有效 JSON，请先修复原文件"))?;
    ensure!(value.is_object(), "Ponytail 配置必须是 JSON 对象");
    Ok(value)
}

fn revision(bytes: Option<&[u8]>) -> String {
    bytes
        .map(crate::fs_util::sha256_hex)
        .unwrap_or_else(|| "missing".into())
}

pub(crate) fn settings(path: &Path, env_mode: Option<&str>) -> Result<Value> {
    let bytes = config_bytes(path)?;
    let config = document(bytes.as_deref())?;
    let mode = config["defaultMode"]
        .as_str()
        .and_then(normalized_mode)
        .unwrap_or(Mode::Full);
    let override_mode = env_mode.and_then(normalized_mode);
    Ok(
        json!({"defaultMode":mode,"effectiveMode":override_mode.unwrap_or(mode),
        "environmentOverride":override_mode,"revision":revision(bytes.as_deref()),"configPath":path}),
    )
}

pub(crate) fn save_settings(path: &Path, mode: Mode, expected_revision: &str) -> Result<()> {
    use fs2::FileExt;
    let parent = path.parent().context("Ponytail 配置路径无效")?;
    std::fs::create_dir_all(parent).context("无法创建 Ponytail 配置目录")?;
    let lock_path = parent.join(".codey-config.lock");
    if let Ok(metadata) = std::fs::symlink_metadata(&lock_path) {
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "Ponytail 配置锁不是普通文件"
        );
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .context("无法锁定 Ponytail 配置")?;
    FileExt::try_lock_exclusive(&lock).context("Ponytail 配置正在保存，请稍后重试")?;
    let bytes = config_bytes(path)?;
    ensure!(
        revision(bytes.as_deref()) == expected_revision,
        "Ponytail 配置已被修改；请先撤销参数修改以加载最新配置，再重新选择并保存"
    );
    let mut config = document(bytes.as_deref())?;
    config["defaultMode"] = json!(mode);
    // 官方配置可含其他宿主的字段；仅修改 Codex 支持的默认级别。
    ensure!(
        config_bytes(path)? == bytes,
        "Ponytail 配置已被修改；请先撤销参数修改以加载最新配置，再重新选择并保存"
    );
    crate::fs_util::atomic_write_private(path, &serde_json::to_vec_pretty(&config)?)
        .context("保存 Ponytail 配置失败")
}

fn official_marketplace(doc: &DocumentMut) -> Result<bool> {
    let Some(entry) = doc.get("marketplaces").and_then(|v| v.get("ponytail")) else {
        return Ok(false);
    };
    let source = entry
        .get("source")
        .and_then(Item::as_str)
        .unwrap_or_default();
    ensure!(
        entry.get("source_type").and_then(Item::as_str) == Some("git")
            && [
                SOURCE,
                "https://github.com/DietrichGebert/ponytail",
                "https://github.com/DietrichGebert/ponytail.git"
            ]
            .contains(&source),
        "ponytail 市场名称已被其他来源占用，请先在 Codex 插件管理中处理"
    );
    Ok(true)
}

fn cached_version(home: &Path) -> Result<Option<String>> {
    let root = home.join("plugins/cache/ponytail/ponytail");
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => anyhow::bail!("无法读取 Ponytail 插件缓存"),
    };
    let mut versions = Vec::new();
    for entry in entries.take(100) {
        let entry = entry.context("无法读取 Ponytail 插件缓存")?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let manifest = entry.path().join(".codex-plugin/plugin.json");
        let Ok(bytes) = crate::fs_util::read_bounded(&manifest, MAX_CONFIG) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if value["name"] == "ponytail"
            && entry.path().join("skills/ponytail/SKILL.md").is_file()
            && entry.path().join("hooks/claude-codex-hooks.json").is_file()
            && let Some(version) = value["version"]
                .as_str()
                .and_then(|s| semver::Version::parse(s).ok())
        {
            versions.push(version);
        }
    }
    Ok(versions.into_iter().max().map(|v| v.to_string()))
}

pub(crate) fn status(home: &Path, path: &Path, env_mode: Option<&str>) -> Result<Value> {
    let snapshot = ConfigManager::for_home(home)
        .load()
        .map_err(|_| anyhow::anyhow!("无法读取 Codex 配置，请检查 config.toml 的格式和权限"))?;
    let doc = snapshot.document();
    let conflict = official_marketplace(doc)
        .err()
        .map(|error| error.to_string());
    let plugin = doc.get("plugins").and_then(|v| v.get(PLUGIN));
    let version = cached_version(home)?;
    let enabled = plugin
        .map(|entry| -> Result<bool> {
            let table = entry.as_table_like().context("Ponytail 插件配置必须为表")?;
            table
                .get("enabled")
                .map(|value| value.as_bool().context("Ponytail enabled 必须为布尔值"))
                .transpose()
                .map(|value| value.unwrap_or(true))
        })
        .transpose()?
        .unwrap_or(false);
    Ok(
        json!({"installed":plugin.is_some() && version.is_some(),"enabled":enabled,
        "version":version,"conflict":conflict,"settings":settings(path, env_mode).with_context(|| format!("读取 Ponytail 配置失败：{}", path.display()))?}),
    )
}

pub(crate) fn set_enabled(home: &Path, enabled: bool) -> Result<()> {
    let manager = ConfigManager::for_home(home);
    let snapshot = manager
        .load()
        .map_err(|_| anyhow::anyhow!("无法读取 Codex 配置，请检查 config.toml 的格式和权限"))?;
    official_marketplace(snapshot.document())?;
    let mut doc = snapshot.document().clone();
    let Some(plugin) = doc.get_mut("plugins").and_then(|v| v.get_mut(PLUGIN)) else {
        ensure!(!enabled, "Ponytail 尚未安装，请先安装");
        return Ok(());
    };
    plugin
        .as_table_like_mut()
        .context("Ponytail 插件配置无效")?
        .insert("enabled", toml_edit::value(enabled));
    manager
        .replace_document(
            Some(snapshot.revision()),
            doc,
            "切换 Ponytail 插件",
            "builtin_tools",
        )
        .map_err(|_| {
            anyhow::anyhow!("无法保存 Ponytail 启停状态，Codex 配置可能已变化，请刷新后重试")
        })?;
    Ok(())
}

pub(crate) fn cli(configured_path: &str) -> Result<PathBuf> {
    let saved = (!configured_path.trim().is_empty()).then(|| Path::new(configured_path.trim()));
    if let Some(app_dir) = codey_runtime_core::app_paths::resolve_codex_app_dir(saved) {
        #[cfg(windows)]
        {
            return crate::launcher::windows_cli_wrapper_target(&app_dir);
        }
        #[cfg(not(windows))]
        if let Some(path) = codey_runtime_core::app_paths::codex_runtime_executable(&app_dir) {
            return Ok(path);
        }
    }
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|path| path.join(name))
                .find(|path| path.is_absolute() && path.is_file())
        })
        .context("未找到 Codex CLI，请先安装或在基础功能中选择 Codex 客户端")
}

async fn run(executable: &Path, home: &Path, args: &[&str], phase: &str) -> Result<()> {
    let mut command = std::process::Command::new(executable);
    command
        .args(args)
        .current_dir(home)
        .env("CODEX_HOME", home)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let phase = phase.to_owned();
    tokio::task::spawn_blocking(move || run_command(&mut command, &phase, Duration::from_secs(180)))
        .await?
}

fn run_command(command: &mut std::process::Command, phase: &str, timeout: Duration) -> Result<()> {
    // 复用受管进程树，超时与退出时连同安装器的 Git 子进程一起回收。
    let mut child = crate::codegraph_runtime::process::OwnedChild::spawn(command)
        .with_context(|| format!("{phase}无法启动，请检查程序安装与执行权限"))?;
    let started = std::time::Instant::now();
    loop {
        if let Some(result) = child
            .try_wait()
            .with_context(|| format!("{phase}无法读取进程状态"))?
        {
            ensure!(
                result.success(),
                "{phase}失败（退出码 {:?}）；请检查网络、Git 和 Codex 版本是否支持 plugin 命令",
                result.code()
            );
            return Ok(());
        }
        ensure!(
            started.elapsed() < timeout,
            "{phase}超时，结果尚未确认，请刷新安装状态后再重试"
        );
        std::thread::sleep(Duration::from_millis(40));
    }
}

pub(crate) async fn install(executable: &Path, home: &Path) -> Result<()> {
    // Hooks 使用 PATH 中的 node，Git 由 Codex 官方安装流程使用。
    run(
        Path::new(if cfg!(windows) { "node.exe" } else { "node" }),
        home,
        &["--version"],
        "检查 Node.js",
    )
    .await?;
    run(
        Path::new(if cfg!(windows) { "git.exe" } else { "git" }),
        home,
        &["--version"],
        "检查 Git",
    )
    .await?;
    install_with(home, |args, phase| run(executable, home, args, phase)).await
}

async fn install_with<F, Fut>(home: &Path, mut command: F) -> Result<()>
where
    F: FnMut(&'static [&'static str], &'static str) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let home_copy = home.to_path_buf();
    let registered = tokio::task::spawn_blocking(move || {
        let snapshot = ConfigManager::for_home(&home_copy)
            .load()
            .map_err(|_| anyhow::anyhow!("无法读取 Codex 插件市场配置，请检查格式和权限"))?;
        official_marketplace(snapshot.document())
    })
    .await??;
    if !registered {
        command(
            &["plugin", "marketplace", "add", SOURCE],
            "注册 Ponytail 官方市场",
        )
        .await?;
    }
    let home_copy = home.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let snapshot = ConfigManager::for_home(&home_copy)
            .load()
            .map_err(|_| anyhow::anyhow!("无法复核 Codex 插件市场配置"))?;
        ensure!(
            official_marketplace(snapshot.document())?,
            "Ponytail 官方市场尚未注册，请刷新后重试"
        );
        Ok(())
    })
    .await??;
    command(&["plugin", "add", PLUGIN], "安装 Ponytail 官方插件").await?;
    let home = home.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        ensure!(
            cached_version(&home)?.is_some(),
            "安装命令已完成，但尚未发现完整 Ponytail 插件，请刷新后检查"
        );
        set_enabled(&home, true)
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests;
