//! Content of the Get Info window: pure formatting of a `FileEntry`.

use vfs_core::{
    FileEntry, FileKind,
    entry::{format_size, format_time},
};

/// Finder-style kind: "Folder", "Rust source"-ish is out of scope, so files read
/// "RS File" / "Document" and links "Alias".
pub fn kind_label(entry: &FileEntry) -> String {
    match entry.kind {
        FileKind::Directory => "Folder".to_string(),
        FileKind::Symlink => "Alias".to_string(),
        FileKind::Special => "Special file".to_string(),
        FileKind::File => match entry.extension() {
            Some(ext) => format!("{} File", ext.to_uppercase()),
            None => "Document".to_string(),
        },
    }
}

/// `drwxr-xr-x`-free permissions: octal, the form `chmod` users expect.
pub fn permissions_label(mode: Option<u32>) -> Option<String> {
    mode.map(|mode| format!("{:03o}", mode & 0o777))
}

/// Label/value rows in display order. `folder_bytes` is the recursively measured
/// size of a folder, which arrives later than the rest, so `None` shows
/// "Calculating…" for folders.
pub fn info_rows(entry: &FileEntry, folder_bytes: Option<u64>) -> Vec<(&'static str, String)> {
    let size = if entry.is_dir() {
        match folder_bytes {
            Some(bytes) => format_size(bytes),
            None => "Calculating…".to_string(),
        }
    } else {
        let exact = entry.size;
        format!("{} ({exact} bytes)", format_size(exact))
    };
    let location = entry
        .uri
        .as_local_path()
        .and_then(|path| path.parent().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| entry.uri.to_string());

    let mut rows = vec![
        ("Kind", kind_label(entry)),
        ("Size", size),
        ("Where", location),
        ("Created", format_time(entry.created)),
        ("Modified", format_time(entry.modified)),
    ];
    if let Some(mode) = permissions_label(entry.permissions) {
        rows.push(("Permissions", mode));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs_core::VfsUri;

    fn entry(name: &str, kind: FileKind) -> FileEntry {
        FileEntry::new(VfsUri::local(format!("/home/me/{name}")), name, kind)
    }

    #[test]
    fn kind_reflects_file_type_and_extension() {
        assert_eq!(kind_label(&entry("d", FileKind::Directory)), "Folder");
        assert_eq!(kind_label(&entry("l", FileKind::Symlink)), "Alias");
        assert_eq!(kind_label(&entry("main.rs", FileKind::File)), "RS File");
        assert_eq!(kind_label(&entry("Makefile", FileKind::File)), "Document");
    }

    #[test]
    fn files_show_exact_bytes_and_their_parent_folder() {
        let mut file = entry("a.txt", FileKind::File);
        file.size = 2048;
        let rows = info_rows(&file, None);
        let get = |label: &str| rows.iter().find(|(l, _)| *l == label).unwrap().1.clone();
        assert!(get("Size").contains("2048 bytes"));
        assert_eq!(get("Where"), "/home/me");
    }

    #[test]
    fn folder_size_is_pending_until_measured() {
        let dir = entry("d", FileKind::Directory);
        let pending = info_rows(&dir, None);
        assert_eq!(pending[1].1, "Calculating…");
        let done = info_rows(&dir, Some(1024));
        assert_ne!(done[1].1, "Calculating…");
    }

    #[test]
    fn permissions_are_octal_and_optional() {
        assert_eq!(permissions_label(Some(0o100644)), Some("644".to_string()));
        assert_eq!(permissions_label(None), None);
        let mut file = entry("a", FileKind::File);
        assert!(
            info_rows(&file, None)
                .iter()
                .all(|(l, _)| *l != "Permissions")
        );
        file.permissions = Some(0o755);
        assert!(
            info_rows(&file, None)
                .iter()
                .any(|(l, _)| *l == "Permissions")
        );
    }
}
