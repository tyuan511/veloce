//! Background file operations: the transfer scheduler behind the task panel.
//!
//! Every operation that can take longer than a frame (large copies, recursive
//! deletes, cross-protocol transfers) runs here instead of on the UI thread.
//! Progress is published through atomics so the panel can poll snapshots without
//! locking the workers.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

use crate::{
    error::{Error, Result},
    provider::ProviderRegistry,
    transfer::{self, TransferSink},
    uri::VfsUri,
};

/// Buffer used for copy loops (1 MiB: big enough to keep syscalls rare, small
/// enough to keep memory flat when many tasks run at once).
const COPY_BUFFER: usize = 1024 * 1024;

/// What an operation does, for the task panel's icon and verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    Copy,
    Move,
    Delete,
    Trash,
    CreateDir,
    CreateFile,
    Rename,
}

impl TaskKind {
    pub fn verb(self) -> &'static str {
        match self {
            TaskKind::Copy => "Copy",
            TaskKind::Move => "Move",
            TaskKind::Delete => "Delete",
            TaskKind::Trash => "Trash",
            TaskKind::CreateDir => "New folder",
            TaskKind::CreateFile => "New file",
            TaskKind::Rename => "Rename",
        }
    }
}

/// Lifecycle of one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Queued,
    Running,
    Done,
    Failed(String),
    Cancelled,
}

impl TaskState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskState::Done | TaskState::Failed(_) | TaskState::Cancelled
        )
    }

    pub fn label(&self) -> &str {
        match self {
            TaskState::Queued => "Queued",
            TaskState::Running => "Running",
            TaskState::Done => "Done",
            TaskState::Failed(_) => "Failed",
            TaskState::Cancelled => "Cancelled",
        }
    }
}

/// What a caller must decide when the target already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    /// Leave the existing file untouched and skip the source.
    Skip,
    /// Replace the existing entry.
    #[default]
    Overwrite,
    /// Keep both: `name 2.ext`, `name 3.ext`, ...
    Rename,
}

/// Immutable view of a task, cheap to hand to the UI.
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    pub id: u64,
    pub kind: TaskKind,
    pub label: String,
    pub source: Option<PathBuf>,
    pub target: Option<PathBuf>,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub files_done: u64,
    pub files_total: u64,
    pub state: TaskState,
    /// Average throughput in bytes/second, `None` before any progress.
    pub speed_bps: Option<f64>,
    pub elapsed_secs: f64,
}

impl TaskSnapshot {
    /// 0.0..=1.0, or `None` when the total size is not known yet.
    pub fn progress(&self) -> Option<f32> {
        if self.total_bytes == 0 {
            return None;
        }
        Some((self.done_bytes as f32 / self.total_bytes as f32).clamp(0.0, 1.0))
    }

    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }
}

struct TaskRecord {
    id: u64,
    kind: TaskKind,
    label: String,
    source: Option<PathBuf>,
    target: Option<PathBuf>,
    total_bytes: AtomicU64,
    done_bytes: AtomicU64,
    files_total: AtomicU64,
    files_done: AtomicU64,
    cancel: AtomicBool,
    state: Mutex<TaskState>,
    started: Mutex<Option<Instant>>,
    finished: Mutex<Option<Instant>>,
}

impl TaskRecord {
    fn new(id: u64, kind: TaskKind, source: Option<PathBuf>, target: Option<PathBuf>) -> Self {
        let label = match (&source, &target) {
            (Some(source), Some(target)) => format!(
                "{} → {}",
                source
                    .file_name()
                    .unwrap_or(source.as_os_str())
                    .to_string_lossy(),
                target
                    .file_name()
                    .unwrap_or(target.as_os_str())
                    .to_string_lossy()
            ),
            (Some(source), None) => source
                .file_name()
                .unwrap_or(source.as_os_str())
                .to_string_lossy()
                .to_string(),
            (None, Some(target)) => target
                .file_name()
                .unwrap_or(target.as_os_str())
                .to_string_lossy()
                .to_string(),
            (None, None) => kind.verb().to_string(),
        };
        Self {
            id,
            kind,
            label,
            source,
            target,
            total_bytes: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            state: Mutex::new(TaskState::Queued),
            started: Mutex::new(None),
            finished: Mutex::new(None),
        }
    }

    fn set_state(&self, state: TaskState) {
        *self.state.lock().unwrap() = state;
    }

