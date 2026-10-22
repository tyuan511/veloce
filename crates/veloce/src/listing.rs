//! Pure listing logic: sorting, filtering, selection and cursor arithmetic.
//!
//! Everything here takes plain data and returns plain data. Nothing needs a
//! `Context`, a `Window` or a render pass, which is what makes the behaviour of
//! the file list testable without a GPU or a window server.
//!
//! `Pane` owns one of these per tab and only adds the parts that must talk to
//! GPUI: streaming, notifications and file operations.

use std::path::PathBuf;

use vfs_core::{FileEntry, Filter, SortSpec, VfsUri};

use crate::{i18n::trf, pane::JumpTarget};

/// A directory listing after sort and filter, plus the selection state that has
/// to be kept consistent with it.
#[derive(Debug, Default, Clone)]
pub struct Listing {
    /// Every entry of the directory, sorted.
    pub entries: Vec<FileEntry>,
    /// Indices into `entries`, in display order (filter applied).
    pub visible: Vec<usize>,
    /// Indices into `entries`, always a subset of what is visible after a resync.
    pub selected: Vec<usize>,
    /// Position inside `visible`, not inside `entries`.
    pub cursor: usize,
    /// Set by a click on blank space: nothing is selected and no row counts as
    /// the cursor, so actions and the preview have no target. Any move clears it.
    pub cursor_hidden: bool,
    /// Dot-files are hidden unless this is on (Finder's ⇧⌘.).
    pub show_hidden: bool,
}

/// Unix convention, which Finder follows: a leading dot hides an entry.
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

