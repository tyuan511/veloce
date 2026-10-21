//! Workspace-wide recursive fuzzy search (`Cmd+P`).
//!
//! Traversal uses `ignore`'s parallel walker so `.gitignore` is honoured and every
//! core is used; scoring uses `nucleo` so results come in quality order rather than
//! filesystem order. Hits are flushed in batches so the UI can render
//! progressively instead of waiting for the whole tree.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use ignore::{DirEntry, ParallelVisitor, ParallelVisitorBuilder, WalkBuilder, WalkState};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::error::{Error, Result};

/// Hits flushed to the caller in one callback.
const BATCH: usize = 64;

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub name: String,
    /// `nucleo` score; higher is better.
    pub score: u32,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Hard cap on retained hits (defaults to 200: more than a palette can show).
    pub max_results: usize,
    /// Include dotfiles. Off by default, matching `Cmd+P` in most editors.
    pub hidden: bool,
    /// Honour `.gitignore` / `.ignore`. On by default.
    pub git_ignore: bool,
    /// Descend into symlinked directories. Off: avoids cycles.
    pub follow_links: bool,
    /// Maximum traversal depth (relative to root).
    pub max_depth: Option<usize>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_results: 200,
            hidden: false,
            git_ignore: true,
            follow_links: false,
            max_depth: None,
        }
    }
}

/// Shared state for one search run, cloned into each walker thread.
struct SearchContext<'a> {
    root: PathBuf,
    pattern: Pattern,
    max_results: usize,
    cancel: Arc<AtomicBool>,
    collected: Arc<Mutex<usize>>,
    on_batch: &'a (dyn Fn(Vec<SearchHit>) + Send + Sync),
}

impl SearchContext<'_> {
    fn push(&self, hits: Vec<SearchHit>) {
        if hits.is_empty() {
            return;
        }
        let mut total = self.collected.lock().unwrap();
        if *total >= self.max_results {
            return;
        }
        let room = self.max_results - *total;
        let hits: Vec<SearchHit> = hits.into_iter().take(room).collect();
        *total += hits.len();
        (self.on_batch)(hits);
    }
}

struct HitVisitor<'a> {
    context: &'a SearchContext<'a>,
    matcher: Matcher,
    buffer: Vec<char>,
    pending: Vec<SearchHit>,
}

impl<'a> HitVisitor<'a> {
    fn new(context: &'a SearchContext<'a>) -> Self {
        Self {
            context,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            buffer: Vec::with_capacity(256),
            pending: Vec::with_capacity(BATCH),
        }
    }
}

impl ParallelVisitor for HitVisitor<'_> {
    fn visit(&mut self, result: std::result::Result<DirEntry, ignore::Error>) -> WalkState {
        if self.context.cancel.load(Ordering::Relaxed) {
            return WalkState::Quit;
        }
        let Ok(entry) = result else {
            return WalkState::Continue;
        };
        // The root itself is never a hit.
        if entry.path() == self.context.root.as_path() {
            return WalkState::Continue;
        }
        let Some(name) = entry.file_name().to_str() else {
            return WalkState::Continue;
        };
        let relative = entry
            .path()
            .strip_prefix(&self.context.root)
            .unwrap_or(entry.path())
            .to_string_lossy();
        let utf32 = Utf32Str::new(&relative, &mut self.buffer);
        let Some(score) = self.context.pattern.score(utf32, &mut self.matcher) else {
            return WalkState::Continue;
        };

        self.pending.push(SearchHit {
            path: entry.path().to_path_buf(),
            name: name.to_string(),
            score,
            is_dir: entry.file_type().is_some_and(|t| t.is_dir()),
        });

        if self.pending.len() >= BATCH {
            self.context.push(std::mem::take(&mut self.pending));
            if self
                .context
                .collected
                .lock()
                .unwrap()
                .ge(&self.context.max_results)
            {
                return WalkState::Quit;
            }
        }
        WalkState::Continue
    }
}

impl Drop for HitVisitor<'_> {
    fn drop(&mut self) {
        // Flush the tail this worker still holds.
        self.context.push(std::mem::take(&mut self.pending));
    }
}

struct VisitorBuilder<'a> {
    context: &'a SearchContext<'a>,
}

impl<'s> ParallelVisitorBuilder<'s> for VisitorBuilder<'s> {
    fn build(&mut self) -> Box<dyn ParallelVisitor + 's> {
        Box::new(HitVisitor::new(self.context))
    }
}

