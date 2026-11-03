// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
use super::{AppState, argument, string_argument};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

async fn snapshot(state: &Arc<AppState>) -> Result<Value> {
    let mut status = tokio::task::spawn_blocking(|| {
        crate::ponytail::status(
            crate::codex_config::codex_home(),
            &crate::ponytail::user_config_path()?,
            std::env::var("PONYTAIL_DEFAULT_MODE").ok().as_deref(),
        )
    })
    .await??;
    status["job"] = json!(state.builtin_tools.ponytail_job.lock().await.clone());
    Ok(status)
}

pub(super) async fn invoke(state: &Arc<AppState>, args: &Value) -> Result<Value> {
    let action = string_argument(args, "action").map_err(anyhow::Error::msg)?;
    if action == "status" {
        return snapshot(state).await;
    }
    ensure!(
        matches!(action.as_str(), "set_enabled" | "save_settings"),
        "未知 Ponytail 操作"
    );
    ensure!(
        !state.is_shutting_down() && !state.restart_in_progress.load(Ordering::Acquire),
        "Codey 正在退出或重启，请稍后重试"
    );
    let permit = state
        .builtin_tools
        .ponytail_operation
        .clone()
        .try_acquire_owned()
        .context("已有 Ponytail 操作进行中，请等待完成")?;
    if action == "save_settings" {
        let mode = argument::<crate::ponytail::Mode>(args, "defaultMode")
            .map_err(|_| anyhow::anyhow!("Ponytail 默认模式必须是 lite、full、ultra 或 off"))?;
        let revision = string_argument(args, "revision").map_err(anyhow::Error::msg)?;
        tokio::task::spawn_blocking(move || {
            crate::ponytail::save_settings(&crate::ponytail::user_config_path()?, mode, &revision)
        })
        .await??;
        let mut status = snapshot(state)
            .await
            .context("Ponytail 参数已保存，但无法刷新状态，请刷新确认后再操作")?;
        status["message"] = json!(
            "默认模式已保存，新聊天生效；现有聊天可用 /ponytail 切换。环境变量如有设置会优先使用。"
        );
        return Ok(status);
    }
    let enabled = argument::<bool>(args, "enabled").map_err(anyhow::Error::msg)?;
    let mut current = snapshot(state).await?;
    ensure!(
        current["conflict"].is_null(),
        "Ponytail 市场来源冲突，请先在 Codex 插件管理中处理"
    );
    if !enabled || current["installed"] == true {
        tokio::task::spawn_blocking(move || {
            crate::ponytail::set_enabled(crate::codex_config::codex_home(), enabled)
        })
        .await??;
        *state.builtin_tools.ponytail_job.lock().await = None;
        let mut status = snapshot(state)
            .await
            .context("Ponytail 启停状态已保存，但无法刷新状态，请刷新确认后再操作")?;
        status["message"] = json!(if enabled {
            "Ponytail 已启用，请重启 Codex 并开始新聊天。首次使用请在 /hooks 中检查并信任官方钩子。"
        } else {
            "Ponytail 已停用，请重启 Codex 并开始新聊天；安装文件和参数已保留。"
        });
        return Ok(status);
    }
    let configured = state.config.read().await.codex_app_path.clone();
    let executable =
        tokio::task::spawn_blocking(move || crate::ponytail::cli(&configured)).await??;
    let home: PathBuf = crate::codex_config::codex_home().into();
    let job = json!({"id":uuid::Uuid::new_v4().to_string(),"phase":"running","message":"正在通过 Codex 安装 Ponytail 官方插件…"});
    current["job"] = job.clone();
    *state.builtin_tools.ponytail_job.lock().await = Some(job.clone());
    let worker = Arc::clone(state);
    tokio::spawn(async move {
        let _permit = permit;
        let outcome =
            tokio::spawn(async move { crate::ponytail::install(&executable, &home).await }).await;
        let mut result = job;
        match outcome {
            Ok(Ok(())) => {
                result["phase"] = json!("succeeded");
                result["message"] = json!(
                    "Ponytail 安装完成。请重启 Codex，在 /hooks 中检查并信任官方钩子，再开始新聊天。"
                );
            }
            error => {
                result["phase"] = json!("failed");
                result["message"] = json!(match error {
                    Ok(Err(error)) => format!("{error:#}"),
                    _ => "Ponytail 安装任务异常退出，请刷新后重试".into(),
                });
            }
        }
        *worker.builtin_tools.ponytail_job.lock().await = Some(result);
    });
    Ok(current)
}
