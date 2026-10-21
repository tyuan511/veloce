//! The native local provider.
//!
//! Deliberately not OpenDAL's `fs` backend: local files need OS-level features
//! (trash, extended attributes, native permissions) and the fastest possible
//! directory walk. Reads use `std` on a blocking pool, listings stream in chunks.

use std::{
    fs::{self, File, Metadata},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, stream::BoxStream};

use crate::{
    entry::{FileEntry, FileKind},
    error::{Error, Result},
    provider::{Capability, ListChunk, VfsProvider},
    uri::{ProtocolScheme, VfsUri},
};

/// Entries per streamed chunk. 512 paints a full 4K viewport several times over
/// while keeping the per-chunk allocation trivial.
pub const CHUNK_SIZE: usize = 512;

/// Read buffer used when streaming file contents.
const READ_BUFFER: usize = 64 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct LocalProvider;

impl LocalProvider {
    pub fn new() -> Self {
        Self
    }

    fn path(uri: &VfsUri) -> Result<PathBuf> {
        uri.as_local_path().ok_or(Error::Unsupported {
            scheme: uri.scheme.to_string(),
            operation: "local",
        })
    }
}

fn entry_from_metadata(path: &Path, name: String, metadata: &Metadata) -> FileEntry {
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        FileKind::Symlink
    } else if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_file() {
        FileKind::File
    } else {
        FileKind::Special
    };

    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode())
    };
    #[cfg(not(unix))]
    let permissions = None;

    FileEntry {
        uri: VfsUri::local(path),
        name,
        kind,
        size: if kind == FileKind::Directory {
            0
        } else {
            metadata.len()
        },
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        permissions,
        mime_type: None,
    }
}

/// Sort entries by name before they leave the provider so the first chunk is
/// already in a stable, human-sensible order.
fn read_dir_sorted(dir: &Path) -> Result<Vec<FileEntry>> {
    let iterator = fs::read_dir(dir).map_err(|e| Error::from_io(&VfsUri::local(dir), e))?;
    let mut entries = Vec::new();
    for item in iterator {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().to_string();
        let path = item.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        entries.push(entry_from_metadata(&path, name, &metadata));
    }
    entries.sort_by(|a, b| {
        let dir_ordering = b.is_dir().cmp(&a.is_dir());
        dir_ordering.then_with(|| crate::entry::natural_cmp(&a.name, &b.name))
    });
    Ok(entries)
}

#[async_trait]
impl VfsProvider for LocalProvider {
    fn scheme(&self) -> ProtocolScheme {
        ProtocolScheme::File
    }

