use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;
use tokio::time::Instant;

#[cfg(target_os = "macos")]
mod macos;

const COALESCE_WINDOW: Duration = Duration::from_millis(50);
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(1);
const MAX_DIRTY_PATHS: usize = 256;

#[derive(Debug, Default)]
pub(crate) struct SessionFileChanges {
    pub rollout_paths: Vec<PathBuf>,
    pub discovery_required: bool,
}

impl SessionFileChanges {
    pub(crate) fn is_empty(&self) -> bool {
        self.rollout_paths.is_empty() && !self.discovery_required
    }
}

#[derive(Default)]
struct PendingChanges {
    paths: HashSet<PathBuf>,
    first_rollout: Option<Instant>,
    first_discovery: Option<Instant>,
    last_discovery: Option<Instant>,
    force_discovery: bool,
    refresh_roots: bool,
    reset_roots: bool,
    wake_outstanding: bool,
}

impl PendingChanges {
    fn discovery(&mut self, now: Instant) {
        self.first_discovery.get_or_insert(now);
    }

    fn add_path(&mut self, path: PathBuf, now: Instant) {
        if self.paths.contains(&path) {
            return;
        }
        if self.paths.len() < MAX_DIRTY_PATHS {
            self.paths.insert(path);
            self.first_rollout.get_or_insert(now);
        } else {
            // A full scan repairs an overflowing hint set; pending paths are
            // never allowed to grow with the number of native events.
            self.discovery(now);
            self.force_discovery = true;
        }
    }

    fn discovery_deadline(&self) -> Option<Instant> {
        self.first_discovery.map(|first| {
            let coalesced = first + COALESCE_WINDOW;
            if self.force_discovery {
                coalesced
            } else {
                self.last_discovery
                    .map(|last| coalesced.max(last + DISCOVERY_INTERVAL))
                    .unwrap_or(coalesced)
            }
        })
    }

    fn deadline(&self) -> Option<Instant> {
        if self.wake_outstanding {
            return None;
        }
        self.first_rollout
            .map(|first| first + COALESCE_WINDOW)
            .into_iter()
            .chain(self.discovery_deadline())
            .min()
    }

    fn take(&mut self, now: Instant) -> SessionFileChanges {
        let discovery_required = self
            .discovery_deadline()
            .is_some_and(|deadline| deadline <= now);
        if discovery_required {
            self.first_discovery = None;
            self.force_discovery = false;
            self.last_discovery = Some(now);
        }
        self.first_rollout = None;
        self.wake_outstanding = false;
        let mut rollout_paths = self.paths.drain().collect::<Vec<_>>();
        rollout_paths.sort();
        SessionFileChanges {
            rollout_paths,
            discovery_required,
        }
    }
}

struct SharedChanges {
    home: PathBuf,
    native_home: PathBuf,
    pending: Mutex<PendingChanges>,
    changed: Notify,
    healthy: AtomicBool,
    stopped: AtomicBool,
}

impl SharedChanges {
    fn pending(&self) -> MutexGuard<'_, PendingChanges> {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn degraded(&self) {
        self.healthy.store(false, Ordering::Release);
        let mut pending = self.pending();
        pending.discovery(Instant::now());
        pending.force_discovery = true;
        drop(pending);
        self.changed.notify_one();
    }

    fn record(&self, result: notify::Result<Event>) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let Ok(event) = result else {
            self.degraded();
            return;
        };
        if event.need_rescan() {
            self.degraded();
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let now = Instant::now();
        let mut pending = self.pending();
        let mut relevant = false;
        for path in event.paths {
            let Ok(relative) = path
                .strip_prefix(&self.native_home)
                .or_else(|_| path.strip_prefix(&self.home))
            else {
                continue;
            };
            match classify_path(relative) {
                PathKind::Rollout => {
                    pending.add_path(self.home.join(relative), now);
                    relevant = true;
                }
                PathKind::Directory => {
                    pending.discovery(now);
                    pending.refresh_roots = true;
                    if (relative == Path::new("sessions") || relative == Path::new("sqlite"))
                        && matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Remove(_)
                                | EventKind::Modify(notify::event::ModifyKind::Name(_))
                        )
                    {
                        pending.reset_roots = true;
                    }
                    relevant = true;
                }
                PathKind::Database => {
                    pending.discovery(now);
                    relevant = true;
                }
                PathKind::Unrelated => {}
            }
        }
        if relevant
            && matches!(
                event.kind,
                EventKind::Modify(notify::event::ModifyKind::Name(_))
            )
        {
            // Some backends report only the old path. Rediscover the index
            // even when the renamed destination never produces its own hint.
            pending.discovery(now);
        }
        drop(pending);
        if relevant {
            self.changed.notify_one();
        }
    }

    #[cfg(target_os = "macos")]
    fn record_direct(&self, path: PathBuf, kind: PathKind, event: EventKind) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut pending = self.pending();
        let now = Instant::now();
        let replaced = matches!(
            event,
            EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
        );
        match kind {
            // These paths came from the authoritative cache and may be outside
            // home/sessions. Do not apply the discovery-path filter to them.
            PathKind::Rollout => pending.add_path(path, now),
            PathKind::Database => pending.discovery(now),
            PathKind::Directory => {
                pending.discovery(now);
                pending.refresh_roots = true;
                pending.reset_roots |= replaced;
            }
            PathKind::Unrelated => return,
        }
        if replaced {
            pending.discovery(now);
            pending.refresh_roots = true;
        }
        drop(pending);
        self.changed.notify_one();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathKind {
    Rollout,
    Directory,
    Database,
    Unrelated,
}

