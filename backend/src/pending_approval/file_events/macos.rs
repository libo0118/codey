use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use notify::EventKind;
use notify::event::{DataChange, ModifyKind, RemoveKind, RenameMode};

use super::{PathKind, SharedChanges};

// Each registration owns one descriptor. Never recursively register sessions:
// only authoritative cache entries and the two discovery directories belong here.
const MAX_WATCHES: usize = 512;
const STOP_EVENT: usize = 1;

struct Registration {
    _file: File,
    identity: (u64, u64),
    token: usize,
    kind: PathKind,
}

#[derive(Default)]
struct Registrations {
    paths: HashMap<PathBuf, Registration>,
    tokens: HashMap<usize, PathBuf>,
    next_token: usize,
    invalidated: HashSet<PathBuf>,
}

struct Inner {
    queue: OwnedFd,
    registrations: Mutex<Registrations>,
    stopped: AtomicBool,
    shared: Arc<SharedChanges>,
}

pub(super) struct DirectFileWatches {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
    rollouts: Vec<PathBuf>,
    databases: Vec<PathBuf>,
}

fn event(ident: usize, filter: i16, flags: u16, fflags: u32, token: usize) -> libc::kevent {
    libc::kevent {
        ident,
        filter,
        flags,
        fflags,
        data: 0,
        udata: token as *mut libc::c_void,
    }
}

