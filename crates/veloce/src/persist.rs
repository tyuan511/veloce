//! The bridge between the UI and the SQLite store.
//!
//! Reads happen once, before the window opens, into memory; after that the UI
//! only ever touches the in-memory copy, and every write is queued on a
//! background thread. A store that fails to open degrades to "remember
//! nothing" instead of stopping the app.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, RwLock},
};

use gpui::App;
use veloce_store::{FolderPrefs, Store, TabState};
use vfs_core::SortSpec;

use crate::{
    prefs::{
        LayoutPrefs, default_folder_view, folder_prefs_for, tabs_from_state, tabs_to_state,
        view_and_sort,
    },
    view::ViewMode,
};

/// Folders shown under "Recent" in the sidebar.
pub const RECENT_SHOWN: usize = 6;

/// Everything restored at start-up.
#[derive(Debug, Default)]
pub struct Startup {
    pub layout: LayoutPrefs,
    /// Saved tabs and the active index, per pane (left, right).
    pub tabs: [(Vec<String>, usize); 2],
    pub recents: Vec<String>,
}

#[derive(Clone)]
pub struct Persist {
    store: Option<Arc<Store>>,
    /// Per-folder view and sort, loaded whole so navigating never waits on disk.
    folders: Arc<RwLock<HashMap<String, FolderPrefs>>>,
}

/// Read the saved state out of `store`. Anything unreadable counts as "nothing
/// saved": a damaged table must not stop the app from starting.
pub fn load_startup(
    store: &Store,
    exists: impl Fn(&str) -> bool,
) -> (Startup, HashMap<String, FolderPrefs>) {
    let layout = LayoutPrefs::from_lookup(|key| store.get_setting(key).ok().flatten());
    let tabs_for = |pane: u8| tabs_from_state(store.load_tabs(pane).unwrap_or_default(), &exists);
    let folders = store
        .all_folder_prefs()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let recents = store.recent_paths(64).unwrap_or_default();
    (
        Startup {
            layout,
            tabs: [tabs_for(0), tabs_for(1)],
            recents,
        },
        folders,
    )
}

impl Persist {
    /// Open the default database and load everything. On failure the app runs
    /// without persistence and says why on stderr.
    pub fn open() -> (Self, Startup) {
        let exists = |path: &str| Path::new(path).is_dir();
        let store = Store::default_path()
            .ok_or_else(|| "this platform has no data directory".to_string())
            .and_then(|path| Store::open(&path).map_err(|error| error.to_string()));
        match store {
            Ok(store) => {
                let (startup, folders) = load_startup(&store, exists);
                let store = Arc::new(store);
                // Trim the thumbnail cache once per launch, off the main thread.
                let pruning = store.clone();
                std::thread::spawn(move || {
                    let _ = pruning.prune_thumbnails(crate::thumbs::CACHE_LIMIT_BYTES);
                    // Older builds kept preview renders as loose files in the
                    // system temp directory; they live in the database now.
                    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("veloce-thumbnails"));
                });
                (
                    Self {
                        store: Some(store),
                        folders: Arc::new(RwLock::new(folders)),
                    },
                    startup,
                )
            }
            Err(error) => {
                eprintln!("veloce: state will not be saved: {error}");
                (Self::disabled(), Startup::default())
            }
        }
    }

    /// No persistence at all.
    pub fn disabled() -> Self {
        Self {
            store: None,
            folders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn store(&self) -> Option<Arc<Store>> {
        self.store.clone()
    }

    /// View and sort for `path`: the saved choice, else Finder's default.
    pub fn folder_view(&self, path: &str) -> (ViewMode, SortSpec) {
        self.folders
            .read()
            .ok()
            .and_then(|folders| folders.get(path).map(view_and_sort))
            .unwrap_or_else(default_folder_view)
    }

    /// Remember `path`'s view and sort: memory now, disk in the background.
    pub fn remember_folder(&self, cx: &App, path: &str, view: ViewMode, sort: SortSpec) {
        let prefs = folder_prefs_for(view, sort);
        if let Ok(mut folders) = self.folders.write() {
            folders.insert(path.to_string(), prefs.clone());
        }
        let path = path.to_string();
        self.write(cx, move |store| store.set_folder_prefs(&path, &prefs));
    }

    pub fn save_layout(&self, cx: &App, layout: &LayoutPrefs) {
        let entries = layout.entries();
        self.write(cx, move |store| store.set_settings(&entries));
    }

    pub fn save_tabs(&self, cx: &App, pane: u8, paths: &[String], active: usize) {
        let tabs: Vec<TabState> = tabs_to_state(paths, active);
        self.write(cx, move |store| store.save_tabs(pane, &tabs));
    }

    pub fn record_visit(&self, cx: &App, path: &str) {
        let path = path.to_string();
        self.write(cx, move |store| store.record_visit(&path));
    }

    /// Queue a write on the background executor; failures only reach stderr
    /// because losing a preference is not worth interrupting the user.
    fn write(
        &self,
        cx: &App,
        job: impl FnOnce(&Store) -> veloce_store::Result<()> + Send + 'static,
    ) {
        let Some(store) = self.store.clone() else {
            return;
        };
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = job(&store) {
                    eprintln!("veloce: could not save state: {error}");
                }
            })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_data() -> Store {
        let store = Store::in_memory().unwrap();
        store.set_setting("layout.sidebar_width", "222").unwrap();
        store
            .set_folder_prefs(
                "/photos",
                &folder_prefs_for(ViewMode::Icons, SortSpec::default()),
            )
            .unwrap();
        store
            .save_tabs(
                0,
                &[
                    TabState {
                        path: "/a".into(),
                        active: false,
                    },
                    TabState {
                        path: "/gone".into(),
                        active: false,
                    },
                    TabState {
                        path: "/b".into(),
                        active: true,
                    },
                ],
            )
            .unwrap();
        store.record_visit_at("/a", 1).unwrap();
        store.record_visit_at("/b", 2).unwrap();
        store
    }

    #[test]
    fn startup_collects_layout_tabs_folders_and_recents() {
        let store = store_with_data();
        let (startup, folders) = load_startup(&store, |path| path != "/gone");
        assert_eq!(startup.layout.sidebar_width, 222.0);
        assert_eq!(
            startup.tabs[0],
            (vec!["/a".to_string(), "/b".to_string()], 1)
        );
        assert!(startup.tabs[1].0.is_empty(), "right pane saved nothing");
        assert_eq!(startup.recents, ["/b", "/a"]);
        assert!(folders.contains_key("/photos"));
    }

    #[test]
    fn an_empty_store_starts_with_defaults() {
        let store = Store::in_memory().unwrap();
        let (startup, folders) = load_startup(&store, |_| true);
        assert_eq!(startup.layout, LayoutPrefs::default());
        assert!(startup.tabs[0].0.is_empty());
        assert!(startup.recents.is_empty());
        assert!(folders.is_empty());
    }

    #[test]
    fn saved_folders_use_their_view_and_others_the_default() {
        let store = store_with_data();
        let (_, folders) = load_startup(&store, |_| true);
        let persist = Persist {
            store: None,
            folders: Arc::new(RwLock::new(folders)),
        };
        assert_eq!(persist.folder_view("/photos").0, ViewMode::Icons);
        assert_eq!(persist.folder_view("/elsewhere"), default_folder_view());
    }

    #[test]
    fn a_disabled_persist_remembers_nothing_but_still_answers() {
        let persist = Persist::disabled();
        assert!(persist.store().is_none());
        assert_eq!(persist.folder_view("/x"), default_folder_view());
    }
}
