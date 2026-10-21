use std::{fmt, time::SystemTime};

use crate::uri::VfsUri;

/// What a directory entry actually is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    /// Sockets, fifos, devices: visible, never transferred by accident.
    Special,
}

impl FileKind {
    pub fn is_dir(self) -> bool {
        matches!(self, FileKind::Directory)
    }

    pub fn label(self) -> &'static str {
        match self {
            FileKind::File => "file",
            FileKind::Directory => "directory",
            FileKind::Symlink => "symlink",
            FileKind::Special => "special",
        }
    }
}

/// A single row in the file list. Cheap to clone: the UI holds many copies.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub uri: VfsUri,
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    /// Unix mode, or synthetic permissions for object stores.
    pub permissions: Option<u32>,
    pub mime_type: Option<String>,
}

impl FileEntry {
    pub fn new(uri: VfsUri, name: impl Into<String>, kind: FileKind) -> Self {
        Self {
            uri,
            name: name.into(),
            kind,
            size: 0,
            modified: None,
            created: None,
            permissions: None,
            mime_type: None,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind.is_dir()
    }

    /// Lowercase extension without the dot, for icon and preview dispatch.
    pub fn extension(&self) -> Option<String> {
        if self.is_dir() {
            return None;
        }
        self.name
            .rsplit_once('.')
            .filter(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
            .map(|(_, ext)| ext.to_ascii_lowercase())
    }

    /// A stable identity for selection bookkeeping across re-lists.
    pub fn id(&self) -> String {
        self.uri.to_string()
    }
}

/// Column the list is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    Kind,
}

impl SortKey {
    pub const ALL: [SortKey; 4] = [
        SortKey::Name,
        SortKey::Size,
        SortKey::Modified,
        SortKey::Kind,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Size => "Size",
            SortKey::Modified => "Modified",
            SortKey::Kind => "Kind",
        }
    }

    pub fn next(self) -> Self {
        match self {
            SortKey::Name => SortKey::Size,
            SortKey::Size => SortKey::Modified,
            SortKey::Modified => SortKey::Kind,
            SortKey::Kind => SortKey::Name,
        }
    }
}

/// Sort key plus direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SortSpec {
    pub key: SortKey,
    pub descending: bool,
    /// Keep directories above files regardless of the sort key (Finder-like).
    pub dirs_first: bool,
}

impl SortSpec {
    pub fn new(key: SortKey, descending: bool) -> Self {
        Self {
            key,
            descending,
            dirs_first: true,
        }
    }

    /// Clicking a column header: same key toggles direction, a new key starts ascending.
    pub fn toggle(self, key: SortKey) -> Self {
        if self.key == key {
            Self {
                descending: !self.descending,
                ..self
            }
        } else {
            Self {
                key,
                descending: false,
                ..self
            }
        }
    }

    pub fn sort(&self, entries: &mut [FileEntry]) {
        entries.sort_by(|a, b| {
            if self.dirs_first && a.is_dir() != b.is_dir() {
                return if a.is_dir() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
            let ordering = match self.key {
                SortKey::Name => natural_cmp(&a.name, &b.name),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Modified => a.modified.cmp(&b.modified),
                SortKey::Kind => a
                    .extension()
                    .unwrap_or_default()
                    .cmp(&b.extension().unwrap_or_default())
                    .then_with(|| natural_cmp(&a.name, &b.name)),
            };
            let ordering = if self.descending {
                ordering.reverse()
            } else {
                ordering
            };
            ordering.then_with(|| natural_cmp(&a.name, &b.name))
        });
    }
}

/// Case-insensitive comparison where digit runs compare numerically
/// (`file2` < `file10`, unlike plain byte order).
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) => {
                if l.is_ascii_digit() && r.is_ascii_digit() {
                    let mut ln = String::new();
                    while let Some(c) = left.peek().copied().filter(char::is_ascii_digit) {
                        ln.push(c);
                        left.next();
                    }
                    let mut rn = String::new();
                    while let Some(c) = right.peek().copied().filter(char::is_ascii_digit) {
                        rn.push(c);
                        right.next();
                    }
                    let lt = ln.trim_start_matches('0');
                    let rt = rn.trim_start_matches('0');
                    let ordering = lt
                        .len()
                        .cmp(&rt.len())
                        .then_with(|| lt.cmp(rt))
                        .then_with(|| ln.len().cmp(&rn.len()));
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                } else {
                    let lo = l.to_lowercase().next().unwrap_or(l);
                    let ro = r.to_lowercase().next().unwrap_or(r);
                    let ordering = lo.cmp(&ro);
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                    left.next();
                    right.next();
                }
            }
        }
    }
}

