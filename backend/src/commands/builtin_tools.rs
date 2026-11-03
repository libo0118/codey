// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
//! 内置工具入口：开关接入用户 MCP，项目由 MCP 启动钩子自动识别和维护。
mod ponytail;
use crate::codegraph_runtime as runtime;

use super::{AppState, argument, string_argument};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Mutex, Semaphore};

const SERVER_ID: &str = "codey_codegraph";

pub(super) struct ToolState {
    operation: Arc<Semaphore>,
    job: Mutex<Option<Value>>,
    ponytail_operation: Arc<Semaphore>,
    ponytail_job: Mutex<Option<Value>>,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            operation: Arc::new(Semaphore::new(1)),
            job: Mutex::new(None),
            ponytail_operation: Arc::new(Semaphore::new(1)),
            ponytail_job: Mutex::new(None),
        }
    }
}

fn root(state: &AppState) -> Result<PathBuf> {
    Ok(state
        .store
        .path()
        .parent()
        .context("Codey 配置目录无效")?
        .join("builtin-tools/codegraph"))
}

fn canonical_directory(path: &Path) -> Option<PathBuf> {
    (path.is_absolute() && path.is_dir())
        .then(|| path.canonicalize().ok())
        .flatten()
}

fn workspace_roots(home: &Path, codex_home: &Path) -> Result<Vec<PathBuf>> {
    let location = codex_home.join(".codex-global-state.json");
    if !location
        .try_exists()
        .context("无法读取 Codex 工作区项目清单")?
    {
        return Ok(Vec::new());
    }
    let bytes = crate::fs_util::read_bounded(&location, 16 * 1024 * 1024)
        .context("无法读取 Codex 工作区项目清单")?;
    // 忽略无关状态中的孤立 UTF-16 代理字符，与 Codex 全局状态读取约定一致。
    let state: BTreeMap<String, Box<serde_json::value::RawValue>> = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Codex 工作区项目清单格式无效"))?;
    let mut candidates = Vec::new();
    if let Some(projects) = state.get("local-projects") {
        let projects: Value =
            serde_json::from_str(projects.get()).context("Codex 工作区项目清单格式无效")?;
        for project in projects
            .as_object()
            .context("Codex 工作区项目清单格式无效")?
            .values()
        {
            if let Some(roots) = project["rootPaths"].as_array() {
                candidates.extend(roots.iter().filter_map(Value::as_str).map(PathBuf::from));
            }
        }
    } else if let Some(roots) = state.get("electron-saved-workspace-roots") {
        let roots: Value =
            serde_json::from_str(roots.get()).context("Codex 工作区项目清单格式无效")?;
        candidates.extend(
            roots
                .as_array()
                .context("Codex 工作区项目清单格式无效")?
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from),
        );
    }
    let default_home = home.join(".codex");
    let excluded: Vec<_> = [home, codex_home, default_home.as_path()]
        .into_iter()
        .filter_map(canonical_directory)
        .collect();
    let mut roots = Vec::new();
    for path in candidates
        .iter()
        .filter_map(|path| canonical_directory(path))
    {
        if path.parent().is_some() && !excluded.contains(&path) && !roots.contains(&path) {
            roots.push(path);
        }
    }
    Ok(roots)
}

fn project_record_path(root: &Path, project: &Path) -> PathBuf {
    let path = super::file_manager_path(project)
        .to_string_lossy()
        .into_owned();
    // 与 auto_mcp.cjs 的 writeRecord 保持相同的路径哈希。
    let key = if cfg!(windows) {
        path.to_lowercase()
    } else {
        path
    };
    root.join("projects")
        .join(format!("{:x}.json", Sha256::digest(key.as_bytes())))
}

