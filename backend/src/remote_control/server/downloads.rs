use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};

use super::super::files::{self, Selection};
use super::*;
use http_body_util::StreamBody;

fn check(core: &Core, token: &str, selection: &Selection) -> Result<(), String> {
    authorize_action(core, token)?;
    if !core
        .files
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .current(selection)
    {
        return Err("会话连接或文件引用已变化，请重新打开会话".into());
    }
    Ok(())
}

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) async fn resolve(core: &Arc<Core>, token: &str, args: Value) -> Result<Value, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Resolve {
        thread_id: String,
        reference: String,
    }
    let request: Resolve = serde_json::from_value(args).map_err(|_| "文件解析字段无效")?;
    authorize_action(core, token)?;
    let selection = core
        .files
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .selection(token, &request.thread_id, &request.reference)?;
    let cancelled = Cancel(Arc::new(AtomicBool::new(false)));
    let stop = cancelled.0.clone();
    let current = Arc::clone(core);
    let credential = token.to_string();
    let result = tokio::task::spawn_blocking(move || {
        files::describe(selection.clone(), || {
            if stop.load(Ordering::Acquire) {
                return Err("文件解析已取消".into());
            }
            check(&current, &credential, &selection)
        })
    });
    let descriptor = tokio::time::timeout(Duration::from_secs(10), result)
        .await
        .map_err(|_| "文件解析超时，请稍后重试")?
        .map_err(|_| "文件解析任务失败")??;
    authorize_action(core, token)?;
    core.files
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(descriptor)
}

pub(super) async fn download(core: Arc<Core>, token: String, thread: String, id: String) -> Reply {
    if let Err(error) = authorize_action(&core, &token) {
        return failure(StatusCode::UNAUTHORIZED, &error);
    }
    let descriptor = match core
        .files
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cached(&token, &thread, &id)
    {
        Ok(descriptor) => descriptor,
        Err(error) => return failure(StatusCode::NOT_FOUND, &error),
    };
    let Ok(permit) = core.file_transfers.clone().try_acquire_owned() else {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "同时下载的文件过多，请稍后重试",
        );
    };
    let selection = descriptor.selection();
    let cancelled = Cancel(Arc::new(AtomicBool::new(false)));
    let stop = cancelled.0.clone();
    let current = Arc::clone(&core);
    let credential = token.clone();
    let selected = selection.clone();
    let preparation = tokio::task::spawn_blocking(move || {
        let snapshot = files::prepare_snapshot(&current.files, &descriptor, || {
            if stop.load(Ordering::Acquire) {
                return Err("文件下载已取消".into());
            }
            check(&current, &credential, &selected)
        });
        snapshot.map(|snapshot| (Arc::new(snapshot), permit))
    });
    let (snapshot, permit) = match tokio::time::timeout(Duration::from_secs(30), preparation).await
    {
        Ok(Ok(Ok(result))) => result,
        Ok(Ok(Err(error))) => return failure(StatusCode::CONFLICT, &error),
        _ => {
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "下载文件校验未完成，请稍后重试",
            );
        }
    };
    if let Err(error) = check(&core, &token, &selection) {
        return failure(StatusCode::UNAUTHORIZED, &error);
    }
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<hyper::body::Frame<Bytes>, io::Error>>(2);
    let current = Arc::clone(&core);
    let credential = token.clone();
    let selected = selection.clone();
    let name = files::disposition(&snapshot.name);
    let size = snapshot.size;
    let mime = snapshot.mime;
    let etag = format!("\"sha256-{}\"", snapshot.digest);
    tokio::spawn(async move {
        let _permit = permit;
        let mut shutdown = current.shutdown.subscribe();
        let mut heartbeat = tokio::time::interval(Duration::from_millis(250));
        let mut offset = 0u64;
        let result = async {
            while offset < snapshot.size {
                let idle_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                let slot = loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.changed() => return Err("远程服务已停止".to_string()),
                        _ = sender.closed() => return Ok(()),
                        _ = tokio::time::sleep_until(idle_deadline) => return Err("下载连接空闲超时，请重新下载".to_string()),
                        _ = heartbeat.tick() => check(&current,&credential,&selected)?,
                        slot = sender.reserve() => break slot.map_err(|_|"下载连接已断开")?,
                    }
                };
                check(&current,&credential,&selected)?;
                let file = Arc::clone(&snapshot);
                let bytes = tokio::select! {
                    _ = shutdown.changed() => return Err("远程服务已停止".to_string()),
                    _ = sender.closed() => return Ok(()),
                    bytes = tokio::task::spawn_blocking(move ||file.read(offset)) => bytes.map_err(|_|"下载读取任务失败")??,
                };
                check(&current,&credential,&selected)?;
                offset += bytes.len() as u64;
                slot.send(Ok(hyper::body::Frame::data(Bytes::from(bytes))));
            }
            Ok::<_,String>(())
        }.await;
        if let Err(error) = result {
            let _ = sender.try_send(Err(io::Error::other(error)));
        }
    });
    // The body owns no snapshot: a stalled peer cannot retain private files after
    // the producer observes revocation/shutdown or the receiver is dropped.
    let stream = futures_util::stream::try_unfold(
        (receiver, core, token, selection),
        |(mut receiver, core, token, selection)| async move {
            check(&core, &token, &selection).map_err(io::Error::other)?;
            let next = receiver.recv().await;
            check(&core, &token, &selection).map_err(io::Error::other)?;
            match next {
                Some(frame) => Ok(Some((frame?, (receiver, core, token, selection)))),
                None => Ok::<_, io::Error>(None),
            }
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", mime)
        .header("content-length", size)
        .header("content-disposition", name)
        .header("etag", etag)
        .header("cache-control", "no-store")
        .header("x-content-type-options", "nosniff")
        .body(StreamBody::new(stream).boxed_unsync())
        .expect("generated file headers")
}
