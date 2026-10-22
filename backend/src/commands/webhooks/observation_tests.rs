async fn hold_session_metadata_lock(
    state: &Arc<AppState>,
) -> (oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let state = Arc::clone(state);
    let (locked_tx, locked_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let task = tokio::task::spawn_blocking(move || {
        let _metadata = state.session_metadata_cache.lock().unwrap();
        let _ = locked_tx.send(());
        // 测试结束或断言失败时，发送端被丢弃也会结束等待并释放锁。
        let _ = release_rx.blocking_recv();
    });
    locked_rx.await.unwrap();
    (release_tx, task)
}

#[tokio::test]
async fn notification_is_enqueued_while_metadata_lock_is_held() {
    let directory = tempfile::tempdir().unwrap();
    let state = channel_delivery_test_state(
        directory.path(),
        vec![NotificationChannelConfig {
            id: "isolated-feishu".into(),
            url: "http://127.0.0.1:9/never-sent".into(),
            allow_insecure_test_url: true,
            ..NotificationChannelConfig::default()
        }],
    );
    // 建库与元数据锁无关，先完成初始化再验证持锁期间的入队行为。
    delivery::outbox(&state).await.unwrap();
    let (release_metadata, metadata_task) = hold_session_metadata_lock(&state).await;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        notify_webhook_completion(
            &state,
            &json!({
                "sessionId":"contended-session", "turnId":"failed-turn",
                "confirmedByRollout":true, "rolloutError":"isolated failure",
                "sessionName":"原始提示词不能用作回退标题",
            }),
        ),
    )
    .await
    .expect("metadata contention must not stop enqueue")
    .unwrap();
    assert_eq!(result["status"], "queued");
    let claimed = delivery::outbox(&state)
        .await
        .unwrap()
        .claim_due(delivery::now_ms(), 1, vec![])
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].event.event, "session.failed");
    assert_eq!(claimed[0].event.session_name, "未命名会话");
    drop(release_metadata);
    metadata_task.await.unwrap();
}

#[tokio::test]
async fn notification_title_fallback_uses_only_validated_titles() {
    let directory = tempfile::tempdir().unwrap();
    let state = channel_delivery_test_state(
        directory.path(),
        vec![NotificationChannelConfig {
            id: "isolated-title".into(),
            url: "http://127.0.0.1:9/never-sent".into(),
            allow_insecure_test_url: true,
            ..NotificationChannelConfig::default()
        }],
    );
    let home = notification_codex_home(&state);
    fs::create_dir_all(&home).unwrap();
    let connection = rusqlite::Connection::open(home.join("state_5.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY, name TEXT, title TEXT,
                first_user_message TEXT, preview TEXT
             );
             INSERT INTO threads VALUES (
                'safe-session', '  已验证标题  ', '这是用户的私密提示词',
                '这是用户的私密提示词', '这是用户的私密提示词'
             );",
        )
        .unwrap();
    drop(connection);
    let queued = notify_webhook_completion(
        &state,
        &json!({
            "sessionId":"safe-session", "turnId":"named-turn",
            "confirmedByRollout":true, "sessionName":"这是用户的私密提示词",
        }),
    )
    .await
    .unwrap();
    assert_eq!(queued["status"], "queued");
    let claimed = delivery::outbox(&state)
        .await
        .unwrap()
        .claim_due(delivery::now_ms(), 1, vec![])
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].event.session_name, "已验证标题");
    assert!(
        state
            .session_titles
            .read()
            .await
            .get("safe-session")
            .is_none()
    );
    let (release_metadata, metadata_task) = hold_session_metadata_lock(&state).await;
    assert_eq!(
        webhook_session_name(&state, "safe-session").await,
        "已验证标题"
    );
    state
        .session_titles
        .write()
        .await
        .insert("unknown-session".into(), "缓存的首条提示词".into());
    assert_eq!(
        webhook_session_name(&state, "unknown-session").await,
        "未命名会话"
    );
    assert_eq!(
        state.notification_title_cache.lock().unwrap().titles.len(),
        1
    );
    drop(release_metadata);
    metadata_task.await.unwrap();
}

#[test]
fn notification_title_queries_remain_bounded_after_timeout() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let state = Arc::new(isolated_test_state(directory.path()));
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (started_tx, started_rx) = oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.await.unwrap();
        let mut queries = tokio::task::JoinSet::new();
        for index in 0..128 {
            let state = Arc::clone(&state);
            queries.spawn(async move {
                webhook_session_name(&state, &format!("session-{index}")).await
            });
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(result) = queries.join_next().await {
                assert_eq!(result.unwrap(), "未命名会话");
            }
        })
        .await
        .expect("concurrent callers must finish within the lookup budget");
        assert_eq!(state.notification_title_query.available_permits(), 0);
        assert!(
            state
                .notification_title_cache
                .lock()
                .unwrap()
                .titles
                .is_empty()
        );
        assert_eq!(
            webhook_session_name(&state, "after-timeout").await,
            "未命名会话"
        );
        assert_eq!(state.notification_title_query.available_permits(), 0);
        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while state.notification_title_query.available_permits() == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            state.notification_title_cache.lock().unwrap().titles.len(),
            1
        );
    });
}

#[test]
fn notification_title_cache_is_bounded_and_updates_existing_entries() {
    let mut cache = NotificationTitleCache::default();
    for index in 0..NOTIFICATION_TITLE_CACHE_LIMIT + 40 {
        cache.insert_resolved(format!("session-{index}"), format!("已清理标题 {index}"));
    }
    assert_eq!(cache.titles.len(), NOTIFICATION_TITLE_CACHE_LIMIT);
    assert_eq!(cache.order.len(), NOTIFICATION_TITLE_CACHE_LIMIT);
    assert_eq!(cache.get("session-0"), "未命名会话");
    for _ in 0..128 {
        cache.insert_resolved("session-295".into(), "更新后的标题".into());
    }
    assert_eq!(cache.get("session-295"), "更新后的标题");
    assert_eq!(cache.titles.len(), NOTIFICATION_TITLE_CACHE_LIMIT);
    assert_eq!(cache.order.len(), NOTIFICATION_TITLE_CACHE_LIMIT);
}
