//! The FUSE-independent half of mounting a remote folder as a local one: path
//! handling, how an entry looks to `stat`, the cache lifetime and read clamping.
//!
//! Kept free of any FUSE types so it is testable without a kernel driver; the
//! `veloce-mount` crate only translates these into `fuser` calls.

use std::time::{Duration, Instant, SystemTime};

use crate::entry::{FileEntry, FileKind};

/// How long listings and attributes are trusted before the server is asked again.
/// Short, because a mounted folder is a live view, but long enough that `ls -l` or a
/// Finder window does not turn into one request per file.
pub const CACHE_TTL: Duration = Duration::from_secs(5);

/// What an inode is, as far as the kernel cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
}

/// What `getattr` reports for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    pub size: u64,
    pub mtime: SystemTime,
    /// Unix permission bits. The mount is read-only, so nothing is writable.
    pub perm: u16,
}

impl Node {
    pub fn dir() -> Self {
        Self {
            kind: NodeKind::Dir,
            size: 0,
            mtime: SystemTime::UNIX_EPOCH,
            perm: 0o555,
        }
    }

    pub fn from_entry(entry: &FileEntry) -> Self {
        let kind = if entry.kind == FileKind::Directory {
            NodeKind::Dir
        } else {
            NodeKind::File
        };
        Self {
            kind,
            size: if kind == NodeKind::Dir { 0 } else { entry.size },
            mtime: entry.modified.unwrap_or(SystemTime::UNIX_EPOCH),
            perm: if kind == NodeKind::Dir { 0o555 } else { 0o444 },
        }
    }

    /// 512-byte blocks, as `st_blocks` counts them.
    pub fn blocks(&self) -> u64 {
        self.size.div_ceil(512)
    }
}

/// The path of `name` inside directory `dir` (`/` is the mount root); `None` for a name
/// that could escape the directory (`/`, `..`, empty).
pub fn child_path(dir: &str, name: &str) -> Option<String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return None;
    }
    Some(if dir == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", dir.trim_end_matches('/'))
    })
}

/// The directory a path lives in (the root is its own parent).
pub fn parent_path(path: &str) -> &str {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => "/",
        Some(index) => &path[..index],
    }
}

/// Children as (name, kind), sorted by name so repeated reads of a directory agree.
pub fn dir_names(entries: &[FileEntry]) -> Vec<(String, NodeKind)> {
    let mut names: Vec<(String, NodeKind)> = entries
        .iter()
        .map(|entry| (entry.name.clone(), Node::from_entry(entry).kind))
        .collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    names
}

/// The byte range to actually fetch for a read of `requested` bytes at `offset` from a
/// file of `size`; `None` when it starts at or past the end.
pub fn clamp_read(size: u64, offset: u64, requested: u32) -> Option<(u64, u64)> {
    if offset >= size || requested == 0 {
        return None;
    }
    Some((offset, u64::from(requested).min(size - offset)))
}

/// A value kept for [`CACHE_TTL`].
#[derive(Debug)]
pub struct Cached<T> {
    at: Instant,
    pub value: T,
}

impl<T> Cached<T> {
    pub fn new(value: T, now: Instant) -> Self {
        Self { at: now, value }
    }

    pub fn is_fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < CACHE_TTL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uri::VfsUri;

    fn entry(name: &str, kind: FileKind, size: u64) -> FileEntry {
        let mut entry = FileEntry::new(VfsUri::local(format!("/x/{name}")), name, kind);
        entry.size = size;
        entry
    }

    #[test]
    fn child_paths_join_under_the_root_and_nested_folders() {
        assert_eq!(child_path("/", "docs").as_deref(), Some("/docs"));
        assert_eq!(child_path("/docs", "a.txt").as_deref(), Some("/docs/a.txt"));
        assert_eq!(child_path("/docs/", "a.txt").as_deref(), Some("/docs/a.txt"));
    }

    #[test]
    fn child_names_cannot_escape_their_folder() {
        for bad in ["", ".", "..", "a/b", "/etc"] {
            assert_eq!(child_path("/", bad), None, "{bad:?}");
        }
    }

    #[test]
    fn parents_walk_up_and_stop_at_the_root() {
        assert_eq!(parent_path("/docs/deep/a.txt"), "/docs/deep");
        assert_eq!(parent_path("/docs"), "/");
        assert_eq!(parent_path("/"), "/");
        assert_eq!(parent_path("/docs/"), "/");
    }

    #[test]
    fn nodes_are_read_only_and_count_blocks_rounding_up() {
        let file = Node::from_entry(&entry("a.bin", FileKind::File, 513));
        assert_eq!(file.kind, NodeKind::File);
        assert_eq!(file.perm, 0o444, "no write bit");
        assert_eq!(file.blocks(), 2);
        let dir = Node::from_entry(&entry("d", FileKind::Directory, 4096));
        assert_eq!((dir.kind, dir.perm, dir.size), (NodeKind::Dir, 0o555, 0));
        assert_eq!(Node::from_entry(&entry("z", FileKind::File, 0)).blocks(), 0);
    }

    #[test]
    fn directory_names_are_sorted_and_carry_their_kind() {
        let names = dir_names(&[
            entry("b.txt", FileKind::File, 1),
            entry("a", FileKind::Directory, 0),
        ]);
        assert_eq!(
            names,
            [
                ("a".to_string(), NodeKind::Dir),
                ("b.txt".to_string(), NodeKind::File)
            ]
        );
        assert!(dir_names(&[]).is_empty());
    }

    #[test]
    fn reads_are_clamped_to_the_file() {
        assert_eq!(clamp_read(100, 0, 4096), Some((0, 100)));
        assert_eq!(clamp_read(100, 40, 10), Some((40, 10)));
        assert_eq!(clamp_read(100, 95, 10), Some((95, 5)));
        assert_eq!(clamp_read(100, 100, 10), None, "at the end");
        assert_eq!(clamp_read(100, 500, 10), None, "past the end");
        assert_eq!(clamp_read(0, 0, 10), None, "empty file");
        assert_eq!(clamp_read(100, 0, 0), None, "nothing asked");
    }

    #[test]
    fn cached_values_go_stale_after_the_ttl() {
        let start = Instant::now();
        let value = Cached::new(1, start);
        assert!(value.is_fresh(start));
        assert!(value.is_fresh(start + CACHE_TTL - Duration::from_millis(1)));
        assert!(!value.is_fresh(start + CACHE_TTL));
        assert!(value.is_fresh(start - Duration::from_secs(1)), "clock going back");
    }
}
