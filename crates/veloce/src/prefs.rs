//! Typed view of what the store keeps as strings: layout, per-folder view and
//! sort, and the saved session. Pure conversions, so every parsing rule and every
//! fallback for a damaged value is covered by tests.

use veloce_store::{FolderPrefs, TabState};
use vfs_core::{SortKey, SortSpec};

use crate::view::{
    INSPECTOR_DEFAULT, INSPECTOR_MAX, INSPECTOR_MIN, SIDEBAR_DEFAULT, SIDEBAR_MAX, SIDEBAR_MIN,
    ViewMode,
};

const KEY_SIDEBAR: &str = "layout.sidebar_width";
const KEY_INSPECTOR: &str = "layout.inspector_width";
const KEY_RATIO: &str = "layout.split_ratio";
const KEY_SPLIT: &str = "layout.split";
const KEY_PREVIEW: &str = "layout.preview_open";
const KEY_TASKS: &str = "layout.tasks_open";

/// Window layout and the toggles that should come back after a restart.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutPrefs {
    pub sidebar_width: f32,
    pub inspector_width: f32,
    pub split_ratio: f32,
    pub split: bool,
    pub preview_open: bool,
    pub tasks_open: bool,
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        Self {
            sidebar_width: SIDEBAR_DEFAULT,
            inspector_width: INSPECTOR_DEFAULT,
            split_ratio: 0.5,
            split: false,
            preview_open: false,
            tasks_open: false,
        }
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// A finite number inside `[min, max]`, or `None` if the text is not one.
fn parse_in_range(value: &str, min: f32, max: f32) -> Option<f32> {
    let number: f32 = value.trim().parse().ok()?;
    number.is_finite().then(|| number.clamp(min, max))
}

impl LayoutPrefs {
    /// Build from a key lookup. Anything missing, unparsable or out of range falls
    /// back to the default (or is clamped): a bad row must never break start-up.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let number = |key, min, max, fallback| {
            get(key)
                .and_then(|v| parse_in_range(&v, min, max))
                .unwrap_or(fallback)
        };
        let flag = |key, fallback| get(key).and_then(|v| parse_bool(&v)).unwrap_or(fallback);
        Self {
            sidebar_width: number(
                KEY_SIDEBAR,
                SIDEBAR_MIN,
                SIDEBAR_MAX,
                defaults.sidebar_width,
            ),
            inspector_width: number(
                KEY_INSPECTOR,
                INSPECTOR_MIN,
                INSPECTOR_MAX,
                defaults.inspector_width,
            ),
            split_ratio: number(KEY_RATIO, 0.1, 0.9, defaults.split_ratio),
            split: flag(KEY_SPLIT, defaults.split),
            preview_open: flag(KEY_PREVIEW, defaults.preview_open),
            tasks_open: flag(KEY_TASKS, defaults.tasks_open),
        }
    }

    /// Key/value pairs to write, all in one transaction.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let flag = |value: bool| if value { "1" } else { "0" }.to_string();
        vec![
            (KEY_SIDEBAR, self.sidebar_width.to_string()),
            (KEY_INSPECTOR, self.inspector_width.to_string()),
            (KEY_RATIO, self.split_ratio.to_string()),
            (KEY_SPLIT, flag(self.split)),
            (KEY_PREVIEW, flag(self.preview_open)),
            (KEY_TASKS, flag(self.tasks_open)),
        ]
    }
}

pub fn view_key(view: ViewMode) -> &'static str {
    match view {
        ViewMode::List => "list",
        ViewMode::Icons => "icons",
        ViewMode::Columns => "columns",
    }
}

pub fn parse_view(key: &str) -> Option<ViewMode> {
    match key {
        "list" => Some(ViewMode::List),
        "icons" => Some(ViewMode::Icons),
        "columns" => Some(ViewMode::Columns),
        _ => None,
    }
}

pub fn sort_key_name(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "name",
        SortKey::Kind => "kind",
        SortKey::Modified => "modified",
        SortKey::Created => "created",
        SortKey::Size => "size",
    }
}

pub fn parse_sort_key(name: &str) -> Option<SortKey> {
    SortKey::ALL
        .into_iter()
        .find(|key| sort_key_name(*key) == name)
}

/// What a folder shows when nothing was saved for it, as in Finder.
pub fn default_folder_view() -> (ViewMode, SortSpec) {
    (ViewMode::List, SortSpec::default())
}

/// The record to save for a folder.
pub fn folder_prefs_for(view: ViewMode, sort: SortSpec) -> FolderPrefs {
    FolderPrefs {
        view: Some(view_key(view).to_string()),
        sort_key: Some(sort_key_name(sort.key).to_string()),
        sort_desc: sort.descending,
        dirs_first: sort.dirs_first,
    }
}

/// View and sort from a saved record; unknown values fall back to the defaults.
pub fn view_and_sort(prefs: &FolderPrefs) -> (ViewMode, SortSpec) {
    let (default_view, default_sort) = default_folder_view();
    let view = prefs
        .view
        .as_deref()
        .and_then(parse_view)
        .unwrap_or(default_view);
    let key = prefs
        .sort_key
        .as_deref()
        .and_then(parse_sort_key)
        .unwrap_or(default_sort.key);
    (
        view,
        SortSpec {
            key,
            descending: prefs.sort_desc,
            dirs_first: prefs.dirs_first,
        },
    )
}