fn classify_path(relative: &Path) -> PathKind {
    if relative.as_os_str().is_empty()
        || relative == Path::new("sessions")
        || relative == Path::new("sqlite")
    {
        return PathKind::Directory;
    }
    if relative.starts_with("sessions") {
        return match relative
            .extension()
            .and_then(|extension| extension.to_str())
        {
            Some("jsonl") => PathKind::Rollout,
            None => PathKind::Directory,
            _ => PathKind::Unrelated,
        };
    }
    let Some(name) = relative.file_name().and_then(|name| name.to_str()) else {
        return PathKind::Unrelated;
    };
    // WAL carries live commits; SHM is deliberately excluded because read
    // connections update it too and would create self-induced wakeups.
    let base = name
        .strip_suffix("-wal")
        .or_else(|| name.strip_suffix("-journal"))
        .unwrap_or(name);
    if relative.parent() == Some(Path::new("")) {
        return if base == "state_5.sqlite" {
            PathKind::Database
        } else {
            PathKind::Unrelated
        };
    }
    if relative.parent() == Some(Path::new("sqlite"))
        && matches!(
            Path::new(base)
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("db" | "sqlite" | "sqlite3")
        )
    {
        return PathKind::Database;
    }
    PathKind::Unrelated
}

struct NativeWatches {
    watcher: Option<RecommendedWatcher>,
    children: HashSet<PathBuf>,
    #[cfg(target_os = "macos")]
    direct: Option<macos::DirectFileWatches>,
}

impl NativeWatches {
    fn reset_children(&mut self) {
        if let Some(watcher) = self.watcher.as_mut() {
            for path in self.children.drain() {
                let _ = watcher.unwatch(&path);
            }
        }
    }

    fn refresh_children(&mut self, home: &Path) -> Result<(), String> {
        if let Some(watcher) = self.watcher.as_mut() {
            for (name, mode) in [
                ("sessions", RecursiveMode::Recursive),
                ("sqlite", RecursiveMode::NonRecursive),
            ] {
                let path = home.join(name);
                if path.is_dir() {
                    if !self.children.contains(&path) {
                        watcher
                            .watch(&path, mode)
                            .map_err(|error| format!("无法监听会话目录: {error}"))?;
                        self.children.insert(path);
                    }
                } else if self.children.remove(&path) {
                    // The backend may have removed the watch when the directory
                    // disappeared. A later creation is registered from home events.
                    let _ = watcher.unwatch(&path);
                }
            }
        }
        if !home.is_dir() {
            return Err("会话主目录已移除".to_string());
        }
        #[cfg(target_os = "macos")]
        if let Some(direct) = self.direct.as_mut() {
            direct
                .refresh(false)
                .map_err(|error| format!("无法监听会话文件: {error}"))?;
        }
        Ok(())
    }
}

/// Native filesystem events only wake the authoritative rollout scanner. The
/// handle owns all registrations and cancels its idle-free merge task on drop.
pub(crate) struct SessionFileWatcher {
    native: Arc<Mutex<NativeWatches>>,
    shared: Arc<SharedChanges>,
    merge_task: tokio::task::JoinHandle<()>,
}