    fn snapshot(&self) -> TaskSnapshot {
        let state = self.state.lock().unwrap().clone();
        let started = *self.started.lock().unwrap();
        let finished = *self.finished.lock().unwrap();
        let done_bytes = self.done_bytes.load(Ordering::Relaxed);
        let elapsed_secs = started
            .map(|start| {
                finished
                    .unwrap_or_else(Instant::now)
                    .duration_since(start)
                    .as_secs_f64()
            })
            .unwrap_or(0.0);
        let speed_bps =
            (elapsed_secs > 0.0 && done_bytes > 0).then(|| done_bytes as f64 / elapsed_secs);
        TaskSnapshot {
            id: self.id,
            kind: self.kind,
            label: self.label.clone(),
            source: self.source.clone(),
            target: self.target.clone(),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            done_bytes,
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total.load(Ordering::Relaxed),
            state,
            speed_bps,
            elapsed_secs,
        }
    }
}

type Job = Arc<dyn Fn(&Arc<TaskRecord>) -> Result<()> + Send + Sync>;

/// Lets the transfer code publish progress straight into the task's counters.
struct RecordSink(Arc<TaskRecord>);

impl TransferSink for RecordSink {
    fn cancelled(&self) -> bool {
        self.0.cancel.load(Ordering::Relaxed)
    }
    fn add_total(&self, bytes: u64, files: u64) {
        self.0.total_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.0.files_total.fetch_add(files, Ordering::Relaxed);
    }
    fn add_done(&self, bytes: u64) {
        self.0.done_bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    fn file_done(&self) {
        self.0.files_done.fetch_add(1, Ordering::Relaxed);
    }
}

struct Shared {
    tasks: Mutex<Vec<Arc<TaskRecord>>>,
    index: Mutex<HashMap<u64, Arc<TaskRecord>>>,
    next_id: AtomicU64,
    queue: mpsc::Sender<Arc<TaskRecord>>,
}

/// Runs file operations on a small worker pool and tracks their progress.
///
/// The pool size bounds concurrent I/O so a bulk paste cannot open thousands of
/// file handles at once.
pub struct TaskManager {
    shared: Arc<Shared>,
    jobs: Arc<Mutex<HashMap<u64, Job>>>,
}

impl TaskManager {
    /// Create a manager with `concurrency` worker threads (clamped to 1..=8).
    pub fn new(concurrency: usize) -> Self {
        let concurrency = concurrency.clamp(1, 8);
        let (queue, receiver) = mpsc::channel::<Arc<TaskRecord>>();
        let receiver = Arc::new(Mutex::new(receiver));
        let shared = Arc::new(Shared {
            tasks: Mutex::new(Vec::new()),
            index: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            queue,
        });
        let jobs: Arc<Mutex<HashMap<u64, Job>>> = Arc::new(Mutex::new(HashMap::new()));

        for _ in 0..concurrency {
            let receiver = receiver.clone();
            let jobs = jobs.clone();
            thread::spawn(move || {
                loop {
                    let task = {
                        let receiver = receiver.lock().unwrap();
                        match receiver.recv() {
                            Ok(task) => task,
                            Err(_) => return,
                        }
                    };
                    if task.cancel.load(Ordering::Relaxed) {
                        task.set_state(TaskState::Cancelled);
                        *task.finished.lock().unwrap() = Some(Instant::now());
                        continue;
                    }
                    let job = jobs.lock().unwrap().get(&task.id).cloned();
                    let Some(job) = job else { continue };
                    *task.started.lock().unwrap() = Some(Instant::now());
                    task.set_state(TaskState::Running);
                    let result = job(&task);
                    *task.finished.lock().unwrap() = Some(Instant::now());
                    let state = if task.cancel.load(Ordering::Relaxed) {
                        TaskState::Cancelled
                    } else {
                        match result {
                            Ok(()) => TaskState::Done,
                            Err(Error::Cancelled) => TaskState::Cancelled,
                            Err(error) => TaskState::Failed(error.to_string()),
                        }
                    };
                    task.set_state(state);
                }
            });
        }

        Self { shared, jobs }
    }

    /// Default pool: 4 workers, matching the "SFTP 4 streams" rule of thumb.
    pub fn with_defaults() -> Self {
        Self::new(4)
    }

    fn enqueue(&self, record: TaskRecord, job: Job) -> u64 {
        let id = record.id;
        let record = Arc::new(record);
        self.shared.tasks.lock().unwrap().push(record.clone());
        self.shared.index.lock().unwrap().insert(id, record.clone());
        self.jobs.lock().unwrap().insert(id, job);
        let _ = self.shared.queue.send(record);
        id
    }

    fn next_record(
        &self,
        kind: TaskKind,
        source: Option<PathBuf>,
        target: Option<PathBuf>,
    ) -> TaskRecord {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        TaskRecord::new(id, kind, source, target)
    }

