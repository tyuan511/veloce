//! User-facing settings: what they are, their defaults, and how they are stored.
//!
//! The window that edits them lives in `settings_window`; the rest of the app only
//! reads the current values from the global [`AppSettings`]. Everything here is
//! pure, so every default, every parse and every fallback for a damaged value is
//! covered by tests.

use vfs_core::{SortSpec, ops::ConflictPolicy};

use crate::{
    keymap::{self, Overrides},
    prefs::{parse_sort_key, parse_view, sort_key_name, view_key},
    view::ViewMode,
};

/// How the colours are chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemePref {
    /// Follow the operating system's light/dark setting.
    System,
    Light,
    Dark,
}

impl ThemePref {
    pub const ALL: [ThemePref; 3] = [ThemePref::System, ThemePref::Light, ThemePref::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemePref::System => "System",
            ThemePref::Light => "Light",
            ThemePref::Dark => "Dark",
        }
    }

    fn key(self) -> &'static str {
        match self {
            ThemePref::System => "system",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pref| pref.key() == value)
    }
}

/// How tall list rows are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Compact,
    Regular,
    Comfortable,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Compact, Density::Regular, Density::Comfortable];

    pub fn label(self) -> &'static str {
        match self {
            Density::Compact => "Compact",
            Density::Regular => "Regular",
            Density::Comfortable => "Comfortable",
        }
    }

    /// Row height in logical pixels. Whole numbers only: the list is virtualized
    /// on a fixed row height.
    pub fn row_height(self) -> f32 {
        match self {
            Density::Compact => 24.0,
            Density::Regular => 28.0,
            Density::Comfortable => 34.0,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Density::Compact => "compact",
            Density::Regular => "regular",
            Density::Comfortable => "comfortable",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|density| density.key() == value)
    }
}

/// What to do when a pasted file's name already exists in the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConflict {
    KeepBoth,
    Skip,
    Replace,
}

impl OnConflict {
    pub const ALL: [OnConflict; 3] = [OnConflict::KeepBoth, OnConflict::Skip, OnConflict::Replace];

    pub fn label(self) -> &'static str {
        match self {
            OnConflict::KeepBoth => "Keep both",
            OnConflict::Skip => "Skip",
            OnConflict::Replace => "Replace",
        }
    }

    pub fn policy(self) -> ConflictPolicy {
        match self {
            OnConflict::KeepBoth => ConflictPolicy::Rename,
            OnConflict::Skip => ConflictPolicy::Skip,
            OnConflict::Replace => ConflictPolicy::Overwrite,
        }
    }

    fn key(self) -> &'static str {
        match self {
            OnConflict::KeepBoth => "keep_both",
            OnConflict::Skip => "skip",
            OnConflict::Replace => "replace",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.key() == value)
    }
}

/// What opens when Veloce starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startup {
    /// The tabs of the last session.
    RestoreSession,
    /// The home folder, ignoring the last session.
    HomeFolder,
}

impl Startup {
    pub const ALL: [Startup; 2] = [Startup::RestoreSession, Startup::HomeFolder];

    pub fn label(self) -> &'static str {
        match self {
            Startup::RestoreSession => "Restore last session",
            Startup::HomeFolder => "Open the home folder",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Startup::RestoreSession => "restore",
            Startup::HomeFolder => "home",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|startup| startup.key() == value)
    }
}

/// Cache size limits offered in the UI (megabytes).
pub const CACHE_CHOICES_MB: [u32; 4] = [64, 256, 1024, 4096];

#[derive(Debug, Clone, PartialEq)]
pub struct AppSettings {
    pub theme: ThemePref,
    pub density: Density,
    pub startup: Startup,
    pub default_view: ViewMode,
    pub default_sort: SortSpec,
    pub show_hidden: bool,
    pub thumbnails: bool,
    pub thumbnail_cache_mb: u32,
    pub on_conflict: OnConflict,
    /// Application `Open in Terminal` launches (a name `open -a` understands).
    pub terminal_app: String,
    /// Include dot-files when searching.
    pub search_hidden: bool,
    /// Skip what `.gitignore` excludes when searching.
    pub search_respects_gitignore: bool,
    /// Customised key bindings; anything absent keeps its default.
    pub shortcuts: Overrides,
}

impl gpui::Global for AppSettings {}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePref::System,
            density: Density::Regular,
            startup: Startup::RestoreSession,
            default_view: ViewMode::List,
            default_sort: SortSpec::default(),
            show_hidden: false,
            thumbnails: true,
            thumbnail_cache_mb: 256,
            on_conflict: OnConflict::KeepBoth,
            terminal_app: "Terminal".to_string(),
            search_hidden: false,
            search_respects_gitignore: true,
            shortcuts: Overrides::new(),
        }
    }
}