// 与自动接入一致，子目录归属最深的已添加项目；不从最近项目列表回退。
fn project(root: &Path, home: &Path, codex_home: &Path, session_id: &str) -> Result<Option<Value>> {
    ensure!(uuid::Uuid::parse_str(session_id).is_ok(), "会话 ID 无效");
    let Some((thread, _)) = crate::session_transfer::find_thread(codex_home, session_id)? else {
        return Ok(None);
    };
    let Some(cwd) = thread
        .get("cwd")
        .and_then(Value::as_str)
        .and_then(|cwd| canonical_directory(Path::new(cwd)))
    else {
        return Ok(None);
    };
    let Some(project_path) = workspace_roots(home, codex_home)?
        .into_iter()
        .filter(|path| cwd.starts_with(path))
        .max_by_key(|path| path.components().count())
    else {
        return Ok(None);
    };
    let record_path = project_record_path(root, &project_path);
    if !record_path
        .try_exists()
        .context("无法读取 CodeGraph 项目状态")?
    {
        return Ok(Some(
            json!({"projectPath":super::file_manager_path(&project_path),"phase":"waiting",
            "message":"等待项目聊天，索引将自动开始。"}),
        ));
    }
    let bytes = crate::fs_util::read_bounded(&record_path, 16 * 1024)?;
    let record: Value = serde_json::from_slice(&bytes).context("CodeGraph 项目状态格式无效")?;
    ensure!(
        record["projectPath"]
            .as_str()
            .and_then(|path| canonical_directory(Path::new(path)))
            .is_some_and(|path| path == project_path)
            && record["updatedAt"].is_u64()
            && matches!(
                record["phase"].as_str(),
                Some("indexing" | "watching" | "failed")
            ),
        "CodeGraph 项目状态与当前目录不匹配或格式无效"
    );
    Ok(Some(record))
}

pub(super) async fn session_status(
    state: &Arc<AppState>,
    session_id: &str,
) -> Result<Option<Value>> {
    let root = root(state)?;
    let home = crate::codex_config::codex_home().to_path_buf();
    let session_id = session_id.to_owned();
    let installed = runtime::installed(&root);
    let current = tokio::task::spawn_blocking(move || {
        let user = directories::BaseDirs::new().context("无法确定当前用户目录")?;
        project(&root, user.home_dir(), &home, &session_id)
    })
    .await??;
    let Some(mut current) = current else {
        return Ok(None);
    };
    let mcp = mcp_status(state).await?;
    if let Some((phase, message)) = if mcp["enabled"] != true {
        Some(("disabled", "CodeGraph 未启用，可在内置工具设置中开启。"))
    } else if mcp["managed"] != true || mcp["automatic"] != true {
        Some((
            "external",
            "当前 CodeGraph 由 MCP 配置管理，无法确认本项目的自动索引状态。",
        ))
    } else if !installed {
        Some(("uninstalled", "CodeGraph 运行包尚未安装完成。"))
    } else {
        None
    } {
        current["phase"] = json!(phase);
        current["message"] = json!(message);
    }
    Ok(Some(current))
}

async fn extension(state: &Arc<AppState>, request: Value) -> Result<Value> {
    super::extensions::invoke(state, &json!({"request":request}))
        .await
        .map_err(anyhow::Error::msg)
}

fn is_codegraph(id: &str, config: &Value) -> bool {
    let mentions = |text: &str| text.to_ascii_lowercase().contains("codegraph");
    mentions(id)
        || config["command"].as_str().is_some_and(mentions)
        || config["args"]
            .as_array()
            .is_some_and(|args| args.iter().filter_map(Value::as_str).any(mentions))
}

fn is_managed(id: &str, config: &Value, root: &Path) -> bool {
    id == SERVER_ID && runtime::is_managed_mcp(config, root)
}

// 人工配置（包括停用状态）优先，只有 Codey 创建且启动参数未改写的条目可切换。
fn classify_mcp_status(
    entries: impl IntoIterator<Item = Result<Value>>,
    root: &Path,
) -> Result<Value> {
    let mut managed = None;
    for entry in entries {
        let entry = entry?;
        let id = entry["id"].as_str().context("MCP 标识无效")?;
        let config = &entry["configJson"];
        if !is_codegraph(id, config) {
            continue;
        }
        let own = is_managed(id, config, root);
        let status = json!({"id":id,"enabled":entry["enabled"] == true,"managed":own,
            "automatic":own && config["args"] == runtime::mcp_config(root)["args"]});
        if !own {
            return Ok(status);
        }
        managed = Some(status);
    }
    Ok(managed.unwrap_or(json!({"enabled":false,"managed":false,"automatic":false})))
}

async fn mcp_status(state: &Arc<AppState>) -> Result<Value> {
    let codex_home = crate::codex_config::codex_home().to_path_buf();
    let user_home = directories::BaseDirs::new()
        .context("无法确定当前用户目录")?
        .home_dir()
        .to_path_buf();
    let data = state
        .store
        .path()
        .parent()
        .context("Codey 配置目录无效")?
        .join("codex-extensions");
    let entries = tokio::task::spawn_blocking(move || {
        let service = codey_codex_extensions::ExtensionService::new(codex_home, data, user_home);
        mcp_status_configurations(&service)
    })
    .await
    .context("读取 MCP 状态的任务异常退出")??;
    classify_mcp_status(entries, &root(state)?)
}