impl SessionFileWatcher {
    pub(crate) fn start(home: PathBuf, wake: Arc<Notify>) -> Result<Self, String> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "会话文件监听需要异步运行环境".to_string())?;
        let native_home =
            std::fs::canonicalize(&home).map_err(|error| format!("无法访问会话主目录: {error}"))?;
        let shared = Arc::new(SharedChanges {
            home,
            native_home,
            pending: Mutex::new(PendingChanges::default()),
            changed: Notify::new(),
            healthy: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
        });
        let callback = Arc::clone(&shared);
        let mut watcher = notify::recommended_watcher(move |event| callback.record(event))
            .map_err(|error| format!("无法启动会话文件监听: {error}"))?;
        watcher
            .watch(&shared.native_home, RecursiveMode::NonRecursive)
            .map_err(|error| format!("无法监听会话主目录: {error}"))?;
        let mut native = NativeWatches {
            watcher: Some(watcher),
            children: HashSet::new(),
            #[cfg(target_os = "macos")]
            direct: Some(
                macos::DirectFileWatches::new(Arc::clone(&shared))
                    .map_err(|error| format!("无法启动会话文件监听: {error}"))?,
            ),
        };
        native.refresh_children(&shared.native_home)?;
        let native = Arc::new(Mutex::new(native));
        let merge_task = runtime.spawn(merge_changes(
            Arc::clone(&shared),
            Arc::clone(&native),
            wake,
        ));
        Ok(Self {
            native,
            shared,
            merge_task,
        })
    }

    pub(crate) fn is_healthy(&self) -> bool {
        self.shared.healthy.load(Ordering::Acquire) && !self.merge_task.is_finished()
    }

    pub(crate) fn take_changes(&self) -> SessionFileChanges {
        let changes = self.shared.pending().take(Instant::now());
        // A throttled database discovery remains pending and gets its own
        // deadline, even if no further filesystem event arrives.
        self.shared.changed.notify_one();
        changes
    }

    /// On macOS, FSEvents may delay writes until the producer closes its file.
    /// Register only indexed files with kqueue, then reread newly added identities.
    pub(crate) fn sync_cache(&self, cache: &super::RecentSessionEventCache, verify_existing: bool) {
        #[cfg(target_os = "macos")]
        {
            let rollouts = cache.rollouts.keys().cloned().collect::<Vec<_>>();
            let databases = cache
                .database_connections
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            if self
                .sync_paths(&rollouts, &databases, verify_existing)
                .is_err()
            {
                self.shared.degraded();
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (cache, verify_existing);
    }

    #[cfg(target_os = "macos")]
    fn sync_paths(
        &self,
        rollouts: &[PathBuf],
        databases: &[PathBuf],
        verify_existing: bool,
    ) -> Result<(), String> {
        let mut native = self
            .native
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(direct) = native.direct.as_mut() {
            direct
                .sync(rollouts, databases, verify_existing)
                .map_err(|error| format!("无法更新会话文件监听: {error}"))?;
        }
        Ok(())
    }
}

impl Drop for SessionFileWatcher {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        self.merge_task.abort();
        let mut native = self
            .native
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let watcher = native.watcher.take();
        #[cfg(target_os = "macos")]
        let direct = native.direct.take();
        drop(native);
        drop(watcher);
        #[cfg(target_os = "macos")]
        drop(direct);
    }
}

