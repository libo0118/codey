use std::{path::Path, sync::Arc};

use serde_json::{Value, json};

use crate::commands::AppState;

pub(super) async fn desktop_request(state: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let runtime = state
        .runtime
        .lock()
        .await
        .clone()
        .ok_or("Codex 桌面尚未启动")?;
    let websocket = runtime.renderer_websocket_url().await;
    crate::cdp::remote_control_request(&websocket, &args).await
}

pub(super) async fn open(state: &Arc<AppState>, id: &str) -> Result<(), String> {
    super::desktop::validate_thread(id)?;
    desktop_request(state, json!({"action":"resume","threadId":id})).await?;
    Ok(())
}

// Called only for the first submitted message; an untouched phone draft does
// not create a persisted thread or a second app-server.
pub(super) async fn create(
    state: &Arc<AppState>,
    args: &Value,
    authorize: impl Fn() -> Result<(), String>,
) -> Result<Value, String> {
    let project = saved_project(args).await?;
    let mut params = creation_params(&project, args)?;
    if let Some(cwd) = params["cwd"].as_str()
        && !Path::new(cwd).is_dir()
    {
        return Err("项目目录不存在，请在电脑端检查工作区".into());
    }
    if !super::images::message_files(args)?.is_empty() {
        let (input, attachments) = prepare_files(state, args, &authorize).await?;
        params["attachments"] = json!(attachments);
        params["input"] = json!(input);
    } else {
        authorize()?;
    }
    let result = desktop_request(state, json!({"action":"create","params":params})).await?;
    super::desktop::validate_thread(result["id"].as_str().ok_or("创建会话结果无效")?)?;
    Ok(result)
}

pub(super) async fn prepare_files(
    state: &Arc<AppState>,
    args: &Value,
    authorize: impl Fn() -> Result<(), String>,
) -> Result<(Vec<Value>, Vec<Value>), String> {
    let input = super::images::message_input(args)?;
    let files = args["files"]
        .as_array()
        .filter(|files| !files.is_empty())
        .ok_or("附件列表无效")?
        .iter()
        .map(|file| json!({"name":file["name"],"url":file["url"]}))
        .collect::<Vec<_>>();
    authorize()?;
    let uploaded = desktop_request(state, json!({"action":"attachments","files":files})).await?;
    finish_files(
        args,
        &uploaded,
        input,
        async {
            let runtime = state
                .runtime
                .lock()
                .await
                .clone()
                .ok_or("Codex 桌面尚未启动")?;
            super::server::require_model_request_gate(&runtime.renderer_websocket_url().await)
                .await?;
            authorize()
        },
        async {
            desktop_request(
                state,
                json!({"action":"discard-attachments","files":uploaded["files"]}),
            )
            .await?;
            Ok(())
        },
    )
    .await
}

