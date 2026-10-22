//! Icons: the Lucide set (ISC, see `assets/icons/LICENSE-lucide.txt`), embedded
//! in the binary.
//!
//! Every glyph is monochrome: GPUI tints an SVG with the element's text color, so
//! one asset serves every state (idle / hover / selected / accent).

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use vfs_core::{FileEntry, FileKind};

pub const FOLDER: &str = "icons/folder.svg";
pub const FOLDER_OPEN: &str = "icons/folder-open.svg";
pub const FILE: &str = "icons/file.svg";
pub const FILE_CODE: &str = "icons/file-code.svg";
pub const FILE_TEXT: &str = "icons/file-text.svg";
pub const FILE_IMAGE: &str = "icons/file-image.svg";
pub const FILE_VIDEO: &str = "icons/file-video.svg";
pub const FILE_AUDIO: &str = "icons/file-audio.svg";
pub const FILE_ARCHIVE: &str = "icons/file-archive.svg";
pub const FILE_JSON: &str = "icons/file-json.svg";
pub const FILE_SHEET: &str = "icons/file-spreadsheet.svg";
pub const FILE_SCRIPT: &str = "icons/file-terminal.svg";
pub const FILE_CONFIG: &str = "icons/file-cog.svg";
pub const PACKAGE: &str = "icons/package.svg";
pub const APP: &str = "icons/app-window.svg";
pub const LINK: &str = "icons/link.svg";
pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
pub const ARROW_UP: &str = "icons/arrow-up.svg";
pub const SEARCH: &str = "icons/search.svg";
pub const CLOSE: &str = "icons/x.svg";
pub const PLUS: &str = "icons/plus.svg";
pub const TERMINAL: &str = "icons/terminal.svg";
pub const TASKS: &str = "icons/list-checks.svg";
pub const EYE: &str = "icons/eye.svg";
pub const HOME: &str = "icons/house.svg";
pub const DESKTOP: &str = "icons/monitor.svg";
pub const DOWNLOAD: &str = "icons/download.svg";
pub const DISK: &str = "icons/hard-drive.svg";

const ICONS: &[(&str, &str)] = &[
    (FOLDER, include_str!("../assets/icons/folder.svg")),
    (FOLDER_OPEN, include_str!("../assets/icons/folder-open.svg")),
    (FILE, include_str!("../assets/icons/file.svg")),
    (FILE_CODE, include_str!("../assets/icons/file-code.svg")),
    (FILE_TEXT, include_str!("../assets/icons/file-text.svg")),
    (FILE_IMAGE, include_str!("../assets/icons/file-image.svg")),
    (FILE_VIDEO, include_str!("../assets/icons/file-video.svg")),
    (FILE_AUDIO, include_str!("../assets/icons/file-audio.svg")),
    (FILE_ARCHIVE, include_str!("../assets/icons/file-archive.svg")),
    (FILE_JSON, include_str!("../assets/icons/file-json.svg")),
    (FILE_SHEET, include_str!("../assets/icons/file-spreadsheet.svg")),
    (FILE_SCRIPT, include_str!("../assets/icons/file-terminal.svg")),
    (FILE_CONFIG, include_str!("../assets/icons/file-cog.svg")),
    (PACKAGE, include_str!("../assets/icons/package.svg")),
    (APP, include_str!("../assets/icons/app-window.svg")),
    (LINK, include_str!("../assets/icons/link.svg")),
    (CHEVRON_RIGHT, include_str!("../assets/icons/chevron-right.svg")),
    (CHEVRON_DOWN, include_str!("../assets/icons/chevron-down.svg")),
    (ARROW_UP, include_str!("../assets/icons/arrow-up.svg")),
    (SEARCH, include_str!("../assets/icons/search.svg")),
    (CLOSE, include_str!("../assets/icons/x.svg")),
    (PLUS, include_str!("../assets/icons/plus.svg")),
    (TERMINAL, include_str!("../assets/icons/terminal.svg")),
    (TASKS, include_str!("../assets/icons/list-checks.svg")),
    (EYE, include_str!("../assets/icons/eye.svg")),
    (HOME, include_str!("../assets/icons/house.svg")),
    (DESKTOP, include_str!("../assets/icons/monitor.svg")),
    (DOWNLOAD, include_str!("../assets/icons/download.svg")),
    (DISK, include_str!("../assets/icons/hard-drive.svg")),
];