async fn merge_changes(
    shared: Arc<SharedChanges>,
    native: Arc<Mutex<NativeWatches>>,
    wake: Arc<Notify>,
) {
    loop {
        let deadline = shared.pending().deadline();
        let Some(deadline) = deadline else {
            shared.changed.notified().await;
            continue;
        };
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {}
            _ = shared.changed.notified() => continue,
        }
        let (refresh_roots, reset_roots) = {
            let mut pending = shared.pending();
            pending.wake_outstanding = true;
            (
                std::mem::take(&mut pending.refresh_roots),
                std::mem::take(&mut pending.reset_roots),
            )
        };
        if refresh_roots {
            let mut native = native.lock().unwrap_or_else(|error| error.into_inner());
            if reset_roots {
                native.reset_children();
            }
            let result = native.refresh_children(&shared.native_home);
            if result.is_err() {
                shared.degraded();
            }
        }
        if !shared.stopped.load(Ordering::Acquire) {
            wake.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn filters_unrelated_files_and_read_sidecars() {
        for path in [
            "config.toml",
            "sessions/2026/notes.txt",
            "state_5.sqlite-shm",
            "sqlite/codex.db-shm",
            "sqlite/logs.txt",
            "sqlite/nested/other.db",
        ] {
            assert_eq!(
                classify_path(Path::new(path)),
                PathKind::Unrelated,
                "{path}"
            );
        }
        for path in [
            "state_5.sqlite",
            "state_5.sqlite-wal",
            "sqlite/codex.db-wal",
        ] {
            assert_eq!(classify_path(Path::new(path)), PathKind::Database);
        }
        assert_eq!(
            classify_path(Path::new("sessions/2026/rollout.jsonl")),
            PathKind::Rollout
        );
        assert_eq!(
            classify_path(Path::new("sessions/2026/10")),
            PathKind::Directory
        );
    }

    #[test]
    fn coalesces_with_fixed_deadline_and_bounded_paths() {
        let mut pending = PendingChanges::default();
        let now = Instant::now();
        for index in 0..(MAX_DIRTY_PATHS * 2) {
            pending.add_path(
                PathBuf::from(format!("{index}.jsonl")),
                now + Duration::from_millis(index as u64),
            );
        }
        assert_eq!(pending.paths.len(), MAX_DIRTY_PATHS);
        assert_eq!(pending.deadline(), Some(now + COALESCE_WINDOW));
        assert!(pending.force_discovery);
        pending.wake_outstanding = true;
        assert_eq!(pending.deadline(), None);
    }

    #[test]
    fn a_duplicate_path_at_capacity_does_not_force_discovery() {
        let mut pending = PendingChanges::default();
        let now = Instant::now();
        for index in 0..MAX_DIRTY_PATHS {
            pending.add_path(PathBuf::from(format!("{index}.jsonl")), now);
        }
        pending.add_path(PathBuf::from("0.jsonl"), now + COALESCE_WINDOW);
        assert_eq!(pending.paths.len(), MAX_DIRTY_PATHS);
        assert!(!pending.force_discovery);
        assert!(pending.first_discovery.is_none());
        assert_eq!(pending.deadline(), Some(now + COALESCE_WINDOW));

        pending.add_path(PathBuf::from("overflow.jsonl"), now + COALESCE_WINDOW);
        assert_eq!(pending.paths.len(), MAX_DIRTY_PATHS);
        assert!(pending.force_discovery);
    }

    #[test]
    fn a_source_only_rename_requests_discovery() {
        for mode in [
            notify::event::RenameMode::From,
            notify::event::RenameMode::Any,
        ] {
            let home = PathBuf::from("test-home");
            let old_path = home.join("sessions/rollout-old.jsonl");
            let shared = SharedChanges {
                native_home: home.clone(),
                home,
                pending: Mutex::new(PendingChanges::default()),
                changed: Notify::new(),
                healthy: AtomicBool::new(true),
                stopped: AtomicBool::new(false),
            };
            shared.record(Ok(Event::new(EventKind::Modify(
                notify::event::ModifyKind::Name(mode),
            ))
            .add_path(old_path.clone())));

            let changes = shared.pending().take(Instant::now() + COALESCE_WINDOW);
            assert_eq!(changes.rollout_paths, vec![old_path]);
            assert!(changes.discovery_required);
        }
    }

    #[test]
    fn delayed_discovery_survives_an_earlier_rollout_scan() {
        let now = Instant::now();
        let mut pending = PendingChanges {
            last_discovery: Some(now),
            ..Default::default()
        };
        pending.discovery(now);
        pending.add_path(PathBuf::from("rollout.jsonl"), now);
        let changes = pending.take(now + COALESCE_WINDOW);
        assert_eq!(changes.rollout_paths.len(), 1);
        assert!(!changes.discovery_required);
        assert_eq!(pending.deadline(), Some(now + DISCOVERY_INTERVAL));
        let changes = pending.take(now + DISCOVERY_INTERVAL);
        assert!(changes.discovery_required);
        assert_eq!(pending.deadline(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_discovery_wakes_without_a_followup_file_event() {
        let now = Instant::now();
        let shared = Arc::new(SharedChanges {
            home: PathBuf::new(),
            native_home: PathBuf::new(),
            pending: Mutex::new(PendingChanges {
                last_discovery: Some(now),
                ..Default::default()
            }),
            changed: Notify::new(),
            healthy: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
        });
        shared.pending().discovery(now);
        shared
            .pending()
            .add_path(PathBuf::from("rollout.jsonl"), now);
        let native = Arc::new(Mutex::new(NativeWatches {
            watcher: None,
            children: HashSet::new(),
            #[cfg(target_os = "macos")]
            direct: None,
        }));
        let wake = Arc::new(Notify::new());
        let task = tokio::spawn(merge_changes(
            Arc::clone(&shared),
            native,
            Arc::clone(&wake),
        ));
        wake.notified().await;
        assert!(!shared.pending().take(Instant::now()).discovery_required);
        shared.changed.notify_one();
        wake.notified().await;
        assert!(shared.pending().take(Instant::now()).discovery_required);
        assert!(Instant::now() - now >= DISCOVERY_INTERVAL);
        assert!(shared.pending().deadline().is_none());
        shared.changed.notify_one();
        assert!(
            tokio::time::timeout(Duration::from_secs(60), wake.notified())
                .await
                .is_err()
        );
        shared.record(Err(notify::Error::generic("test watcher failure")));
        wake.notified().await;
        assert!(!shared.healthy.load(Ordering::Acquire));
        assert!(shared.pending().take(Instant::now()).discovery_required);
        task.abort();
    }

    #[tokio::test]
    async fn native_changes_wake_promptly_and_drop_releases_backend() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        std::fs::create_dir(home.join("sessions")).unwrap();
        let rollout = home.join("sessions/rollout-test.jsonl");
        std::fs::write(&rollout, b"{}\n").unwrap();
        let wake = Arc::new(Notify::new());
        let watcher = SessionFileWatcher::start(home.clone(), Arc::clone(&wake)).unwrap();
        #[cfg(target_os = "macos")]
        watcher
            .sync_paths(std::slice::from_ref(&rollout), &[], false)
            .unwrap();
        // Discard the one-time registration reread before producing new data.
        watcher.take_changes();
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap();
        writeln!(writer, "{{}}").unwrap();
        writer.flush().unwrap();
        // Windows 的目录变更通知依赖写缓存刷新；File::flush 本身不会刷新它。
        #[cfg(windows)]
        writer.sync_all().unwrap();
        let started = Instant::now();
        wait_for_rollout(&watcher, &wake, &rollout).await;
        assert!(watcher.is_healthy());
        assert!(started.elapsed() < Duration::from_secs(2));
        eprintln!("native rollout wake latency: {:?}", started.elapsed());
        drop(writer);
        let native = Arc::clone(&watcher.native);
        drop(watcher);
        assert!(native.lock().unwrap().watcher.is_none());
        #[cfg(target_os = "macos")]
        assert!(native.lock().unwrap().direct.is_none());
        std::fs::write(&rollout, b"replacement\n").unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(150), wake.notified())
                .await
                .is_err()
        );
    }

    async fn wait_for_rollout(watcher: &SessionFileWatcher, wake: &Notify, path: &Path) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                wake.notified().await;
                if watcher
                    .take_changes()
                    .rollout_paths
                    .iter()
                    .any(|dirty| dirty == path)
                {
                    return;
                }
            }
        })
        .await
        .expect("an open rollout should wake its scanner");
    }

    #[cfg(target_os = "macos")]
    fn direct_only_watcher(home: PathBuf, wake: Arc<Notify>) -> SessionFileWatcher {
        let watcher = SessionFileWatcher::start(home, wake).unwrap();
        // Isolate kqueue so delayed FSEvents cannot make these tests pass.
        let fsevents = watcher.native.lock().unwrap().watcher.take();
        drop(fsevents);
        watcher
    }

    #[cfg(target_os = "macos")]
    async fn drain_discovery(watcher: &SessionFileWatcher, wake: &Notify) {
        while tokio::time::timeout(Duration::from_millis(1100), wake.notified())
            .await
            .is_ok()
        {
            watcher.take_changes();
        }
    }

    #[cfg(target_os = "macos")]
    async fn wait_for_discovery(watcher: &SessionFileWatcher, wake: &Notify) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                wake.notified().await;
                if watcher.take_changes().discovery_required {
                    return;
                }
            }
        })
        .await
        .expect("an open index should wake session discovery");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn open_database_and_new_wal_writes_request_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        let database = home.join("state_5.sqlite");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE example(value INTEGER);")
            .unwrap();
        let wake = Arc::new(Notify::new());
        let watcher = direct_only_watcher(home, Arc::clone(&wake));
        watcher
            .sync_paths(&[], std::slice::from_ref(&database), false)
            .unwrap();
        drain_discovery(&watcher, &wake).await;
        connection
            .execute_batch("INSERT INTO example VALUES (1);")
            .unwrap();
        wait_for_discovery(&watcher, &wake).await;
        drain_discovery(&watcher, &wake).await;
        // Creating WAL after startup must register it without closing the DB.
        connection
            .execute_batch("PRAGMA journal_mode=WAL; INSERT INTO example VALUES (2);")
            .unwrap();
        wait_for_discovery(&watcher, &wake).await;
        drain_discovery(&watcher, &wake).await;
        let started = Instant::now();
        connection
            .execute_batch("INSERT INTO example VALUES (3);")
            .unwrap();
        wait_for_discovery(&watcher, &wake).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        eprintln!("open WAL discovery latency: {:?}", started.elapsed());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn newly_created_index_directory_registers_open_databases() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        let wake = Arc::new(Notify::new());
        let watcher = direct_only_watcher(home.clone(), Arc::clone(&wake));
        drain_discovery(&watcher, &wake).await;
        std::fs::create_dir(home.join("sqlite")).unwrap();
        wait_for_discovery(&watcher, &wake).await;
        drain_discovery(&watcher, &wake).await;
        let database = home.join("sqlite/session.sqlite");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE example(value INTEGER);")
            .unwrap();
        wait_for_discovery(&watcher, &wake).await;
        // The authoritative discovery now supplies the newly indexed database.
        watcher
            .sync_paths(&[], std::slice::from_ref(&database), false)
            .unwrap();
        drain_discovery(&watcher, &wake).await;
        connection
            .execute_batch("INSERT INTO example VALUES (1);")
            .unwrap();
        wait_for_discovery(&watcher, &wake).await;
        assert!(watcher.is_healthy());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn indexed_external_rollouts_and_replacements_remain_watched() {
        let temp = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let rollout = external.path().join("rollout.jsonl");
        std::fs::write(&rollout, b"{}\n").unwrap();
        let wake = Arc::new(Notify::new());
        let watcher = direct_only_watcher(temp.path().to_path_buf(), Arc::clone(&wake));
        watcher
            .sync_paths(std::slice::from_ref(&rollout), &[], false)
            .unwrap();
        drain_discovery(&watcher, &wake).await;
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap();
        writeln!(writer, "{{}}").unwrap();
        wait_for_rollout(&watcher, &wake, &rollout).await;
        let replacement = external.path().join("replacement.jsonl");
        std::fs::write(&replacement, b"replacement\n").unwrap();
        std::fs::rename(replacement, &rollout).unwrap();
        wait_for_rollout(&watcher, &wake, &rollout).await;
        drain_discovery(&watcher, &wake).await;
        let mut replacement_writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap();
        writeln!(replacement_writer, "{{}}").unwrap();
        wait_for_rollout(&watcher, &wake, &rollout).await;
        replacement_writer.set_len(0).unwrap();
        wait_for_rollout(&watcher, &wake, &rollout).await;
        assert!(watcher.is_healthy());
    }

    #[tokio::test]
    async fn newly_created_session_directories_are_registered() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        let wake = Arc::new(Notify::new());
        let watcher = SessionFileWatcher::start(home.clone(), Arc::clone(&wake)).unwrap();
        std::fs::create_dir_all(home.join("sessions/2026/10/19")).unwrap();
        tokio::time::timeout(Duration::from_secs(2), wake.notified())
            .await
            .unwrap();
        assert!(watcher.take_changes().discovery_required);
        let rollout = home.join("sessions/2026/10/19/rollout-created.jsonl");
        std::fs::write(&rollout, b"{}\n").unwrap();
        tokio::time::timeout(Duration::from_secs(2), wake.notified())
            .await
            .unwrap();
        assert!(watcher.take_changes().rollout_paths.contains(&rollout));
    }
}
