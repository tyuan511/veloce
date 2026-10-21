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

use crate::pane::JumpTarget;

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
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    pub fn len(&self) -> usize {
        self.visible.len()
    }

    pub fn cursor_entry(&self) -> Option<&FileEntry> {
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
        self.selected = self.visible.clone();
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    /// The status line: item counts, folder counts and the selection's byte size.
    pub fn status(&self) -> String {
        let total = self.entries.len();
        let visible = self.visible.len();
        let dirs = self.entries.iter().filter(|entry| entry.is_dir()).count();
        let mut status = if visible == total {
            format!("{total} items · {dirs} folders")
        } else {
            format!("{visible}/{total} items · {dirs} folders")
        };
        if !self.selected.is_empty() {
            let bytes: u64 = self
                .selected
                .iter()
                .filter_map(|index| self.entries.get(*index))
                .filter(|entry| !entry.is_dir())
                .map(|entry| entry.size)
                .sum();
            status.push_str(&format!(
                " · {} selected ({})",
                self.selected.len(),
                vfs_core::entry::format_size(bytes)
            ));
        }
        status
    }

    /// Move the cursor onto the entry with this uri, if it is visible.
    pub fn focus_uri(&mut self, uri: &VfsUri) {
        let Some(position) = self
            .visible
            .iter()
            .position(|index| &self.entries[*index].uri == uri)
        else {
            return;
        };
        self.set_cursor(position, false);
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

/// Turn whatever the user typed into the path bar into a uri.
///
/// Accepts `file://` / `s3://...` style uris, absolute paths, `~` paths and
/// relative names (resolved against the current directory). Returns `None` for
/// blank input so the caller can just ignore an aborted edit.
pub fn parse_path_input(value: &str, current: &VfsUri) -> Option<VfsUri> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(uri) = VfsUri::parse(value) {
        return Some(uri);
    }
    let path = PathBuf::from(value);
    let resolved = if path.is_absolute() || value.starts_with('~') {
        expand_home(value)
    } else {
        current
            .as_local_path()
            .unwrap_or_else(|| PathBuf::from("/"))
            .join(&path)
    };
    Some(VfsUri::local(resolved))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs_core::{FileKind, SortKey, filter::FilterMode};

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
    fn parse_path_input_accepts_uris_paths_and_relative_names() {
        let current = VfsUri::local("/tmp");

        let uri = parse_path_input("s3://bucket/a", &current).unwrap();
        assert_eq!(uri.to_string(), "s3://bucket/a");

        let absolute = parse_path_input("/usr/local", &current).unwrap();
        assert_eq!(absolute.path, "/usr/local");

        let relative = parse_path_input("nested/dir", &current).unwrap();
        assert_eq!(relative.path, "/tmp/nested/dir");

        assert!(parse_path_input("   ", &current).is_none());
    }

    #[test]
    fn parse_path_input_resolves_relative_against_the_current_dir() {
        let current = VfsUri::local("/Users/me/projects");
        let uri = parse_path_input("../other", &current).unwrap();
        assert_eq!(uri.path, "/Users/me/other", "dot segments are resolved");
    }

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