    /// Copy or move between any two backends (local, SFTP, S3, WebDAV ...). Returns
    /// the task id.
    ///
    /// The task carries no local paths, since either end may be remote; the label
    /// names the item instead.
    pub fn transfer(
        &self,
        source: VfsUri,
        target: VfsUri,
        registry: Arc<std::sync::RwLock<ProviderRegistry>>,
        policy: ConflictPolicy,
        remove_source: bool,
    ) -> u64 {
        let kind = if remove_source {
            TaskKind::Move
        } else {
            TaskKind::Copy
        };
        let mut record = self.next_record(kind, None, None);
        record.label = format!("{} → {}", source.file_name(), target.file_name());
        self.enqueue(
            record,
            Arc::new(move |task| {
                let providers = registry
                    .read()
                    .map_err(|_| Error::Other("provider registry is poisoned".into()))
                    .and_then(|registry| {
                        Ok((registry.for_uri(&source)?, registry.for_uri(&target)?))
                    })?;
                futures::executor::block_on(transfer::transfer(
                    providers.0.as_ref(),
                    providers.1.as_ref(),
                    &source,
                    &target,
                    policy,
                    remove_source,
                    Arc::new(RecordSink(task.clone())),
                ))
            }),
        )
    }

    /// Copy a file or directory tree. Returns the task id.
    pub fn copy(&self, source: PathBuf, target: PathBuf, policy: ConflictPolicy) -> u64 {
        let record = self.next_record(TaskKind::Copy, Some(source.clone()), Some(target.clone()));
        self.enqueue(
            record,
            Arc::new(move |task| run_copy(task, &source, &target, policy)),
        )
    }

    /// Move (rename) or, across filesystems, copy-then-delete.
    pub fn move_path(&self, source: PathBuf, target: PathBuf, policy: ConflictPolicy) -> u64 {
        let record = self.next_record(TaskKind::Move, Some(source.clone()), Some(target.clone()));
        self.enqueue(
            record,
            Arc::new(move |task| run_move(task, &source, &target, policy)),
        )
    }

    /// Delete permanently.
    pub fn delete(&self, target: PathBuf) -> u64 {
        let record = self.next_record(TaskKind::Delete, Some(target.clone()), None);
        self.enqueue(
            record,
            Arc::new(move |task| {
                task.total_bytes.store(1, Ordering::Relaxed);
                remove_tree(task, &target, true)?;
                task.done_bytes.store(1, Ordering::Relaxed);
                Ok(())
            }),
        )
    }

    /// Move to the OS trash (recoverable), never a hard delete.
    pub fn trash(&self, target: PathBuf) -> u64 {
        let record = self.next_record(TaskKind::Trash, Some(target.clone()), None);
        self.enqueue(
            record,
            Arc::new(move |task| {
                task.total_bytes.store(1, Ordering::Relaxed);
                trash::delete(&target).map_err(|e| {
                    Error::Other(format!("failed to trash {}: {e}", target.display()))
                })?;
                task.done_bytes.store(1, Ordering::Relaxed);
                Ok(())
            }),
        )
    }

    /// Create a directory (parents included).
    pub fn create_dir(&self, target: PathBuf) -> u64 {
        let record = self.next_record(TaskKind::CreateDir, None, Some(target.clone()));
        self.enqueue(
            record,
            Arc::new(move |task| {
                task.total_bytes.store(1, Ordering::Relaxed);
                fs::create_dir_all(&target)
                    .map_err(|e| Error::io(format!("create {}", target.display()), e))?;
                task.done_bytes.store(1, Ordering::Relaxed);
                Ok(())
            }),
        )
    }

    /// Create an empty file. Never touches an existing one: `create_new` makes the
    /// task fail instead of truncating somebody's data.
    pub fn create_file(&self, target: PathBuf) -> u64 {
        let record = self.next_record(TaskKind::CreateFile, None, Some(target.clone()));
        self.enqueue(
            record,
            Arc::new(move |task| {
                task.total_bytes.store(1, Ordering::Relaxed);
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(|e| Error::io(format!("create {}", target.display()), e))?;
                task.done_bytes.store(1, Ordering::Relaxed);
                Ok(())
            }),
        )
    }

    /// Rename in place (same directory, new name).
    pub fn rename(&self, source: PathBuf, target: PathBuf) -> u64 {
        let record = self.next_record(TaskKind::Rename, Some(source.clone()), Some(target.clone()));
        self.enqueue(
            record,
            Arc::new(move |task| {
                task.total_bytes.store(1, Ordering::Relaxed);
                fs::rename(&source, &target).map_err(|e| {
                    Error::io(
                        format!("rename {} → {}", source.display(), target.display()),
                        e,
                    )
                })?;
                task.done_bytes.store(1, Ordering::Relaxed);
                Ok(())
            }),
        )
    }