async fn finish_files(
    args: &Value,
    uploaded: &Value,
    mut input: Vec<Value>,
    check: impl std::future::Future<Output = Result<(), String>>,
    discard: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(Vec<Value>, Vec<Value>), String> {
    let prepared = async {
        let attachments = super::images::attach_files(&mut input, args, uploaded)?;
        check.await?;
        Ok::<_, String>((input, attachments))
    }
    .await;
    match prepared {
        Ok(value) => Ok(value),
        Err(error) => {
            if discard.await.is_err() {
                return Err(format!("{error}；附件清理未确认，请在电脑端检查"));
            }
            Err(error)
        }
    }
}

pub(super) async fn defaults(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let project = saved_project(args).await?;
    let request = if project.is_null() {
        json!({"action":"defaults"})
    } else {
        json!({"action":"defaults","cwd":project["cwd"]})
    };
    desktop_request(state, request).await
}

async fn saved_project(args: &Value) -> Result<Value, String> {
    match args.get("projectId") {
        Some(Value::Null) => return Ok(Value::Null),
        Some(Value::String(_)) => {}
        _ => return Err("请选择工作区或明确选择不指定工作目录".into()),
    }
    let projects =
        tokio::task::spawn_blocking(|| super::store::projects(crate::codex_config::codex_home()))
            .await
            .map_err(|_| "读取项目任务失败")??;
    projects
        .as_array()
        .into_iter()
        .flatten()
        .find(|project| project["id"] == args["projectId"])
        .cloned()
        .ok_or_else(|| "请选择 Codex 中已保存的本地项目".into())
}

fn creation_params(project: &Value, args: &Value) -> Result<Value, String> {
    let projectless = project.is_null();
    let (_, _, turn) = super::desktop::action_params(&json!({}), "send", args)?;
    let mut params = json!({
        "input":turn["turnStart"]["request"]["input"],
        "clientUserMessageId":args["requestId"],
        "useAppServerPermissionDefault":true,
        "attachments":[], "commentAttachments":[], "collaborationMode":null,
    });
    if projectless {
        params["workspaceKind"] = json!("projectless");
        params["workspaceRoots"] = json!([]);
    } else {
        let cwd = project["cwd"]
            .as_str()
            .filter(|cwd| !cwd.is_empty())
            .ok_or("项目目录无效")?;
        params["cwd"] = json!(cwd);
        params["workspaceRoots"] = json!(
            project["rootPaths"]
                .as_array()
                .filter(|roots| !roots.is_empty())
                .cloned()
                .unwrap_or_else(|| vec![json!(cwd)])
        );
        params["workspaceKind"] = json!("project");
        params["projectAssignment"] = json!({"projectId":project["id"],"projectKind":"local"});
    }
    if args.get("model").is_some()
        || args.get("effort").is_some()
        || args.get("serviceTier").is_some()
        || args.get("permissionMode").is_some()
    {
        let (_, _, settings) = super::desktop::action_params(
            &json!({"workspaceKind":params["workspaceKind"]}),
            "settings",
            args,
        )?;
        let settings = &settings["threadSettings"];
        if settings.get("effort").is_some() && settings.get("model").is_none() {
            return Err("请先选择模型再设置思考程度".into());
        }
        if let Some(model) = settings.get("model") {
            params["collaborationMode"] = json!({"mode":"default","settings":{"model":model,"reasoning_effort":settings["effort"],"developer_instructions":null}});
        }
        if let Some(tier) = settings.get("serviceTier") {
            params["serviceTier"] = tier.clone();
        }
        if settings.get("permissions").is_some() {
            let mode = args["permissionMode"].as_str().ok_or("权限模式无效")?;
            params["permissionSelection"] = json!({"kind":"agent-mode","agentMode":if projectless && mode == "auto" { "granular" } else { mode }});
            params["usePermissionSelection"] = json!(true);
        }
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prepared_creation_files_recheck_authorization_and_discard_on_failure() {
        use std::cell::Cell;

        let args = json!({"text":"","files":[{"name":"a.txt","url":"data:application/octet-stream;base64,YQ=="}],"requestId":uuid::Uuid::new_v4().to_string()});
        let project = json!({"id":"p","cwd":"E:/code/codey"});
        let params = creation_params(&project, &args).unwrap();
        let input = params["input"].as_array().unwrap().clone();
        let native =
            json!({"files":[{"label":"a.txt","path":"/managed/a.txt","fsPath":"/managed/a.txt"}]});
        let discarded = Cell::new(false);
        let (prepared, attachments) =
            finish_files(&args, &native, input.clone(), async { Ok(()) }, async {
                discarded.set(true);
                Ok(())
            })
            .await
            .unwrap();
        assert!(!discarded.get());
        assert!(
            prepared[0]["text"]
                .as_str()
                .unwrap()
                .contains("## a.txt: /managed/a.txt")
        );
        assert_eq!(json!(attachments), native["files"]);
        let error = finish_files(
            &args,
            &native,
            input.clone(),
            async { Err("登录会话已撤销".into()) },
            async {
                discarded.set(true);
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error, "登录会话已撤销");
        assert!(discarded.get());
        discarded.set(false);
        let invalid =
            json!({"files":[{"label":"wrong","path":"/managed/a.txt","fsPath":"/managed/a.txt"}]});
        assert!(
            finish_files(
                &args,
                &invalid,
                input.clone(),
                async { panic!("invalid metadata must not pass the gate") },
                async {
                    discarded.set(true);
                    Ok(())
                }
            )
            .await
            .is_err()
        );
        assert!(discarded.get());
        let error = finish_files(
            &args,
            &native,
            input,
            async { Err("登录会话已撤销".into()) },
            async { Err("offline".into()) },
        )
        .await
        .unwrap_err();
        assert!(error.contains("附件清理未确认"));
    }

    #[test]
    fn first_message_uses_native_creation_without_a_manual_title() {
        let project =
            json!({"id":"p","cwd":"E:/code/codey","rootPaths":["E:/code/codey","E:/other"]});
        let args = json!({"text":"开始任务","requestId":uuid::Uuid::new_v4().to_string()});
        let params = creation_params(&project, &args).unwrap();
        assert_eq!(params["input"][0]["text"], "开始任务");
        assert_eq!(
            params["projectAssignment"],
            json!({"projectId":"p","projectKind":"local"})
        );
        assert_eq!(params["workspaceRoots"], project["rootPaths"]);
        assert_eq!(params["useAppServerPermissionDefault"], true);
        assert!(params.get("initialTitle").is_none());
        assert!(params.get("permissionsConfig").is_none());
        assert!(params.get("serviceTier").is_none());
    }

    #[test]
    fn creation_validates_messages_and_only_applies_explicit_settings() {
        let project = json!({"id":"p","cwd":"E:/code/codey"});
        let args = json!({"text":"测试","requestId":uuid::Uuid::new_v4().to_string(),"model":"route/model","effort":"high","permissionMode":"read-only"});
        let params = creation_params(&project, &args).unwrap();
        assert_eq!(
            params["collaborationMode"]["settings"]["model"],
            "route/model"
        );
        assert_eq!(
            params["permissionSelection"],
            json!({"kind":"agent-mode","agentMode":"read-only"})
        );
        assert_eq!(params["usePermissionSelection"], true);
        assert!(params.get("permissionsConfig").is_none());
        for patch in [
            json!({"text":" "}),
            json!({"text":"a".repeat(100_001)}),
            json!({"requestId":"invalid"}),
            json!({"effort":"invalid"}),
            json!({"permissionMode":"invalid"}),
        ] {
            let mut invalid = args.clone();
            for (key, value) in patch.as_object().unwrap() {
                invalid[key] = value.clone();
            }
            assert!(creation_params(&project, &invalid).is_err());
        }
        assert!(creation_params(&json!({"id":"p"}), &args).is_err());
        assert!(
            creation_params(
                &project,
                &json!({"text":"ok","requestId":args["requestId"],"effort":"high"})
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn projectless_is_explicit_and_uses_native_workspace_and_permission_selection() {
        assert_eq!(
            saved_project(&json!({"projectId":null})).await.unwrap(),
            Value::Null
        );
        assert!(saved_project(&json!({})).await.is_err());
        assert!(saved_project(&json!({"projectId":true})).await.is_err());
        for (mode, native_mode) in [
            ("read-only", "read-only"),
            ("auto", "granular"),
            ("full-access", "full-access"),
        ] {
            let args = json!({"projectId":null,"text":"开始","requestId":uuid::Uuid::new_v4().to_string(),"permissionMode":mode,"cwd":"/untrusted","workspaceRoots":["/"]});
            let params = creation_params(&Value::Null, &args).unwrap();
            assert_eq!(params["workspaceKind"], "projectless");
            assert_eq!(params["workspaceRoots"], json!([]));
            assert!(params.get("cwd").is_none());
            assert!(params.get("projectAssignment").is_none());
            assert!(params.get("permissionsConfig").is_none());
            assert_eq!(
                params["permissionSelection"],
                json!({"kind":"agent-mode","agentMode":native_mode})
            );
        }
    }

    #[test]
    fn creation_passes_fast_and_explicit_standard_to_the_first_turn() {
        let project = json!({"id":"p","cwd":"E:/code/codey"});
        for tier in [json!("priority"), json!("default"), Value::Null] {
            let args = json!({"text":"开始","requestId":uuid::Uuid::new_v4().to_string(),"serviceTier":tier});
            let params = creation_params(&project, &args).unwrap();
            assert_eq!(params.get("serviceTier"), Some(&tier));
            assert_eq!(params["collaborationMode"], Value::Null);
            assert_eq!(params["useAppServerPermissionDefault"], true);
        }
        let args = json!({"text":"开始","requestId":uuid::Uuid::new_v4().to_string(),"serviceTier":"fast"});
        assert!(
            creation_params(&project, &args)
                .unwrap_err()
                .contains("速度模式")
        );
    }

    #[test]
    fn first_message_supports_photos_without_text_and_rejects_invalid_images() {
        let project = json!({"id":"p","cwd":"E:/code/codey"});
        let url = "data:image/gif;base64,R0lGODlh";
        let mut args =
            json!({"text":"","images":[{"url":url}],"requestId":uuid::Uuid::new_v4().to_string()});
        let params = creation_params(&project, &args).unwrap();
        assert_eq!(params["input"], json!([{"type":"image","url":url}]));
        assert_eq!(params["attachments"], json!([]));
        assert_eq!(params["useAppServerPermissionDefault"], true);
        args["images"][0]["url"] = json!("https://example.test/private.png");
        assert!(creation_params(&project, &args).is_err());
    }
}