    async fn list_stream(&self, uri: &VfsUri) -> Result<BoxStream<'static, Result<ListChunk>>> {
        let dir = Self::path(uri)?;
        let entries = read_dir_sorted(&dir)?;
        let chunks: Vec<Result<ListChunk>> = if entries.is_empty() {
            Vec::new()
        } else {
            entries
                .chunks(CHUNK_SIZE)
                .map(|chunk| Ok(chunk.to_vec()))
                .collect()
        };
        Ok(futures::stream::iter(chunks).boxed())
    }

    async fn stat(&self, uri: &VfsUri) -> Result<FileEntry> {
        let path = Self::path(uri)?;
        let metadata = fs::symlink_metadata(&path).map_err(|e| Error::from_io(uri, e))?;
        let name = uri.file_name().to_string();
        Ok(entry_from_metadata(&path, name, &metadata))
    }

    async fn read_stream(
        &self,
        uri: &VfsUri,
        offset: u64,
        length: Option<u64>,
    ) -> Result<BoxStream<'static, Result<Bytes>>> {
        let path = Self::path(uri)?;
        let mut file = File::open(&path).map_err(|e| Error::from_io(uri, e))?;
        if offset > 0 {
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| Error::from_io(uri, e))?;
        }
        let remaining = length;
        let stream =
            futures::stream::unfold((file, remaining), |(mut file, remaining)| async move {
                if remaining == Some(0) {
                    return None;
                }
                let want = remaining
                    .map(|r| r.min(READ_BUFFER as u64) as usize)
                    .unwrap_or(READ_BUFFER);
                let mut buffer = vec![0u8; want];
                match file.read(&mut buffer) {
                    Ok(0) => None,
                    Ok(read) => {
                        buffer.truncate(read);
                        let next_remaining = remaining.map(|r| r.saturating_sub(read as u64));
                        Some((Ok(Bytes::from(buffer)), (file, next_remaining)))
                    }
                    Err(e) => Some((
                        Err(Error::Io {
                            context: "read".to_string(),
                            source: e,
                        }),
                        (file, Some(0)),
                    )),
                }
            });
        Ok(stream.boxed())
    }

    async fn write_stream(
        &self,
        uri: &VfsUri,
        mut stream: BoxStream<'static, Result<Bytes>>,
        _total_size: Option<u64>,
        on_progress: Option<Box<dyn Fn(u64) + Send + Sync>>,
    ) -> Result<()> {
        let path = Self::path(uri)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::from_io(uri, e))?;
        }
        let mut file = File::create(&path).map_err(|e| Error::from_io(uri, e))?;
        let mut written = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).map_err(|e| Error::from_io(uri, e))?;
            written += chunk.len() as u64;
            if let Some(callback) = &on_progress {
                callback(written);
            }
        }
        file.flush().map_err(|e| Error::from_io(uri, e))?;
        Ok(())
    }

    async fn rename(&self, from: &VfsUri, to: &VfsUri) -> Result<()> {
        let source = Self::path(from)?;
        let target = Self::path(to)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::from_io(to, e))?;
        }
        fs::rename(&source, &target).map_err(|e| Error::from_io(from, e))
    }

    async fn delete(&self, uri: &VfsUri, recursive: bool) -> Result<()> {
        let path = Self::path(uri)?;
        let metadata = fs::symlink_metadata(&path).map_err(|e| Error::from_io(uri, e))?;
        if metadata.is_dir() {
            if recursive {
                fs::remove_dir_all(&path).map_err(|e| Error::from_io(uri, e))
            } else {
                fs::remove_dir(&path).map_err(|e| Error::from_io(uri, e))
            }
        } else {
            fs::remove_file(&path).map_err(|e| Error::from_io(uri, e))
        }
    }

    async fn create_dir(&self, uri: &VfsUri) -> Result<()> {
        let path = Self::path(uri)?;
        fs::create_dir_all(&path).map_err(|e| Error::from_io(uri, e))
    }

    fn supports(&self, capability: Capability) -> bool {
        matches!(
            capability,
            Capability::ServerSideRename
                | Capability::Trash
                | Capability::Watch
                | Capability::Permissions
        )
    }
}