    /// Ask a queued or running task to stop. Its state becomes `Cancelled`.
    pub fn cancel(&self, id: u64) -> bool {
        let record = self.shared.index.lock().unwrap().get(&id).cloned();
        let Some(record) = record else {
            return false;
        };
        record.cancel.store(true, Ordering::Relaxed);
        if matches!(*record.state.lock().unwrap(), TaskState::Queued) {
            record.set_state(TaskState::Cancelled);
        }
        true
    }

    /// Drop finished tasks from the panel.
    pub fn clear_finished(&self) {
        let mut tasks = self.shared.tasks.lock().unwrap();
        tasks.retain(|task| !task.state.lock().unwrap().is_terminal());
        let live: Vec<u64> = tasks.iter().map(|task| task.id).collect();
        let mut index = self.shared.index.lock().unwrap();
        index.retain(|id, _| live.contains(id));
    }

    /// Current tasks, newest last.
    pub fn snapshots(&self) -> Vec<TaskSnapshot> {
        self.shared
            .tasks
            .lock()
            .unwrap()
            .iter()
            .map(|task| task.snapshot())
            .collect()
    }

    pub fn active_count(&self) -> usize {
        self.shared
            .tasks
            .lock()
            .unwrap()
            .iter()
            .filter(|task| !task.state.lock().unwrap().is_terminal())
            .count()
    }

