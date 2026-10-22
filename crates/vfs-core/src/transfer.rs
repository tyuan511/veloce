//! Copy and move between any two providers: local to SFTP, S3 to WebDAV, and so on.
//!
//! The bytes flow through memory in the provider's own chunk size, never touching a
//! temporary file, so a transfer costs one pass over the data and no extra disk.
//! The same code serves local-to-local too, which is how it is tested without a
//! server.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use futures::StreamExt;

use crate::{
    entry::FileEntry,
    error::{Error, Result},
    ops::ConflictPolicy,
    provider::{Capability, VfsProvider},
    uri::VfsUri,
};

/// How a running transfer reports back and is told to stop. Implemented by the task
/// record so the panel sees live progress.
pub trait TransferSink: Send + Sync {
    fn cancelled(&self) -> bool;
    /// Called once, after the source tree has been measured.
    fn add_total(&self, bytes: u64, files: u64);
    fn add_done(&self, bytes: u64);
    fn file_done(&self);
}

/// `report.txt` + 2 -> `report 2.txt`; a leading dot is part of the name, so
/// `.env` -> `.env 2`.
pub fn numbered_name(name: &str, index: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} {index}{}", &name[..dot], &name[dot..]),
        _ => format!("{name} {index}"),
    }
}

async fn exists(provider: &dyn VfsProvider, uri: &VfsUri) -> Result<bool> {
    match provider.stat(uri).await {
        Ok(_) => Ok(true),
        Err(Error::NotFound { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

/// The destination actually used for `target`, or `None` when the policy says to
/// leave the existing one alone.
async fn resolve_target(
    provider: &dyn VfsProvider,
    target: &VfsUri,
    policy: ConflictPolicy,
) -> Result<Option<VfsUri>> {
    if !exists(provider, target).await? {
        return Ok(Some(target.clone()));
    }
    match policy {
        ConflictPolicy::Skip => Ok(None),
        ConflictPolicy::Overwrite => Ok(Some(target.clone())),
        ConflictPolicy::Rename => {
            // The backend root has no sibling names to try.
            let Some(parent) = target.parent() else {
                return Err(Error::AlreadyExists {
                    uri: target.clone(),
                });
            };
            for index in 2..10_000 {
                let candidate = parent.join(&numbered_name(target.file_name(), index));
                if !exists(provider, &candidate).await? {
                    return Ok(Some(candidate));
                }
            }
            Err(Error::AlreadyExists {
                uri: target.clone(),
            })
        }
    }
}

fn same_backend(a: &VfsUri, b: &VfsUri) -> bool {
    a.scheme == b.scheme && a.connection_id == b.connection_id
}

/// Whether `inner` is `outer` itself or lives under it.
fn is_within(inner: &VfsUri, outer: &VfsUri) -> bool {
    same_backend(inner, outer)
        && (inner.path == outer.path
            || inner
                .path
                .strip_prefix(outer.path.trim_end_matches('/'))
                .is_some_and(|rest| rest.starts_with('/')))
}

struct FileJob {
    from: VfsUri,
    to: VfsUri,
    size: u64,
}

/// Everything under `root`, parents before children.
async fn walk(
    provider: &dyn VfsProvider,
    root: &VfsUri,
    target_root: &VfsUri,
    sink: &dyn TransferSink,
) -> Result<(Vec<VfsUri>, Vec<FileJob>)> {
    let mut dirs = vec![target_root.clone()];
    let mut files = Vec::new();
    let mut pending = vec![(root.clone(), target_root.clone())];
    while let Some((from, to)) = pending.pop() {
        if sink.cancelled() {
            return Err(Error::Cancelled);
        }
        let mut stream = provider.list_stream(&from).await?;
        while let Some(chunk) = stream.next().await {
            for entry in chunk? {
                let destination = to.join(&entry.name);
                if entry.is_dir() {
                    dirs.push(destination.clone());
                    pending.push((entry.uri.clone(), destination));
                } else {
                    files.push(FileJob {
                        from: entry.uri.clone(),
                        to: destination,
                        size: entry.size,
                    });
                }
            }
        }
    }
    Ok((dirs, files))
}

async fn copy_file(
    source: &dyn VfsProvider,
    destination: &dyn VfsProvider,
    job: &FileJob,
    sink: &Arc<dyn TransferSink>,
) -> Result<()> {
    let stream = source.read_stream(&job.from, 0, None).await?;
    let watcher = sink.clone();
    let stream = stream
        .map(move |chunk| {
            if watcher.cancelled() {
                Err(Error::Cancelled)
            } else {
                chunk
            }
        })
        .boxed();
    // Providers report cumulative bytes per file; the sink wants increments.
    let reported = Arc::new(AtomicU64::new(0));
    let progress = {
        let sink = sink.clone();
        let reported = reported.clone();
        Box::new(move |written: u64| {
            let before = reported.swap(written, Ordering::Relaxed);
            sink.add_done(written.saturating_sub(before));
        }) as Box<dyn Fn(u64) + Send + Sync>
    };
    destination
        .write_stream(&job.to, stream, Some(job.size), Some(progress))
        .await
}

/// Copy `source` (a file or a whole tree) to `target`; with `remove_source`, move it.
///
/// A move only deletes the source after every file was written and has the size it
/// had at the source, so a failed or short transfer never loses data.
pub async fn transfer(
    source_provider: &dyn VfsProvider,
    target_provider: &dyn VfsProvider,
    source: &VfsUri,
    target: &VfsUri,
    policy: ConflictPolicy,
    remove_source: bool,
    sink: Arc<dyn TransferSink>,
) -> Result<()> {
    if is_within(target, source) {
        return Err(Error::Other(format!(
            "Cannot copy {source} into itself ({target})"
        )));
    }
    let root: FileEntry = source_provider.stat(source).await?;
    let Some(target) = resolve_target(target_provider, target, policy).await? else {
        return Ok(());
    };

    // Same backend and a server-side rename: no bytes need to move at all.
    if remove_source
        && same_backend(source, &target)
        && source_provider.supports(Capability::ServerSideRename)
        && source_provider.rename(source, &target).await.is_ok()
    {
        return Ok(());
    }

    let (dirs, files) = if root.is_dir() {
        walk(source_provider, source, &target, sink.as_ref()).await?
    } else {
        (
            Vec::new(),
            vec![FileJob {
                from: source.clone(),
                to: target.clone(),
                size: root.size,
            }],
        )
    };
    sink.add_total(files.iter().map(|job| job.size).sum(), files.len() as u64);

    for dir in &dirs {
        match target_provider.create_dir(dir).await {
            Ok(()) | Err(Error::AlreadyExists { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    for job in &files {
        if sink.cancelled() {
            return Err(Error::Cancelled);
        }
        copy_file(source_provider, target_provider, job, &sink).await?;
        if remove_source {
            let written = target_provider.stat(&job.to).await?;
            if written.size != job.size {
                return Err(Error::Other(format!(
                    "{}: wrote {} bytes but the source has {}; the source was kept",
                    job.to, written.size, job.size
                )));
            }
        }
        sink.file_done();
    }
    if remove_source {
        source_provider.delete(source, true).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{local::LocalProvider, remote::RemoteProvider, uri::ProtocolScheme};
    use bytes::Bytes;
    use futures::executor::block_on;
    use opendal::{Operator, services};
    use std::sync::atomic::AtomicBool;

    #[derive(Default)]
    struct Recorder {
        cancel: AtomicBool,
        total_bytes: AtomicU64,
        total_files: AtomicU64,
        done_bytes: AtomicU64,
        files_done: AtomicU64,
    }

    impl TransferSink for Recorder {
        fn cancelled(&self) -> bool {
            self.cancel.load(Ordering::Relaxed)
        }
        fn add_total(&self, bytes: u64, files: u64) {
            self.total_bytes.fetch_add(bytes, Ordering::Relaxed);
            self.total_files.fetch_add(files, Ordering::Relaxed);
        }
        fn add_done(&self, bytes: u64) {
            self.done_bytes.fetch_add(bytes, Ordering::Relaxed);
        }
        fn file_done(&self) {
            self.files_done.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn remote() -> RemoteProvider {
        let operator = Operator::new(services::Memory::default()).unwrap();
        RemoteProvider::with_operator(ProtocolScheme::WebDav, operator)
    }

    fn remote_uri(path: &str) -> VfsUri {
        VfsUri::remote(ProtocolScheme::WebDav, "1", path)
    }

    fn put(provider: &dyn VfsProvider, uri: &VfsUri, body: &'static [u8]) {
        let stream = futures::stream::iter([Ok(Bytes::from_static(body))]).boxed();
        block_on(provider.write_stream(uri, stream, None, None)).unwrap();
    }

    fn get(provider: &dyn VfsProvider, uri: &VfsUri) -> Vec<u8> {
        block_on(async {
            let mut stream = provider.read_stream(uri, 0, None).await.unwrap();
            let mut out = Vec::new();
            while let Some(chunk) = stream.next().await {
                out.extend_from_slice(&chunk.unwrap());
            }
            out
        })
    }

    fn run(
        from: &dyn VfsProvider,
        to: &dyn VfsProvider,
        source: &VfsUri,
        target: &VfsUri,
        policy: ConflictPolicy,
        remove_source: bool,
    ) -> (Result<()>, Arc<Recorder>) {
        let sink = Arc::new(Recorder::default());
        let result = block_on(transfer(
            from,
            to,
            source,
            target,
            policy,
            remove_source,
            sink.clone(),
        ));
        (result, sink)
    }

    #[test]
    fn numbered_names_keep_the_extension_and_dotfiles() {
        assert_eq!(numbered_name("report.txt", 2), "report 2.txt");
        assert_eq!(numbered_name("archive.tar.gz", 3), "archive.tar 3.gz");
        assert_eq!(numbered_name("Makefile", 2), "Makefile 2");
        assert_eq!(numbered_name(".env", 2), ".env 2");
    }

    #[test]
    fn within_means_the_same_backend_and_a_real_path_prefix() {
        let outer = remote_uri("/a/b");
        assert!(is_within(&remote_uri("/a/b"), &outer));
        assert!(is_within(&remote_uri("/a/b/c"), &outer));
        assert!(!is_within(&remote_uri("/a/bc"), &outer), "sibling prefix");
        assert!(!is_within(&remote_uri("/a"), &outer));
        let other = VfsUri::remote(ProtocolScheme::WebDav, "2", "/a/b/c");
        assert!(!is_within(&other, &outer), "another connection");
    }

    #[test]
    fn uploads_a_file_with_progress() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), vec![7u8; 3000]).unwrap();
        let (local, dav) = (LocalProvider::new(), remote());
        let (result, sink) = run(
            &local,
            &dav,
            &VfsUri::local(dir.path().join("a.bin")),
            &remote_uri("/up/a.bin"),
            ConflictPolicy::Overwrite,
            false,
        );
        result.unwrap();
        assert_eq!(get(&dav, &remote_uri("/up/a.bin")), vec![7u8; 3000]);
        assert_eq!(sink.total_bytes.load(Ordering::Relaxed), 3000);
        assert_eq!(sink.done_bytes.load(Ordering::Relaxed), 3000);
        assert_eq!(sink.files_done.load(Ordering::Relaxed), 1);
        assert!(dir.path().join("a.bin").exists(), "a copy keeps the source");
    }

    #[test]
    fn downloads_a_whole_tree() {
        let dav = remote();
        put(&dav, &remote_uri("/proj/readme.md"), b"hi");
        put(&dav, &remote_uri("/proj/src/main.rs"), b"fn main(){}");
        let dir = tempfile::tempdir().unwrap();
        let local = LocalProvider::new();
        let (result, sink) = run(
            &dav,
            &local,
            &remote_uri("/proj"),
            &VfsUri::local(dir.path().join("proj")),
            ConflictPolicy::Overwrite,
            false,
        );
        result.unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("proj/readme.md")).unwrap(),
            b"hi"
        );
        assert_eq!(
            std::fs::read(dir.path().join("proj/src/main.rs")).unwrap(),
            b"fn main(){}"
        );
        assert_eq!(sink.total_files.load(Ordering::Relaxed), 2);
        assert_eq!(sink.files_done.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn an_empty_folder_is_still_created_at_the_target() {
        let dav = remote();
        block_on(dav.create_dir(&remote_uri("/empty"))).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (result, _) = run(
            &dav,
            &LocalProvider::new(),
            &remote_uri("/empty"),
            &VfsUri::local(dir.path().join("empty")),
            ConflictPolicy::Overwrite,
            false,
        );
        result.unwrap();
        assert!(dir.path().join("empty").is_dir());
    }

    #[test]
    fn skip_leaves_an_existing_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"new").unwrap();
        let dav = remote();
        put(&dav, &remote_uri("/a.txt"), b"old");
        let (result, sink) = run(
            &LocalProvider::new(),
            &dav,
            &VfsUri::local(dir.path().join("a.txt")),
            &remote_uri("/a.txt"),
            ConflictPolicy::Skip,
            false,
        );
        result.unwrap();
        assert_eq!(get(&dav, &remote_uri("/a.txt")), b"old");
        assert_eq!(sink.files_done.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn overwrite_replaces_an_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"new").unwrap();
        let dav = remote();
        put(&dav, &remote_uri("/a.txt"), b"old");
        run(
            &LocalProvider::new(),
            &dav,
            &VfsUri::local(dir.path().join("a.txt")),
            &remote_uri("/a.txt"),
            ConflictPolicy::Overwrite,
            false,
        )
        .0
        .unwrap();
        assert_eq!(get(&dav, &remote_uri("/a.txt")), b"new");
    }

    #[test]
    fn rename_keeps_both_under_a_numbered_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"new").unwrap();
        let dav = remote();
        put(&dav, &remote_uri("/a.txt"), b"old");
        put(&dav, &remote_uri("/a 2.txt"), b"older");
        run(
            &LocalProvider::new(),
            &dav,
            &VfsUri::local(dir.path().join("a.txt")),
            &remote_uri("/a.txt"),
            ConflictPolicy::Rename,
            false,
        )
        .0
        .unwrap();
        assert_eq!(get(&dav, &remote_uri("/a.txt")), b"old");
        assert_eq!(get(&dav, &remote_uri("/a 2.txt")), b"older");
        assert_eq!(get(&dav, &remote_uri("/a 3.txt")), b"new");
    }

    #[test]
    fn a_move_removes_the_source_only_after_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("pics")).unwrap();
        std::fs::write(dir.path().join("pics/1.jpg"), b"jpeg").unwrap();
        let dav = remote();
        run(
            &LocalProvider::new(),
            &dav,
            &VfsUri::local(dir.path().join("pics")),
            &remote_uri("/pics"),
            ConflictPolicy::Overwrite,
            true,
        )
        .0
        .unwrap();
        assert_eq!(get(&dav, &remote_uri("/pics/1.jpg")), b"jpeg");
        assert!(!dir.path().join("pics").exists());
    }

    #[test]
    fn a_failed_move_keeps_the_source() {
        let dav = remote();
        put(&dav, &remote_uri("/f.txt"), b"data");
        let (result, _) = run(
            &dav,
            &LocalProvider::new(),
            &remote_uri("/f.txt"),
            // The parent is a file, so the local write must fail.
            &VfsUri::local("/dev/null/f.txt"),
            ConflictPolicy::Overwrite,
            true,
        );
        assert!(result.is_err());
        assert_eq!(get(&dav, &remote_uri("/f.txt")), b"data");
    }

    #[test]
    fn copying_a_folder_into_itself_is_refused() {
        let dav = remote();
        put(&dav, &remote_uri("/d/a.txt"), b"x");
        let (result, _) = run(
            &dav,
            &dav,
            &remote_uri("/d"),
            &remote_uri("/d/inner"),
            ConflictPolicy::Overwrite,
            false,
        );
        assert!(result.unwrap_err().to_string().contains("into itself"));
    }

    #[test]
    fn a_cancelled_transfer_stops_with_the_cancelled_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"abc").unwrap();
        let dav = remote();
        let sink = Arc::new(Recorder::default());
        sink.cancel.store(true, Ordering::Relaxed);
        let result = block_on(transfer(
            &LocalProvider::new(),
            &dav,
            &VfsUri::local(dir.path().join("a.txt")),
            &remote_uri("/a.txt"),
            ConflictPolicy::Overwrite,
            false,
            sink,
        ));
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        assert!(block_on(dav.stat(&remote_uri("/a.txt"))).is_err());
    }

    #[test]
    fn a_missing_source_is_reported_not_created_empty() {
        let dav = remote();
        let dir = tempfile::tempdir().unwrap();
        let (result, _) = run(
            &dav,
            &LocalProvider::new(),
            &remote_uri("/nope"),
            &VfsUri::local(dir.path().join("nope")),
            ConflictPolicy::Overwrite,
            false,
        );
        assert!(matches!(result, Err(Error::NotFound { .. })));
        assert!(!dir.path().join("nope").exists());
    }
}
