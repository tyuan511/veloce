//! Typing a path into the search box: recognising it, resolving it against the
//! current folder, and listing what could complete it.
//!
//! Resolution is lexical (`~`, `.`, `..` are folded away without touching the
//! disk) so it is pure and testable; only [`list_matches`] reads a directory.

use std::{
    io,
    path::{Component, Path, PathBuf},
};

use vfs_core::SearchHit;

/// Rows offered while completing a path.
pub const MAX_MATCHES: usize = 100;

/// Whether the text is meant as a path rather than a fuzzy file search.
pub fn is_path_query(query: &str) -> bool {
    let query = query.trim_start();
    query.starts_with('/')
        || query.starts_with('~')
        || query == "."
        || query == ".."
        || query.starts_with("./")
        || query.starts_with("../")
}

/// Fold `.` and `..` away without consulting the filesystem. `..` at the root
/// stays at the root.
pub fn normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(Component::Normal(_)) => {
                    parts.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => parts.push(component),
            },
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return PathBuf::from("/");
    }
    parts.iter().map(|c| c.as_os_str()).collect()
}

/// A typed path split into the folder to list and the part still being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathQuery {
    /// The folder whose entries complete the path.
    pub dir: PathBuf,
    /// Text after the last `/`; empty when the query ends with one.
    pub partial: String,
}

/// Resolve a typed path. `base` is the folder relative paths start from.
pub fn parse(query: &str, base: &Path, home: &Path) -> Option<PathQuery> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let expanded: PathBuf = match query.strip_prefix('~') {
        // `~user` is not supported; only the current user's home.
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        Some(_) => return None,
        None => PathBuf::from(query),
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    };
    let ends_with_separator = query.ends_with('/') || query == "." || query == ".." || query == "~";
    let resolved = normalize(&absolute);
    // `.` and `..` name folders, never a partial file name.
    let last_is_dot = matches!(
        Path::new(query).components().next_back(),
        Some(Component::CurDir) | Some(Component::ParentDir)
    );
    if ends_with_separator || last_is_dot {
        return Some(PathQuery {
            dir: resolved,
            partial: String::new(),
        });
    }
    let partial = resolved
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = resolved
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    Some(PathQuery { dir, partial })
}