/// Saved tabs for one pane.
pub fn tabs_to_state(paths: &[String], active: usize) -> Vec<TabState> {
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| TabState {
            path: path.clone(),
            active: index == active,
        })
        .collect()
}

/// Paths and active index to restore, dropping tabs whose folder is gone. The
/// active tab falls back to the first when its own folder disappeared.
pub fn tabs_from_state(
    saved: Vec<TabState>,
    exists: impl Fn(&str) -> bool,
) -> (Vec<String>, usize) {
    let mut paths = Vec::new();
    let mut active = 0;
    for tab in saved.into_iter().filter(|tab| exists(&tab.path)) {
        if tab.active {
            active = paths.len();
        }
        paths.push(tab.path);
    }
    (paths, active)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn missing_settings_give_the_defaults() {
        assert_eq!(
            LayoutPrefs::from_lookup(lookup(&[])),
            LayoutPrefs::default()
        );
    }

    #[test]
    fn saved_layout_round_trips_through_entries() {
        let prefs = LayoutPrefs {
            sidebar_width: 210.5,
            inspector_width: 400.0,
            split_ratio: 0.4,
            split: true,
            preview_open: true,
            tasks_open: true,
        };
        let entries = prefs.entries();
        let restored = LayoutPrefs::from_lookup(|key| {
            entries
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        });
        assert_eq!(restored, prefs);
    }

    #[test]
    fn garbage_values_fall_back_instead_of_failing() {
        let prefs = LayoutPrefs::from_lookup(lookup(&[
            (KEY_SIDEBAR, "wide"),
            (KEY_INSPECTOR, "NaN"),
            (KEY_RATIO, ""),
            (KEY_SPLIT, "maybe"),
        ]));
        assert_eq!(prefs, LayoutPrefs::default());
    }

    #[test]
    fn out_of_range_sizes_are_clamped() {
        let prefs = LayoutPrefs::from_lookup(lookup(&[
            (KEY_SIDEBAR, "5"),
            (KEY_INSPECTOR, "99999"),
            (KEY_RATIO, "7"),
        ]));
        assert_eq!(prefs.sidebar_width, SIDEBAR_MIN);
        assert_eq!(prefs.inspector_width, INSPECTOR_MAX);
        assert_eq!(prefs.split_ratio, 0.9);
    }

    #[test]
    fn infinity_is_not_a_width() {
        let prefs = LayoutPrefs::from_lookup(lookup(&[(KEY_SIDEBAR, "inf")]));
        assert_eq!(prefs.sidebar_width, SIDEBAR_DEFAULT);
    }

    #[test]
    fn booleans_accept_the_forms_we_write_and_the_obvious_ones() {
        assert_eq!(parse_bool("1"), Some(true));
        assert_eq!(parse_bool("false"), Some(false));
        assert_eq!(parse_bool("yes"), None);
    }

    #[test]
    fn view_names_round_trip() {
        for view in [ViewMode::List, ViewMode::Icons, ViewMode::Columns] {
            assert_eq!(parse_view(view_key(view)), Some(view));
        }
        assert_eq!(parse_view("gallery"), None);
    }

    #[test]
    fn every_sort_key_round_trips_by_name() {
        for key in SortKey::ALL {
            assert_eq!(parse_sort_key(sort_key_name(key)), Some(key));
        }
        assert_eq!(parse_sort_key("colour"), None);
    }

    #[test]
    fn folder_prefs_round_trip_view_and_sort() {
        let sort = SortSpec {
            key: SortKey::Size,
            descending: true,
            dirs_first: true,
        };
        let saved = folder_prefs_for(ViewMode::Icons, sort);
        assert_eq!(view_and_sort(&saved), (ViewMode::Icons, sort));
    }

    #[test]
    fn unknown_saved_values_use_the_folder_defaults() {
        let saved = FolderPrefs {
            view: Some("gallery".into()),
            sort_key: Some("colour".into()),
            sort_desc: true,
            dirs_first: false,
        };
        let (view, sort) = view_and_sort(&saved);
        assert_eq!(view, default_folder_view().0);
        assert_eq!(sort.key, default_folder_view().1.key);
        assert!(sort.descending, "the flags are still honoured");
    }

    #[test]
    fn tabs_mark_only_the_active_one() {
        let state = tabs_to_state(&["/a".into(), "/b".into()], 1);
        assert!(!state[0].active && state[1].active);
    }

    #[test]
    fn restoring_tabs_drops_missing_folders_and_keeps_the_active_one() {
        let saved = vec![
            TabState {
                path: "/gone".into(),
                active: false,
            },
            TabState {
                path: "/a".into(),
                active: false,
            },
            TabState {
                path: "/b".into(),
                active: true,
            },
        ];
        let (paths, active) = tabs_from_state(saved, |p| p != "/gone");
        assert_eq!(paths, ["/a", "/b"]);
        assert_eq!(active, 1);
    }

    #[test]
    fn a_vanished_active_tab_falls_back_to_the_first() {
        let saved = vec![
            TabState {
                path: "/a".into(),
                active: false,
            },
            TabState {
                path: "/gone".into(),
                active: true,
            },
        ];
        let (paths, active) = tabs_from_state(saved, |p| p != "/gone");
        assert_eq!(paths, ["/a"]);
        assert_eq!(active, 0);
    }

    #[test]
    fn nothing_saved_restores_nothing() {
        let (paths, active) = tabs_from_state(Vec::new(), |_| true);
        assert!(paths.is_empty());
        assert_eq!(active, 0);
    }
}
