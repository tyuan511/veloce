//! The in-app file clipboard, with Windows Explorer semantics: Cut marks files,
//! Copy remembers them, and Paste moves (after Cut) or copies them into a folder.
//!
//! Pure data and planning, so the rules are testable without a window.

use std::path::{Path, PathBuf};

use vfs_core::ops::copy_path;

/// One filesystem job produced by a paste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteOp {
    Copy { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
}

#[derive(Debug, Clone, Default)]
pub struct FileClipboard {
    paths: Vec<PathBuf>,
    /// Cut (move on paste) rather than copy.
    cut: bool,
}

impl FileClipboard {
    pub fn copy(paths: Vec<PathBuf>) -> Self {
        Self { paths, cut: false }
    }

    pub fn cut(paths: Vec<PathBuf>) -> Self {
        Self { paths, cut: true }
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn is_cut(&self) -> bool {
        self.cut
    }

    /// Paths to draw dimmed: Explorer fades cut files until they are pasted.
    pub fn cut_paths(&self) -> &[PathBuf] {
        if self.cut { &self.paths } else { &[] }
    }

    /// What pasting into `dir` should do.
    ///
    /// - Cut into the folder the files already live in does nothing.
    /// - Copy into their own folder makes `name - Copy`, like Explorer.
    /// - A folder is never pasted into itself or a descendant, which would
    ///   recurse forever.
    pub fn plan_paste(&self, dir: &Path) -> Vec<PasteOp> {
        let mut ops = Vec::new();
        for from in &self.paths {
            let Some(name) = from.file_name() else {
                continue;
            };
            if dir.starts_with(from) {
                continue;
            }
            let same_folder = from.parent() == Some(dir);
            match (self.cut, same_folder) {
                (true, true) => {}
                (true, false) => ops.push(PasteOp::Move {
                    from: from.clone(),
                    to: dir.join(name),
                }),
                (false, true) => ops.push(PasteOp::Copy {
                    from: from.clone(),
                    to: copy_path(from),
                }),
                (false, false) => ops.push(PasteOp::Copy {
                    from: from.clone(),
                    to: dir.join(name),
                }),
            }
        }
        ops
    }

    /// Explorer empties the clipboard after a cut-paste (the files moved), but
    /// keeps a copy around so it can be pasted again.
    pub fn after_paste(self) -> Self {
        if self.cut { Self::default() } else { self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn paths(names: &[&str], dir: &Path) -> Vec<PathBuf> {
        names.iter().map(|name| dir.join(name)).collect()
    }

    #[test]
    fn copy_into_another_folder_keeps_names() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let clip = FileClipboard::copy(paths(&["a.txt"], src.path()));
        assert_eq!(
            clip.plan_paste(dst.path()),
            [PasteOp::Copy {
                from: src.path().join("a.txt"),
                to: dst.path().join("a.txt")
            }]
        );
    }

    #[test]
    fn cut_into_another_folder_moves() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let clip = FileClipboard::cut(paths(&["a.txt"], src.path()));
        assert!(matches!(
            clip.plan_paste(dst.path()).as_slice(),
            [PasteOp::Move { .. }]
        ));
    }

    #[test]
    fn cut_into_its_own_folder_does_nothing() {
        let src = tempfile::tempdir().unwrap();
        let clip = FileClipboard::cut(paths(&["a.txt"], src.path()));
        assert!(clip.plan_paste(src.path()).is_empty());
    }

    #[test]
    fn copy_into_its_own_folder_makes_a_dash_copy() {
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join("a.txt"), b"x").unwrap();
        let clip = FileClipboard::copy(paths(&["a.txt"], src.path()));
        assert_eq!(
            clip.plan_paste(src.path()),
            [PasteOp::Copy {
                from: src.path().join("a.txt"),
                to: src.path().join("a - Copy.txt")
            }]
        );
    }

    #[test]
    fn a_folder_is_never_pasted_into_itself_or_a_descendant() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("proj");
        let inner = folder.join("sub");
        fs::create_dir_all(&inner).unwrap();
        let clip = FileClipboard::copy(vec![folder.clone()]);
        assert!(clip.plan_paste(&folder).is_empty());
        assert!(clip.plan_paste(&inner).is_empty());
        assert_eq!(clip.plan_paste(root.path()).len(), 1);
    }

    #[test]
    fn only_cut_files_are_reported_for_dimming() {
        let dir = Path::new("/x");
        assert!(
            FileClipboard::copy(paths(&["a"], dir))
                .cut_paths()
                .is_empty()
        );
        assert_eq!(FileClipboard::cut(paths(&["a"], dir)).cut_paths().len(), 1);
    }

    #[test]
    fn cut_clears_after_paste_but_copy_stays() {
        let dir = Path::new("/x");
        assert!(
            FileClipboard::cut(paths(&["a"], dir))
                .after_paste()
                .is_empty()
        );
        assert!(
            !FileClipboard::copy(paths(&["a"], dir))
                .after_paste()
                .is_empty()
        );
    }

    #[test]
    fn empty_clipboard_plans_nothing() {
        assert!(
            FileClipboard::default()
                .plan_paste(Path::new("/x"))
                .is_empty()
        );
        assert!(FileClipboard::default().is_empty());
    }
}