/// Recursively search `root` for entries matching `query`.
///
/// `cancel` lets the caller abandon a search a newer keystroke superseded;
/// `on_batch` is invoked with up to [`BATCH`] hits at a time. Returns the total
/// number of hits retained.
pub fn search_recursive<F>(
    root: &Path,
    query: &str,
    options: &SearchOptions,
    cancel: Arc<AtomicBool>,
    on_batch: F,
) -> Result<usize>
where
    F: Fn(Vec<SearchHit>) + Send + Sync,
{
    if !root.exists() {
        return Err(Error::NotFound {
            uri: crate::uri::VfsUri::local(root),
        });
    }

    let context = SearchContext {
        root: root.to_path_buf(),
        pattern: Pattern::parse(query.trim(), CaseMatching::Smart, Normalization::Smart),
        max_results: options.max_results.max(1),
        cancel,
        collected: Arc::new(Mutex::new(0)),
        on_batch: &on_batch,
    };

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!options.hidden)
        .git_ignore(options.git_ignore)
        .git_global(options.git_ignore)
        .git_exclude(options.git_ignore)
        .ignore(options.git_ignore)
        .follow_links(options.follow_links)
        .require_git(false)
        .max_depth(options.max_depth);

    builder
        .build_parallel()
        .visit(&mut VisitorBuilder { context: &context });

    let total = *context.collected.lock().unwrap();
    Ok(total)
}

