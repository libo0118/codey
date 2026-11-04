use super::*;
use rusqlite::Connection;

fn id(value: u128) -> String {
    uuid::Uuid::from_u128(value).to_string()
}

fn modern_database(path: &Path) -> Connection {
    let db = Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE threads (
            id TEXT PRIMARY KEY, name TEXT, title TEXT, cwd TEXT DEFAULT '/project',
            updated_at_ms INTEGER, archived INTEGER DEFAULT 0,
            source TEXT, thread_source TEXT, parent_thread_id TEXT, parentThreadId TEXT,
            first_user_message TEXT, preview TEXT, forked_from_id TEXT
         );
         CREATE TABLE thread_spawn_edges (parent_thread_id TEXT, child_thread_id TEXT);",
    )
    .unwrap();
    db
}

fn insert(db: &Connection, value: u128, updated: i64) {
    db.execute(
        "INSERT INTO threads (id, updated_at_ms) VALUES (?1, ?2)",
        params![id(value), updated],
    )
    .unwrap();
}

fn listed_ids(home: &Path) -> Vec<String> {
    threads(home, "", false)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn excludes_more_than_200_newer_children_before_the_inventory_limit() {
    let home = tempfile::tempdir().unwrap();
    let db = modern_database(&home.path().join("state_5.sqlite"));
    insert(&db, 1, 1);
    insert(&db, 2, 2);
    db.execute("UPDATE threads SET name='正式主会话' WHERE id=?1", [id(2)])
        .unwrap();
    for value in 3..230 {
        insert(&db, value, value as i64);
        let source = if value % 2 == 0 {
            "subagent".to_string()
        } else {
            json!({"subagent":{"thread_spawn":{"parent_thread_id":id(1)}}}).to_string()
        };
        db.execute(
            "UPDATE threads SET source=?1, title='已命名子代理' WHERE id=?2",
            params![source, id(value)],
        )
        .unwrap();
    }
    assert_eq!(listed_ids(home.path()), vec![id(2), id(1)]);
    assert_eq!(
        threads(home.path(), "", false).unwrap()[1]["title"],
        "未命名会话"
    );
}

#[test]
fn accepts_only_explicit_subagent_sources_and_valid_parent_relationships() {
    let home = tempfile::tempdir().unwrap();
    let db = modern_database(&home.path().join("state_5.sqlite"));
    let parent = id(1);
    let sources = [
        "subagent".to_string(),
        "subagent_thread_spawn".to_string(),
        "\"subagent\"".to_string(),
        json!({"subagent":{"thread_spawn":{"parent_thread_id":parent}}}).to_string(),
        json!({"subagent":"review"}).to_string(),
        json!({"parent_thread_id":parent}).to_string(),
        json!({"source":{"subagent":{"thread_spawn":{"parent_thread_id":parent}}}}).to_string(),
    ];
    for (index, source) in sources.iter().enumerate() {
        let value = index as u128 + 10;
        insert(&db, value, value as i64);
        db.execute(
            "UPDATE threads SET source=?1 WHERE id=?2",
            params![source, id(value)],
        )
        .unwrap();
    }
    for (value, column) in [
        (20, "thread_source"),
        (21, "parent_thread_id"),
        (22, "parentThreadId"),
    ] {
        insert(&db, value, value as i64);
        db.execute(
            &format!("UPDATE threads SET {column}=?1 WHERE id=?2"),
            params![if value == 20 { "subagent" } else { &parent }, id(value)],
        )
        .unwrap();
    }
    insert(&db, 23, 23);
    db.execute(
        "INSERT INTO thread_spawn_edges VALUES (?1, ?2)",
        params![parent, id(23)],
    )
    .unwrap();

    let unknown_sources = [
        "cli",
        "vscode",
        "future_source",
        "subagentish",
        "{broken",
        r#"{"subagent":null}"#,
        r#"{"subagent":false}"#,
        r#"{"subagent":{"future_kind":{}}}"#,
        r#"{"parent_thread_id":"broken"}"#,
    ];
    let mut expected = Vec::new();
    for (index, source) in unknown_sources.iter().enumerate() {
        let value = index as u128 + 30;
        insert(&db, value, value as i64);
        db.execute(
            "UPDATE threads SET source=?1 WHERE id=?2",
            params![source, id(value)],
        )
        .unwrap();
        expected.push(id(value));
    }
    for value in 40..45 {
        insert(&db, value, value as i64);
        expected.push(id(value));
    }
    db.execute(
        "UPDATE threads SET parent_thread_id='broken' WHERE id=?1",
        [id(40)],
    )
    .unwrap();
    db.execute(
        "UPDATE threads SET parent_thread_id=id WHERE id=?1",
        [id(41)],
    )
    .unwrap();
    db.execute(
        "UPDATE threads SET forked_from_id=?1 WHERE id=?2",
        params![parent, id(42)],
    )
    .unwrap();
    db.execute(
        "INSERT INTO thread_spawn_edges VALUES ('broken', ?1)",
        [id(43)],
    )
    .unwrap();
    db.execute("INSERT INTO thread_spawn_edges VALUES (?1, ?1)", [id(44)])
        .unwrap();
    // Damaged optional metadata is not evidence that a valid unnamed root is a child.
    db.execute(
        "UPDATE threads SET source=x'ff', thread_source=42 WHERE id=?1",
        [id(44)],
    )
    .unwrap();
    for (value, source) in [
        (45, r#"{"subagent":{"thread_spawn":"subagent"}}"#),
        (46, r#"{"subagent":{"thread_spawn":{"source":"subagent"}}}"#),
    ] {
        insert(&db, value, value as i64);
        db.execute(
            "UPDATE threads SET source=?1 WHERE id=?2",
            params![source, id(value)],
        )
        .unwrap();
        expected.push(id(value));
    }
    expected.reverse();
    assert_eq!(listed_ids(home.path()), expected);
}

#[test]
fn searches_the_canonical_cross_database_title_not_hidden_titles_or_prompts() {
    let home = tempfile::tempdir().unwrap();
    let state = modern_database(&home.path().join("state_5.sqlite"));
    for value in 1..225 {
        insert(&state, value, value as i64);
    }
    state
        .execute(
            "UPDATE threads SET name=' \t ', title='隐藏旧标题' WHERE id=?1",
            [id(1)],
        )
        .unwrap();
    state
        .execute(
            "UPDATE threads SET name=' 正式\t名称 ', title='隐藏 legacy' WHERE id=?1",
            [id(2)],
        )
        .unwrap();
    state
        .execute(
            "UPDATE threads SET name=?1, title='  legacy\n标题  ' WHERE id=?2",
            params!["\u{00a0}\u{2003}", id(3)],
        )
        .unwrap();
    state
        .execute(
            "UPDATE threads SET first_user_message='秘密提示词', preview='秘密预览' WHERE id=?1",
            [id(4)],
        )
        .unwrap();
    let long_name = format!("{}不可见尾巴", "a".repeat(80));
    state
        .execute(
            "UPDATE threads SET name=?1 WHERE id=?2",
            params![long_name, id(5)],
        )
        .unwrap();
    fs::create_dir(home.path().join("sqlite")).unwrap();
    let catalog = Connection::open(home.path().join("sqlite/codex-dev.db")).unwrap();
    catalog
        .execute_batch(
            "CREATE TABLE local_thread_catalog (
        host_id TEXT, thread_id TEXT, display_title TEXT,
        source_updated_at REAL, observation_sequence INTEGER
    )",
        )
        .unwrap();
    for (host, value, title, updated, sequence) in [
        ("remote", 1, "远端错标题", 100, 1),
        ("local", 1, "过时目录标题", 9, 9),
        ("local", 1, "同时间旧目录标题", 10, 1),
        ("local", 1, " 唯一\n正式标题 ", 10, 2),
        ("local", 1, " \t ", 11, 1),
        ("local", 2, "隐藏 catalog", 10, 1),
    ] {
        catalog
            .execute(
                "INSERT INTO local_thread_catalog VALUES (?1,?2,?3,?4,?5)",
                params![host, id(value), title, updated, sequence],
            )
            .unwrap();
    }
    for (search, value, title) in [
        ("唯一 正式", 1, "唯一 正式标题"),
        ("正式 名称", 2, "正式 名称"),
        ("legacy 标题", 3, "legacy 标题"),
    ] {
        let found = threads(home.path(), search, false).unwrap();
        assert_eq!(found.as_array().unwrap().len(), 1, "search {search}");
        assert_eq!(found[0]["id"], id(value));
        assert_eq!(found[0]["title"], title);
    }
    for hidden in [
        "隐藏",
        "远端错标题",
        "过时目录",
        "同时间旧",
        "秘密",
        "不可见尾巴",
        "' OR 1=1 --",
    ] {
        assert!(
            threads(home.path(), hidden, false)
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty(),
            "search {hidden}"
        );
    }
    assert_eq!(
        threads(home.path(), &"a".repeat(20), false).unwrap()[0]["title"],
        format!("{}…", "a".repeat(80))
    );
    assert!(threads(home.path(), &"x".repeat(201), false).is_err());
}

#[test]
fn a_known_child_cannot_reappear_from_a_legacy_database_without_metadata() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("sqlite")).unwrap();
    let current = modern_database(&home.path().join("sqlite/codex-dev.db"));
    insert(&current, 1, 1);
    current
        .execute("UPDATE threads SET source='subagent' WHERE id=?1", [id(1)])
        .unwrap();
    insert(&current, 2, 2);
    current
        .execute(
            "INSERT INTO thread_spawn_edges VALUES (?1, ?2)",
            params![id(3000), id(2)],
        )
        .unwrap();
    let old = Connection::open(home.path().join("state_5.sqlite")).unwrap();
    old.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, cwd TEXT, updated_at INTEGER, archived INTEGER)").unwrap();
    old.execute(
        "INSERT INTO threads VALUES (?1,'旧库子代理','/project',99999,0)",
        [id(1)],
    )
    .unwrap();
    old.execute(
        "INSERT INTO threads VALUES (?1,'旧库派生关系','/project',99998,0)",
        [id(2)],
    )
    .unwrap();
    for value in 3..210 {
        old.execute(
            "INSERT INTO threads VALUES (?1,NULL,'/project',?2,0)",
            params![id(value), value as i64],
        )
        .unwrap();
    }
    let found = listed_ids(home.path());
    assert_eq!(found.len(), 200);
    assert_eq!(found, (10..210).rev().map(id).collect::<Vec<_>>());
    assert!(
        threads(home.path(), "旧库子代理", false)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn preserves_archive_legacy_schema_latest_record_deduplication_and_read_only_files() {
    let home = tempfile::tempdir().unwrap();
    let legacy_path = home.path().join("state_5.sqlite");
    let old = Connection::open(&legacy_path).unwrap();
    old.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, cwd TEXT, updated_at INTEGER, archived INTEGER)").unwrap();
    for (value, archived) in [(1, 0), (2, 1), (3, 0)] {
        old.execute(
            "INSERT INTO threads VALUES (?1,'旧名称','/legacy',10,?2)",
            params![id(value), archived],
        )
        .unwrap();
    }
    fs::create_dir(home.path().join("sqlite")).unwrap();
    let current_path = home.path().join("sqlite/codex-dev.db");
    let current = modern_database(&current_path);
    insert(&current, 1, 50_000);
    current
        .execute(
            "UPDATE threads SET name=' 正式名称 ', cwd='/current' WHERE id=?1",
            [id(1)],
        )
        .unwrap();
    drop(old);
    drop(current);
    let before = [
        fs::read(&legacy_path).unwrap(),
        fs::read(&current_path).unwrap(),
    ];
    assert!(
        inventory::open_database(&legacy_path)
            .unwrap()
            .execute("DELETE FROM threads", [])
            .is_err()
    );
    let found = threads(home.path(), "", false).unwrap();
    assert_eq!(found.as_array().unwrap().len(), 2);
    assert_eq!(
        found[0],
        json!({"id":id(1),"title":"正式名称","cwd":"/current","updatedAt":50_000})
    );
    assert_eq!(found[1]["id"], id(3));
    assert_eq!(found[1]["updatedAt"], 10_000);
    assert_eq!(threads(home.path(), "", true).unwrap()[0]["id"], id(2));
    assert_eq!(
        threads(home.path(), "/legacy", false)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fs::read(&legacy_path).unwrap(), before[0]);
    assert_eq!(fs::read(&current_path).unwrap(), before[1]);
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 2);
    assert_eq!(fs::read_dir(home.path().join("sqlite")).unwrap().count(), 1);
}