    /// Wait until every queued or running task reaches a terminal state.
    /// Intended for tests and CLI use, never for the UI thread.
    pub fn wait_idle(&self) {
        loop {
            if self.active_count() == 0 {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

fn run_copy(task: &TaskRecord, source: &Path, target: &Path, policy: ConflictPolicy) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|e| Error::io(format!("stat {}", source.display()), e))?;
    if metadata.is_dir() {
        let (bytes, files) = measure_tree(source);
        task.total_bytes.store(bytes, Ordering::Relaxed);
        task.files_total.store(files, Ordering::Relaxed);
        copy_tree(task, source, target, policy)
    } else {
        task.total_bytes.store(metadata.len(), Ordering::Relaxed);
        task.files_total.store(1, Ordering::Relaxed);
        copy_file(task, source, target, policy)?;
        task.files_done.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn run_move(task: &TaskRecord, source: &Path, target: &Path, policy: ConflictPolicy) -> Result<()> {
    // Fast path: same filesystem, plain rename (zero bytes copied).
    if fs::rename(source, target).is_ok() {
        task.done_bytes.store(1, Ordering::Relaxed);
        task.total_bytes.store(1, Ordering::Relaxed);
        task.files_done.store(1, Ordering::Relaxed);
        task.files_total.store(1, Ordering::Relaxed);
        return Ok(());
    }
    run_copy(task, source, target, policy)?;
    let metadata = fs::symlink_metadata(source)
        .map_err(|e| Error::io(format!("stat {}", source.display()), e))?;
    if metadata.is_dir() {
        fs::remove_dir_all(source).map_err(|e| Error::io(format!("remove {}", source.display()), e))
    } else {
        fs::remove_file(source).map_err(|e| Error::io(format!("remove {}", source.display()), e))
    }
}

/// Recursively sum size and file count, used to drive the progress bar.
pub fn measure_tree(path: &Path) -> (u64, u64) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return (0, 0);
    };
    if !metadata.is_dir() {
        return (metadata.len(), 1);
    }
    let mut bytes = 0;
    let mut files = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                bytes += metadata.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

fn check_cancel(task: &TaskRecord) -> Result<()> {
    if task.cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Resolve a name conflict according to the policy.
///
/// Returns `None` when the entry should be skipped.
fn resolve_conflict(target: &Path, policy: ConflictPolicy) -> Option<PathBuf> {
    if !target.exists() {
        return Some(target.to_path_buf());
    }
    match policy {
        ConflictPolicy::Skip => None,
        ConflictPolicy::Overwrite => Some(target.to_path_buf()),
        ConflictPolicy::Rename => Some(unique_path(target)),
    }
}

/// `report.txt` -> `report 2.txt`, `report 3.txt`, ...
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "untitled".to_string());
    let extension = path.extension().map(|e| e.to_string_lossy().to_string());
    for index in 2..10_000 {
        let name = match &extension {
            Some(extension) => format!("{stem} {index}.{extension}"),
            None => format!("{stem} {index}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

fn copy_file(
    task: &TaskRecord,
    source: &Path,
    target: &Path,
    policy: ConflictPolicy,
) -> Result<()> {
    let Some(target) = resolve_conflict(target, policy) else {
        return Ok(());
    };
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| Error::io(format!("create {}", parent.display()), e))?;
    }
    let mut reader =
        File::open(source).map_err(|e| Error::io(format!("open {}", source.display()), e))?;
    let mut writer =
        File::create(&target).map_err(|e| Error::io(format!("create {}", target.display()), e))?;
    let mut buffer = vec![0u8; COPY_BUFFER];
    loop {
        check_cancel(task)?;
        let read = reader
            .read(&mut buffer)
            .map_err(|e| Error::io(format!("read {}", source.display()), e))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|e| Error::io(format!("write {}", target.display()), e))?;
        task.done_bytes.fetch_add(read as u64, Ordering::Relaxed);
    }
    writer
        .flush()
        .map_err(|e| Error::io(format!("flush {}", target.display()), e))?;
    // Preserve the modification time so copies sort identically to the originals.
    if let Ok(modified) = fs::metadata(source).and_then(|metadata| metadata.modified()) {
        let _ = set_modified(&target, modified);
    }
    Ok(())
}

#[cfg(unix)]
fn set_modified(path: &Path, time: std::time::SystemTime) -> std::io::Result<()> {
    use std::time::UNIX_EPOCH;
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let file = File::options().write(true).open(path)?;
    file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(seconds as u64))
}

#[cfg(not(unix))]
fn set_modified(path: &Path, time: std::time::SystemTime) -> std::io::Result<()> {
    File::options().write(true).open(path)?.set_modified(time)
}

fn copy_tree(
    task: &TaskRecord,
    source: &Path,
    target: &Path,
    policy: ConflictPolicy,
) -> Result<()> {
    let Some(target) = resolve_conflict(target, policy) else {
        // Skipping a directory must still advance the progress bar to completion.
        task.done_bytes
            .store(task.total_bytes.load(Ordering::Relaxed), Ordering::Relaxed);
        return Ok(());
    };
    fs::create_dir_all(&target)
        .map_err(|e| Error::io(format!("create {}", target.display()), e))?;

    let mut stack = vec![(source.to_path_buf(), target.clone())];
    while let Some((from, to)) = stack.pop() {
        check_cancel(task)?;
        let Ok(entries) = fs::read_dir(&from) else {
            continue;
        };
        for entry in entries.flatten() {
            check_cancel(task)?;
            let path = entry.path();
            let name = entry.file_name();
            let destination = to.join(&name);
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                fs::create_dir_all(&destination)
                    .map_err(|e| Error::io(format!("create {}", destination.display()), e))?;
                stack.push((path, destination));
            } else {
                copy_file(task, &path, &destination, policy)?;
                task.files_done.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    Ok(())
}

fn remove_tree(task: &TaskRecord, target: &Path, recursive: bool) -> Result<()> {
    check_cancel(task)?;
    let metadata = fs::symlink_metadata(target)
        .map_err(|e| Error::io(format!("stat {}", target.display()), e))?;
    if metadata.is_dir() {
        if recursive {
            fs::remove_dir_all(target)
                .map_err(|e| Error::io(format!("remove {}", target.display()), e))
        } else {
            fs::remove_dir(target).map_err(|e| Error::io(format!("remove {}", target.display()), e))
        }
    } else {
        fs::remove_file(target).map_err(|e| Error::io(format!("remove {}", target.display()), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn copies_a_file_and_reports_progress() {
        let dir = tempdir();
        let source = dir.path().join("a.txt");
        fs::write(&source, vec![7u8; 4096]).unwrap();
        let target = dir.path().join("b.txt");

        let manager = TaskManager::new(1);
        manager.copy(source.clone(), target.clone(), ConflictPolicy::Overwrite);
        manager.wait_idle();

        assert_eq!(fs::read(&target).unwrap().len(), 4096);
        let snapshot = manager.snapshots().pop().unwrap();
        assert_eq!(snapshot.state, TaskState::Done);
        assert_eq!(snapshot.done_bytes, 4096);
        assert_eq!(snapshot.progress(), Some(1.0));
    }

    #[test]
    fn copies_a_tree_recursively() {
        let dir = tempdir();
        let source = dir.path().join("src");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("one.txt"), b"1").unwrap();
        fs::write(source.join("nested/two.txt"), b"22").unwrap();
        let target = dir.path().join("dst");

        let manager = TaskManager::new(2);
        manager.copy(source, target.clone(), ConflictPolicy::Overwrite);
        manager.wait_idle();

        assert_eq!(fs::read(target.join("one.txt")).unwrap(), b"1");
        assert_eq!(fs::read(target.join("nested/two.txt")).unwrap(), b"22");
    }

    #[test]
    fn conflict_policy_skip_leaves_target_untouched() {
        let dir = tempdir();
        let source = dir.path().join("a.txt");
        fs::write(&source, b"new").unwrap();
        let target = dir.path().join("b.txt");
        fs::write(&target, b"old").unwrap();

        let manager = TaskManager::new(1);
        manager.copy(source, target.clone(), ConflictPolicy::Skip);
        manager.wait_idle();
        assert_eq!(fs::read(&target).unwrap(), b"old");
    }

    #[test]
    fn conflict_policy_rename_keeps_both() {
        let dir = tempdir();
        let source = dir.path().join("a.txt");
        fs::write(&source, b"new").unwrap();
        let target = dir.path().join("b.txt");
        fs::write(&target, b"old").unwrap();

        let manager = TaskManager::new(1);
        manager.copy(source, target.clone(), ConflictPolicy::Rename);
        manager.wait_idle();

        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert_eq!(fs::read(dir.path().join("b 2.txt")).unwrap(), b"new");
    }

    #[test]
    fn move_removes_the_source() {
        let dir = tempdir();
        let source = dir.path().join("a.txt");
        fs::write(&source, b"data").unwrap();
        let target = dir.path().join("moved.txt");

        let manager = TaskManager::new(1);
        manager.move_path(source.clone(), target.clone(), ConflictPolicy::Overwrite);
        manager.wait_idle();

        assert!(!source.exists());
        assert_eq!(fs::read(&target).unwrap(), b"data");
    }

    #[test]
    fn cancel_marks_task_cancelled() {
        let dir = tempdir();
        let source = dir.path().join("big.bin");
        fs::write(&source, vec![0u8; 32 * 1024 * 1024]).unwrap();
        let target = dir.path().join("copy.bin");

        let manager = TaskManager::new(1);
        let id = manager.copy(source, target, ConflictPolicy::Overwrite);
        manager.cancel(id);
        manager.wait_idle();
        let snapshot = manager.snapshots().pop().unwrap();
        assert!(matches!(
            snapshot.state,
            TaskState::Cancelled | TaskState::Done
        ));
    }

    #[test]
    fn delete_and_create_dir() {
        let dir = tempdir();
        let folder = dir.path().join("made/by/task");
        let manager = TaskManager::new(1);
        manager.create_dir(folder.clone());
        manager.wait_idle();
        assert!(folder.is_dir());

        manager.delete(folder.clone());
        manager.wait_idle();
        assert!(!folder.exists());
    }

    #[test]
    fn create_file_makes_an_empty_file() {
        let dir = tempdir();
        let file = dir.path().join("note.txt");
        let manager = TaskManager::new(1);
        manager.create_file(file.clone());
        manager.wait_idle();
        assert_eq!(fs::read(&file).unwrap(), b"");
    }

    #[test]
    fn create_file_fails_instead_of_overwriting() {
        let dir = tempdir();
        let file = dir.path().join("keep.txt");
        fs::write(&file, b"precious").unwrap();
        let manager = TaskManager::new(1);
        manager.create_file(file.clone());
        manager.wait_idle();
        assert_eq!(fs::read(&file).unwrap(), b"precious");
        let snapshot = manager.snapshots().pop().unwrap();
        assert!(matches!(snapshot.state, TaskState::Failed(_)));
    }

    #[test]
    fn unique_path_appends_counter() {
        let dir = tempdir();
        let file = dir.path().join("report.txt");
        fs::write(&file, b"x").unwrap();
        assert_eq!(unique_path(&file), dir.path().join("report 2.txt"));
    }

    #[test]
    fn clear_finished_keeps_active_tasks() {
        let dir = tempdir();
        fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let manager = TaskManager::new(1);
        manager.create_dir(dir.path().join("new"));
        manager.wait_idle();
        assert_eq!(manager.snapshots().len(), 1);
        manager.clear_finished();
        assert!(manager.snapshots().is_empty());
    }

    #[test]
    fn measure_tree_counts_nested_files() {
        let dir = tempdir();
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/one.txt"), vec![0u8; 10]).unwrap();
        fs::write(root.join("a/b/two.txt"), vec![0u8; 20]).unwrap();
        let (bytes, files) = measure_tree(&root);
        assert_eq!(bytes, 30);
        assert_eq!(files, 2);
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn task_state_terminal_classification() {
        assert!(!TaskState::Queued.is_terminal());
        assert!(!TaskState::Running.is_terminal());
        assert!(TaskState::Done.is_terminal());
        assert!(TaskState::Cancelled.is_terminal());
        assert!(TaskState::Failed("boom".into()).is_terminal());
        for state in [
            TaskState::Queued,
            TaskState::Running,
            TaskState::Done,
            TaskState::Cancelled,
        ] {
            assert!(!state.label().is_empty());
        }
    }

    #[test]
    fn every_kind_has_a_verb() {
        for kind in [
            TaskKind::Copy,
            TaskKind::Move,
            TaskKind::Delete,
            TaskKind::Trash,
            TaskKind::CreateDir,
            TaskKind::CreateFile,
            TaskKind::Rename,
        ] {
            assert!(!kind.verb().is_empty());
        }
    }

    #[test]
    fn progress_is_none_before_the_size_is_known() {
        let dir = tempdir();
        let manager = TaskManager::new(1);
        manager.create_dir(dir.path().join("x"));
        // Queued tasks have no total yet; progress must not divide by zero.
        for snapshot in manager.snapshots() {
            if snapshot.total_bytes == 0 {
                assert_eq!(snapshot.progress(), None);
            }
        }
        manager.wait_idle();
        let done = manager.snapshots().pop().unwrap();
        assert_eq!(done.progress(), Some(1.0));
    }

    #[test]
    fn failed_copy_reports_the_reason() {
        let dir = tempdir();
        let missing = dir.path().join("does-not-exist.txt");
        let target = dir.path().join("out.txt");

        let manager = TaskManager::new(1);
        manager.copy(missing, target, ConflictPolicy::Overwrite);
        manager.wait_idle();

        let snapshot = manager.snapshots().pop().unwrap();
        match snapshot.state {
            TaskState::Failed(message) => {
                assert!(message.contains("does-not-exist.txt"), "{message}")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn cancelling_an_unknown_task_is_a_no_op() {
        let manager = TaskManager::new(1);
        assert!(!manager.cancel(9_999));
    }

    #[test]
    fn cancelling_a_queued_task_skips_the_work() {
        // One worker, two tasks: the second is still queued when cancelled.
        let dir = tempdir();
        fs::write(dir.path().join("big.bin"), vec![0u8; 8 * 1024 * 1024]).unwrap();
        let manager = TaskManager::new(1);
        manager.copy(
            dir.path().join("big.bin"),
            dir.path().join("first.bin"),
            ConflictPolicy::Overwrite,
        );
        let second = manager.copy(
            dir.path().join("big.bin"),
            dir.path().join("second.bin"),
            ConflictPolicy::Overwrite,
        );
        manager.cancel(second);
        manager.wait_idle();

        assert!(
            !dir.path().join("second.bin").exists(),
            "cancelled task wrote nothing"
        );
    }

    #[test]
    fn concurrent_workers_all_finish() {
        let dir = tempdir();
        let manager = TaskManager::new(4);
        for index in 0..8 {
            manager.create_dir(dir.path().join(format!("d{index}")));
        }
        manager.wait_idle();
        for index in 0..8 {
            assert!(dir.path().join(format!("d{index}")).is_dir());
        }
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn concurrency_is_clamped() {
        // Zero would deadlock the pool; >8 would open too many handles.
        let manager = TaskManager::new(0);
        manager.create_dir(std::env::temp_dir().join("veloce-clamp-test"));
        manager.wait_idle();
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn delete_is_recursive_for_directories() {
        let dir = tempdir();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/c.txt"), b"x").unwrap();

        let manager = TaskManager::new(1);
        manager.delete(root.clone());
        manager.wait_idle();
        assert!(!root.exists());
    }

    #[test]
    fn moving_across_a_rename_failure_falls_back_to_copy() {
        // Copy-then-delete is the fallback path; verify the data survives it.
        let dir = tempdir();
        let source = dir.path().join("src.txt");
        fs::write(&source, b"payload").unwrap();
        let target_dir = dir.path().join("nested/deep");
        fs::create_dir_all(&target_dir).unwrap();
        let target = target_dir.join("dst.txt");

        let manager = TaskManager::new(1);
        manager.move_path(source.clone(), target.clone(), ConflictPolicy::Overwrite);
        manager.wait_idle();

        assert!(!source.exists());
        assert_eq!(fs::read(&target).unwrap(), b"payload");
    }

    #[test]
    fn unique_path_handles_files_without_extension() {
        let dir = tempdir();
        let file = dir.path().join("README");
        fs::write(&file, b"x").unwrap();
        let unique = unique_path(&file);
        assert_eq!(unique.file_name().unwrap(), "README 2");
    }

    #[test]
    fn unique_path_is_identity_for_missing_files() {
        let dir = tempdir();
        let missing = dir.path().join("nope.txt");
        assert_eq!(unique_path(&missing), missing);
    }

    #[test]
    fn copy_preserves_the_modification_time() {
        let dir = tempdir();
        let source = dir.path().join("a.txt");
        fs::write(&source, b"x").unwrap();
        let target = dir.path().join("b.txt");

        let manager = TaskManager::new(1);
        manager.copy(source.clone(), target.clone(), ConflictPolicy::Overwrite);
        manager.wait_idle();

        let source_time = fs::metadata(&source).unwrap().modified().unwrap();
        let target_time = fs::metadata(&target).unwrap().modified().unwrap();
        let delta = source_time
            .duration_since(target_time)
            .or_else(|_| target_time.duration_since(source_time))
            .unwrap();
        assert!(delta < std::time::Duration::from_secs(2), "mtime copied");
    }

    #[test]
    fn measure_tree_of_a_single_file() {
        let dir = tempdir();
        let file = dir.path().join("a.bin");
        fs::write(&file, vec![0u8; 42]).unwrap();
        assert_eq!(measure_tree(&file), (42, 1));
    }

    #[test]
    fn measure_tree_of_a_missing_path_is_zero() {
        assert_eq!(measure_tree(Path::new("/definitely/not/here")), (0, 0));
    }

    #[test]
    fn empty_directory_copy_creates_the_target() {
        let dir = tempdir();
        let source = dir.path().join("empty");
        fs::create_dir(&source).unwrap();
        let target = dir.path().join("copy");

        let manager = TaskManager::new(1);
        manager.copy(source, target.clone(), ConflictPolicy::Overwrite);
        manager.wait_idle();
        assert!(target.is_dir());
    }

    #[test]
    fn snapshots_keep_labels_and_paths() {
        let dir = tempdir();
        fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let manager = TaskManager::new(1);
        manager.copy(
            dir.path().join("a.txt"),
            dir.path().join("b.txt"),
            ConflictPolicy::Overwrite,
        );
        manager.wait_idle();

        let snapshot = manager.snapshots().pop().unwrap();
        assert_eq!(snapshot.kind, TaskKind::Copy);
        assert!(snapshot.label.contains("a.txt"));
        assert!(snapshot.label.contains("b.txt"));
        assert_eq!(snapshot.source, Some(dir.path().join("a.txt")));
        assert!(snapshot.elapsed_secs >= 0.0);
    }
}

/// Explorer-style name for pasting into the folder the source already lives in:
/// `report.txt` -> `report - Copy.txt`, then `report - Copy (2).txt`, ...
pub fn copy_path(path: &Path) -> PathBuf {
    suffixed_path(path, "Copy")
}

/// Explorer-style shortcut name: `report.txt` -> `report - Shortcut.txt`.
pub fn shortcut_path(path: &Path) -> PathBuf {
    suffixed_path(path, "Shortcut")
}

/// First candidate name (`None` = the plain name, then `Some(2)`, `Some(3)` ...)
/// that does not exist in `parent`.
pub(crate) fn first_free(parent: &Path, name: impl Fn(Option<u32>) -> String) -> PathBuf {
    let plain = parent.join(name(None));
    if !plain.exists() {
        return plain;
    }
    for index in 2..10_000 {
        let candidate = parent.join(name(Some(index)));
        if !candidate.exists() {
            return candidate;
        }
    }
    plain
}

fn suffixed_path(path: &Path, word: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    // `.bashrc` has no extension and `a.tar.gz` keeps only `.gz`, as in Explorer.
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "untitled".to_string());
    let extension = path.extension().map(|e| e.to_string_lossy().to_string());
    first_free(parent, |index| {
        let label = match index {
            None => word.to_string(),
            Some(index) => format!("{word} ({index})"),
        };
        match &extension {
            Some(extension) => format!("{stem} - {label}.{extension}"),
            None => format!("{stem} - {label}"),
        }
    })
}

#[cfg(test)]
mod naming_tests {
    use super::*;
    use std::fs;

    #[test]
    fn copy_appends_dash_copy_before_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("report.txt");
        fs::write(&file, b"x").unwrap();
        assert_eq!(copy_path(&file), dir.path().join("report - Copy.txt"));
    }

    #[test]
    fn copy_numbers_in_parentheses_when_the_copy_exists() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"x").unwrap();
        fs::write(dir.path().join("a - Copy.txt"), b"x").unwrap();
        assert_eq!(copy_path(&file), dir.path().join("a - Copy (2).txt"));
        fs::write(dir.path().join("a - Copy (2).txt"), b"x").unwrap();
        assert_eq!(copy_path(&file), dir.path().join("a - Copy (3).txt"));
    }

    #[test]
    fn copy_handles_folders_and_extensionless_names() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            copy_path(&dir.path().join("Makefile")),
            dir.path().join("Makefile - Copy")
        );
        assert_eq!(
            copy_path(&dir.path().join("src")),
            dir.path().join("src - Copy")
        );
    }

    #[test]
    fn shortcut_uses_its_own_suffix() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            shortcut_path(&dir.path().join("photo.png")),
            dir.path().join("photo - Shortcut.png")
        );
    }
}
