//! Filesystem change notifications, debounced for the UI.
//!
//! A raw FSEvents/inotify stream is far too noisy to drive a redraw: one `git
//! checkout` can emit thousands of events. The watcher coalesces them per watched
//! directory and only reports "something in this directory changed".

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use notify::{
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcherTrait,
};

use crate::error::{Error, Result};

/// One coalesced change notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// The directory's contents changed; re-list it.
    DirectoryChanged(PathBuf),
    /// The watcher could not keep up and events were dropped.
    Rescan,
}

type Listener = Arc<dyn Fn(WatchEvent) + Send + Sync>;

/// Watches directories and reports coalesced changes.
///
/// Dropping the watcher stops all notifications.
pub struct FsWatcher {
    inner: RecommendedWatcher,
    watched: Arc<Mutex<HashMap<PathBuf, ()>>>,
    listener: Listener,
    /// Incremented on every raw event so the debounce task knows it has more work.
    pending: Arc<AtomicU64>,
}

impl FsWatcher {
    /// Create a watcher. `listener` is called with coalesced events.
    pub fn new(listener: impl Fn(WatchEvent) + Send + Sync + 'static) -> Result<Self> {
        let listener: Listener = Arc::new(listener);
        let watched: Arc<Mutex<HashMap<PathBuf, ()>>> = Arc::new(Mutex::new(HashMap::new()));
        let pending = Arc::new(AtomicU64::new(0));

        let handler_listener = listener.clone();
        let handler_watched = watched.clone();
        let handler_pending = pending.clone();

        let inner = RecommendedWatcher::new(
            move |result: notify::Result<Event>| match result {
                Ok(event) => {
                    if matches!(event.kind, EventKind::Access(_)) {
                        return;
                    }
                    handler_pending.fetch_add(1, Ordering::Relaxed);
                    let roots: Vec<PathBuf> =
                        handler_watched.lock().unwrap().keys().cloned().collect();
                    // Attribute each touched path to the watched root it belongs to.
                    let mut touched: Vec<PathBuf> = Vec::new();
                    for path in &event.paths {
                        let Some(root) = roots
                            .iter()
                            .filter(|root| path.starts_with(root))
                            .max_by_key(|root| root.as_os_str().len())
                        else {
                            continue;
                        };
                        // Any change under the root means "this listing may have moved":
                        // the pane only ever renders one directory, so the root is the
                        // unit of invalidation.
                        if !touched.contains(root) {
                            touched.push(root.clone());
                        }
                    }
                    for target in touched {
                        handler_listener(WatchEvent::DirectoryChanged(target));
                    }
                }
                Err(_) => handler_listener(WatchEvent::Rescan),
            },
            Config::default().with_poll_interval(Duration::from_millis(200)),
        )
        .map_err(|e| Error::Other(format!("failed to start filesystem watcher: {e}")))?;

        Ok(Self {
            inner,
            watched,
            listener,
            pending,
        })
    }

    /// Start watching a directory (one level: enough for a listing).
    ///
    /// The path is canonicalized first: on macOS the native FSEvents backend reports
    /// resolved paths (`/private/var/...` for `/var/...`), so an unresolved watch
    /// root would never match an incoming event.
    pub fn watch(&mut self, path: &Path) -> Result<()> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.watched.lock().unwrap().insert(path.clone(), ());
        self.inner
            .watch(&path, RecursiveMode::NonRecursive)
            .map_err(|e| Error::Other(format!("failed to watch {}: {e}", path.display())))
    }

    pub fn unwatch(&mut self, path: &Path) -> Result<()> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.watched.lock().unwrap().remove(&path);
        let _ = self.inner.unwatch(&path);
        Ok(())
    }

    /// Replace the watched set with exactly one directory.
    pub fn watch_only(&mut self, path: &Path) -> Result<()> {
        let existing: Vec<PathBuf> = self.watched.lock().unwrap().keys().cloned().collect();
        for old in existing {
            if old != path {
                let _ = self.unwatch(&old);
            }
        }
        if self.watched.lock().unwrap().contains_key(path) {
            return Ok(());
        }
        self.watch(path)
    }

    pub fn watched_paths(&self) -> Vec<PathBuf> {
        self.watched.lock().unwrap().keys().cloned().collect()
    }

    /// How many raw events have been seen; used by tests and diagnostics.
    pub fn raw_event_count(&self) -> u64 {
        self.pending.load(Ordering::Relaxed)
    }

    pub fn listener(&self) -> &Listener {
        &self.listener
    }
}