/// Human readable byte size, Finder style (1000-based, 1 decimal).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `YYYY-MM-DD HH:MM` in local time, `--` when unknown.
pub fn format_time(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return "--".to_string();
    };
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%Y-%m-%d %H:%M").to_string()
}

/// Short relative time for the detail column ("2h ago").
pub fn format_relative(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return String::new();
    };
    let Ok(elapsed) = SystemTime::now().duration_since(time) else {
        return "just now".to_string();
    };
    let secs = elapsed.as_secs();
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        86400..=2591999 => format!("{}d ago", secs / 86400),
        _ => format!("{}mo ago", secs / 2592000),
    }
}

impl fmt::Display for FileEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, size: u64, dir: bool) -> FileEntry {
        let mut e = FileEntry::new(
            VfsUri::local(format!("/tmp/{name}")),
            name,
            if dir {
                FileKind::Directory
            } else {
                FileKind::File
            },
        );
        e.size = size;
        e
    }

    #[test]
    fn natural_sort_orders_numbers() {
        let mut entries = vec![
            entry("file10.txt", 0, false),
            entry("file2.txt", 0, false),
            entry("file1.txt", 0, false),
        ];
        SortSpec::new(SortKey::Name, false).sort(&mut entries);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["file1.txt", "file2.txt", "file10.txt"]);
    }

    #[test]
    fn dirs_first_by_default() {
        let mut entries = vec![entry("a.txt", 10, false), entry("zdir", 0, true)];
        SortSpec::new(SortKey::Name, false).sort(&mut entries);
        assert_eq!(entries[0].name, "zdir");
    }

    #[test]
    fn toggling_same_key_flips_direction() {
        let spec = SortSpec::new(SortKey::Name, false);
        assert!(spec.toggle(SortKey::Name).descending);
        assert!(!spec.toggle(SortKey::Size).descending);
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1500), "1.5 KB");
        assert_eq!(format_size(2_500_000), "2.5 MB");
        assert_eq!(format_size(120_000_000), "120 MB");
    }

    #[test]
    fn extension_ignores_dirs_and_dotfiles() {
        assert_eq!(entry("a.TXT", 0, false).extension().as_deref(), Some("txt"));
        assert_eq!(entry(".gitignore", 0, false).extension(), None);
        assert_eq!(entry("dir.d", 0, true).extension(), None);
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn file(name: &str, size: u64) -> FileEntry {
        let mut entry = FileEntry::new(VfsUri::local(format!("/tmp/{name}")), name, FileKind::File);
        entry.size = size;
        entry
    }

    #[test]
    fn natural_cmp_handles_leading_zeros_and_prefixes() {
        use std::cmp::Ordering;
        assert_eq!(
            natural_cmp("a1", "a01"),
            Ordering::Less,
            "fewer zeros first"
        );
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
        assert_eq!(natural_cmp("a1", "a"), Ordering::Greater);
        assert_eq!(natural_cmp("same", "same"), Ordering::Equal);
        assert_eq!(natural_cmp("A", "a"), Ordering::Equal, "case-insensitive");
        assert_eq!(natural_cmp("x999", "x1000"), Ordering::Less);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_is_consistent_with_sorting() {
        let mut names = vec!["v10", "v2", "V1", "v20", "v3"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["V1", "v2", "v3", "v10", "v20"]);
    }

    #[test]
    fn sort_is_stable_for_equal_keys() {
        let mut entries = vec![file("b.txt", 10), file("a.txt", 10), file("c.txt", 10)];
        SortSpec::new(SortKey::Size, false).sort(&mut entries);
        // Equal sizes fall back to the name, so the order is deterministic.
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn descending_reverses_but_keeps_dirs_first() {
        let mut entries = vec![
            file("small.txt", 1),
            file("big.txt", 100),
            FileEntry::new(VfsUri::local("/tmp/dir"), "dir", FileKind::Directory),
        ];
        SortSpec::new(SortKey::Size, true).sort(&mut entries);
        assert_eq!(entries[0].name, "dir", "directories still come first");
        assert_eq!(entries[1].name, "big.txt");
        assert_eq!(entries[2].name, "small.txt");
    }

    #[test]
    fn dirs_first_can_be_disabled() {
        let mut entries = vec![
            FileEntry::new(VfsUri::local("/tmp/zzz"), "zzz", FileKind::Directory),
            file("aaa.txt", 0),
        ];
        let mut sort = SortSpec::new(SortKey::Name, false);
        sort.dirs_first = false;
        sort.sort(&mut entries);
        assert_eq!(entries[0].name, "aaa.txt");
    }

    #[test]
    fn sort_by_kind_groups_extensions() {
        let mut entries = vec![
            file("z.rs", 0),
            file("a.md", 0),
            file("m.rs", 0),
            file("noext", 0),
        ];
        SortSpec::new(SortKey::Kind, false).sort(&mut entries);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        // Empty extension first, then `.md`, then `.rs` (sorted by name inside).
        assert_eq!(names, ["noext", "a.md", "m.rs", "z.rs"]);
    }

    #[test]
    fn sort_by_modified_puts_unknown_times_first_ascending() {
        let mut newer = file("newer.txt", 0);
        newer.modified = Some(SystemTime::now());
        let mut older = file("older.txt", 0);
        older.modified = Some(SystemTime::now() - Duration::from_secs(3600));
        let mut unknown = file("unknown.txt", 0);
        unknown.modified = None;

        let mut entries = vec![newer, older, unknown];
        SortSpec::new(SortKey::Modified, false).sort(&mut entries);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["unknown.txt", "older.txt", "newer.txt"]);
    }

    #[test]
    fn sort_key_cycles_through_all_columns() {
        let mut key = SortKey::Name;
        let mut seen = vec![key];
        for _ in 0..SortKey::ALL.len() - 1 {
            key = key.next();
            seen.push(key);
        }
        assert_eq!(seen.len(), SortKey::ALL.len());
        assert_eq!(key.next(), SortKey::Name, "the cycle closes");
        for key in SortKey::ALL {
            assert!(!key.label().is_empty());
        }
    }

    #[test]
    fn format_size_boundaries() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1000), "1.0 KB");
        assert_eq!(format_size(999_999), "1000 KB");
        assert_eq!(format_size(1_000_000), "1.0 MB");
        assert_eq!(format_size(1_000_000_000), "1.0 GB");
        // u64::MAX is 18.4 EB: the loop stops at the last unit and does not invent one.
        assert_eq!(format_size(u64::MAX), "18447 PB");
    }

    #[test]
    fn format_time_is_dash_for_none() {
        assert_eq!(format_time(None), "--");
        assert!(format_time(Some(SystemTime::now())).contains('-'));
    }

    #[test]
    fn format_relative_covers_each_bucket() {
        let now = SystemTime::now();
        assert_eq!(format_relative(None), "");
        assert_eq!(format_relative(Some(now)), "just now");
        assert_eq!(
            format_relative(Some(now - Duration::from_secs(120))),
            "2m ago"
        );
        assert_eq!(
            format_relative(Some(now - Duration::from_secs(7200))),
            "2h ago"
        );
        assert_eq!(
            format_relative(Some(now - Duration::from_secs(172_800))),
            "2d ago"
        );
        // A future timestamp (clock skew) must not panic or print a negative age.
        assert_eq!(
            format_relative(Some(now + Duration::from_secs(60))),
            "just now"
        );
    }

    #[test]
    fn file_kind_labels_and_dir_check() {
        assert!(FileKind::Directory.is_dir());
        for kind in [FileKind::File, FileKind::Symlink, FileKind::Special] {
            assert!(!kind.is_dir());
            assert!(!kind.label().is_empty());
        }
    }

    #[test]
    fn extension_handles_multiple_dots_and_case() {
        assert_eq!(file("archive.tar.gz", 0).extension().as_deref(), Some("gz"));
        assert_eq!(file("UPPER.RS", 0).extension().as_deref(), Some("rs"));
        assert_eq!(file("trailing.", 0).extension(), None);
        assert_eq!(file("noext", 0).extension(), None);
    }

    #[test]
    fn entry_id_is_the_uri_string() {
        let entry = file("a.txt", 0);
        assert_eq!(entry.id(), entry.uri.to_string());
        assert!(entry.id().starts_with("file://"));
    }

    #[test]
    fn display_shows_the_name() {
        assert_eq!(file("a.txt", 0).to_string(), "a.txt");
    }
}