#[test]
fn supports_minimal_threads_and_catalog_without_host_or_ordering_columns() {
    let home = tempfile::tempdir().unwrap();
    let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE threads (id TEXT PRIMARY KEY, cwd TEXT);
        CREATE TABLE local_thread_catalog (thread_id TEXT, display_title TEXT);",
    )
    .unwrap();
    db.execute("INSERT INTO threads VALUES (?1, '/project')", [id(1)])
        .unwrap();
    db.execute(
        "INSERT INTO local_thread_catalog VALUES (?1, '旧目录名称')",
        [id(1)],
    )
    .unwrap();
    db.execute("INSERT INTO threads VALUES ('invalid-id', '/project')", [])
        .unwrap();
    assert_eq!(
        threads(home.path(), "旧目录", false).unwrap()[0],
        json!({"id":id(1),"title":"旧目录名称","cwd":"/project","updatedAt":0})
    );
    assert!(
        threads(home.path(), "", true)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn metadata_is_read_once_per_id_and_refreshed_on_the_next_inventory_request() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("state_5.sqlite");
    let db = modern_database(&path);
    insert(&db, 1, 1);
    db.execute("UPDATE threads SET name='原名称'", []).unwrap();
    let query = Connection::open_in_memory().unwrap();
    let register = || {
        inventory::register_inventory_functions(
            &query,
            Arc::new(Mutex::new(
                inventory::ThreadInventory::new(home.path(), std::slice::from_ref(&path)).unwrap(),
            )),
        )
        .unwrap();
    };
    let read = || {
        query
            .query_row(
                "SELECT codey_root_thread(?1), codey_thread_title(?1)",
                [id(1)],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap()
    };
    register();
    assert_eq!(read(), (true, "原名称".to_string()));
    db.execute("UPDATE threads SET name='新名称', source='subagent'", [])
        .unwrap();
    assert_eq!(read(), (true, "原名称".to_string()));
    register();
    assert_eq!(read(), (false, "新名称".to_string()));
}

#[test]
fn identity_query_errors_fail_the_udf_instead_of_allowing_a_known_child() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("state_5.sqlite");
    let db = modern_database(&path);
    insert(&db, 1, 1);
    db.execute("UPDATE threads SET source='subagent'", [])
        .unwrap();
    let inventory = inventory::ThreadInventory::new(home.path(), &[path]).unwrap();
    db.execute_batch("DROP TABLE thread_spawn_edges").unwrap();
    let query = Connection::open_in_memory().unwrap();
    inventory::register_inventory_functions(&query, Arc::new(Mutex::new(inventory))).unwrap();
    assert!(
        query
            .query_row("SELECT codey_root_thread(?1)", [id(1)], |row| row
                .get::<_, bool>(0))
            .is_err()
    );
}