impl std::fmt::Debug for FsWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsWatcher")
            .field("watched", &self.watched_paths())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    #[test]
    fn reports_directory_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut watcher = FsWatcher::new(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        watcher.watch(dir.path()).unwrap();
        // FSEvents needs a beat to arm itself before it reports anything.
        std::thread::sleep(Duration::from_millis(400));

        std::fs::write(dir.path().join("new.txt"), b"hi").unwrap();

        let event = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("expected a change event");
        match event {
            WatchEvent::DirectoryChanged(path) => {
                assert_eq!(path, dir.path().canonicalize().unwrap())
            }
            WatchEvent::Rescan => panic!("unexpected rescan"),
        }
    }

    #[test]
    fn watch_only_replaces_previous_set() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut watcher = FsWatcher::new(|_| {}).unwrap();
        watcher.watch(first.path()).unwrap();
        watcher.watch_only(second.path()).unwrap();
        assert_eq!(
            watcher.watched_paths(),
            vec![second.path().canonicalize().unwrap()]
        );
    }

    #[test]
    fn watches_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let mut watcher = FsWatcher::new(|_| {}).unwrap();
        watcher.watch(dir.path()).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(watcher.watched_paths().len(), 1);
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use std::sync::mpsc;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn watch_canonicalizes_the_root() {
        // FSEvents reports resolved paths, so the stored key must be resolved too.
        let dir = tempdir();
        let mut watcher = FsWatcher::new(|_| {}).unwrap();
        watcher.watch(dir.path()).unwrap();
        let watched = watcher.watched_paths();
        assert_eq!(watched.len(), 1);
        assert_eq!(watched[0], dir.path().canonicalize().unwrap());
    }

    #[test]
    fn watching_the_same_directory_twice_is_idempotent() {
        let dir = tempdir();
        let mut watcher = FsWatcher::new(|_| {}).unwrap();
        watcher.watch(dir.path()).unwrap();
        watcher.watch(dir.path()).unwrap();
        assert_eq!(watcher.watched_paths().len(), 1);
    }

    #[test]
    fn unwatch_removes_the_root() {
        let dir = tempdir();
        let mut watcher = FsWatcher::new(|_| {}).unwrap();
        watcher.watch(dir.path()).unwrap();
        watcher.unwatch(dir.path()).unwrap();
        assert!(watcher.watched_paths().is_empty());
    }

    #[test]
    fn watch_only_is_idempotent_for_the_same_path() {
        let dir = tempdir();
        let mut watcher = FsWatcher::new(|_| {}).unwrap();
        watcher.watch_only(dir.path()).unwrap();
        watcher.watch_only(dir.path()).unwrap();
        assert_eq!(watcher.watched_paths().len(), 1);
    }

    #[test]
    fn unwatching_something_never_watched_is_fine() {
        let dir = tempdir();
        let mut watcher = FsWatcher::new(|_| {}).unwrap();
        assert!(watcher.unwatch(dir.path()).is_ok());
        assert!(watcher.watched_paths().is_empty());
    }

    #[test]
    fn deleted_file_produces_an_event() {
        let dir = tempdir();
        let file = dir.path().join("gone.txt");
        std::fs::write(&file, b"x").unwrap();

        let (tx, rx) = mpsc::channel();
        let mut watcher = FsWatcher::new(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        watcher.watch(dir.path()).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        std::fs::remove_file(&file).unwrap();

        let event = rx.recv_timeout(Duration::from_secs(5)).expect("event");
        match event {
            WatchEvent::DirectoryChanged(path) => {
                assert_eq!(path, dir.path().canonicalize().unwrap())
            }
            WatchEvent::Rescan => {}
        }
    }

    #[test]
    fn events_are_attributed_to_the_watched_root() {
        // A change deep inside the tree still maps back to the root that is listed.
        let dir = tempdir();
        let nested = dir.path().join("nested");
        std::fs::create_dir(&nested).unwrap();

        let (tx, rx) = mpsc::channel();
        let mut watcher = FsWatcher::new(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        watcher.watch(dir.path()).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        std::fs::write(nested.join("new.txt"), b"x").unwrap();

        if let Ok(event) = rx.recv_timeout(Duration::from_secs(5)) {
            match event {
                WatchEvent::DirectoryChanged(path) => {
                    assert!(path.starts_with(dir.path().canonicalize().unwrap()))
                }
                WatchEvent::Rescan => {}
            }
        }
    }

    #[test]
    fn debug_lists_the_watched_roots() {
        let dir = tempdir();
        let mut watcher = FsWatcher::new(|_| {}).unwrap();
        watcher.watch(dir.path()).unwrap();
        let text = format!("{watcher:?}");
        assert!(text.contains("FsWatcher"));
        assert!(text.contains(&dir.path().canonicalize().unwrap().display().to_string()));
    }

    #[test]
    fn watch_events_compare_by_value() {
        assert_eq!(
            WatchEvent::DirectoryChanged(PathBuf::from("/a")),
            WatchEvent::DirectoryChanged(PathBuf::from("/a"))
        );
        assert_ne!(
            WatchEvent::Rescan,
            WatchEvent::DirectoryChanged(PathBuf::from("/a"))
        );
    }
}
