use std::future::Future;

use futures_util::stream::{SplitSink, SplitStream};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::watch,
};

use super::super::projection::{Encoder, MessageWindow, PublicState, StreamVersion};
use super::*;

pub(super) async fn session<S: AsyncRead + AsyncWrite + Unpin>(
    socket: WebSocketStream<S>,
    core: &Core,
    token: &str,
    version: StreamVersion,
    following: impl Future<Output = Result<Desktop, String>>,
) {
    let (mut writer, reader) = socket.split();
    // One replaceable pending public state, not a queue of Native events.
    let (states, latest) = watch::channel(None);
    let (pongs, pong) = watch::channel(None);
    let mut writing = false;
    let result = match authorize_action(core, token) {
        Err(error) => Err(error),
        Ok(()) => tokio::select! {
            result = read_desktop(reader, core, token, version, following, states, pongs) => result,
            result = write_states(&mut writer, latest, pong, version, &mut writing) => result,
            result = authorization(core, token) => result,
        },
    };
    // A cancelled send may have a partial frame buffered. Never flush stale
    // state after a Native failure or revocation just to send a nicer error.
    if writing {
        return;
    }
    if let Err(error) = result {
        let disconnected = Message::Text(
            json!({"type":"disconnected","message":error})
                .to_string()
                .into(),
        );
        if !matches!(
            tokio::time::timeout(Duration::from_secs(2), writer.send(disconnected)).await,
            Ok(Ok(()))
        ) {
            return;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), writer.close()).await;
}

async fn authorization(core: &Core, token: &str) -> Result<(), String> {
    let mut shutdown = core.shutdown.subscribe();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    loop {
        authorize_action(core, token)?;
        tokio::select! {
            _ = shutdown.changed() => return authorize_action(core, token),
            _ = heartbeat.tick() => {},
        }
    }
}

async fn read_desktop<S: AsyncRead + AsyncWrite + Unpin>(
    mut reader: SplitStream<WebSocketStream<S>>,
    core: &Core,
    token: &str,
    version: StreamVersion,
    following: impl Future<Output = Result<Desktop, String>>,
    states: watch::Sender<Option<Arc<PublicState>>>,
    pongs: watch::Sender<Option<Bytes>>,
) -> Result<(), String> {
    let mut desktop = following.await?;
    authorize_action(core, token)?;
    let binding = super::super::files::BindingGuard::new(
        Arc::clone(&core.files),
        token,
        &desktop.thread,
        &desktop.state,
        &desktop.provenance()?,
    )?;
    let mut window = MessageWindow::default();
    let mut pending = Some(tokio::time::Instant::now());
    loop {
        authorize_action(core, token)?;
        if pending.is_some_and(|deadline| deadline <= tokio::time::Instant::now()) {
            let state = Arc::new(desktop.public_state(version, &mut window));
            let unchanged = states.borrow().as_deref() == Some(state.as_ref());
            if !unchanged {
                drop(states.send_replace(Some(state)));
            }
            pending = None;
        }
        tokio::select! {
            _ = tokio::time::sleep_until(pending.unwrap_or_else(tokio::time::Instant::now)), if pending.is_some() => {},
            result = desktop.update() => {
                if result? {
                    // Always bind the complete current Native references/CWD,
                    // including messages outside this client's display window.
                    binding.update(&desktop.state, &desktop.provenance()?)?;
                    schedule_state(&mut pending, desktop.urgent);
                }
            },
            message = reader.next() => match message {
                Some(Ok(Message::Text(text))) if version == StreamVersion::V2 && history_command(&text) => {
                    window.expand();
                    schedule_state(&mut pending, true);
                },
                Some(Ok(Message::Pong(_))) => {},
                Some(Ok(Message::Ping(bytes))) => {
                    drop(pongs.send_replace(Some(bytes)));
                },
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Err(_)) => return Err("手机连接已断开".into()),
                _ => return Err("实时连接指令无效".into()),
            }
        }
    }
}

fn history_command(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|value| {
        value.as_object().is_some_and(|fields| {
            fields.len() == 1 && fields.get("type").is_some_and(|kind| kind == "history")
        })
    })
}

async fn write_states<S: AsyncRead + AsyncWrite + Unpin>(
    writer: &mut SplitSink<WebSocketStream<S>, Message>,
    mut latest: watch::Receiver<Option<Arc<PublicState>>>,
    mut pongs: watch::Receiver<Option<Bytes>>,
    version: StreamVersion,
    writing: &mut bool,
) -> Result<(), String> {
    let mut encoder = Encoder::new(version);
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(15),
        Duration::from_secs(15),
    );
    loop {
        tokio::select! {
            changed = latest.changed() => {
                changed.map_err(|_| "公开消息流已结束")?;
                // Drop the watch read guard before awaiting a slow network write.
                let state = latest.borrow_and_update().clone();
                if let Some(state) = state
                    && let Some(mut frame) = encoder.prepare(state)?
                {
                    let budget = send_budget(frame.text.len());
                    write_message(writer, Message::Text(std::mem::take(&mut frame.text).into()), budget, writing).await?;
                    encoder.delivered(frame);
                }
            },
            changed = pongs.changed() => {
                changed.map_err(|_| "手机连接已断开")?;
                let bytes = pongs.borrow_and_update().clone();
                if let Some(bytes) = bytes {
                    write_message(writer, Message::Pong(bytes), Duration::from_secs(2), writing).await?;
                }
            },
            _ = heartbeat.tick() => write_message(writer, Message::Ping(Bytes::new()), Duration::from_secs(2), writing).await?,
        }
    }
}

fn send_budget(bytes: usize) -> Duration {
    // Small frames keep their old budget; exceptional large items get a bounded
    // size allowance without delaying Native reads or authorization checks.
    Duration::from_secs((10 + bytes.saturating_sub(256 * 1024).div_ceil(128 * 1024) as u64).min(60))
}

async fn write_message<S: AsyncRead + AsyncWrite + Unpin>(
    writer: &mut SplitSink<WebSocketStream<S>, Message>,
    message: Message,
    budget: Duration,
    writing: &mut bool,
) -> Result<(), String> {
    *writing = true;
    tokio::time::timeout(budget, writer.send(message))
        .await
        .map_err(|_| "手机连接超时")?
        .map_err(|_| "手机连接已断开")?;
    *writing = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_read_only_history_commands_are_accepted() {
        assert!(history_command(r#"{"type":"history"}"#));
        for message in [
            r#"{"type":"send"}"#,
            r#"{"type":"history","action":"send"}"#,
            r#"{"type":"history","count":1000000}"#,
            "null",
            "invalid",
        ] {
            assert!(!history_command(message));
        }
    }

    #[test]
    fn exceptional_large_public_items_have_a_bounded_size_based_send_budget() {
        assert_eq!(send_budget(1), Duration::from_secs(10));
        assert_eq!(send_budget(256 * 1024), Duration::from_secs(10));
        assert!(send_budget(1024 * 1024) > Duration::from_secs(10));
        assert_eq!(send_budget(16 * 1024 * 1024), Duration::from_secs(60));
        assert_eq!(send_budget(usize::MAX), Duration::from_secs(60));
    }
}
