//! Zero-latency filtering of an in-memory directory listing.
//!
//! Two modes, matching the product spec:
//!
//! - **Filter mode** (`/`): substring / fuzzy match over the entries already loaded
//!   for the current directory. Runs on every keystroke, must stay under a frame.
//! - **Path mode**: `nucleo` scores the full path so `src/lib` finds `src/a/b/lib.rs`.

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::entry::FileEntry;

/// Which text a query is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterMode {
    /// Match the entry name only (Finder's "filter current folder").
    #[default]
    Name,
    /// Match the full path, favouring path-boundary hits.
    Path,
}

/// A reusable fuzzy matcher. Keep one instance alive: `Matcher` allocates scratch
/// buffers that would otherwise be rebuilt for every keystroke.
pub struct Filter {
    mode: FilterMode,
    query: String,
    matcher: Matcher,
    /// Scratch buffer for the UTF-32 conversion `nucleo` needs.
    char_buffer: Vec<char>,
}

impl std::fmt::Debug for Filter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Filter")
            .field("mode", &self.mode)
            .field("query", &self.query)
            .finish_non_exhaustive()
    }
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    pub fn new() -> Self {
        Self {
            mode: FilterMode::Name,
            query: String::new(),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            char_buffer: Vec::with_capacity(256),
        }
    }

    pub fn mode(&self) -> FilterMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: FilterMode) {
        self.mode = mode;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Update the query. Returns `true` when the active query changed, so callers
    /// can skip a re-filter (and a redraw) when nothing happened.
    pub fn set_query(&mut self, query: impl Into<String>) -> bool {
        let query = query.into();
        if query == self.query {
            return false;
        }
        self.query = query;
        true
    }

    pub fn clear(&mut self) -> bool {
        self.set_query(String::new())
    }

    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Indices into `entries` that match, best score first.
    pub fn matching_indices(&mut self, entries: &[FileEntry]) -> Vec<usize> {
        if !self.is_active() {
            return (0..entries.len()).collect();
        }
        let pattern = Pattern::parse(self.query.trim(), CaseMatching::Smart, Normalization::Smart);

        let mut scored: Vec<(usize, u32)> = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let haystack = match self.mode {
                FilterMode::Name => entry.name.as_str(),
                FilterMode::Path => entry.uri.path.as_str(),
            };
            let utf32 = Utf32Str::new(haystack, &mut self.char_buffer);
            if let Some(score) = pattern.score(utf32, &mut self.matcher) {
                scored.push((index, score));
            }
        }
        // Highest score first; ties keep the caller's (sorted) order stable.
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.into_iter().map(|(index, _)| index).collect()
    }

    /// Convenience wrapper returning the matching entries themselves.
    pub fn matching<'a>(&mut self, entries: &'a [FileEntry]) -> Vec<&'a FileEntry> {
        self.matching_indices(entries)
            .into_iter()
            .map(|index| &entries[index])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{entry::FileKind, uri::VfsUri};

    fn entries(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|name| {
                FileEntry::new(VfsUri::local(format!("/tmp/{name}")), *name, FileKind::File)
            })
            .collect()
    }

    #[test]
    fn empty_query_matches_everything() {
        let list = entries(&["a.txt", "b.txt"]);
        let mut filter = Filter::new();
        assert_eq!(filter.matching_indices(&list), vec![0, 1]);
    }

    #[test]
    fn filters_by_fuzzy_name() {
        let list = entries(&["main.rs", "lib.rs", "notes.md"]);
        let mut filter = Filter::new();
        filter.set_query("mrs");
        let matched = filter.matching_indices(&list);
        assert_eq!(matched, vec![0]);
    }

    #[test]
    fn set_query_reports_change() {
        let mut filter = Filter::new();
        assert!(filter.set_query("a"));
        assert!(!filter.set_query("a"));
        assert!(filter.clear());
        assert!(!filter.clear());
    }

    #[test]
    fn path_mode_matches_parent_directories() {
        let mut list = entries(&["lib.rs", "main.rs"]);
        list[0].uri = VfsUri::local("/tmp/src/lib.rs");
        list[1].uri = VfsUri::local("/tmp/other/main.rs");

        let mut filter = Filter::new();
        filter.set_mode(FilterMode::Path);
        filter.set_query("src");
        assert_eq!(filter.matching_indices(&list), vec![0]);
    }

    #[test]
    fn unmatched_query_returns_empty() {
        let list = entries(&["a.txt"]);
        let mut filter = Filter::new();
        filter.set_query("zzzzzz");
        assert!(filter.matching_indices(&list).is_empty());
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use crate::{entry::FileKind, uri::VfsUri};

    fn entries(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|name| {
                FileEntry::new(
                    VfsUri::local(format!("/tmp/{name}")),
                    *name,
                    if name.ends_with('/') {
                        FileKind::Directory
                    } else {
                        FileKind::File
                    },
                )
            })
            .collect()
    }

    #[test]
    fn is_active_ignores_whitespace() {
        let mut filter = Filter::new();
        filter.set_query("   ");
        assert!(!filter.is_active(), "a blank query is not a filter");
        filter.set_query("a");
        assert!(filter.is_active());
    }

    #[test]
    fn mode_switch_takes_effect_on_the_next_match() {
        let list = entries(&["lib.rs"]);
        let mut filter = Filter::new();
        filter.set_query("tmp");
        assert!(
            filter.matching_indices(&list).is_empty(),
            "name has no 'tmp'"
        );

        filter.set_mode(FilterMode::Path);
        assert_eq!(filter.mode(), FilterMode::Path);
        assert_eq!(filter.matching_indices(&list), vec![0], "the path does");
    }

    #[test]
    fn scores_order_results_by_quality() {
        let list = entries(&["main.rs", "m_a_i_n.rs", "unrelated.rs"]);
        let mut filter = Filter::new();
        filter.set_query("main");
        let matched = filter.matching_indices(&list);
        assert_eq!(matched.first(), Some(&0), "the exact prefix wins");
        assert!(!matched.contains(&2));
    }

    #[test]
    fn matching_returns_entries_not_indices() {
        let list = entries(&["alpha.txt", "beta.txt"]);
        let mut filter = Filter::new();
        filter.set_query("beta");
        let matched = filter.matching(&list);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "beta.txt");
    }

    #[test]
    fn filter_is_reusable_across_queries() {
        // Reusing one Filter must not leak state between queries (the scratch
        // buffer and matcher are kept alive on purpose).
        let list = entries(&["a.rs", "b.rs", "c.md"]);
        let mut filter = Filter::new();

        filter.set_query("rs");
        assert_eq!(filter.matching_indices(&list).len(), 2);

        filter.set_query("md");
        assert_eq!(filter.matching_indices(&list), vec![2]);

        filter.clear();
        assert_eq!(filter.matching_indices(&list).len(), 3);
    }

    #[test]
    fn case_matching_is_smart() {
        let list = entries(&["README.md", "readme.txt"]);
        let mut filter = Filter::new();
        // Lowercase query matches both cases.
        filter.set_query("readme");
        assert_eq!(filter.matching_indices(&list).len(), 2);
        // An uppercase query only matches the uppercase name.
        filter.set_query("README");
        assert_eq!(filter.matching_indices(&list), vec![0]);
    }

    #[test]
    fn unicode_names_are_matched() {
        let list = entries(&["报告.md", "笔记.txt"]);
        let mut filter = Filter::new();
        filter.set_query("报告");
        assert_eq!(filter.matching_indices(&list), vec![0]);
    }

    #[test]
    fn empty_list_is_handled() {
        let mut filter = Filter::new();
        assert!(filter.matching_indices(&[]).is_empty());
        filter.set_query("x");
        assert!(filter.matching_indices(&[]).is_empty());
    }

    #[test]
    fn debug_impl_does_not_leak_the_matcher() {
        let mut filter = Filter::new();
        filter.set_query("abc");
        let text = format!("{filter:?}");
        assert!(text.contains("abc"));
        assert!(!text.contains("Matcher"));
    }
}