fn submit(queue: &OwnedFd, change: &libc::kevent) -> io::Result<()> {
    loop {
        // The queue and change remain valid for this synchronous call; no output
        // event buffer is requested when changing registrations.
        let result = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if result >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

impl DirectFileWatches {
    pub(super) fn new(shared: Arc<SharedChanges>) -> io::Result<Self> {
        // A successful kqueue call returns a new descriptor owned only here.
        let descriptor = unsafe { libc::kqueue() };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let queue = unsafe { OwnedFd::from_raw_fd(descriptor) };
        // The kqueue descriptor must not leak into spawned tools.
        if unsafe { libc::fcntl(queue.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        submit(
            &queue,
            &event(
                STOP_EVENT,
                libc::EVFILT_USER,
                libc::EV_ADD | libc::EV_CLEAR,
                0,
                0,
            ),
        )?;
        let inner = Arc::new(Inner {
            queue,
            registrations: Mutex::new(Registrations::default()),
            stopped: AtomicBool::new(false),
            shared,
        });
        let thread_inner = Arc::clone(&inner);
        let thread = std::thread::Builder::new()
            .name("codey-session-file-events".into())
            .spawn(move || receive_events(thread_inner))?;
        Ok(Self {
            inner,
            thread: Some(thread),
            rollouts: Vec::new(),
            databases: Vec::new(),
        })
    }

    pub(super) fn sync(
        &mut self,
        rollouts: &[PathBuf],
        databases: &[PathBuf],
        verify_existing: bool,
    ) -> io::Result<()> {
        let mut rollouts = rollouts.to_vec();
        let mut databases = databases.to_vec();
        rollouts.sort();
        rollouts.dedup();
        databases.sort();
        databases.dedup();
        if self.rollouts == rollouts
            && self.databases == databases
            && !verify_existing
            && self
                .inner
                .registrations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .invalidated
                .is_empty()
        {
            return Ok(());
        }
        self.rollouts = rollouts;
        self.databases = databases;
        self.refresh(verify_existing)
    }

    pub(super) fn refresh(&mut self, verify_existing: bool) -> io::Result<()> {
        let home = &self.inner.shared.native_home;
        let mut desired = HashMap::new();
        desired.insert(home.clone(), PathKind::Directory);
        desired.insert(home.join("sqlite"), PathKind::Directory);
        for path in &self.rollouts {
            desired.insert(path.clone(), PathKind::Rollout);
        }
        for path in &self.databases {
            desired.insert(path.clone(), PathKind::Database);
            for suffix in ["-wal", "-journal"] {
                let mut sidecar = path.as_os_str().to_os_string();
                sidecar.push(suffix);
                desired.insert(PathBuf::from(sidecar), PathKind::Database);
            }
        }
        self.reconcile(desired, verify_existing)
    }

    fn reconcile(
        &self,
        desired: HashMap<PathBuf, PathKind>,
        verify_existing: bool,
    ) -> io::Result<()> {
        let mut registrations = self
            .inner
            .registrations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut registered = Vec::new();
        let mut error = None;
        let obsolete = registrations
            .paths
            .keys()
            .filter(|path| !desired.contains_key(*path))
            .cloned()
            .collect::<Vec<_>>();
        for path in obsolete {
            remove_registration(&self.inner.queue, &mut registrations, &path);
        }
        for (path, kind) in desired {
            let invalidated = registrations.invalidated.remove(&path);
            if !verify_existing
                && !invalidated
                && registrations
                    .paths
                    .get(&path)
                    .is_some_and(|old| old.kind == kind)
            {
                continue;
            }
            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    remove_registration(&self.inner.queue, &mut registrations, &path);
                    continue;
                }
                Err(err) => {
                    error.get_or_insert(err);
                    continue;
                }
            };
            let identity = (metadata.dev(), metadata.ino());
            if !invalidated
                && registrations
                    .paths
                    .get(&path)
                    .is_some_and(|old| old.identity == identity && old.kind == kind)
            {
                continue;
            }
            remove_registration(&self.inner.queue, &mut registrations, &path);
            if registrations.paths.len() >= MAX_WATCHES {
                error.get_or_insert_with(|| io::Error::other("会话文件监听数量超出限制"));
                continue;
            }
            let result = (|| -> io::Result<Registration> {
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_EVTONLY | libc::O_CLOEXEC)
                    .open(&path)?;
                let metadata = file.metadata()?;
                let token = registrations
                    .next_token
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("会话文件监听标识已耗尽"))?;
                registrations.next_token = token;
                submit(
                    &self.inner.queue,
                    &event(
                        file.as_raw_fd() as usize,
                        libc::EVFILT_VNODE,
                        libc::EV_ADD | libc::EV_CLEAR,
                        libc::NOTE_WRITE
                            | libc::NOTE_EXTEND
                            | libc::NOTE_ATTRIB
                            | libc::NOTE_RENAME
                            | libc::NOTE_DELETE
                            | libc::NOTE_REVOKE,
                        token,
                    ),
                )?;
                Ok(Registration {
                    _file: file,
                    identity: (metadata.dev(), metadata.ino()),
                    token,
                    kind,
                })
            })();
            match result {
                Ok(registration) => {
                    registrations
                        .tokens
                        .insert(registration.token, path.clone());
                    registrations.paths.insert(path.clone(), registration);
                    registered.push((path, kind));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    error.get_or_insert(err);
                }
            }
        }
        drop(registrations);
        // The scanner may have read before this descriptor was registered. One
        // hint per new identity closes that gap, without waking unchanged watches.
        for (path, kind) in registered {
            self.inner.shared.record_direct(
                path,
                kind,
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            );
        }
        error.map_or(Ok(()), Err)
    }
}

fn remove_registration(queue: &OwnedFd, registrations: &mut Registrations, path: &Path) {
    registrations.invalidated.remove(path);
    if let Some(registration) = registrations.paths.remove(path) {
        registrations.tokens.remove(&registration.token);
        let _ = submit(
            queue,
            &event(
                registration._file.as_raw_fd() as usize,
                libc::EVFILT_VNODE,
                libc::EV_DELETE,
                0,
                0,
            ),
        );
        // Closing the descriptor removes its filter even if EV_DELETE failed.
        drop(registration);
    }
}

