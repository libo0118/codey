use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, functions::FunctionFlags, params, types::ValueRef};
use serde_json::Value;

use crate::{session_metadata::SessionMetadataCache, sqlite_util::table_columns};

pub(super) struct ThreadInventory {
    home: PathBuf,
    names: SessionMetadataCache,
    child_sources: Vec<(Connection, String)>,
    roots: HashMap<String, bool>,
    titles: HashMap<String, String>,
}

pub(super) fn open_database(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(250))?;
    Ok(connection)
}

impl ThreadInventory {
    pub(super) fn new(home: &Path, paths: &[PathBuf]) -> Result<Self, String> {
        let mut child_sources = Vec::new();
        for path in paths {
            if !path
                .try_exists()
                .map_err(|_| "无法检查 Codex 会话身份数据库")?
            {
                continue;
            }
            let connection = open_database(path).map_err(|_| "无法读取 Codex 会话身份数据库")?;
            let columns = table_columns(&connection, "threads")
                .map_err(|_| "无法读取 Codex 会话身份元数据")?;
            let mut predicates = Vec::new();
            if columns.contains("id") {
                let mut fields = Vec::new();
                for column in ["source", "thread_source"] {
                    if columns.contains(column) {
                        fields.push(format!("codey_child_source({column}, ?1)"));
                    }
                }
                for column in ["parent_thread_id", "parentThreadId"] {
                    if columns.contains(column) {
                        fields.push(format!("codey_parent_thread({column}, ?1)"));
                    }
                }
                if !fields.is_empty() {
                    predicates.push(format!(
                        "EXISTS(SELECT 1 FROM threads WHERE id = ?1 AND ({}))",
                        fields.join(" OR ")
                    ));
                }
            }
            let edges = table_columns(&connection, "thread_spawn_edges")
                .map_err(|_| "无法读取 Codex 会话关系元数据")?;
            if ["parent_thread_id", "child_thread_id"]
                .iter()
                .all(|column| edges.contains(*column))
            {
                predicates.push("EXISTS(SELECT 1 FROM thread_spawn_edges WHERE child_thread_id = ?1 AND codey_parent_thread(parent_thread_id, ?1))".to_string());
            }
            if predicates.is_empty() {
                continue;
            }
            let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
            connection
                .create_scalar_function("codey_child_source", 2, flags, |context| {
                    let id = context.get::<String>(1)?;
                    Ok(text(context.get_raw(0)).is_some_and(|value| child_source(value, &id)))
                })
                .map_err(|_| "无法读取 Codex 会话来源元数据")?;
            connection
                .create_scalar_function("codey_parent_thread", 2, flags, |context| {
                    let id = context.get::<String>(1)?;
                    Ok(text(context.get_raw(0)).is_some_and(|value| valid_parent(value, &id)))
                })
                .map_err(|_| "无法读取 Codex 会话关系元数据")?;
            child_sources.push((connection, format!("SELECT {}", predicates.join(" OR "))));
        }
        Ok(Self {
            home: home.to_path_buf(),
            names: SessionMetadataCache::default(),
            child_sources,
            roots: HashMap::new(),
            titles: HashMap::new(),
        })
    }

    fn is_root(&mut self, id: &str) -> rusqlite::Result<bool> {
        if super::super::desktop::validate_thread(id).is_err() {
            return Ok(false);
        }
        if let Some(root) = self.roots.get(id) {
            return Ok(*root);
        }
        // Identity is global: a stale database without metadata must not resurrect a known child.
        let mut root = true;
        for (connection, query) in &self.child_sources {
            if connection
                .prepare_cached(query)?
                .query_row(params![id], |row| row.get::<_, bool>(0))?
            {
                root = false;
                break;
            }
        }
        self.roots.insert(id.to_string(), root);
        Ok(root)
    }

    pub(super) fn title(&mut self, id: &str) -> String {
        self.titles
            .entry(id.to_string())
            .or_insert_with(|| {
                self.names
                    .resolve_session_name_with_preferred(&self.home, id, None)
            })
            .clone()
    }
}

pub(super) fn register_inventory_functions(
    connection: &Connection,
    inventory: Arc<Mutex<ThreadInventory>>,
) -> rusqlite::Result<()> {
    let roots = Arc::clone(&inventory);
    connection.create_scalar_function(
        "codey_root_thread",
        1,
        FunctionFlags::SQLITE_UTF8,
        move |context| {
            let id = context.get::<String>(0)?;
            roots
                .lock()
                .map_err(|_| rusqlite::Error::InvalidQuery)?
                .is_root(&id)
        },
    )?;
    connection.create_scalar_function(
        "codey_thread_title",
        1,
        FunctionFlags::SQLITE_UTF8,
        move |context| {
            let id = context.get::<String>(0)?;
            Ok(inventory
                .lock()
                .map_err(|_| rusqlite::Error::InvalidQuery)?
                .title(&id))
        },
    )
}

fn text(value: ValueRef<'_>) -> Option<&str> {
    match value {
        ValueRef::Text(value) => std::str::from_utf8(value).ok(),
        _ => None,
    }
}

fn valid_parent(parent: &str, id: &str) -> bool {
    let parent = crate::session_metadata::normalize_session_id(parent);
    match (uuid::Uuid::parse_str(parent), uuid::Uuid::parse_str(id)) {
        (Ok(parent), Ok(id)) => parent != id,
        _ => false,
    }
}

fn source_marker(value: &str) -> bool {
    matches!(
        value.trim(),
        "subagent"
            | "subagent_thread_spawn"
            | "subagent_review"
            | "subagent_compact"
            | "subagent_memory_consolidation"
            | "subagent_other"
    )
}

fn child_source(value: &str, id: &str) -> bool {
    source_marker(value)
        || serde_json::from_str::<Value>(value)
            .ok()
            .is_some_and(|value| child_source_json(&value, id))
}

fn child_source_json(value: &Value, id: &str) -> bool {
    match value {
        Value::String(value) => source_marker(value),
        Value::Object(fields) => {
            if json_parent(value, id) {
                return true;
            }
            if ["source", "thread_source"].iter().any(|key| {
                fields
                    .get(*key)
                    .is_some_and(|source| child_source_json(source, id))
            }) {
                return true;
            }
            match fields.get("subagent") {
                Some(Value::String(kind)) => {
                    matches!(kind.as_str(), "review" | "compact" | "memory_consolidation")
                }
                Some(Value::Object(agent)) => {
                    agent
                        .get("thread_spawn")
                        .is_some_and(|spawn| json_parent(spawn, id))
                        || agent
                            .get("other")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| !kind.trim().is_empty())
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn json_parent(value: &Value, id: &str) -> bool {
    ["parent_thread_id", "parentThreadId"].iter().any(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|parent| valid_parent(parent, id))
    })
}