/// Icon for a directory entry, chosen by kind and then by extension.
pub fn file_icon(entry: &FileEntry) -> &'static str {
    if entry.is_dir() {
        return FOLDER;
    }
    if entry.kind == FileKind::Symlink {
        return LINK;
    }
    let Some(ext) = entry.extension() else {
        return FILE;
    };
    icon_for_extension(&ext)
}

/// Extension → icon. `ext` must already be lowercase (as `extension()` returns).
pub fn icon_for_extension(ext: &str) -> &'static str {
    match ext {
        "rs" | "go" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp" | "java"
        | "rb" | "lua" | "zig" | "swift" | "kt" | "html" | "css" | "scss" | "vue" | "php" => {
            FILE_CODE
        }
        "sh" | "bash" | "zsh" | "fish" | "bat" | "ps1" => FILE_SCRIPT,
        "json" | "jsonc" | "json5" => FILE_JSON,
        "toml" | "yaml" | "yml" | "ini" | "conf" | "env" | "plist" | "lock" => FILE_CONFIG,
        "md" | "txt" | "rst" | "log" | "pdf" | "doc" | "docx" | "rtf" => FILE_TEXT,
        "csv" | "tsv" | "xls" | "xlsx" | "numbers" => FILE_SHEET,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" | "tiff" | "ico" => {
            FILE_IMAGE
        }
        "mp4" | "mov" | "mkv" | "avi" | "webm" => FILE_VIDEO,
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" => FILE_AUDIO,
        "zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar" | "bz2" => FILE_ARCHIVE,
        "dmg" | "pkg" | "deb" | "rpm" => PACKAGE,
        "app" | "exe" => APP,
        _ => FILE,
    }
}

/// Serves the embedded icons to GPUI's SVG renderer.
pub struct EmbeddedAssets;

impl AssetSource for EmbeddedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, svg)| Cow::Borrowed(svg.as_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::new_static(name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs_core::VfsUri;

    fn entry(name: &str, kind: FileKind) -> FileEntry {
        FileEntry::new(VfsUri::local(format!("/tmp/{name}")), name, kind)
    }

    #[test]
    fn serves_every_icon() {
        let assets = EmbeddedAssets;
        for (name, _) in ICONS {
            let data = assets.load(name).unwrap().expect("icon present");
            let text = std::str::from_utf8(&data).unwrap();
            assert!(text.contains("<svg"), "{name}");
        }
    }

    #[test]
    fn unknown_asset_is_none() {
        assert!(EmbeddedAssets.load("icons/nope.svg").unwrap().is_none());
    }

    #[test]
    fn picks_icon_by_kind_then_extension() {
        assert_eq!(file_icon(&entry("d", FileKind::Directory)), FOLDER);
        assert_eq!(file_icon(&entry("l", FileKind::Symlink)), LINK);
        assert_eq!(file_icon(&entry("main.rs", FileKind::File)), FILE_CODE);
        assert_eq!(file_icon(&entry("PHOTO.PNG", FileKind::File)), FILE_IMAGE);
        assert_eq!(file_icon(&entry("a.tar", FileKind::File)), FILE_ARCHIVE);
    }

    #[test]
    fn unknown_or_missing_extension_falls_back_to_plain_file() {
        assert_eq!(file_icon(&entry("Makefile", FileKind::File)), FILE);
        assert_eq!(file_icon(&entry("x.weird", FileKind::File)), FILE);
    }
}