impl Listing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the entry set and re-derive the view.
    pub fn set_entries(
        &mut self,
        mut entries: Vec<FileEntry>,
        sort: &SortSpec,
        filter: &mut Filter,
    ) {
        sort.sort(&mut entries);
        self.entries = entries;
        self.resync(sort, filter);
    }

    /// Append a streamed chunk, keeping the current cursor anchored to its file.
    ///
    /// Streaming is why this cannot be a plain `set_entries`: the first chunk
    /// arrives alone, and re-sorting the whole set on every chunk would make the
    /// cursor jump while the user is already reading the list.
    pub fn extend_entries(&mut self, chunk: Vec<FileEntry>, sort: &SortSpec, filter: &mut Filter) {
        self.entries.extend(chunk);
        self.resync(sort, filter);
    }

    /// Re-apply sort and filter, preserving the cursor by file identity.
    pub fn resync(&mut self, sort: &SortSpec, filter: &mut Filter) {
        let cursor_id = self.cursor_entry().map(|entry| entry.id());

        let mut sorted = std::mem::take(&mut self.entries);
        sort.sort(&mut sorted);
        self.entries = sorted;

        self.visible = filter.matching_indices(&self.entries);
        if !self.show_hidden {
            let entries = &self.entries;
            self.visible
                .retain(|index| !is_hidden(&entries[*index].name));
        }
        self.selected.retain(|index| *index < self.entries.len());

        if let Some(position) = cursor_id.and_then(|id| {
            self.visible
                .iter()
                .position(|index| self.entries[*index].id() == id)
        }) {
            self.cursor = position;
            return;
        }
        self.cursor = clamp_cursor(self.cursor, self.visible.len());
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.visible.clear();
        self.selected.clear();
        self.cursor = 0;
        self.cursor_hidden = false;
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    pub fn len(&self) -> usize {
        self.visible.len()
    }

    pub fn cursor_entry(&self) -> Option<&FileEntry> {
        if self.cursor_hidden {
            return None;
        }
        self.visible
            .get(self.cursor)
            .and_then(|index| self.entries.get(*index))
    }

    /// The row under the cursor as a full entry, cloned for callers that outlive
    /// the borrow (file operations, preview).
    pub fn cursor_entry_cloned(&self) -> Option<FileEntry> {
        self.cursor_entry().cloned()
    }

    /// Selected entries, falling back to the cursor row so every action works
    /// without an explicit multi-selection.
    pub fn selection_or_cursor(&self) -> Vec<FileEntry> {
        if self.selected.is_empty() {
            return self.cursor_entry_cloned().into_iter().collect();
        }
        self.selected
            .iter()
            .filter_map(|index| self.entries.get(*index).cloned())
            .collect()
    }

    /// Put the cursor on `position`, replacing the selection (or extending it).
    pub fn set_cursor(&mut self, position: usize, extend: bool) {
        if position >= self.visible.len() {
            return;
        }
        self.cursor_hidden = false;
        self.cursor = position;
        let index = self.visible[position];
        if extend {
            if !self.selected.contains(&index) {
                self.selected.push(index);
                self.selected.sort_unstable();
            }
        } else {
            self.selected.clear();
            self.selected.push(index);
        }
    }

    /// Move by `delta` rows, clamped to the list.
    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
        if self.visible.is_empty() {
            return;
        }
        let position = step(self.cursor, delta, self.visible.len());
        self.set_cursor(position, extend);
    }

    /// Visual-mode extension: grow the selection while moving the cursor.
    ///
    /// The anchor row is always selected, so a single `shift-j` from an empty
    /// selection selects two rows instead of one — the behaviour a keyboard user
    /// expects from a range selection.
    pub fn extend_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        self.cursor_hidden = false;
        if self.selected.is_empty() {
            self.selected.push(self.visible[self.cursor]);
        }
        let position = step(self.cursor, delta, self.visible.len());
        self.cursor = position;
        let index = self.visible[position];
        if !self.selected.contains(&index) {
            self.selected.push(index);
            self.selected.sort_unstable();
        }
    }

    /// Jump to the first or last row.
    pub fn jump(&mut self, target: JumpTarget) {
        if self.visible.is_empty() {
            return;
        }
        let position = match target {
            JumpTarget::Top => 0,
            JumpTarget::Bottom => self.visible.len() - 1,
        };
        self.set_cursor(position, false);
    }

    pub fn select_all(&mut self) {
        self.cursor_hidden = false;
        self.selected = self.visible.clone();
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    /// Blank-space click: drop the selection and the cursor highlight too.
    pub fn deselect_all(&mut self) {
        self.selected.clear();
        self.cursor_hidden = true;
    }

    /// The cursor row, unless a blank click has hidden it.
    pub fn visible_cursor(&self) -> Option<usize> {
        (!self.cursor_hidden).then_some(self.cursor)
    }

    /// The status line: item counts, folder counts and the selection's byte size.
    /// Entries that could be shown: everything, or everything not hidden.
    pub fn shown_total(&self) -> usize {
        if self.show_hidden {
            self.entries.len()
        } else {
            self.entries
                .iter()
                .filter(|entry| !is_hidden(&entry.name))
                .count()
        }
    }

    pub fn status(&self) -> String {
        let total = self.shown_total();
        let visible = self.visible.len();
        let dirs = self
            .entries
            .iter()
            .filter(|entry| entry.is_dir() && (self.show_hidden || !is_hidden(&entry.name)))
            .count();
        let mut status = if visible == total {
            trf(
                "{} items · {} folders",
                &[&total.to_string(), &dirs.to_string()],
            )
        } else {
            trf(
                "{}/{} items · {} folders",
                &[&visible.to_string(), &total.to_string(), &dirs.to_string()],
            )
        };
        if !self.selected.is_empty() {
            let bytes: u64 = self
                .selected
                .iter()
                .filter_map(|index| self.entries.get(*index))
                .filter(|entry| !entry.is_dir())
                .map(|entry| entry.size)
                .sum();
            status.push_str(&trf(
                " · {} selected ({})",
                &[
                    &self.selected.len().to_string(),
                    &vfs_core::entry::format_size(bytes),
                ],
            ));
        }
        status
    }

    /// Move the cursor onto the entry with this uri, if it is visible.
    pub fn focus_uri(&mut self, uri: &VfsUri) {
        if let Some(position) = self.position_of(uri) {
            self.set_cursor(position, false);
        }
    }

    /// Row position of the visible entry with this uri.
    pub fn position_of(&self, uri: &VfsUri) -> Option<usize> {
        self.visible
            .iter()
            .position(|index| &self.entries[*index].uri == uri)
    }
}

/// Clamp a cursor into `0..len`, tolerating an empty list.
pub fn clamp_cursor(cursor: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    cursor.min(len - 1)
}