const K_THEME: &str = "app.theme";
const K_DENSITY: &str = "app.density";
const K_STARTUP: &str = "app.startup";
const K_VIEW: &str = "app.default_view";
const K_SORT_KEY: &str = "app.default_sort_key";
const K_SORT_DESC: &str = "app.default_sort_desc";
const K_FOLDERS_FIRST: &str = "app.folders_first";
const K_HIDDEN: &str = "files.show_hidden";
const K_THUMBS: &str = "app.thumbnails";
const K_CACHE: &str = "app.thumbnail_cache_mb";
const K_CONFLICT: &str = "app.on_conflict";
const K_TERMINAL: &str = "app.terminal_app";
const K_SEARCH_HIDDEN: &str = "app.search_hidden";
const K_SEARCH_GIT: &str = "app.search_gitignore";
const K_KEYS: &str = "app.shortcuts";

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

fn flag(value: bool) -> String {
    if value { "1" } else { "0" }.to_string()
}

/// A terminal application name is passed to `open -a`, so it must be a plain name:
/// no path separators, no leading dash that would read as an option.
pub fn clean_terminal_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.starts_with('-') || name.contains('/') || name.contains('\0') {
        return None;
    }
    Some(name.strip_suffix(".app").unwrap_or(name).to_string())
}