fn mcp_status_configurations(
    service: &codey_codex_extensions::ExtensionService,
) -> Result<Vec<Result<Value>>> {
    let scope = codey_codex_extensions::Scope::User;
    let inventory = service
        .inventory(&scope)
        .map_err(|_| anyhow::anyhow!("无法读取 Codex MCP 状态，请检查配置格式和权限"))?;
    Ok(inventory["mcps"]
        .as_array()
        .context("MCP 清单无效")?
        .iter()
        .map(|entry| {
            let id = entry["id"].as_str().context("MCP 标识无效")?;
            let config = service
                .mcp_configuration(&scope, id)
                .map_err(|_| anyhow::anyhow!("MCP 配置无效，请先在 MCP 管理中检查"))?;
            Ok(json!({"id":id,"enabled":entry["enabled"],"configJson":config}))
        })
        .collect())
}

async fn mcp_edit_status(state: &Arc<AppState>) -> Result<(Value, Value)> {
    let scope = json!({"kind":"user"});
    let inventory = extension(state, json!({"action":"list", "scope":scope})).await?;
    let root = root(state)?;
    let mut entries = Vec::new();
    for entry in inventory["mcps"].as_array().context("MCP 清单无效")? {
        let id = entry["id"].as_str().context("MCP 标识无效")?;
        let document =
            extension(state, json!({"action":"get_mcp", "scope":scope, "id":id})).await?;
        let config = &document["configJson"];
        entries.push(json!({"id":id,"enabled":entry["enabled"],"configJson":config}));
        if is_codegraph(id, config) && !is_managed(id, config, &root) {
            break;
        }
    }
    Ok((
        classify_mcp_status(entries.into_iter().map(Ok), &root)?,
        inventory,
    ))
}

async fn status(state: &Arc<AppState>) -> Result<Value> {
    let root = root(state)?;
    let mcp = mcp_status(state).await?;
    let installed = runtime::installed(&root);
    Ok(
        json!({"installed":installed,"version":runtime::VERSION,"mcp":mcp,
        "job":state.builtin_tools.job.lock().await.clone()}),
    )
}

async fn enable(state: &Arc<AppState>) -> Result<String> {
    let root = root(state)?;
    runtime::install(root.clone(), state.http_client.current_for_any_host().await).await?;
    let launcher_root = root.clone();
    tokio::task::spawn_blocking(move || runtime::prepare_launcher(&launcher_root)).await??;
    // 下载可能耗时，落盘前重新读取并校验，保留期间人工配置的改动。
    let (current, inventory) = mcp_edit_status(state).await?;
    ensure!(
        !current["id"].is_string() || current["managed"] == true,
        "已存在用户管理的 CodeGraph MCP，请在 MCP 管理中处理"
    );
    let config = if current["id"].is_string() {
        let document = extension(state, json!({"action":"get_mcp","id":SERVER_ID})).await?;
        let mut config = document["configJson"].clone();
        ensure!(
            is_managed(SERVER_ID, &config, &root),
            "CodeGraph 配置已被修改，请刷新后重试"
        );
        config["args"] = runtime::mcp_config(&root)["args"].clone();
        if config["env"].is_null() {
            config["env"] = json!({});
        }
        config["env"]
            .as_object_mut()
            .context("CodeGraph 环境配置无效，请检查 MCP 配置")?
            .entry("CODEX_HOME")
            .or_insert_with(|| json!(crate::codex_config::codex_home()));
        config["enabled"] = json!(true);
        config
    } else {
        runtime::mcp_config(&root)
    };
    let saved = extension(
        state,
        json!({"action":"save_mcp","id":SERVER_ID,"createOnly":!current["id"].is_string(),
        "confirmed":true,"revision":inventory["revision"],"configJson":config}),
    )
    .await?;
    Ok(saved["message"]
        .as_str()
        .unwrap_or("配置已保存，请重新打开项目聊天。")
        .into())
}

pub(super) async fn invoke(
    state: &Arc<AppState>,
    args: &Value,
) -> std::result::Result<Value, String> {
    invoke_inner(state, args)
        .await
        .map_err(|error| format!("{error:#}"))
}