/// Entries of `query.dir` that continue the typed text, best first: exact name,
/// then names starting with it, then names containing it; folders before files.
/// Dot-files appear only once the user types a leading dot.
pub fn list_matches(query: &PathQuery) -> io::Result<Vec<SearchHit>> {
    let needle = query.partial.to_lowercase();
    let show_hidden = query.partial.starts_with('.');
    let mut hits: Vec<(u8, SearchHit)> = Vec::new();
    for entry in std::fs::read_dir(&query.dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let rank = if needle.is_empty() {
            2
        } else if lower == needle {
            0
        } else if lower.starts_with(&needle) {
            1
        // A typed leading dot asks for dot-files, so only a prefix counts then;
        // otherwise `.` would match every file with an extension.
        } else if !show_hidden && lower.contains(&needle) {
            2
        } else {
            continue;
        };
        // `metadata` follows links, so a link to a folder counts as a folder.
        let is_dir = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir());
        hits.push((
            rank,
            SearchHit {
                path: entry.path(),
                name,
                score: 0,
                is_dir,
            },
        ));
    }
    hits.sort_by(|(rank_a, a), (rank_b, b)| {
        rank_a
            .cmp(rank_b)
            .then_with(|| b.is_dir.cmp(&a.is_dir))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    hits.truncate(MAX_MATCHES);
    Ok(hits.into_iter().map(|(_, hit)| hit).collect())
}

/// The text to put in the box when completing to `hit`: its path, with a trailing
/// `/` for folders so the next level can be typed straight away.
pub fn completion(hit: &SearchHit) -> String {
    let mut text = hit.path.to_string_lossy().into_owned();
    if hit.is_dir && !text.ends_with('/') {
        text.push('/');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn parse_in(query: &str) -> Option<PathQuery> {
        parse(query, Path::new("/work/proj"), Path::new("/Users/me"))
    }

    #[test]
    fn path_like_text_is_recognised_and_plain_words_are_not() {
        for yes in ["/usr", "~", "~/code", "./a", "../b", "..", ".", "  /tmp"] {
            assert!(is_path_query(yes), "{yes}");
        }
        for no in ["readme", "src/main.rs", "a.b", "", "foo/bar"] {
            assert!(!is_path_query(no), "{no}");
        }
    }

    #[test]
    fn normalizing_folds_dots_without_touching_the_disk() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/a/b/../../..")), PathBuf::from("/"));
        assert_eq!(normalize(Path::new("/../x")), PathBuf::from("/x"));
        assert_eq!(normalize(Path::new("/")), PathBuf::from("/"));
    }

    #[test]
    fn absolute_paths_split_into_folder_and_partial() {
        let query = parse_in("/usr/loc").unwrap();
        assert_eq!(query.dir, PathBuf::from("/usr"));
        assert_eq!(query.partial, "loc");
    }

    #[test]
    fn a_trailing_slash_means_list_that_folder() {
        let query = parse_in("/usr/local/").unwrap();
        assert_eq!(query.dir, PathBuf::from("/usr/local"));
        assert_eq!(query.partial, "");
    }

    #[test]
    fn tilde_expands_to_home() {
        assert_eq!(parse_in("~").unwrap().dir, PathBuf::from("/Users/me"));
        let code = parse_in("~/co").unwrap();
        assert_eq!(code.dir, PathBuf::from("/Users/me"));
        assert_eq!(code.partial, "co");
        assert!(
            parse_in("~other/x").is_none(),
            "other users are not resolved"
        );
    }

    #[test]
    fn relative_paths_start_from_the_current_folder() {
        assert_eq!(parse_in("./src").unwrap().dir, PathBuf::from("/work/proj"));
        assert_eq!(parse_in("./src").unwrap().partial, "src");
        let up = parse_in("../other/").unwrap();
        assert_eq!(up.dir, PathBuf::from("/work/other"));
        assert_eq!(up.partial, "");
    }

    #[test]
    fn dot_and_dotdot_name_folders_not_partials() {
        assert_eq!(parse_in(".").unwrap().dir, PathBuf::from("/work/proj"));
        assert_eq!(parse_in("..").unwrap().dir, PathBuf::from("/work"));
        assert_eq!(parse_in("..").unwrap().partial, "");
        assert_eq!(parse_in("/a/b/..").unwrap().dir, PathBuf::from("/a"));
    }

    #[test]
    fn the_root_has_no_partial() {
        let root = parse_in("/").unwrap();
        assert_eq!(root.dir, PathBuf::from("/"));
        assert_eq!(root.partial, "");
    }

    #[test]
    fn blank_input_is_not_a_path() {
        assert!(parse_in("   ").is_none());
    }

    fn sample_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "code",
            "codex",
            "Documents",
            "notes.txt",
            "my-code",
            ".hidden",
        ] {
            if name == "notes.txt" || name == ".hidden" {
                fs::write(dir.path().join(name), b"x").unwrap();
            } else {
                fs::create_dir(dir.path().join(name)).unwrap();
            }
        }
        dir
    }

    fn names(hits: &[SearchHit]) -> Vec<&str> {
        hits.iter().map(|h| h.name.as_str()).collect()
    }

    #[test]
    fn listing_a_folder_shows_folders_first_and_hides_dotfiles() {
        let dir = sample_dir();
        let query = PathQuery {
            dir: dir.path().to_path_buf(),
            partial: String::new(),
        };
        let hits = list_matches(&query).unwrap();
        assert_eq!(
            names(&hits),
            ["code", "codex", "Documents", "my-code", "notes.txt"]
        );
        assert!(hits[0].is_dir && !hits[4].is_dir);
    }

    #[test]
    fn matches_are_ranked_exact_then_prefix_then_substring() {
        let dir = sample_dir();
        let query = PathQuery {
            dir: dir.path().to_path_buf(),
            partial: "CODE".into(),
        };
        let hits = list_matches(&query).unwrap();
        assert_eq!(names(&hits), ["code", "codex", "my-code"]);
    }

    #[test]
    fn a_leading_dot_reveals_hidden_entries() {
        let dir = sample_dir();
        let query = PathQuery {
            dir: dir.path().to_path_buf(),
            partial: ".".into(),
        };
        assert_eq!(names(&list_matches(&query).unwrap()), [".hidden"]);
    }

    #[test]
    fn nothing_matching_is_an_empty_list_not_an_error() {
        let dir = sample_dir();
        let query = PathQuery {
            dir: dir.path().to_path_buf(),
            partial: "zzz".into(),
        };
        assert!(list_matches(&query).unwrap().is_empty());
    }

    #[test]
    fn a_missing_folder_is_an_error_the_caller_can_show() {
        let dir = sample_dir();
        let query = PathQuery {
            dir: dir.path().join("nope"),
            partial: String::new(),
        };
        assert!(list_matches(&query).is_err());
    }

    #[test]
    fn completion_adds_a_slash_for_folders_only() {
        let folder = SearchHit {
            path: PathBuf::from("/a/code"),
            name: "code".into(),
            score: 0,
            is_dir: true,
        };
        let file = SearchHit {
            is_dir: false,
            path: PathBuf::from("/a/notes.txt"),
            name: "notes.txt".into(),
            ..folder.clone()
        };
        assert_eq!(completion(&folder), "/a/code/");
        assert_eq!(completion(&file), "/a/notes.txt");
    }

    #[test]
    fn a_symlink_to_a_folder_counts_as_a_folder() {
        let dir = sample_dir();
        std::os::unix::fs::symlink(dir.path().join("code"), dir.path().join("link")).unwrap();
        let query = PathQuery {
            dir: dir.path().to_path_buf(),
            partial: "link".into(),
        };
        assert!(list_matches(&query).unwrap()[0].is_dir);
    }
}