impl AppSettings {
    /// Build from a key lookup. Anything missing or unreadable keeps its default.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let text = |key| get(key);
        let on_off = |key, fallback| text(key).and_then(|v| parse_bool(&v)).unwrap_or(fallback);
        let sort_key = text(K_SORT_KEY)
            .and_then(|v| parse_sort_key(&v))
            .unwrap_or(defaults.default_sort.key);
        let cache = text(K_CACHE)
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|mb| CACHE_CHOICES_MB.contains(mb))
            .unwrap_or(defaults.thumbnail_cache_mb);
        Self {
            theme: text(K_THEME)
                .and_then(|v| ThemePref::parse(&v))
                .unwrap_or(defaults.theme),
            density: text(K_DENSITY)
                .and_then(|v| Density::parse(&v))
                .unwrap_or(defaults.density),
            startup: text(K_STARTUP)
                .and_then(|v| Startup::parse(&v))
                .unwrap_or(defaults.startup),
            default_view: text(K_VIEW)
                .and_then(|v| parse_view(&v))
                .unwrap_or(defaults.default_view),
            default_sort: SortSpec {
                key: sort_key,
                descending: on_off(K_SORT_DESC, defaults.default_sort.descending),
                dirs_first: on_off(K_FOLDERS_FIRST, defaults.default_sort.dirs_first),
            },
            show_hidden: on_off(K_HIDDEN, defaults.show_hidden),
            thumbnails: on_off(K_THUMBS, defaults.thumbnails),
            thumbnail_cache_mb: cache,
            on_conflict: text(K_CONFLICT)
                .and_then(|v| OnConflict::parse(&v))
                .unwrap_or(defaults.on_conflict),
            terminal_app: text(K_TERMINAL)
                .and_then(|v| clean_terminal_name(&v))
                .unwrap_or(defaults.terminal_app),
            search_hidden: on_off(K_SEARCH_HIDDEN, defaults.search_hidden),
            search_respects_gitignore: on_off(K_SEARCH_GIT, defaults.search_respects_gitignore),
            shortcuts: text(K_KEYS).map(|v| keymap::parse(&v)).unwrap_or_default(),
        }
    }

    /// Key/value pairs to write, all in one transaction.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        vec![
            (K_THEME, self.theme.key().to_string()),
            (K_DENSITY, self.density.key().to_string()),
            (K_STARTUP, self.startup.key().to_string()),
            (K_VIEW, view_key(self.default_view).to_string()),
            (K_SORT_KEY, sort_key_name(self.default_sort.key).to_string()),
            (K_SORT_DESC, flag(self.default_sort.descending)),
            (K_FOLDERS_FIRST, flag(self.default_sort.dirs_first)),
            (K_HIDDEN, flag(self.show_hidden)),
            (K_THUMBS, flag(self.thumbnails)),
            (K_CACHE, self.thumbnail_cache_mb.to_string()),
            (K_CONFLICT, self.on_conflict.key().to_string()),
            (K_TERMINAL, self.terminal_app.clone()),
            (K_SEARCH_HIDDEN, flag(self.search_hidden)),
            (K_SEARCH_GIT, flag(self.search_respects_gitignore)),
            (K_KEYS, keymap::serialize(&self.shortcuts)),
        ]
    }

    /// The thumbnail cache limit in bytes.
    pub fn cache_limit_bytes(&self) -> u64 {
        u64::from(self.thumbnail_cache_mb) * 1024 * 1024
    }

    /// Search options for the palette's recursive file search.
    pub fn search_options(&self) -> vfs_core::search::SearchOptions {
        vfs_core::search::SearchOptions {
            hidden: self.search_hidden,
            git_ignore: self.search_respects_gitignore,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use vfs_core::SortKey;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn missing_settings_give_the_documented_defaults() {
        let settings = AppSettings::from_lookup(lookup(&[]));
        assert_eq!(settings, AppSettings::default());
        assert_eq!(settings.theme, ThemePref::System);
        assert_eq!(settings.default_view, ViewMode::List);
        assert!(!settings.show_hidden);
        assert!(settings.thumbnails);
        assert_eq!(settings.thumbnail_cache_mb, 256);
        assert_eq!(settings.on_conflict, OnConflict::KeepBoth);
        assert_eq!(settings.terminal_app, "Terminal");
    }

    #[test]
    fn every_setting_survives_a_save_and_load() {
        let settings = AppSettings {
            theme: ThemePref::Dark,
            density: Density::Comfortable,
            startup: Startup::HomeFolder,
            default_view: ViewMode::Icons,
            default_sort: SortSpec {
                key: SortKey::Modified,
                descending: true,
                dirs_first: true,
            },
            show_hidden: true,
            thumbnails: false,
            thumbnail_cache_mb: 1024,
            on_conflict: OnConflict::Replace,
            terminal_app: "iTerm".into(),
            search_hidden: true,
            search_respects_gitignore: false,
            shortcuts: Overrides::new(),
        };
        let entries = settings.entries();
        let restored = AppSettings::from_lookup(|key| {
            entries
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        });
        assert_eq!(restored, settings);
    }

    #[test]
    fn unreadable_values_fall_back_one_at_a_time() {
        let settings = AppSettings::from_lookup(lookup(&[
            (K_THEME, "neon"),
            (K_DENSITY, "comfortable"),
            (K_VIEW, "gallery"),
            (K_CACHE, "300"),
            (K_THUMBS, "maybe"),
        ]));
        assert_eq!(settings.theme, ThemePref::System, "bad value → default");
        assert_eq!(settings.density, Density::Comfortable, "good value kept");
        assert_eq!(settings.default_view, ViewMode::List);
        assert_eq!(settings.thumbnail_cache_mb, 256, "not one of the choices");
        assert!(settings.thumbnails);
    }

    #[test]
    fn entries_cover_every_key_exactly_once() {
        let entries = AppSettings::default().entries();
        let mut keys: Vec<&str> = entries.iter().map(|(k, _)| *k).collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate key");
        assert_eq!(before, 15);
    }

    #[test]
    fn row_heights_are_whole_and_ordered() {
        let heights: Vec<f32> = Density::ALL.iter().map(|d| d.row_height()).collect();
        assert!(heights.windows(2).all(|w| w[0] < w[1]));
        assert!(heights.iter().all(|h| h.fract() == 0.0));
        assert_eq!(
            Density::Regular.row_height(),
            28.0,
            "matches the old fixed height"
        );
    }

    #[test]
    fn conflict_choices_map_to_the_copy_engine() {
        assert_eq!(OnConflict::KeepBoth.policy(), ConflictPolicy::Rename);
        assert_eq!(OnConflict::Skip.policy(), ConflictPolicy::Skip);
        assert_eq!(OnConflict::Replace.policy(), ConflictPolicy::Overwrite);
    }

    #[test]
    fn terminal_names_are_cleaned_and_unsafe_ones_rejected() {
        assert_eq!(clean_terminal_name("  iTerm "), Some("iTerm".into()));
        assert_eq!(clean_terminal_name("Ghostty.app"), Some("Ghostty".into()));
        assert_eq!(clean_terminal_name(""), None);
        assert_eq!(clean_terminal_name("   "), None);
        assert_eq!(clean_terminal_name("-a"), None, "would read as an option");
        assert_eq!(clean_terminal_name("/bin/sh"), None, "no paths");
    }

    #[test]
    fn a_stored_unsafe_terminal_name_is_ignored() {
        let settings = AppSettings::from_lookup(lookup(&[(K_TERMINAL, "--evil")]));
        assert_eq!(settings.terminal_app, "Terminal");
    }

    #[test]
    fn cache_limit_is_reported_in_bytes() {
        let settings = AppSettings {
            thumbnail_cache_mb: 64,
            ..Default::default()
        };
        assert_eq!(settings.cache_limit_bytes(), 64 * 1024 * 1024);
    }

    #[test]
    fn search_options_follow_the_search_toggles() {
        let options = AppSettings {
            search_hidden: true,
            search_respects_gitignore: false,
            ..Default::default()
        }
        .search_options();
        assert!(options.hidden);
        assert!(!options.git_ignore);
        assert_eq!(
            options.max_results, 200,
            "the rest stays at the engine default"
        );
    }

    #[test]
    fn every_choice_enum_round_trips_through_its_key() {
        for theme in ThemePref::ALL {
            assert_eq!(ThemePref::parse(theme.key()), Some(theme));
        }
        for density in Density::ALL {
            assert_eq!(Density::parse(density.key()), Some(density));
        }
        for choice in OnConflict::ALL {
            assert_eq!(OnConflict::parse(choice.key()), Some(choice));
        }
        for startup in Startup::ALL {
            assert_eq!(Startup::parse(startup.key()), Some(startup));
        }
    }

    #[test]
    fn every_choice_has_a_label() {
        assert!(ThemePref::ALL.iter().all(|c| !c.label().is_empty()));
        assert!(Density::ALL.iter().all(|c| !c.label().is_empty()));
        assert!(OnConflict::ALL.iter().all(|c| !c.label().is_empty()));
        assert!(Startup::ALL.iter().all(|c| !c.label().is_empty()));
    }
}