/// Step a cursor by `delta`, saturating at both ends instead of wrapping.
///
/// Wrapping would be wrong here: pressing `j` on the last row should stay put, not
/// teleport to the top of the directory.
pub fn step(cursor: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = len as isize - 1;
    (cursor as isize + delta).clamp(0, max) as usize
}

/// Next tab index for `⌘⇧]` / `⌘⇧[`, wrapping in both directions.
pub fn cycle_index(current: usize, len: usize, forward: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if forward {
        (current + 1) % len
    } else {
        (current + len - 1) % len
    }
}

/// Index to keep after closing tab `closed` from a strip of `len` tabs.
///
/// Closing a tab to the left of the active one must shift the active index down,
/// otherwise the user ends up looking at a different tab than they were.
pub fn index_after_close(active: usize, closed: usize, len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    let remaining = len - 1;
    if closed < active {
        active - 1
    } else {
        active.min(remaining - 1)
    }
}

/// Expand a leading `~` to the home directory.
pub fn expand_home(value: &str) -> PathBuf {
    if let (Some(rest), Some(home)) = (value.strip_prefix('~'), std::env::var_os("HOME")) {
        return PathBuf::from(home).join(rest.trim_start_matches('/'));
    }
    PathBuf::from(value)
}

/// A fixed shortcut in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    pub icon: &'static str,
    /// A starred file: clicking reveals it in its folder instead of entering it.
    pub is_file: bool,
    /// One the user starred (as opposed to a built-in place): it has its own menu.
    pub starred: bool,
}

fn place(label: impl Into<String>, path: impl Into<PathBuf>, icon: &'static str) -> Place {
    Place {
        label: label.into(),
        path: path.into(),
        icon,
        is_file: false,
        starred: false,
    }
}

/// Finder's default "Favorites": Applications, Desktop, Documents, Downloads and
/// the home folder, which Finder labels with the account name.
///
/// Existence is deliberately not probed here (that would be file I/O on the UI
/// thread); a missing folder just shows the pane's normal load error.
pub fn favorite_places(home: &std::path::Path) -> Vec<Place> {
    use crate::icons;
    let home_label = home
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Home".to_string());
    vec![
        place("Applications", "/Applications", icons::APPS),
        place("Desktop", home.join("Desktop"), icons::DESKTOP),
        place("Documents", home.join("Documents"), icons::FILE),
        place("Downloads", home.join("Downloads"), icons::DOWNLOAD),
        place(home_label, home, icons::HOME),
    ]
}

/// Finder's "Locations": the startup disk.
pub fn location_places() -> Vec<Place> {
    vec![place("Macintosh HD", "/", crate::icons::DISK)]
}

/// A file or folder the user starred. Whether it is a folder is remembered so the
/// sidebar can pick an icon and a click action without touching the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favorite {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Settings key the starred list is stored under.
pub const FAVORITES_KEY: &str = "favorites";

pub fn favorites_to_json(favorites: &[Favorite]) -> String {
    serde_json::Value::Array(
        favorites
            .iter()
            .map(|favorite| {
                serde_json::json!({
                    "path": favorite.path.to_string_lossy(),
                    "dir": favorite.is_dir,
                })
            })
            .collect(),
    )
    .to_string()
}

/// Read the stored list. Anything unreadable counts as "nothing starred": a damaged
/// value must not stop the app from starting. Duplicates are dropped.
pub fn favorites_from_json(text: &str) -> Vec<Favorite> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(text)
    else {
        return Vec::new();
    };
    let mut out: Vec<Favorite> = Vec::new();
    for item in items {
        let Some(path) = item
            .get("path")
            .and_then(|value| value.as_str())
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let path = PathBuf::from(path);
        if out.iter().any(|existing| existing.path == path) {
            continue;
        }
        let is_dir = item
            .get("dir")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        out.push(Favorite { path, is_dir });
    }
    out
}

pub fn is_favorite(favorites: &[Favorite], path: &std::path::Path) -> bool {
    favorites.iter().any(|favorite| favorite.path == path)
}