fn receive_events(inner: Arc<Inner>) {
    loop {
        // kevent initializes the returned portion; the zeroed remainder is never
        // inspected. A null timeout blocks until a file or explicit stop event.
        let mut events: [libc::kevent; 64] = unsafe { std::mem::zeroed() };
        let count = unsafe {
            libc::kevent(
                inner.queue.as_raw_fd(),
                std::ptr::null(),
                0,
                events.as_mut_ptr(),
                events.len() as i32,
                std::ptr::null(),
            )
        };
        if inner.stopped.load(Ordering::Acquire) {
            return;
        }
        if count < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            inner.shared.degraded();
            return;
        }
        for event in events.iter().take(count as usize) {
            if event.filter != libc::EVFILT_VNODE {
                continue;
            }
            if event.flags & libc::EV_ERROR != 0 {
                inner.shared.degraded();
                continue;
            }
            let registration = {
                let mut registrations = inner
                    .registrations
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let registration =
                    registrations
                        .tokens
                        .get(&(event.udata as usize))
                        .and_then(|path| {
                            registrations
                                .paths
                                .get(path)
                                .map(|registration| (path.clone(), registration.kind))
                        });
                if event.fflags & (libc::NOTE_DELETE | libc::NOTE_REVOKE | libc::NOTE_RENAME) != 0
                    && let Some((path, _)) = &registration
                {
                    registrations.invalidated.insert(path.clone());
                }
                registration
            };
            // Monotonic tokens prevent queued events for a closed/reused fd from
            // being attributed to its new file.
            let Some((path, kind)) = registration else {
                continue;
            };
            let event_kind = if event.fflags & (libc::NOTE_DELETE | libc::NOTE_REVOKE) != 0 {
                EventKind::Remove(RemoveKind::Any)
            } else if event.fflags & libc::NOTE_RENAME != 0 {
                EventKind::Modify(ModifyKind::Name(RenameMode::Any))
            } else {
                EventKind::Modify(ModifyKind::Data(DataChange::Any))
            };
            inner.shared.record_direct(path, kind, event_kind);
        }
    }
}

impl Drop for DirectFileWatches {
    fn drop(&mut self) {
        self.inner.stopped.store(true, Ordering::Release);
        // This owned queue keeps the registered user filter until after joining.
        let _ = submit(
            &self.inner.queue,
            &event(STOP_EVENT, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER, 0),
        );
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_bounded_to_selected_files_and_unchanged_sync_stays_idle() {
        let directory = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(directory.path()).unwrap();
        std::fs::create_dir_all(home.join("sessions/history")).unwrap();
        for index in 0..100 {
            std::fs::write(
                home.join(format!("sessions/history/{index}.jsonl")),
                b"{}\n",
            )
            .unwrap();
        }
        let rollout = home.join("sessions/history/0.jsonl");
        let shared = Arc::new(SharedChanges {
            home: home.clone(),
            native_home: home,
            pending: Mutex::new(Default::default()),
            changed: tokio::sync::Notify::new(),
            healthy: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
        });
        let mut watcher = DirectFileWatches::new(Arc::clone(&shared)).unwrap();
        watcher
            .sync(std::slice::from_ref(&rollout), &[], false)
            .unwrap();
        let original_token = {
            let registrations = watcher.inner.registrations.lock().unwrap();
            assert_eq!(registrations.paths.len(), 2);
            registrations.next_token
        };
        *shared.pending() = Default::default();
        for _ in 0..100 {
            watcher
                .sync(std::slice::from_ref(&rollout), &[], false)
                .unwrap();
        }
        assert_eq!(
            watcher.inner.registrations.lock().unwrap().next_token,
            original_token
        );
        assert!(shared.pending().paths.is_empty());
        assert!(shared.pending().first_discovery.is_none());
        watcher.sync(&[], &[], false).unwrap();
        assert_eq!(watcher.inner.registrations.lock().unwrap().paths.len(), 1);
        let inner = Arc::downgrade(&watcher.inner);
        let started = std::time::Instant::now();
        drop(watcher);
        assert!(
            inner.upgrade().is_none(),
            "drop must join and release the event thread"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