/// Modification time of a path, if it exists.
pub fn modified_at(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{SortKey, SortSpec};
    use futures::StreamExt;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn entry_kinds_and_sizes() {
        let dir = tempdir();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"hello").unwrap();
        let metadata = fs::symlink_metadata(&file).unwrap();
        let entry = entry_from_metadata(&file, "a.txt".into(), &metadata);
        assert_eq!(entry.kind, FileKind::File);
        assert_eq!(entry.size, 5);
        assert_eq!(entry.extension().as_deref(), Some("txt"));
    }

    #[test]
    fn reads_directory_sorted_dirs_first() {
        let dir = tempdir();
        fs::write(dir.path().join("b.txt"), b"b").unwrap();
        fs::write(dir.path().join("a.txt"), b"a").unwrap();
        fs::create_dir(dir.path().join("zdir")).unwrap();
        let entries = read_dir_sorted(dir.path()).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zdir", "a.txt", "b.txt"]);
    }

    #[test]
    fn streams_directory_in_chunks() {
        let dir = tempdir();
        for i in 0..(CHUNK_SIZE + 10) {
            fs::write(dir.path().join(format!("f{i:04}.txt")), b"x").unwrap();
        }
        let provider = LocalProvider::new();
        let stream = futures::executor::block_on(provider.list_stream(&VfsUri::local(dir.path())))
            .expect("list");
        let chunks = futures::executor::block_on(stream.collect::<Vec<_>>());
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].as_ref().unwrap().len(), CHUNK_SIZE);
        assert_eq!(chunks[1].as_ref().unwrap().len(), 10);
    }

    #[test]
    fn missing_directory_is_structured_error() {
        let provider = LocalProvider::new();
        let result = futures::executor::block_on(
            provider.list_stream(&VfsUri::local("/definitely/not/here")),
        );
        assert!(matches!(result, Err(Error::NotFound { .. })));
    }

    #[test]
    fn read_stream_respects_offset_and_length() {
        let dir = tempdir();
        let file = dir.path().join("data.bin");
        fs::write(&file, b"0123456789").unwrap();
        let provider = LocalProvider::new();
        let stream =
            futures::executor::block_on(provider.read_stream(&VfsUri::local(&file), 2, Some(3)))
                .unwrap();
        let chunks: Vec<_> = futures::executor::block_on(stream.collect::<Vec<_>>());
        let bytes: Vec<u8> = chunks
            .into_iter()
            .flat_map(|c| c.unwrap().to_vec())
            .collect();
        assert_eq!(bytes, b"234");
    }

    #[test]
    fn write_rename_delete_roundtrip() {
        let dir = tempdir();
        let provider = LocalProvider::new();
        let target = VfsUri::local(dir.path().join("nested/dir/out.txt"));
        let payload: BoxStream<'static, Result<Bytes>> = futures::stream::iter(vec![
            Ok(Bytes::from_static(b"hello")),
            Ok(Bytes::from_static(b" world")),
        ])
        .boxed();
        futures::executor::block_on(provider.write_stream(&target, payload, None, None)).unwrap();
        assert_eq!(
            fs::read(dir.path().join("nested/dir/out.txt")).unwrap(),
            b"hello world"
        );

        let renamed = target.with_path(dir.path().join("nested/dir/renamed.txt").to_string_lossy());
        futures::executor::block_on(provider.rename(&target, &renamed)).unwrap();
        assert!(fs::metadata(dir.path().join("nested/dir/renamed.txt")).is_ok());

        futures::executor::block_on(provider.delete(&renamed, false)).unwrap();
        assert!(fs::metadata(dir.path().join("nested/dir/renamed.txt")).is_err());
    }

    #[test]
    fn stat_reports_directory() {
        let dir = tempdir();
        fs::create_dir(dir.path().join("child")).unwrap();
        let provider = LocalProvider::new();
        let entry =
            futures::executor::block_on(provider.stat(&VfsUri::local(dir.path().join("child"))))
                .unwrap();
        assert!(entry.is_dir());
        assert_eq!(entry.name, "child");
    }

    #[test]
    fn sort_spec_orders_listing() {
        let dir = tempdir();
        fs::write(dir.path().join("small.txt"), b"1").unwrap();
        fs::write(dir.path().join("big.txt"), vec![0u8; 4096]).unwrap();
        let mut entries = read_dir_sorted(dir.path()).unwrap();
        SortSpec::new(SortKey::Size, true).sort(&mut entries);
        assert_eq!(entries[0].name, "big.txt");
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use futures::StreamExt;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn empty_directory_streams_no_chunks() {
        let dir = tempdir();
        let provider = LocalProvider::new();
        let stream =
            futures::executor::block_on(provider.list_stream(&VfsUri::local(dir.path()))).unwrap();
        let chunks: Vec<_> = futures::executor::block_on(stream.collect::<Vec<_>>());
        assert!(chunks.is_empty(), "no chunk for an empty directory");
    }

    #[test]
    fn file_uri_is_required() {
        let provider = LocalProvider::new();
        let remote = VfsUri::remote(ProtocolScheme::S3, "bucket", "/key");
        let result = futures::executor::block_on(provider.stat(&remote));
        assert!(matches!(result, Err(Error::Unsupported { .. })));
    }

    #[test]
    fn stat_reports_missing_file_as_not_found() {
        let provider = LocalProvider::new();
        let result =
            futures::executor::block_on(provider.stat(&VfsUri::local("/no/such/file.txt")));
        assert!(matches!(result, Err(Error::NotFound { .. })));
    }

    #[test]
    fn read_stream_without_length_reads_to_eof() {
        let dir = tempdir();
        let file = dir.path().join("data.bin");
        fs::write(&file, b"abcdefghij").unwrap();
        let provider = LocalProvider::new();
        let stream =
            futures::executor::block_on(provider.read_stream(&VfsUri::local(&file), 0, None))
                .unwrap();
        let chunks: Vec<_> = futures::executor::block_on(stream.collect::<Vec<_>>());
        let total: usize = chunks.iter().map(|c| c.as_ref().unwrap().len()).sum();
        assert_eq!(total, 10);
    }

    #[test]
    fn read_stream_past_eof_is_empty() {
        let dir = tempdir();
        let file = dir.path().join("small.txt");
        fs::write(&file, b"abc").unwrap();
        let provider = LocalProvider::new();
        let stream =
            futures::executor::block_on(provider.read_stream(&VfsUri::local(&file), 100, Some(10)))
                .unwrap();
        let chunks: Vec<_> = futures::executor::block_on(stream.collect::<Vec<_>>());
        assert!(chunks.is_empty());
    }

    #[test]
    fn read_stream_of_a_missing_file_is_an_error() {
        let provider = LocalProvider::new();
        let result = futures::executor::block_on(provider.read_stream(
            &VfsUri::local("/no/such/file"),
            0,
            None,
        ));
        assert!(matches!(result, Err(Error::NotFound { .. })));
    }

    #[test]
    fn symlinks_are_reported_as_symlinks() {
        #[cfg(unix)]
        {
            let dir = tempdir();
            let target = dir.path().join("target.txt");
            fs::write(&target, b"x").unwrap();
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(&target, &link).unwrap();

            let provider = LocalProvider::new();
            let entry = futures::executor::block_on(provider.stat(&VfsUri::local(&link))).unwrap();
            assert_eq!(entry.kind, FileKind::Symlink);
        }
    }

    #[test]
    fn directory_size_is_reported_as_zero() {
        // The real size needs a recursive walk; reporting 0 keeps the list cheap
        // and matches what Finder shows before it measures.
        let dir = tempdir();
        let child = dir.path().join("child");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("a.txt"), vec![0u8; 100]).unwrap();

        let provider = LocalProvider::new();
        let entry = futures::executor::block_on(provider.stat(&VfsUri::local(&child))).unwrap();
        assert_eq!(entry.size, 0);
        assert!(entry.is_dir());
    }

    #[test]
    fn permissions_are_exposed_on_unix() {
        let dir = tempdir();
        let file = dir.path().join("mode.txt");
        fs::write(&file, b"x").unwrap();
        let provider = LocalProvider::new();
        let entry = futures::executor::block_on(provider.stat(&VfsUri::local(&file))).unwrap();
        #[cfg(unix)]
        {
            let mode = entry.permissions.expect("mode bits");
            assert_eq!(mode & 0o777, 0o644, "default umask yields 0644");
        }
        #[cfg(not(unix))]
        assert!(entry.permissions.is_none());
    }

    #[test]
    fn create_dir_is_idempotent() {
        let dir = tempdir();
        let target = VfsUri::local(dir.path().join("a/b/c"));
        let provider = LocalProvider::new();
        futures::executor::block_on(provider.create_dir(&target)).unwrap();
        futures::executor::block_on(provider.create_dir(&target)).unwrap();
        assert!(dir.path().join("a/b/c").is_dir());
    }

    #[test]
    fn delete_non_empty_directory_requires_recursive() {
        let dir = tempdir();
        let root = dir.path().join("tree");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), b"x").unwrap();

        let provider = LocalProvider::new();
        let shallow = futures::executor::block_on(provider.delete(&VfsUri::local(&root), false));
        assert!(shallow.is_err(), "a non-empty dir must not vanish silently");
        assert!(root.exists());

        futures::executor::block_on(provider.delete(&VfsUri::local(&root), true)).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn rename_creates_missing_parents() {
        let dir = tempdir();
        let source = dir.path().join("a.txt");
        fs::write(&source, b"x").unwrap();
        let target = VfsUri::local(dir.path().join("new/dir/a.txt"));

        let provider = LocalProvider::new();
        futures::executor::block_on(provider.rename(&VfsUri::local(&source), &target)).unwrap();
        assert!(dir.path().join("new/dir/a.txt").exists());
    }

    #[test]
    fn capability_probe_matches_the_native_backend() {
        let provider = LocalProvider::new();
        assert!(provider.supports(Capability::Trash));
        assert!(provider.supports(Capability::Watch));
        assert!(provider.supports(Capability::ServerSideRename));
        assert!(provider.supports(Capability::Permissions));
        // Multipart upload is an object-store feature, not a local one.
        assert!(!provider.supports(Capability::MultipartUpload));
    }

    #[test]
    fn scheme_is_file() {
        assert_eq!(LocalProvider::new().scheme(), ProtocolScheme::File);
    }

    #[test]
    fn write_stream_reports_progress() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        let dir = tempdir();
        let target = VfsUri::local(dir.path().join("out.bin"));
        let seen = Arc::new(AtomicU64::new(0));
        let sink = seen.clone();

        let chunks: BoxStream<'static, Result<Bytes>> = futures::stream::iter(vec![
            Ok(Bytes::from_static(b"aaaa")),
            Ok(Bytes::from_static(b"bbbb")),
        ])
        .boxed();
        let provider = LocalProvider::new();
        futures::executor::block_on(provider.write_stream(
            &target,
            chunks,
            Some(8),
            Some(Box::new(move |written| {
                sink.store(written, Ordering::Relaxed);
            })),
        ))
        .unwrap();

        assert_eq!(seen.load(Ordering::Relaxed), 8);
        assert_eq!(fs::read(dir.path().join("out.bin")).unwrap(), b"aaaabbbb");
    }

    #[test]
    fn modified_at_reports_a_time_for_existing_paths() {
        let dir = tempdir();
        assert!(modified_at(dir.path()).is_some());
        assert!(modified_at(Path::new("/definitely/not/here")).is_none());
    }
}