#[test]
fn locked_identity_database_cannot_resurrect_a_child_from_a_stale_database() {
    for current_in_sqlite in [false, true] {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join("sqlite")).unwrap();
        let current_path = home.path().join(if current_in_sqlite {
            "sqlite/codex-dev.db"
        } else {
            "state_5.sqlite"
        });
        let old_path = home.path().join(if current_in_sqlite {
            "state_5.sqlite"
        } else {
            "sqlite/codex-dev.db"
        });
        let current = modern_database(&current_path);
        insert(&current, 1, 1);
        insert(&current, 2, 2);
        current
            .execute("UPDATE threads SET source='subagent' WHERE id=?1", [id(2)])
            .unwrap();
        let old = Connection::open(old_path).unwrap();
        old.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, cwd TEXT, updated_at INTEGER, archived INTEGER)").unwrap();
        old.execute(
            "INSERT INTO threads VALUES (?1,'旧库无来源记录','/project',99999,0)",
            [id(2)],
        )
        .unwrap();
        current.execute_batch("BEGIN EXCLUSIVE").unwrap();
        assert!(threads(home.path(), "", false).is_err());
        current.execute_batch("ROLLBACK").unwrap();
        assert_eq!(listed_ids(home.path()), vec![id(1)]);
    }
}

#[test]
fn nonexistent_optional_database_is_not_an_identity_read_failure() {
    let home = tempfile::tempdir().unwrap();
    let missing = home.path().join("missing.sqlite");
    assert!(inventory::ThreadInventory::new(home.path(), &[missing]).is_ok());
}