/// Star `favorite`, or unstar it when it already is. Returns whether it is starred
/// afterwards.
pub fn toggle_favorite(favorites: &mut Vec<Favorite>, favorite: Favorite) -> bool {
    match favorites
        .iter()
        .position(|existing| existing.path == favorite.path)
    {
        Some(index) => {
            favorites.remove(index);
            false
        }
        None => {
            favorites.push(favorite);
            true
        }
    }
}

/// The sidebar rows for starred items, skipping any that duplicate a built-in place.
pub fn favorite_rows(favorites: &[Favorite], builtin: &[Place]) -> Vec<Place> {
    favorites
        .iter()
        .filter(|favorite| !builtin.iter().any(|place| place.path == favorite.path))
        .map(|favorite| {
            let label = favorite
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| favorite.path.to_string_lossy().into_owned());
            let icon = if favorite.is_dir {
                crate::icons::FOLDER
            } else {
                crate::icons::FILE
            };
            Place {
                is_file: !favorite.is_dir,
                starred: true,
                ..place(label, favorite.path.clone(), icon)
            }
        })
        .collect()
}

/// Whether `place` is the sidebar entry that matches `current` exactly.
pub fn place_is_current(place: &Place, current: &str) -> bool {
    let current = current.trim_end_matches('/');
    let place_path = place.path.to_string_lossy();
    let place_path = place_path.trim_end_matches('/');
    place_path == current
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs_core::{FileKind, SortKey, filter::FilterMode};

    #[test]
    fn favorites_follow_finder_and_end_with_the_home_folder() {
        let places = favorite_places(std::path::Path::new("/Users/me"));
        let labels: Vec<&str> = places.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Applications", "Desktop", "Documents", "Downloads", "me"]
        );
        assert_eq!(places[0].path, PathBuf::from("/Applications"));
        assert_eq!(places[3].path, PathBuf::from("/Users/me/Downloads"));
        assert_eq!(places[4].path, PathBuf::from("/Users/me"));
    }

    #[test]
    fn home_labelled_home_when_it_has_no_file_name() {
        let places = favorite_places(std::path::Path::new("/"));
        assert_eq!(places.last().unwrap().label, "Home");
    }

    #[test]
    fn locations_list_the_startup_disk() {
        let places = location_places();
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].path, PathBuf::from("/"));
    }

    fn fav(path: &str, is_dir: bool) -> Favorite {
        Favorite {
            path: PathBuf::from(path),
            is_dir,
        }
    }

    #[test]
    fn favorites_survive_a_json_round_trip_with_their_kind() {
        let list = vec![
            fav("/Users/me/code", true),
            fav("/Users/me/notes.md", false),
        ];
        assert_eq!(favorites_from_json(&favorites_to_json(&list)), list);
    }

    #[test]
    fn damaged_favorites_load_as_empty_or_skip_bad_rows() {
        assert!(favorites_from_json("not json").is_empty());
        assert!(favorites_from_json("{}").is_empty());
        assert!(favorites_from_json("").is_empty());
        let loaded = favorites_from_json(
            r#"[{"path":"/a"},{"nope":1},{"path":""},{"path":"/a","dir":false},{"path":"/b","dir":false}]"#,
        );
        assert_eq!(loaded, [fav("/a", true), fav("/b", false)]);
    }

    #[test]
    fn toggling_stars_then_unstars() {
        let mut list = Vec::new();
        assert!(toggle_favorite(&mut list, fav("/a", true)));
        assert!(toggle_favorite(&mut list, fav("/b", false)));
        assert!(is_favorite(&list, std::path::Path::new("/a")));
        assert!(!toggle_favorite(&mut list, fav("/a", true)));
        assert!(!is_favorite(&list, std::path::Path::new("/a")));
        assert_eq!(list, [fav("/b", false)]);
    }

    #[test]
    fn favorite_rows_label_files_and_folders_and_skip_builtins() {
        let builtin = favorite_places(std::path::Path::new("/Users/me"));
        let rows = favorite_rows(
            &[
                fav("/Users/me/Downloads", true),
                fav("/Users/me/code", true),
                fav("/Users/me/todo.txt", false),
            ],
            &builtin,
        );
        let labels: Vec<&str> = rows.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["code", "todo.txt"]);
        assert!(!rows[0].is_file);
        assert!(rows[1].is_file);
        assert!(rows.iter().all(|row| row.starred));
        assert!(builtin.iter().all(|place| !place.starred));
    }

    #[test]
    fn place_matches_current_ignoring_trailing_slash() {
        let places = favorite_places(std::path::Path::new("/Users/me"));
        let home = places.last().unwrap();
        assert!(place_is_current(home, "/Users/me/"));
        assert!(!place_is_current(home, "/Users/me/Desktop"));
        let root = &location_places()[0];
        assert!(place_is_current(root, "/"));
    }

    fn entry(name: &str, size: u64, dir: bool) -> FileEntry {
        let mut entry = FileEntry::new(
            VfsUri::local(format!("/tmp/{name}")),
            name,
            if dir {
                FileKind::Directory
            } else {
                FileKind::File
            },
        );
        entry.size = size;
        entry
    }

    fn sample() -> Vec<FileEntry> {
        vec![
            entry("zebra.txt", 1, false),
            entry("alpha", 0, true),
            entry("main.rs", 2048, false),
            entry("readme.md", 6, false),
        ]
    }

    fn listing() -> Listing {
        let mut listing = Listing::new();
        let mut filter = Filter::new();
        listing.set_entries(sample(), &SortSpec::default(), &mut filter);
        listing
    }

    fn with_hidden(show_hidden: bool) -> Listing {
        let mut listing = Listing::new();
        listing.show_hidden = show_hidden;
        let mut filter = Filter::new();
        let entries = vec![
            entry(".git", 0, true),
            entry(".env", 3, false),
            entry("src", 0, true),
            entry("main.rs", 10, false),
        ];
        listing.set_entries(entries, &SortSpec::default(), &mut filter);
        listing
    }

    #[test]
    fn dot_files_are_hidden_by_default() {
        let listing = with_hidden(false);
        let names: Vec<&str> = listing
            .visible
            .iter()
            .map(|i| listing.entries[*i].name.as_str())
            .collect();
        assert_eq!(names, ["main.rs", "src"]);
        assert!(!Listing::new().show_hidden);
    }

    #[test]
    fn showing_hidden_files_lists_everything() {
        assert_eq!(with_hidden(true).len(), 4);
    }

    #[test]
    fn toggling_hidden_and_resyncing_changes_the_view() {
        let mut listing = with_hidden(false);
        listing.show_hidden = true;
        listing.resync(&SortSpec::default(), &mut Filter::new());
        assert_eq!(listing.len(), 4);
        listing.show_hidden = false;
        listing.resync(&SortSpec::default(), &mut Filter::new());
        assert_eq!(listing.len(), 2);
    }

    #[test]
    fn status_counts_only_what_can_be_shown() {
        assert_eq!(with_hidden(false).status(), "2 items · 1 folders");
        assert_eq!(with_hidden(true).status(), "4 items · 2 folders");
    }

    #[test]
    fn a_folder_of_only_hidden_files_reads_as_empty() {
        let mut listing = Listing::new();
        listing.set_entries(
            vec![entry(".hidden", 1, false)],
            &SortSpec::default(),
            &mut Filter::new(),
        );
        assert!(listing.is_empty());
        assert_eq!(listing.entries.len(), 1);
    }

    #[test]
    fn hidden_means_a_leading_dot() {
        assert!(is_hidden(".gitignore"));
        assert!(!is_hidden("a.rs"));
        assert!(!is_hidden("dir.d"));
    }

    #[test]
    fn position_of_finds_visible_entries_only() {
        let listing = listing();
        let target = listing.entries[2].uri.clone();
        assert_eq!(listing.position_of(&target), Some(2));
        assert_eq!(listing.position_of(&VfsUri::local("/tmp/missing")), None);
    }

    #[test]
    fn sorts_directories_first_then_names() {
        let listing = listing();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "main.rs", "readme.md", "zebra.txt"]);
        assert!(listing.entries[0].is_dir());
    }

    #[test]
    fn filter_narrows_visible_and_reports_counts() {
        let mut listing = listing();
        let mut filter = Filter::new();
        filter.set_query("rs");
        listing.resync(&SortSpec::default(), &mut filter);

        assert_eq!(listing.len(), 1);
        assert_eq!(listing.cursor_entry().unwrap().name, "main.rs");
        assert_eq!(listing.status(), "1/4 items · 1 folders");
    }

    #[test]
    fn resync_keeps_the_cursor_on_the_same_file() {
        let mut listing = listing();
        let mut filter = Filter::new();

        // Name order: alpha(0), main.rs(1), readme.md(2), zebra.txt(3).
        listing.set_cursor(1, false);
        assert_eq!(listing.cursor_entry().unwrap().name, "main.rs");

        // By ascending size the biggest file moves to the end: the cursor index
        // must follow it instead of staying at 1.
        let sort = SortSpec::new(SortKey::Size, false);
        listing.resync(&sort, &mut filter);

        assert_eq!(listing.cursor_entry().unwrap().name, "main.rs");
        assert_eq!(listing.cursor, 3, "the row moved, so the index had to move");
    }

    #[test]
    fn resync_clamps_the_cursor_when_the_file_disappears() {
        let mut listing = listing();
        let mut filter = Filter::new();
        listing.set_cursor(3, false);

        let mut remaining = listing.entries.clone();
        remaining.retain(|entry| entry.name != "zebra.txt");
        listing.set_entries(remaining, &SortSpec::default(), &mut filter);

        assert_eq!(listing.cursor, 2, "clamped to the new last row");
        assert!(listing.cursor_entry().is_some());
    }

    #[test]
    fn extend_entries_appends_without_losing_the_cursor() {
        let mut listing = Listing::new();
        let mut filter = Filter::new();
        listing.set_entries(
            vec![entry("aaa", 0, false)],
            &SortSpec::default(),
            &mut filter,
        );
        listing.set_cursor(0, false);

        listing.extend_entries(
            vec![entry("bbb", 0, false), entry("ccc", 0, false)],
            &SortSpec::default(),
            &mut filter,
        );

        assert_eq!(listing.entries.len(), 3);
        assert_eq!(listing.cursor_entry().unwrap().name, "aaa");
    }

    #[test]
    fn blank_click_leaves_nothing_selected_until_the_next_move() {
        let mut listing = listing();
        listing.set_cursor(1, false);
        listing.deselect_all();
        assert!(listing.selected.is_empty());
        assert!(listing.visible_cursor().is_none());
        assert!(listing.cursor_entry().is_none());
        assert!(listing.selection_or_cursor().is_empty());

        listing.move_cursor(1, false);
        assert_eq!(listing.visible_cursor(), Some(2));
        assert_eq!(listing.selection_or_cursor().len(), 1);
    }

    #[test]
    fn cursor_never_wraps() {
        let mut listing = listing();
        listing.set_cursor(0, false);
        for _ in 0..10 {
            listing.move_cursor(-1, false);
        }
        assert_eq!(listing.cursor, 0);

        for _ in 0..10 {
            listing.move_cursor(1, false);
        }
        assert_eq!(listing.cursor, listing.len() - 1);
    }

    #[test]
    fn empty_listing_tolerates_every_operation() {
        let mut listing = Listing::new();
        listing.move_cursor(1, false);
        listing.move_cursor(-1, true);
        listing.extend_selection(1);
        listing.jump(JumpTarget::Bottom);
        listing.select_all();
        listing.clear_selection();

        assert_eq!(listing.cursor, 0);
        assert!(listing.cursor_entry().is_none());
        assert!(listing.selection_or_cursor().is_empty());
        assert_eq!(listing.status(), "0 items · 0 folders");
    }

    #[test]
    fn single_entry_cannot_move() {
        let mut listing = Listing::new();
        let mut filter = Filter::new();
        listing.set_entries(
            vec![entry("only", 0, false)],
            &SortSpec::default(),
            &mut filter,
        );
        listing.move_cursor(5, false);
        assert_eq!(listing.cursor, 0);
        listing.move_cursor(-5, false);
        assert_eq!(listing.cursor, 0);
    }

    #[test]
    fn extend_selection_selects_the_anchor_row() {
        let mut listing = listing();
        listing.set_cursor(1, false);
        listing.selected.clear();

        listing.extend_selection(1);
        assert_eq!(listing.cursor, 2);
        assert_eq!(listing.selected, vec![1, 2]);

        // Moving back inside the selection must not deselect anything.
        listing.extend_selection(-1);
        assert_eq!(listing.cursor, 1);
        assert_eq!(listing.selected, vec![1, 2]);
    }

    #[test]
    fn selection_or_cursor_falls_back_to_the_cursor_row() {
        let mut listing = listing();
        listing.set_cursor(0, false);
        listing.selected.clear();
        let picked = listing.selection_or_cursor();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "alpha");

        listing.select_all();
        assert_eq!(listing.selection_or_cursor().len(), 4);
    }

    #[test]
    fn status_reports_the_selected_byte_size() {
        let mut listing = listing();
        listing.set_cursor(1, false);
        listing.selected = vec![1, 2];
        let status = listing.status();
        assert!(status.contains("2 selected"), "{status}");
        // main.rs (2048) + readme.md (6)
        assert!(status.contains("2.1 KB"), "{status}");
    }

    #[test]
    fn path_mode_filter_matches_parent_directories() {
        let mut listing = Listing::new();
        let mut filter = Filter::new();
        filter.set_mode(FilterMode::Path);
        let mut deep = entry("lib.rs", 0, false);
        deep.uri = VfsUri::local("/tmp/src/lib.rs");
        listing.set_entries(
            vec![deep, entry("main.rs", 0, false)],
            &SortSpec::default(),
            &mut filter,
        );

        filter.set_query("src");
        listing.resync(&SortSpec::default(), &mut filter);
        assert_eq!(listing.len(), 1);
        assert_eq!(listing.cursor_entry().unwrap().name, "lib.rs");
    }

    #[test]
    fn focus_uri_moves_the_cursor_or_does_nothing() {
        let mut listing = listing();
        let target = VfsUri::local("/tmp/readme.md");
        listing.focus_uri(&target);
        assert_eq!(listing.cursor_entry().unwrap().name, "readme.md");

        let before = listing.cursor;
        listing.focus_uri(&VfsUri::local("/tmp/not-here"));
        assert_eq!(listing.cursor, before, "unknown uri is a no-op");
    }

    #[test]
    fn clear_resets_everything() {
        let mut listing = listing();
        listing.set_cursor(2, false);
        listing.clear();
        assert!(listing.entries.is_empty());
        assert!(listing.visible.is_empty());
        assert!(listing.selected.is_empty());
        assert_eq!(listing.cursor, 0);
    }

    // ---- cursor / tab arithmetic -----------------------------------------

    #[test]
    fn step_saturates() {
        assert_eq!(step(0, -1, 5), 0);
        assert_eq!(step(4, 1, 5), 4);
        assert_eq!(step(2, 2, 5), 4);
        assert_eq!(step(0, 0, 0), 0);
    }

    #[test]
    fn clamp_cursor_handles_empty() {
        assert_eq!(clamp_cursor(7, 0), 0);
        assert_eq!(clamp_cursor(7, 3), 2);
        assert_eq!(clamp_cursor(1, 3), 1);
    }

    #[test]
    fn cycle_index_wraps_both_ways() {
        assert_eq!(cycle_index(0, 3, true), 1);
        assert_eq!(cycle_index(2, 3, true), 0);
        assert_eq!(cycle_index(0, 3, false), 2);
        assert_eq!(cycle_index(5, 0, true), 0);
    }

    #[test]
    fn closing_a_tab_keeps_the_same_view() {
        // Closing a tab to the left shifts the active index down.
        assert_eq!(index_after_close(2, 0, 3), 1);
        // Closing the active tab lands on the next one, clamped at the end.
        assert_eq!(index_after_close(2, 2, 3), 1);
        assert_eq!(index_after_close(0, 0, 3), 0);
        // The last tab cannot be closed.
        assert_eq!(index_after_close(0, 0, 1), 0);
    }

    // ---- path input -------------------------------------------------------

    #[test]
    fn expand_home_resolves_tilde() {
        let home = std::env::var("HOME").expect("HOME is set on macOS/Linux");
        assert_eq!(expand_home("~/code"), PathBuf::from(&home).join("code"));
        assert_eq!(expand_home("~"), PathBuf::from(&home));
        assert_eq!(
            expand_home("/absolute/path"),
            PathBuf::from("/absolute/path")
        );
    }
}
