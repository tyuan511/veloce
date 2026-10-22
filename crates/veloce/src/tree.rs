//! Folder tree for the sidebar: pure state + flattening, no GPUI.
//!
//! Children are loaded lazily (one `read_dir` per expanded folder) and cached, so
//! expanding a folder never walks the whole disk. Loading itself happens on a
//! background task in the workspace; this module only owns the data and the rules.

use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};

/// One visible line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub expanded: bool,
}

#[derive(Debug, Default)]
pub struct TreeState {
    pub root: PathBuf,
    pub expanded: HashSet<PathBuf>,
    /// Sorted sub-directories per loaded folder. Absent = not loaded yet.
    pub children: HashMap<PathBuf, Vec<PathBuf>>,
}

impl TreeState {
    pub fn new(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        Self {
            root,
            expanded,
            children: HashMap::new(),
        }
    }

    pub fn is_loaded(&self, path: &Path) -> bool {
        self.children.contains_key(path)
    }

    pub fn set_children(&mut self, path: PathBuf, children: Vec<PathBuf>) {
        self.children.insert(path, children);
    }

    /// Flip a folder open/closed. Returns `true` if it now needs its children loaded.
    pub fn toggle(&mut self, path: &Path) -> bool {
        if self.expanded.remove(path) {
            return false;
        }
        self.expanded.insert(path.to_path_buf());
        !self.is_loaded(path)
    }

    /// Expand every ancestor of `path` (below the root) so it becomes visible.
    /// Returns the folders whose children still have to be loaded.
    pub fn reveal(&mut self, path: &Path) -> Vec<PathBuf> {
        let mut to_load = Vec::new();
        if !path.starts_with(&self.root) {
            return to_load;
        }
        let mut chain: Vec<&Path> = path
            .ancestors()
            .take_while(|ancestor| ancestor.starts_with(&self.root))
            .collect();
        chain.reverse();
        for ancestor in chain {
            // The target itself stays as the user left it; only its parents open.
            if ancestor == path {
                break;
            }
            self.expanded.insert(ancestor.to_path_buf());
            if !self.is_loaded(ancestor) {
                to_load.push(ancestor.to_path_buf());
            }
        }
        to_load
    }

    /// The rows to draw, depth-first. The root itself is not a row.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.push_children(&self.root, 0, &mut rows);
        rows
    }

    fn push_children(&self, dir: &Path, depth: usize, rows: &mut Vec<TreeRow>) {
        let Some(children) = self.children.get(dir) else {
            return;
        };
        for child in children {
            let expanded = self.expanded.contains(child);
            rows.push(TreeRow {
                path: child.clone(),
                name: child
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                depth,
                expanded,
            });
            if expanded {
                self.push_children(child, depth + 1, rows);
            }
        }
    }
}

/// Visible sub-directories of `dir`, sorted case-insensitively. Dot-folders are
/// left out: they would drown the tree in `.cache`-style noise, and the main
/// pane still shows them.
pub fn list_subdirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        // `metadata` follows symlinks, so a link to a folder is navigable too.
        if std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir()) {
            dirs.push(entry.path());
        }
    }
    dirs.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree_with(root: &Path, loaded: &[(&str, &[&str])]) -> TreeState {
        let mut state = TreeState::new(root.to_path_buf());
        for (dir, kids) in loaded {
            state.set_children(
                root.join(dir),
                kids.iter().map(|kid| root.join(dir).join(kid)).collect(),
            );
        }
        state
    }

    #[test]
    fn lists_only_visible_directories_sorted_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["beta", "Alpha", ".hidden"] {
            fs::create_dir(dir.path().join(name)).unwrap();
        }
        fs::write(dir.path().join("file.txt"), b"x").unwrap();
        let names: Vec<String> = list_subdirs(dir.path())
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Alpha", "beta"]);
    }

    #[test]
    fn listing_a_missing_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_subdirs(&dir.path().join("nope")).is_err());
    }

    #[test]
    fn rows_are_empty_until_the_root_is_loaded() {
        let state = TreeState::new(PathBuf::from("/r"));
        assert!(state.rows().is_empty());
    }

    #[test]
    fn expanded_folders_show_their_children_indented() {
        let root = PathBuf::from("/r");
        let mut state = TreeState::new(root.clone());
        state.set_children(root.clone(), vec![root.join("a"), root.join("b")]);
        state.set_children(root.join("a"), vec![root.join("a/x")]);
        assert_eq!(state.rows().len(), 2);

        assert!(!state.toggle(&root.join("a")), "already loaded");
        let rows = state.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[1].name.as_str(), rows[1].depth), ("x", 1));
        assert!(rows[0].expanded);
    }

    #[test]
    fn toggle_reports_when_children_must_be_loaded() {
        let mut state = TreeState::new(PathBuf::from("/r"));
        assert!(state.toggle(Path::new("/r/a")), "first expand needs a load");
        assert!(!state.toggle(Path::new("/r/a")), "collapse needs nothing");
    }

    #[test]
    fn collapsing_hides_the_whole_subtree() {
        let root = PathBuf::from("/r");
        let mut state = tree_with(Path::new("/r"), &[("a", &["x"])]);
        state.set_children(root.clone(), vec![root.join("a")]);
        state.expanded.insert(root.join("a"));
        assert_eq!(state.rows().len(), 2);
        state.toggle(&root.join("a"));
        assert_eq!(state.rows().len(), 1);
    }

    #[test]
    fn reveal_opens_ancestors_but_not_the_target() {
        let mut state = TreeState::new(PathBuf::from("/r"));
        let to_load = state.reveal(Path::new("/r/a/b/c"));
        // The root comes first: its children have to exist before a descendant can show.
        assert_eq!(
            to_load,
            [
                PathBuf::from("/r"),
                PathBuf::from("/r/a"),
                PathBuf::from("/r/a/b")
            ]
        );
        assert!(state.expanded.contains(Path::new("/r/a/b")));
        assert!(!state.expanded.contains(Path::new("/r/a/b/c")));
    }

    #[test]
    fn reveal_ignores_paths_outside_the_root() {
        let mut state = TreeState::new(PathBuf::from("/r"));
        assert!(state.reveal(Path::new("/elsewhere/x")).is_empty());
        assert_eq!(state.expanded.len(), 1);
    }
}