/// Sort hits best-score-first, dropping directories below files on ties.
pub fn rank(hits: &mut [SearchHit]) {
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.is_dir.cmp(&b.is_dir))
            .then_with(|| a.path.cmp(&b.path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn collect(root: &Path, query: &str, options: &SearchOptions) -> Vec<SearchHit> {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let sink = hits.clone();
        search_recursive(
            root,
            query,
            options,
            Arc::new(AtomicBool::new(false)),
            move |batch| sink.lock().unwrap().extend(batch),
        )
        .unwrap();
        hits.lock().unwrap().clone()
    }

    #[test]
    fn finds_nested_files_by_fuzzy_name() {
        let dir = tempdir();
        fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        fs::write(dir.path().join("src/deep/palette.rs"), b"x").unwrap();
        fs::write(dir.path().join("README.md"), b"x").unwrap();

        let hits = collect(dir.path(), "plt", &SearchOptions::default());
        assert!(hits.iter().any(|h| h.name == "palette.rs"));
        assert!(!hits.iter().any(|h| h.name == "README.md"));
    }

    #[test]
    fn honours_gitignore() {
        let dir = tempdir();
        fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        fs::create_dir_all(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("target/build.rs"), b"x").unwrap();
        fs::write(dir.path().join("build.rs"), b"x").unwrap();

        let hits = collect(dir.path(), "build", &SearchOptions::default());
        assert!(hits.iter().any(|h| h.name == "build.rs"));
        assert!(
            !hits
                .iter()
                .any(|h| h.path.to_string_lossy().contains("target"))
        );
    }

    #[test]
    fn respects_max_results() {
        let dir = tempdir();
        for i in 0..30 {
            fs::write(dir.path().join(format!("file{i}.txt")), b"x").unwrap();
        }
        let options = SearchOptions {
            max_results: 5,
            ..Default::default()
        };
        let hits = collect(dir.path(), "file", &options);
        assert_eq!(hits.len(), 5);
    }

    #[test]
    fn cancelling_stops_early() {
        let dir = tempdir();
        for i in 0..20 {
            fs::write(dir.path().join(format!("file{i}.txt")), b"x").unwrap();
        }
        let total = search_recursive(
            dir.path(),
            "file",
            &SearchOptions::default(),
            Arc::new(AtomicBool::new(true)),
            |_| {},
        )
        .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn missing_root_is_an_error() {
        let result = search_recursive(
            Path::new("/definitely/not/here"),
            "x",
            &SearchOptions::default(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        );
        assert!(result.is_err());
    }

    #[test]
    fn rank_prefers_higher_scores() {
        let mut hits = vec![
            SearchHit {
                path: PathBuf::from("/a/lib.rs"),
                name: "lib.rs".into(),
                score: 10,
                is_dir: false,
            },
            SearchHit {
                path: PathBuf::from("/a/lib"),
                name: "lib".into(),
                score: 90,
                is_dir: true,
            },
        ];
        rank(&mut hits);
        assert_eq!(hits[0].name, "lib");
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use std::fs;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn collect(root: &Path, query: &str, options: &SearchOptions) -> Vec<SearchHit> {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let sink = hits.clone();
        search_recursive(
            root,
            query,
            options,
            Arc::new(AtomicBool::new(false)),
            move |batch| sink.lock().unwrap().extend(batch),
        )
        .unwrap();
        hits.lock().unwrap().clone()
    }

    #[test]
    fn hidden_files_are_skipped_by_default() {
        let dir = tempdir();
        fs::write(dir.path().join(".secret"), b"x").unwrap();
        fs::write(dir.path().join("visible.txt"), b"x").unwrap();

        let hits = collect(dir.path(), "secret", &SearchOptions::default());
        assert!(hits.is_empty(), "dotfiles need opts.hidden");

        let options = SearchOptions {
            hidden: true,
            ..Default::default()
        };
        let hits = collect(dir.path(), "secret", &options);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn max_depth_limits_the_walk() {
        let dir = tempdir();
        fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        fs::write(dir.path().join("a/b/c/deep.txt"), b"x").unwrap();

        let options = SearchOptions {
            max_depth: Some(1),
            ..Default::default()
        };
        let hits = collect(dir.path(), "deep", &options);
        assert!(hits.is_empty(), "deeper than max_depth");

        let options = SearchOptions {
            max_depth: Some(4),
            ..Default::default()
        };
        assert_eq!(collect(dir.path(), "deep", &options).len(), 1);
    }

    #[test]
    fn directories_are_reported_as_directories() {
        let dir = tempdir();
        fs::create_dir(dir.path().join("myfolder")).unwrap();
        let hits = collect(dir.path(), "myfolder", &SearchOptions::default());
        assert_eq!(hits.len(), 1);
        assert!(hits[0].is_dir);
    }

    #[test]
    fn empty_query_matches_every_entry() {
        let dir = tempdir();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }
        let hits = collect(dir.path(), "", &SearchOptions::default());
        assert_eq!(hits.len(), 3, "an empty pattern scores everything");
    }

    #[test]
    fn gitignore_can_be_disabled() {
        let dir = tempdir();
        fs::write(dir.path().join(".gitignore"), "build/\n").unwrap();
        fs::create_dir(dir.path().join("build")).unwrap();
        fs::write(dir.path().join("build/artifact.bin"), b"x").unwrap();

        let ignored = collect(dir.path(), "artifact", &SearchOptions::default());
        assert!(ignored.is_empty());

        let options = SearchOptions {
            git_ignore: false,
            ..Default::default()
        };
        assert_eq!(collect(dir.path(), "artifact", &options).len(), 1);
    }

    #[test]
    fn rank_orders_files_above_dirs_on_equal_scores() {
        let mut hits = vec![
            SearchHit {
                path: PathBuf::from("/a/lib"),
                name: "lib".into(),
                score: 50,
                is_dir: true,
            },
            SearchHit {
                path: PathBuf::from("/a/lib.rs"),
                name: "lib.rs".into(),
                score: 50,
                is_dir: false,
            },
        ];
        rank(&mut hits);
        assert!(!hits[0].is_dir, "files first on a tie");
    }

    #[test]
    fn rank_is_deterministic_for_equal_scores() {
        let make = |path: &str| SearchHit {
            path: PathBuf::from(path),
            name: "x".into(),
            score: 1,
            is_dir: false,
        };
        let mut a = vec![make("/b/x"), make("/a/x")];
        let mut b = vec![make("/b/x"), make("/a/x")];
        rank(&mut a);
        rank(&mut b);
        assert_eq!(a, b);
        assert_eq!(a[0].path, PathBuf::from("/a/x"));
    }

    #[test]
    fn default_options_are_sane() {
        let options = SearchOptions::default();
        assert_eq!(options.max_results, 200);
        assert!(!options.hidden);
        assert!(options.git_ignore);
        assert!(
            !options.follow_links,
            "symlink loops must be off by default"
        );
        assert!(options.max_depth.is_none());
    }

    #[test]
    fn max_results_zero_is_clamped_to_one() {
        let dir = tempdir();
        fs::write(dir.path().join("a.txt"), b"x").unwrap();
        fs::write(dir.path().join("b.txt"), b"x").unwrap();
        let options = SearchOptions {
            max_results: 0,
            ..Default::default()
        };
        let hits = collect(dir.path(), "txt", &options);
        assert_eq!(hits.len(), 1, "a zero cap must not return nothing forever");
    }
}