async fn invoke_inner(state: &Arc<AppState>, args: &Value) -> Result<Value> {
    if args["tool"] == "ponytail" {
        return ponytail::invoke(state, args).await;
    }
    ensure!(args.get("tool").is_none(), "未知内置工具");
    let action = string_argument(args, "action").map_err(anyhow::Error::msg)?;
    if action == "status" {
        return status(state).await;
    }
    ensure!(action == "set_enabled", "未知内置工具操作");
    ensure!(!state.is_shutting_down(), "Codey 正在退出，请稍后重试");
    let enabled = argument::<bool>(args, "enabled").map_err(anyhow::Error::msg)?;
    let permit = state
        .builtin_tools
        .operation
        .clone()
        .try_acquire_owned()
        .context("已有 CodeGraph 操作进行中，请等待完成")?;
    let (current, inventory) = mcp_edit_status(state).await?;
    ensure!(
        !current["id"].is_string() || current["managed"] == true,
        "该 CodeGraph MCP 由用户管理，请到 MCP 管理中修改"
    );
    if !enabled {
        let mut message = "CodeGraph 自动接入已关闭。".to_string();
        if current["id"].is_string() {
            let saved = extension(state, json!({"action":"set_mcp_enabled","id":SERVER_ID,"enabled":false,"confirmed":true,"revision":inventory["revision"]})).await?;
            if let Some(saved_message) = saved["message"].as_str() {
                message.push_str(saved_message);
            }
        }
        *state.builtin_tools.job.lock().await = None;
        let mut snapshot = status(state).await?;
        snapshot["message"] = json!(message);
        return Ok(snapshot);
    }
    let job = json!({"id":uuid::Uuid::new_v4().to_string(),"phase":"running","message":"正在准备 CodeGraph 自动接入，首次启用需下载官方运行包…"});
    *state.builtin_tools.job.lock().await = Some(job);
    let worker = Arc::clone(state);
    tokio::spawn(async move {
        let _permit = permit;
        let task_state = Arc::clone(&worker);
        let outcome = tokio::spawn(async move { enable(&task_state).await }).await;
        let mut guard = worker.builtin_tools.job.lock().await;
        if let Some(job) = guard.as_mut() {
            match outcome {
                Ok(Ok(message)) => {
                    job["phase"] = json!("succeeded");
                    job["message"] = json!(format!(
                        "自动接入已启用。{message}项目索引会在后台建立并自动同步。"
                    ));
                }
                result => {
                    job["phase"] = json!("failed");
                    job["message"] = json!(match result {
                        Ok(Err(error)) => format!("{error:#}"),
                        _ => "CodeGraph 接入任务异常退出，请重试".into(),
                    });
                }
            }
        }
    });
    status(state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_mcp_status_does_not_expose_config_contents() {
        let directory = tempfile::tempdir().unwrap();
        let service = codey_codex_extensions::ExtensionService::new(
            directory.path().to_path_buf(),
            directory.path().join("data"),
            directory.path().to_path_buf(),
        );
        std::fs::write(
            directory.path().join("config.toml"),
            "api_key = private-config-value",
        )
        .unwrap();
        let error = mcp_status_configurations(&service).unwrap_err();
        assert!(!format!("{error:#}").contains("private-config-value"));
    }

    #[test]
    fn status_classification_keeps_manual_codegraph_priority_and_legacy_states() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let managed = json!({"id":SERVER_ID,"enabled":true,"configJson":runtime::mcp_config(root)});
        let external =
            json!({"id":"manual_codegraph","enabled":false,"configJson":{"command":"custom"}});
        let own = classify_mcp_status([Ok(managed.clone())], root).unwrap();
        assert_eq!(
            own,
            json!({"id":SERVER_ID,"enabled":true,"managed":true,"automatic":true})
        );
        for entries in [
            vec![managed.clone(), external.clone()],
            vec![external.clone(), managed.clone()],
        ] {
            assert_eq!(
                classify_mcp_status(entries.into_iter().map(Ok), root).unwrap(),
                json!({"id":"manual_codegraph","enabled":false,"managed":false,"automatic":false})
            );
        }
        let mut legacy = managed;
        legacy["configJson"]["args"] = runtime::legacy_mcp_args(root);
        let status = classify_mcp_status([Ok(legacy)], root).unwrap();
        assert_eq!(status["managed"], true);
        assert_eq!(status["automatic"], false);
        assert_eq!(
            classify_mcp_status([], root).unwrap(),
            json!({"enabled":false,"managed":false,"automatic":false})
        );
        let service = codey_codex_extensions::ExtensionService::new(
            root.to_path_buf(),
            root.join("data"),
            root.to_path_buf(),
        );
        for (config, readable) in [
            (
                "mcp_servers={manual_codegraph={command='custom',enabled=false},broken=42}",
                true,
            ),
            (
                "mcp_servers={broken=42,manual_codegraph={command='custom',enabled=false}}",
                false,
            ),
        ] {
            std::fs::write(root.join("config.toml"), config).unwrap();
            let entries = mcp_status_configurations(&service).unwrap();
            let status = classify_mcp_status(entries, root);
            assert_eq!(status.is_ok(), readable);
            if let Ok(status) = status {
                assert_eq!(status["id"], "manual_codegraph");
                assert_eq!(status["enabled"], false);
            }
        }
    }

    #[test]
    fn only_owned_launchers_and_legacy_configs_can_be_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = runtime::mcp_config(dir.path());
        assert!(is_managed(SERVER_ID, &config, dir.path()));
        assert!(config.get("cwd").is_none());
        assert_eq!(
            config["env"]["CODEX_HOME"],
            json!(crate::codex_config::codex_home())
        );
        config["args"] = runtime::legacy_mcp_args(dir.path());
        assert!(is_managed(SERVER_ID, &config, dir.path()));
        assert!(!is_managed("user", &config, dir.path()));
        config["cwd"] = json!("custom-project");
        assert!(!is_managed(SERVER_ID, &config, dir.path()));
        let user = json!({"command":"node","args":["custom/codegraph.js"]});
        assert!(is_codegraph("custom", &user));
        assert!(!is_managed(SERVER_ID, &user, dir.path()));
        assert!(!is_codegraph("fastctx", &json!({"command":"fastctx"})));
    }

    #[test]
    fn project_status_follows_the_session_directory_and_ignores_other_records() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let codex_home = home.join(".codex");
        std::fs::create_dir_all(&codex_home).unwrap();
        let session = "11111111-1111-4111-8111-111111111111";
        assert!(
            project(dir.path(), &home, &codex_home, session)
                .unwrap()
                .is_none()
        );
        assert!(project(dir.path(), &home, &codex_home, "../invalid").is_err());
        let db = rusqlite::Connection::open(codex_home.join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, cwd TEXT)")
            .unwrap();
        let records = dir.path().join("projects");
        std::fs::create_dir(&records).unwrap();
        let mut roots = Vec::new();
        for index in 0..12 {
            let path = dir.path().join(format!("project-{index}"));
            std::fs::create_dir(&path).unwrap();
            std::fs::write(
                project_record_path(dir.path(), &path.canonicalize().unwrap()),
                serde_json::to_vec(&json!({
                    "projectPath":path,"updatedAt":index,"phase":"watching","message":"已接入"
                }))
                .unwrap(),
            )
            .unwrap();
            roots.push(path);
        }
        let state_path = codex_home.join(".codex-global-state.json");
        std::fs::write(
            &state_path,
            json!({"local-projects":{"p":{"rootPaths":roots}}}).to_string(),
        )
        .unwrap();
        std::fs::write(records.join("broken.json"), b"{").unwrap();
        let set_cwd = |path: &Path| {
            db.execute(
                "INSERT OR REPLACE INTO threads VALUES (?1, ?2)",
                rusqlite::params![session, path.to_str().unwrap()],
            )
            .unwrap();
        };
        // 当前项目即使排在最近十个以外，也必须返回自己的状态。
        set_cwd(&roots[0]);
        let current = project(dir.path(), &home, &codex_home, session)
            .unwrap()
            .unwrap();
        assert_eq!(current["projectPath"], json!(roots[0]));
        assert_eq!(current["phase"], "watching");
        let selected_record = project_record_path(dir.path(), &roots[0].canonicalize().unwrap());
        for invalid in [
            json!({"projectPath":roots[1],"updatedAt":100,"phase":"watching"}),
            json!({"projectPath":roots[0],"updatedAt":100,"phase":"invalid"}),
            json!({"projectPath":roots[0],"phase":"watching"}),
        ] {
            std::fs::write(&selected_record, invalid.to_string()).unwrap();
            assert!(project(dir.path(), &home, &codex_home, session).is_err());
        }
        std::fs::write(&selected_record, vec![b' '; 16 * 1024 + 1]).unwrap();
        assert!(project(dir.path(), &home, &codex_home, session).is_err());
        std::fs::write(&selected_record, current.to_string()).unwrap();
        let child = roots[0].join("child");
        std::fs::create_dir(&child).unwrap();
        set_cwd(&child);
        assert_eq!(
            project(dir.path(), &home, &codex_home, session)
                .unwrap()
                .unwrap(),
            current
        );
        roots.push(child.clone());
        std::fs::write(
            &state_path,
            json!({"local-projects":{"p":{"rootPaths":roots}}}).to_string(),
        )
        .unwrap();
        let nested = project(dir.path(), &home, &codex_home, session)
            .unwrap()
            .unwrap();
        assert_eq!(
            nested["projectPath"],
            json!(super::super::file_manager_path(
                &child.canonicalize().unwrap()
            ))
        );
        assert_eq!(nested["phase"], "waiting");
        set_cwd(&roots[1]);
        std::fs::write(
            project_record_path(dir.path(), &roots[1].canonicalize().unwrap()),
            b"{",
        )
        .unwrap();
        assert!(project(dir.path(), &home, &codex_home, session).is_err());
        for cwd in [&home, &codex_home, dir.path(), &dir.path().join("missing")] {
            set_cwd(cwd);
            assert!(
                project(dir.path(), &home, &codex_home, session)
                    .unwrap()
                    .is_none()
            );
        }
        set_cwd(&roots[0]);
        std::fs::write(&state_path, r#"{"local-projects":{}}"#).unwrap();
        assert!(
            project(dir.path(), &home, &codex_home, session)
                .unwrap()
                .is_none()
        );
        assert!(project_record_path(dir.path(), &roots[0].canonicalize().unwrap()).is_file());
        std::fs::write(&state_path, "{").unwrap();
        assert!(project(dir.path(), &home, &codex_home, session).is_err());
    }

    #[test]
    fn workspace_roots_use_saved_projects_and_exclude_special_directories() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let default_home = home.join(".codex");
        let codex_home = dir.path().join("custom-codex");
        let project = dir.path().join("中文 project");
        let other = dir.path().join("other");
        for path in [&default_home, &codex_home, &project, &other] {
            std::fs::create_dir_all(path).unwrap();
        }
        let disk = dir.path().ancestors().last().unwrap();
        let state = json!({"local-projects":{
            "p":{"rootPaths":[project,other,project,home,codex_home,default_home,disk,"relative",null,dir.path().join("missing")]},
            "invalid":{"rootPaths":"relative"}
        },"electron-saved-workspace-roots":[dir.path()]});
        let state_path = codex_home.join(".codex-global-state.json");
        std::fs::write(&state_path, state.to_string()).unwrap();
        assert_eq!(
            workspace_roots(&home, &codex_home).unwrap(),
            vec![
                project.canonicalize().unwrap(),
                other.canonicalize().unwrap()
            ]
        );
        std::fs::write(
            &state_path,
            json!({"electron-saved-workspace-roots":[project]}).to_string(),
        )
        .unwrap();
        assert_eq!(
            workspace_roots(&home, &codex_home).unwrap(),
            vec![project.canonicalize().unwrap()]
        );
        std::fs::write(
            &state_path,
            json!({"local-projects":{},"electron-saved-workspace-roots":[project]}).to_string(),
        )
        .unwrap();
        assert!(workspace_roots(&home, &codex_home).unwrap().is_empty());
        std::fs::write(&state_path, r#"{"local-projects":{},"unrelated":"\ud800"}"#).unwrap();
        assert!(workspace_roots(&home, &codex_home).unwrap().is_empty());
    }

    #[test]
    fn missing_or_invalid_workspace_state_never_authorizes_projects() {
        let dir = tempfile::tempdir().unwrap();
        assert!(workspace_roots(dir.path(), dir.path()).unwrap().is_empty());
        let location = dir.path().join(".codex-global-state.json");
        for contents in [
            "{",
            "null",
            "[]",
            r#""private-state""#,
            r#"{"local-projects":null}"#,
            r#"{"local-projects":[]}"#,
            r#"{"electron-saved-workspace-roots":{}}"#,
            r#"{"electron-saved-workspace-roots":null}"#,
        ] {
            std::fs::write(&location, contents).unwrap();
            let error = workspace_roots(dir.path(), dir.path()).unwrap_err();
            assert!(!format!("{error:#}").contains("private-state"));
        }
        std::fs::write(&location, vec![b' '; 16 * 1024 * 1024 + 1]).unwrap();
        assert!(workspace_roots(dir.path(), dir.path()).is_err());
    }

    #[tokio::test]
    async fn concurrent_enable_operations_are_rejected_until_completion() {
        let state = ToolState::default();
        let permit = state.operation.clone().try_acquire_owned().unwrap();
        assert!(state.operation.try_acquire().is_err());
        drop(permit);
        assert!(state.operation.try_acquire().is_ok());
    }
}
