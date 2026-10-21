//! The visual language: a single dark palette plus the spacing scale.
//!
//! Keeping colors in one place makes the "one accent, no chrome" look easy to
//! hold: the UI is mostly neutral greys and only the focus ring, selection and
//! active tab are tinted.

use gpui::Hsla;

pub const BG: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.086,
    a: 1.0,
}; // window background
pub const BG_PANE: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.106,
    a: 1.0,
};
pub const BG_RAISED: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.145,
    a: 1.0,
};
pub const BG_HOVER: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.19,
    a: 1.0,
};
pub const BG_SELECTED: Hsla = Hsla {
    h: 0.58,
    s: 0.55,
    l: 0.36,
    a: 1.0,
};
pub const BORDER: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.2,
    a: 1.0,
};
pub const BORDER_SOFT: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.16,
    a: 1.0,
};

pub const TEXT: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.93,
    a: 1.0,
};
pub const TEXT_MUTED: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.62,
    a: 1.0,
};
pub const TEXT_FAINT: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.42,
    a: 1.0,
};

pub const ACCENT: Hsla = Hsla {
    h: 0.58,
    s: 0.85,
    l: 0.62,
    a: 1.0,
};
pub const ACCENT_DIM: Hsla = Hsla {
    h: 0.58,
    s: 0.5,
    l: 0.45,
    a: 1.0,
};

pub const GREEN: Hsla = Hsla {
    h: 0.36,
    s: 0.6,
    l: 0.55,
    a: 1.0,
};
pub const YELLOW: Hsla = Hsla {
    h: 0.11,
    s: 0.75,
    l: 0.6,
    a: 1.0,
};
pub const RED: Hsla = Hsla {
    h: 0.0,
    s: 0.62,
    l: 0.6,
    a: 1.0,
};
pub const BLUE: Hsla = Hsla {
    h: 0.56,
    s: 0.7,
    l: 0.62,
    a: 1.0,
};
pub const PURPLE: Hsla = Hsla {
    h: 0.75,
    s: 0.5,
    l: 0.65,
    a: 1.0,
};

/// Row height for the file list. Fixed so `uniform_list` can virtualize on it.
pub const ROW_HEIGHT: f32 = 26.0;
pub const STATUS_HEIGHT: f32 = 26.0;

pub const MONO_FAMILY: &str = "Menlo";

// Contrast invariants, checked at compile time: a theme edit that makes text
// unreadable must fail the build, not a test someone forgot to run.
const _: () = {
    assert!(TEXT.l > BG.l);
    assert!(TEXT.l > BG_PANE.l);
    assert!(TEXT.l > BG_RAISED.l);
    assert!(TEXT.l > BG_HOVER.l);
    assert!(TEXT_MUTED.l > BG_PANE.l);
    assert!(TEXT_MUTED.l > BG_HOVER.l);
    assert!(BG_SELECTED.l > BG_PANE.l);
    assert!(ACCENT.l > BG_PANE.l);
    assert!(ROW_HEIGHT.fract() == 0.0, "uniform_list needs whole rows");
};

/// Accent color for a file kind / extension, used by the icon column.
pub fn kind_color(entry: &vfs_core::FileEntry) -> Hsla {
    use vfs_core::FileKind;
    if entry.is_dir() {
        return BLUE;
    }
    match entry.kind {
        FileKind::Symlink => PURPLE,
        FileKind::Special => TEXT_FAINT,
        _ => {
            let Some(ext) = entry.extension() else {
                return TEXT_FAINT;
            };
            match ext.as_str() {
                "rs" | "go" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp"
                | "java" | "rb" | "sh" | "lua" | "zig" | "swift" | "kt" => ACCENT,
                "json" | "toml" | "yaml" | "yml" | "ini" | "conf" | "env" => YELLOW,
                "md" | "txt" | "rst" | "log" => TEXT_MUTED,
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" | "tiff" => GREEN,
                "mp4" | "mov" | "mkv" | "avi" | "webm" => PURPLE,
                "zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar" => YELLOW,
                "pdf" => RED,
                _ => TEXT_FAINT,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs_core::{FileEntry, FileKind, VfsUri};

    fn file(name: &str) -> FileEntry {
        FileEntry::new(VfsUri::local(format!("/tmp/{name}")), name, FileKind::File)
    }

    #[test]
    fn directories_are_always_blue() {
        let dir = FileEntry::new(VfsUri::local("/tmp/d"), "d", FileKind::Directory);
        assert_eq!(kind_color(&dir), BLUE);
    }

    #[test]
    fn source_files_get_the_accent_color() {
        for name in ["a.rs", "b.go", "c.py", "d.ts", "e.tsx", "f.c"] {
            assert_eq!(kind_color(&file(name)), ACCENT, "{name}");
        }
    }

    #[test]
    fn config_files_are_yellow_and_images_green() {
        assert_eq!(kind_color(&file("Cargo.toml")), YELLOW);
        assert_eq!(kind_color(&file("photo.png")), GREEN);
    }

    #[test]
    fn unknown_and_extensionless_files_are_faint() {
        assert_eq!(kind_color(&file("noext")), TEXT_FAINT);
        assert_eq!(kind_color(&file("data.unknownext")), TEXT_FAINT);
    }

    #[test]
    fn symlinks_and_specials_are_distinct() {
        let link = FileEntry::new(VfsUri::local("/tmp/l"), "l", FileKind::Symlink);
        assert_eq!(kind_color(&link), PURPLE);
        let special = FileEntry::new(VfsUri::local("/tmp/s"), "s", FileKind::Special);
        assert_eq!(kind_color(&special), TEXT_FAINT);
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(kind_color(&file("MAIN.RS")), ACCENT);
        assert_eq!(kind_color(&file("PHOTO.PNG")), GREEN);
    }
}
