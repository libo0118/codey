use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags};

use super::{SessionEntry, entry, files};
use crate::sqlite_util::table_columns;

fn paths(home: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for directory in [home.to_path_buf(), home.join("sqlite")] {
        if !files::exists(&directory)? {
            continue;
        }
        files::checked(home, &directory)?;
        for item in fs::read_dir(directory)? {
            let path = item?.path();
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or_default();
            let candidate = if path.parent() == Some(home) {
                [
                    "state_",
                    "thread_history_",
                    "goals_",
                    "memories_",
                    "queue_",
                    "logs_",
                ]
                .iter()
                .any(|prefix| name.starts_with(prefix) && name.ends_with(".sqlite"))
            } else {
                matches!(
                    path.extension().and_then(|v| v.to_str()),
                    Some("db" | "sqlite" | "sqlite3")
                )
            };
            if candidate {
                files::checked(home, &path)?;
                for suffix in ["-wal", "-shm", "-journal"] {
                    let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
                    if files::exists(&sidecar)? {
                        files::checked(home, &sidecar)?;
                    }
                }
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn open(path: &Path, write: bool) -> Result<Connection> {
    let db = Connection::open_with_flags(
        path,
        if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        },
    )
    .with_context(|| format!("无法打开会话数据库 {}", path.display()))?;
    db.busy_timeout(Duration::from_secs(3))?;
    Ok(db)
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn tables(db: &Connection) -> Result<Vec<String>> {
    Ok(db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn local_filter(db: &Connection, columns: &std::collections::HashSet<String>) -> Result<String> {
    if !columns.contains("host_id") {
        return Ok("1=1".into());
    }
    let hosts = table_columns(db, "local_thread_catalog_hosts")?;
    Ok(
        if hosts.contains("host_id") && hosts.contains("host_kind") {
            "(host_id = 'local' OR host_id IN (SELECT host_id FROM local_thread_catalog_hosts WHERE host_kind = 'local'))".into()
        } else {
            "host_id = 'local'".into()
        },
    )
}

fn id_columns(table: &str, columns: &std::collections::HashSet<String>) -> Vec<String> {
    let mut result = [
        "thread_id",
        "session_id",
        "parent_thread_id",
        "child_thread_id",
        "assigned_thread_id",
        "target_thread_id",
    ]
    .into_iter()
    .filter(|column| columns.contains(*column))
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if matches!(table, "threads" | "sessions") && columns.contains("id") {
        result.push("id".into());
    }
    if table == "jobs" && columns.contains("job_key") {
        result.push("job_key".into());
    }
    if table == "rollout_migration_state" && columns.contains("last_checked_thread_id") {
        result.push("last_checked_thread_id".into());
    }
    result
}

fn validate_rollout_paths(home: &Path, db: &Connection) -> Result<()> {
    for table in ["threads", "sessions"] {
        let columns = table_columns(db, table)?;
        if !columns.contains("rollout_path") {
            continue;
        }
        let filter = local_filter(db, &columns)?;
        let sql = format!(
            "SELECT rollout_path FROM {} WHERE ({filter}) AND rollout_path IS NOT NULL AND rollout_path != ''",
            quote(table)
        );
        for value in db
            .prepare(&sql)?
            .query_map([], |row| row.get::<_, String>(0))?
        {
            let path = PathBuf::from(value?);
            let path = if path.is_absolute() {
                path
            } else {
                home.join(path)
            };
            if !files::exists(&path)? {
                continue;
            }
            // Codex 在 Windows 上可能保存带 \\?\ 前缀的规范路径。
            // 先统一为真实路径，再使用本机目录中的相对路径检查边界。
            let canonical = path.canonicalize()?;
            let root = home.canonicalize()?;
            let relative = canonical
                .strip_prefix(&root)
                .context("会话文件不在 Codex 数据目录内，已停止清理")?;
            ensure!(
                ["sessions", "archived_sessions"]
                    .iter()
                    .any(|folder| relative.starts_with(folder)),
                "会话文件不在受支持的会话目录内，已停止清理"
            );
            files::checked(home, &home.join(relative))?;
        }
    }
    Ok(())
}

pub(super) fn inventory(home: &Path, entries: &mut BTreeMap<String, SessionEntry>) -> Result<()> {
    for path in paths(home)? {
        let db = open(&path, false)?;
        validate_rollout_paths(home, &db)?;
        for table in tables(&db)? {
            let columns = table_columns(&db, &table)?;
            let filter = local_filter(&db, &columns)?;
            for id_column in id_columns(&table, &columns) {
                // 旁表和日志也纳入清单，保证孤立历史可以被选择清理。
                let sql = format!(
                    "SELECT DISTINCT {} FROM {} WHERE {filter} AND {} IS NOT NULL AND {} != ''",
                    quote(&id_column),
                    quote(&table),
                    quote(&id_column),
                    quote(&id_column)
                );
                for id in db
                    .prepare(&sql)?
                    .query_map([], |row| row.get::<_, String>(0))?
                {
                    let id = id?;
                    // 云端/远程标识不属于当前本机的 UUID 会话。
                    if super::valid_id(&id) {
                        entry(entries, &id)?;
                    }
                }
            }
            let id = match table.as_str() {
                "threads" | "sessions" => "id",
                "local_thread_catalog" => "thread_id",
                _ => continue,
            };
            if !columns.contains(id) {
                continue;
            }
            let field = |names: &[&str], fallback: &str| {
                names
                    .iter()
                    .find(|name| columns.contains(**name))
                    .map(|name| format!("COALESCE({}, {fallback})", quote(name)))
                    .unwrap_or_else(|| fallback.into())
            };
            let title = ["name", "title", "display_title"]
                .iter()
                .filter(|name| columns.contains(**name))
                .map(|name| format!("NULLIF({}, '')", quote(name)))
                .chain(std::iter::once("''".into()))
                .collect::<Vec<_>>();
            let title = if title.len() > 1 {
                format!("COALESCE({})", title.join(", "))
            } else {
                "''".into()
            };
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {} FROM {} WHERE {filter}",
                quote(id),
                title,
                field(&["cwd"], "''"),
                field(&["updated_at_ms"], "0"),
                field(&["updated_at"], "0"),
                field(&["source_updated_at"], "0"),
                field(&["archived"], "0"),
                quote(&table)
            );
            let rows = db
                .prepare(&sql)?
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        [
                            row.get::<_, f64>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, f64>(5)?,
                        ],
                        row.get::<_, i64>(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, title, cwd, timestamps, archived) in rows {
                if !super::valid_id(&id) {
                    continue;
                }
                let value = entry(entries, &id)?;
                if !title.is_empty() {
                    value.title = title;
                }
                if !cwd.is_empty() {
                    value.cwd = cwd;
                }
                // 兼容字段可能为空或不同步，任一来源的近期活动都应保留会话。
                for timestamp in timestamps {
                    value.updated_at = value.updated_at.max(if timestamp < 100_000_000_000.0 {
                        (timestamp * 1000.0) as i64
                    } else {
                        timestamp as i64
                    });
                }
                value.archived |= archived != 0;
            }
        }
    }
    Ok(())
}

pub(super) fn clean(home: &Path, ids: &BTreeSet<String>, stopped: bool) -> Result<()> {
    for path in paths(home)? {
        clean_database(home, &path, ids, stopped)
            .with_context(|| format!("清理会话数据库 {} 失败", path.display()))?;
    }
    Ok(())
}

fn clean_database(home: &Path, path: &Path, ids: &BTreeSet<String>, stopped: bool) -> Result<()> {
    let mut db = open(path, true)?;
    validate_rollout_paths(home, &db)?;
    db.execute_batch(
        "PRAGMA secure_delete=ON; PRAGMA foreign_keys=ON;
        CREATE TEMP TABLE codey_selected_sessions (id TEXT PRIMARY KEY);",
    )?;
    {
        let transaction = db.transaction()?;
        for id in ids {
            transaction.execute("INSERT INTO codey_selected_sessions VALUES (?1)", [id])?;
        }
        transaction.commit()?;
    }
    let mut targets = Vec::new();
    for table in tables(&db)? {
        let columns = table_columns(&db, &table)?;
        let references = id_columns(&table, &columns);
        let rollout_reference =
            table == "rollout_migration_skipped_rollouts" && columns.contains("rollout_path");
        if references.is_empty() && !rollout_reference {
            continue;
        }
        let filter = local_filter(&db, &columns)?;
        let mut conditions = references
            .iter()
            .map(|column| {
                format!(
                    "{} IN (SELECT id FROM codey_selected_sessions)",
                    quote(column)
                )
            })
            .collect::<Vec<_>>();
        if rollout_reference {
            conditions.push(
                "EXISTS (SELECT 1 FROM codey_selected_sessions WHERE instr(rollout_path, id) > 0)"
                    .into(),
            );
        }
        let selected = conditions.join(" OR ");
        targets.push(format!(
            "DELETE FROM {} WHERE ({selected}) AND ({filter})",
            quote(&table)
        ));
    }
    if targets.is_empty() {
        return Ok(());
    }
    let transaction = db.transaction()?;
    transaction.execute_batch("PRAGMA defer_foreign_keys=ON")?;
    for sql in targets {
        transaction.execute(&sql, [])?;
    }
    transaction.commit()?;
    if stopped {
        // 删除空闲页中的历史内容，并截断 WAL；忙碌不能被当作清理成功。
        db.execute_batch("VACUUM")?;
        let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        ensure!(
            busy == 0,
            "数据库仍被其他进程占用，请退出其他 Codex 实例后重试"
        );
    }
    Ok(())
}
