//! The workspace: two panes, the inspector, the task panel, the palette and the
//! status bar, plus every keyboard action that ties them together.
//!
//! All actions arrive here and are forwarded to the focused pane. That single
//! choke point is what keeps the keymap honest: the keymap binds actions, and this
//! file is the only place that decides what each one means.

use crate::i18n::{tr, trf};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseMoveEvent, MouseUpEvent, Render, Subscription, Window, div, prelude::*, px,
    svg,
};
mod menu_view;

use veloce_store::SavedConnection;
use vfs_core::{FsWatcher, VfsUri, WatchEvent};

use crate::{
    actions::*,
    clipboard::FileClipboard,
    connections::Kind,
    context_menu::{
        ActionItem, AppEntry, MENU_PADDING, MENU_WIDTH, MenuAction, MenuContext, MenuItem,
        MenuTarget, MoreState, OpenWith, build_menu, clamp_origin, favorite_menu, menu_height,
        more_menu, open_with_menu, sort_menu, view_menu,
    },
    icons,
    info::info_rows,
    listing::{
        Favorite, Place, expand_home, favorite_places, favorite_rows, is_favorite, location_places,
        place_is_current, toggle_favorite,
    },
    open_with::{self, AppInfo},
    palette::{Palette, PaletteChoice, PaletteEvent, PaletteMode},
    pane::{Pane, PaneEvent, PaneSide, Services, is_app},
    persist::{Persist, Startup as SavedStartup},
    prefs::LayoutPrefs,
    preview::{PreviewEvent, PreviewView},
    remote::{self, ConnectionsRevision},
    settings::{AppSettings, Startup},
    settings_apply,
    tasks_panel::TaskPanel,
    text_input::{TextInput, TextInputEvent},
    theme,
    view::{
        Divider, HANDLE_WIDTH, ViewMode, inspector_from_pointer, pane_widths, panes_area,
        ratio_from_pointer, sidebar_from_pointer,
    },
};

/// Files created by the inline "new folder / rename" prompt.
pub(crate) enum InlinePrompt {
    NewFolder,
    NewFile,
    Rename,
}

/// An open right-click menu. The origin is the raw cursor position; it is
/// clamped to the window at render time, when the window size is known.
pub(crate) struct OpenMenu {
    /// Distinguishes this menu from a later one when async results arrive.
    id: u64,
    origin: (f32, f32),
    items: Vec<MenuItem>,
    /// Index of the top-level submenu row whose flyout is showing.
    open_sub: Option<usize>,
    /// The row under the cursor, as (in flyout, index). Tracked here because
    /// text colour baked in at layout can't follow a style-level `:hover`.
    hovered: Option<(bool, usize)>,
    target: MenuTarget,
    ctx: MenuContext,
    /// Applications behind the Open With rows, in menu order.
    apps: Vec<AppInfo>,
    /// The menu is just the Open With list (the inspector's Open button).
    open_with_only: bool,
}

/// Ask LaunchServices (and the icon cache) about `path`. `None` when the file is
/// gone or the lookup failed, so callers can keep what they had.
fn query_apps(
    path: &std::path::Path,
    home: &std::path::Path,
) -> Option<(Vec<AppInfo>, Vec<Option<PathBuf>>)> {
    // A placeholder for a remote file does not exist yet; make it. Only those live
    // under the scratch folder, so a real file that vanished still yields `None`.
    if !path.exists() && path.starts_with(std::env::temp_dir().join("veloce-open-with")) {
        open_with::ensure_placeholder(path).ok()?;
    }
    if !path.exists() {
        return None;
    }
    let apps = open_with::list_apps(path, home).ok()?;
    let cache = home.join("Library/Caches/veloce/app-icons");
    let icons = open_with::app_icons(&apps, &cache, open_with::MENU_ICON_PIXELS);
    Some((apps, icons))
}

/// One file type's cached Open With answer.
#[derive(Clone)]
struct AppsEntry {
    at: Instant,
    /// A real file of this type, to re-ask LaunchServices with.
    sample: PathBuf,
    apps: Vec<AppInfo>,
    icons: Vec<Option<PathBuf>>,
}

/// How often the known file types are re-queried in the background.
const APPS_REFRESH_EVERY: Duration = Duration::from_secs(300);

/// The Get Info window's subject plus its slowly computed folder size.
pub(crate) struct InfoState {
    entry: vfs_core::FileEntry,
    folder_bytes: Option<u64>,
}

/// The application root view.
pub struct Workspace {
    pub left: Entity<Pane>,
    pub right: Entity<Pane>,
    pub focused: PaneSide,
    pub preview: Entity<PreviewView>,
    pub tasks: Entity<TaskPanel>,
    pub palette: Entity<Palette>,
    pub tasks_open: bool,
    /// Second pane visible. Off by default, like Finder; ⌘\ turns it on.
    pub split: bool,
    persist: Persist,
    /// Recently visited folders, newest first.
    /// Files and folders the user starred; shown after the built-in favorites.
    favorites: Vec<Favorite>,
    /// The starred item whose menu is (or was last) open.
    favorite_target: Option<(PathBuf, bool)>,
    /// User-resizable layout: sidebar and inspector widths, and the left pane's
    /// share when split.
    sidebar_width: f32,
    inspector_width: f32,
    split_ratio: f32,
    /// The divider being dragged, if any.
    drag: Option<Divider>,
    /// Paths copied with Copy / "Copy Item", pasted by Paste Item.
    clipboard: FileClipboard,
    menu: Option<OpenMenu>,
    menu_counter: u64,
    /// Open With applications of the menu that was just closed by a click.
    menu_apps: Vec<AppInfo>,
    /// Applications that can open each file type, so the menu does not wait on
    /// LaunchServices every time. Keyed by `open_with::app_cache_key`.
    open_with_cache: HashMap<String, AppsEntry>,
    home: PathBuf,
    info: Option<InfoState>,
    /// Finder-style shortcuts down the left edge.
    places: Vec<Place>,
    /// Inline name prompt shown over the list (new folder / rename).
    pub(crate) prompt: Option<(InlinePrompt, Entity<TextInput>)>,
    focus_handle: FocusHandle,
    watcher: Option<Arc<Mutex<FsWatcher>>>,
    watched_dir: Option<PathBuf>,
    subscriptions: Vec<Subscription>,
    /// Saved remote connections, for the sidebar.
    connections: Vec<SavedConnection>,
    services: Services,
    /// Last error surfaced to the user (transient banner).
    banner: Option<(String, std::time::Instant)>,
}

impl EventEmitter<()> for Workspace {}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Workspace {
    pub fn new(persist: Persist, startup: SavedStartup, cx: &mut Context<Self>) -> Self {
        let services = Services::new(persist.clone());

        let home_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let home = home_dir.clone();
        // A folder on the command line wins over the saved session for the left
        // pane; otherwise the last session comes back.
        let cli_start = std::env::args()
            .nth(1)
            .map(|arg| expand_home(&arg))
            .filter(|path| path.is_dir());
        let start = cli_start.clone().unwrap_or_else(|| home.clone());
        let settings = startup.settings.clone();
        let [(left_saved, left_active), (right_saved, right_active)] = startup.tabs;
        let restore = |saved: Vec<String>, active: usize| {
            (saved.iter().map(VfsUri::local).collect::<Vec<_>>(), active)
        };
        let restore_session = settings.startup == Startup::RestoreSession;
        let (left_tabs, left_active) = if cli_start.is_some() || !restore_session {
            (Vec::new(), 0)
        } else {
            restore(left_saved, left_active)
        };
        let (right_tabs, right_active) = if restore_session {
            restore(right_saved, right_active)
        } else {
            (Vec::new(), 0)
        };
        let layout = startup.layout;

        let left = cx.new(|cx| {
            let mut pane = Pane::new(PaneSide::Left, VfsUri::local(&start), services.clone(), cx);
            pane.restore_tabs(left_tabs, left_active);
            pane.preview_open = layout.preview_open;
            pane.listing.show_hidden = settings.show_hidden;
            pane
        });
        let right = cx.new(|cx| {
            let mut pane = Pane::new(PaneSide::Right, VfsUri::local(&start), services.clone(), cx);
            pane.restore_tabs(right_tabs, right_active);
            pane.preview_open = layout.preview_open;
            pane.listing.show_hidden = settings.show_hidden;
            pane
        });

        let preview_store = services.persist.store();
        let preview = cx.new(|cx| PreviewView::new(preview_store, services.registry.clone(), cx));
        let tasks = cx.new(|cx| TaskPanel::new(services.tasks.clone(), cx));
        if layout.tasks_open {
            tasks.update(cx, |panel, cx| panel.toggle(cx));
        }
        let palette = cx.new(Palette::new);

        let mut workspace = Self {
            left: left.clone(),
            right: right.clone(),
            focused: PaneSide::Left,
            preview,
            tasks,
            palette: palette.clone(),
            tasks_open: layout.tasks_open,
            split: layout.split,
            sidebar_width: layout.sidebar_width,
            inspector_width: layout.inspector_width,
            split_ratio: layout.split_ratio,
            persist,
            favorites: startup.favorites.clone(),
            favorite_target: None,
            drag: None,
            clipboard: FileClipboard::default(),
            menu: None,
            menu_counter: 0,
            menu_apps: Vec::new(),
            open_with_cache: HashMap::new(),
            home: home_dir.clone(),
            info: None,
            places: favorite_places(&home_dir),
            prompt: None,
            focus_handle: cx.focus_handle(),
            watcher: None,
            watched_dir: None,
            subscriptions: Vec::new(),
            banner: None,
            connections: Vec::new(),
            services,
        };

        workspace.subscriptions = workspace.wire(cx);
        workspace.reload_connections(cx);
        workspace.spawn_apps_refresher(cx);
        workspace
    }

    /// Keep the Open With cache warm so the menu never has to wait: every few
    /// minutes, re-ask LaunchServices about each file type seen so far, one at a
    /// time so it never spikes the machine.
    fn spawn_apps_refresher(&self, cx: &mut Context<Self>) {
        let home = self.home.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(APPS_REFRESH_EVERY).await;
                let Ok(samples) = this.update(cx, |workspace, _| {
                    workspace
                        .open_with_cache
                        .iter()
                        .map(|(key, entry)| (key.clone(), entry.sample.clone()))
                        .collect::<Vec<_>>()
                }) else {
                    return;
                };
                for (key, sample) in samples {
                    let home = home.clone();
                    let fresh = cx
                        .background_executor()
                        .spawn(async move { query_apps(&sample, &home).map(|r| (sample, r)) })
                        .await;
                    // The sample may be gone or the query may fail: keep the old answer.
                    let Some((sample, (apps, icons))) = fresh else {
                        continue;
                    };
                    let stored = this.update(cx, |workspace, _| {
                        workspace.open_with_cache.insert(
                            key,
                            AppsEntry {
                                at: Instant::now(),
                                sample,
                                apps,
                                icons,
                            },
                        );
                    });
                    if stored.is_err() {
                        return;
                    }
                }
            }
        })
        .detach();
    }

    /// Load both panes and start watching the focused one.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Palette → workspace: run the chosen row. Subscribed with the window,
        // since some commands (a confirmation prompt) need to take focus.
        let palette = self.palette.clone();
        self.subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            |workspace, _, event: &PaletteEvent, window, cx| {
                let PaletteEvent::Chosen(choice) = event;
                workspace.run_palette_choice(choice.clone(), window, cx);
            },
        ));
        // Follow the OS light/dark setting, now and whenever it flips.
        theme::set_appearance(window.appearance());
        self.subscriptions
            .push(window.observe_window_appearance(|window, _cx| {
                if theme::set_appearance(window.appearance()) {
                    window.refresh();
                }
            }));
        self.left.update(cx, |pane, cx| pane.reload(cx));
        self.right.update(cx, |pane, cx| pane.reload(cx));
        self.sync_preview(cx);
        self.retarget_watcher(cx);
        let handle = self.focus_handle.clone();
        window.focus(&handle);
        self.focus_pane(PaneSide::Left, window, cx);
    }

    fn wire(&mut self, cx: &mut Context<Self>) -> Vec<Subscription> {
        let mut subscriptions = Vec::new();

        // Pane → workspace: status, preview, watcher.
        for pane in [self.left.clone(), self.right.clone()] {
            subscriptions.push(
                cx.subscribe(&pane, |workspace, pane, event: &PaneEvent, cx| {
                    match event {
                        PaneEvent::ListingChanged => {
                            workspace.sync_preview(cx);
                            workspace.retarget_watcher(cx);
                        }
                        PaneEvent::OpenExternally(uri) => match uri.as_local_path() {
                            Some(path) => {
                                let _ = open::that_detached(path);
                            }
                            None => workspace.open_remote_file(uri.clone(), cx),
                        },
                        PaneEvent::OpenTerminal(dir) => {
                            let _ = open::that_detached(dir);
                        }
                        PaneEvent::ContextMenu { position, on_entry } => {
                            workspace.open_context_menu(&pane, *position, *on_entry, cx);
                        }
                        PaneEvent::TabsChanged => workspace.save_tabs(cx),
                        PaneEvent::SelectionChanged => {
                            // Clicking in the other pane makes it the active one,
                            // so the preview follows what the user just selected.
                            let side = pane.read(cx).side;
                            if workspace.split && workspace.focused != side {
                                workspace.focused = side;
                                workspace.retarget_watcher(cx);
                            }
                            workspace.sync_preview(cx);
                        }
                        PaneEvent::Notice(message) => {
                            workspace.banner = Some((message.clone(), std::time::Instant::now()));
                        }
                    }
                    cx.notify();
                }),
            );
        }

        // Settings → workspace: react when the Settings window changes something.
        subscriptions.push(cx.observe_global::<AppSettings>(|workspace, cx| {
            workspace.on_settings_changed(cx);
        }));
        subscriptions.push(cx.observe_global::<ConnectionsRevision>(|workspace, cx| {
            if let Some(id) = cx.global::<ConnectionsRevision>().edited
                && let Ok(mut registry) = workspace.services.registry.write()
            {
                // Reconnect with the new settings the next time it is opened.
                registry.retain_connections(|_, key| key != id.to_string());
            }
            workspace.reload_connections(cx);
        }));

        // Inspector → workspace: its Open button and application list.
        subscriptions.push(cx.subscribe(
            &self.preview,
            |workspace, _, event: &PreviewEvent, cx| match event {
                PreviewEvent::Open => {
                    let pane = workspace.focused_pane();
                    pane.update(cx, |pane, cx| pane.open_selection(cx));
                }
                PreviewEvent::OpenWith(at) => workspace.open_apps_menu(*at, cx),
            },
        ));

        subscriptions
    }

    /// Settings changed (in any window): bring the panes in line.
    fn on_settings_changed(&mut self, cx: &mut Context<Self>) {
        let show_hidden = cx.global::<AppSettings>().show_hidden;
        for pane in [self.left.clone(), self.right.clone()] {
            pane.update(cx, |pane, cx| pane.set_show_hidden(show_hidden, cx));
        }
        cx.notify();
    }

    // ---- remote connections --------------------------------------------------

    /// Re-read the saved connections and unmount any that no longer exist.
    fn reload_connections(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.persist.store() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let list = cx
                .background_executor()
                .spawn(async move { store.list_connections().unwrap_or_default() })
                .await;
            this.update(cx, |workspace, cx| {
                if let Ok(mut registry) = workspace.services.registry.write() {
                    registry.retain_connections(|scheme, id| {
                        list.iter().any(|connection| {
                            remote::uri_id(connection) == id
                                && Kind::from_key(&connection.kind)
                                    .is_some_and(|kind| kind.scheme() == scheme)
                        })
                    });
                }
                workspace.connections = list;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Browse a saved connection in the focused pane, connecting first if needed.
    fn open_connection(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = self.connections.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        let Some(root) = remote::root_uri(&connection) else {
            return;
        };
        let side = self.focused;
        let mounted = self.services.registry.read().is_ok_and(|registry| {
            registry.has_connection(root.scheme, &remote::uri_id(&connection))
        });
        if mounted {
            self.pane(side)
                .update(cx, |pane, cx| pane.navigate_to(root, cx));
            self.focus_pane(side, window, cx);
            return;
        }
        self.banner = Some((
            trf("Connecting to {}…", &[&connection.name]),
            std::time::Instant::now(),
        ));
        cx.notify();
        let registry = self.services.registry.clone();
        cx.spawn_in(window, async move |this, cx| {
            let name = connection.name.clone();
            let mounted = cx
                .background_executor()
                .spawn(async move { remote::mount(&connection) })
                .await;
            this.update_in(cx, |workspace, window, cx| match mounted {
                Ok(mounted) => {
                    if let Ok(mut registry) = registry.write() {
                        registry.register_connection(mounted.id, mounted.provider);
                    }
                    workspace.banner = None;
                    workspace
                        .pane(side)
                        .update(cx, |pane, cx| pane.navigate_to(root, cx));
                    workspace.focus_pane(side, window, cx);
                    cx.notify();
                }
                Err(message) => {
                    workspace.banner = Some((
                        trf("Cannot connect to {}: {}", &[&name, &message]),
                        std::time::Instant::now(),
                    ));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Download a remote file to a scratch folder and open it with its default app.
    fn open_remote_file(&mut self, uri: VfsUri, cx: &mut Context<Self>) {
        self.with_local_copies(vec![uri], cx, |_, files, _| {
            for file in files {
                let _ = open::that_detached(&file);
            }
        });
    }

    /// Run `then` with a local path for every uri, in order. Local ones are used as
    /// they are; remote ones are first downloaded to a scratch folder as ordinary
    /// transfer tasks (so progress shows in the task panel). If any download fails
    /// nothing runs and the reason is shown.
    fn with_local_copies(
        &mut self,
        uris: Vec<VfsUri>,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Vec<PathBuf>, &mut Context<Self>) + 'static,
    ) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let tasks = self.services.tasks.clone();
        let mut files = Vec::new();
        let mut pending = Vec::new();
        for (index, uri) in uris.into_iter().enumerate() {
            match uri.as_local_path() {
                Some(path) => files.push(path),
                None => {
                    // One folder per file, so equal names from different places
                    // cannot overwrite each other.
                    let local = std::env::temp_dir()
                        .join("veloce-remote")
                        .join(format!("{stamp}-{index}"))
                        .join(uri.file_name());
                    let id = tasks.transfer(
                        uri,
                        VfsUri::local(&local),
                        self.services.registry.clone(),
                        vfs_core::ops::ConflictPolicy::Overwrite,
                        false,
                    );
                    pending.push(id);
                    files.push(local);
                }
            }
        }
        cx.spawn(async move |this, cx| {
            let mut failure = None;
            while !pending.is_empty() {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let snapshots = tasks.snapshots();
                pending.retain(|id| {
                    let finished = snapshots
                        .iter()
                        .find(|task| task.id == *id)
                        .filter(|task| task.is_terminal());
                    match finished {
                        Some(task) => {
                            match &task.state {
                                vfs_core::TaskState::Done => {}
                                vfs_core::TaskState::Failed(message) => {
                                    failure.get_or_insert(message.clone());
                                }
                                _ => {
                                    failure.get_or_insert_with(String::new);
                                }
                            }
                            false
                        }
                        None => true,
                    }
                });
            }
            this.update(cx, |workspace, cx| {
                match failure {
                    None => then(workspace, files, cx),
                    // Cancelled by the user: nothing to report.
                    Some(message) if message.is_empty() => {}
                    Some(message) => {
                        workspace.banner = Some((message, std::time::Instant::now()));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- persistence -------------------------------------------------------

    /// Right-click on a starred item in the sidebar.
    fn open_favorite_menu(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        removable: bool,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.favorite_target = Some((path, is_dir));
        self.menu_counter += 1;
        self.menu = Some(OpenMenu {
            id: self.menu_counter,
            origin: (f32::from(position.x), f32::from(position.y)),
            items: favorite_menu(is_dir, removable),
            open_sub: None,
            hovered: None,
            target: MenuTarget::Background,
            ctx: MenuContext::default(),
            apps: Vec::new(),
            open_with_only: false,
        });
        cx.notify();
    }

    /// Run a sidebar-favorite menu row on the item it was opened for.
    fn run_favorite_action(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        let Some((path, is_dir)) = self.favorite_target.clone() else {
            return;
        };
        let pane = self.focused_pane();
        match action {
            MenuAction::FavOpen if is_dir => {
                pane.update(cx, |pane, cx| pane.navigate_to(VfsUri::local(&path), cx));
            }
            MenuAction::FavOpen => {
                let _ = open::that_detached(&path);
            }
            MenuAction::FavOpenInNewTab => {
                pane.update(cx, |pane, cx| {
                    pane.open_in_new_tab(VfsUri::local(&path), cx)
                });
            }
            MenuAction::FavReveal => pane.update(cx, |pane, cx| pane.reveal_file(&path, cx)),
            MenuAction::FavCopyPath => Self::copy_text_to_clipboard(&[path], cx),
            MenuAction::FavOpenInTerminal => {
                let app = cx.global::<AppSettings>().terminal_app.clone();
                Self::open_terminal_at(&path, &app);
            }
            MenuAction::FavRemove => {
                self.favorites.retain(|favorite| favorite.path != path);
                self.persist.save_favorites(cx, &self.favorites);
                self.favorite_target = None;
            }
            _ => {}
        }
        cx.notify();
    }

    /// Star or unstar the single selected local item.
    fn toggle_favorite_of_cursor(&mut self, cx: &mut Context<Self>) {
        let entries = self.focused_pane().read(cx).selection_or_cursor();
        let [entry] = entries.as_slice() else {
            return;
        };
        let Some(path) = entry.uri.as_local_path() else {
            return;
        };
        toggle_favorite(
            &mut self.favorites,
            Favorite {
                path,
                is_dir: entry.is_dir(),
            },
        );
        self.persist.save_favorites(cx, &self.favorites);
        cx.notify();
    }

    fn layout_prefs(&self, cx: &App) -> LayoutPrefs {
        LayoutPrefs {
            sidebar_width: self.sidebar_width,
            inspector_width: self.inspector_width,
            split_ratio: self.split_ratio,
            split: self.split,
            preview_open: self.focused_pane().read(cx).preview_open,
            tasks_open: self.tasks_open,
        }
    }

    fn save_layout(&self, cx: &App) {
        self.persist.save_layout(cx, &self.layout_prefs(cx));
    }

    fn save_tabs(&self, cx: &App) {
        for (index, pane) in [&self.left, &self.right].into_iter().enumerate() {
            let (paths, active) = pane.read(cx).tab_paths();
            self.persist.save_tabs(cx, index as u8, &paths, active);
        }
    }

    // ---- context menu & file-menu commands ----------------------------------

    fn open_context_menu(
        &mut self,
        pane: &Entity<Pane>,
        position: gpui::Point<gpui::Pixels>,
        on_entry: bool,
        cx: &mut Context<Self>,
    ) {
        let (side, selection) = {
            let pane = pane.read(cx);
            (pane.side, pane.selection_or_cursor())
        };
        // Right-clicking the other pane acts there, as in any two-pane manager.
        self.focused = if self.split { side } else { PaneSide::Left };
        let target = match (on_entry, selection.first()) {
            (true, Some(first)) => MenuTarget::Items {
                count: selection.len(),
                is_dir: selection.len() == 1 && first.is_dir() && !is_app(first),
                app_bundle: selection.len() == 1 && is_app(first),
                files_only: selection.iter().all(|entry| !entry.is_dir()),
                archive_stem: (selection.len() == 1 && !first.is_dir())
                    .then(|| vfs_core::archive::ArchiveKind::detect(&first.name))
                    .flatten()
                    .map(|_| vfs_core::archive::archive_stem(&first.name)),
            },
            _ => MenuTarget::Background,
        };
        let (view, show_hidden, sort) = {
            let pane = self.focused_pane().read(cx);
            (pane.view, pane.listing.show_hidden, pane.sort)
        };
        let remote = !self.focused_pane().read(cx).current_uri().scheme.is_local();
        let ctx = MenuContext {
            remote,
            can_paste: !self.clipboard.is_empty(),
            view,
            show_hidden,
            sort,
            open_with: OpenWith::Loading,
            favorite: match selection.as_slice() {
                [entry] => entry
                    .uri
                    .as_local_path()
                    .map(|path| is_favorite(&self.favorites, &path)),
                _ => None,
            },
        };
        self.menu_counter += 1;
        let id = self.menu_counter;
        let wants_apps = matches!(
            &target,
            MenuTarget::Items {
                files_only: true,
                ..
            }
        );
        self.menu = Some(OpenMenu {
            id,
            origin: (f32::from(position.x), f32::from(position.y)),
            items: build_menu(&target, &ctx),
            open_sub: None,
            hovered: None,
            target,
            ctx,
            apps: Vec::new(),
            open_with_only: false,
        });
        cx.notify();
        if wants_apps {
            let first = selection.first().map(|entry| {
                entry
                    .uri
                    .as_local_path()
                    .unwrap_or_else(|| self.placeholder_for(entry))
            });
            if let Some(path) = first {
                self.load_open_with(id, path, cx);
            }
        }
    }

    /// Fill the Open With submenu of menu `id` (if that menu is still open).
    fn apply_open_with(
        &mut self,
        id: u64,
        apps: Vec<AppInfo>,
        icons: Vec<Option<PathBuf>>,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.menu.as_mut().filter(|menu| menu.id == id) else {
            return;
        };
        menu.ctx.open_with = OpenWith::Apps(
            apps.iter()
                .zip(icons)
                .map(|(app, icon)| AppEntry {
                    name: app.name.clone(),
                    icon,
                })
                .collect(),
        );
        menu.apps = apps;
        menu.items = if menu.open_with_only {
            open_with_menu(&menu.ctx.open_with)
        } else {
            build_menu(&menu.target, &menu.ctx)
        };
        cx.notify();
    }

    /// Fill the Open With submenu: straight from the per-file-type cache when
    /// there is one, otherwise ask LaunchServices off the UI thread. A stale
    /// cache entry is shown at once and refreshed behind the menu's back.
    /// The stand-in local path an application list is asked for when `entry` lives
    /// on a server. The file itself is created in the background by `query_apps`.
    fn placeholder_for(&self, entry: &vfs_core::FileEntry) -> PathBuf {
        open_with::placeholder_path(
            &std::env::temp_dir().join("veloce-open-with"),
            &entry.extension().unwrap_or_default(),
        )
    }

    fn load_open_with(&mut self, id: u64, path: PathBuf, cx: &mut Context<Self>) {
        const FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(600);
        let key = open_with::app_cache_key(&path);
        if let Some(entry) = self.open_with_cache.get(&key).cloned() {
            self.apply_open_with(id, entry.apps, entry.icons, cx);
            if entry.at.elapsed() < FRESH_FOR {
                return;
            }
        }
        let home = self.home.clone();
        cx.spawn(async move |this, cx| {
            let sample = path.clone();
            let (apps, icons) = cx
                .background_executor()
                .spawn(async move { query_apps(&path, &home).unwrap_or_default() })
                .await;
            this.update(cx, |workspace, cx| {
                workspace.open_with_cache.insert(
                    key,
                    AppsEntry {
                        at: Instant::now(),
                        sample,
                        apps: apps.clone(),
                        icons: icons.clone(),
                    },
                );
                workspace.apply_open_with(id, apps, icons, cx);
            })
            .ok();
        })
        .detach();
    }

    /// The inspector's Open button: the same application list as the context menu's
    /// "Open With", dropped under the button.
    fn open_apps_menu(&mut self, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let Some(entry) = self.focused_pane().read(cx).cursor_entry().cloned() else {
            return;
        };
        let path = entry
            .uri
            .as_local_path()
            .unwrap_or_else(|| self.placeholder_for(&entry));
        let ctx = MenuContext::default();
        self.menu_counter += 1;
        let id = self.menu_counter;
        self.menu = Some(OpenMenu {
            id,
            origin: (f32::from(at.x) - MENU_WIDTH + 24.0, f32::from(at.y) + 14.0),
            items: open_with_menu(&ctx.open_with),
            open_sub: None,
            hovered: None,
            target: MenuTarget::Background,
            ctx,
            apps: Vec::new(),
            open_with_only: true,
        });
        cx.notify();
        // Folders have no "open with": the list simply stays at its placeholder.
        if !entry.is_dir() {
            self.load_open_with(id, path, cx);
        }
    }

    fn set_menu_hover(&mut self, row: (bool, usize), hovered: bool, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.as_mut() {
            let next = if hovered {
                Some(row)
            } else if menu.hovered == Some(row) {
                None
            } else {
                return;
            };
            if menu.hovered != next {
                menu.hovered = next;
                cx.notify();
            }
        }
    }

    fn set_open_sub(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.as_mut()
            && menu.open_sub != index
        {
            menu.open_sub = index;
            cx.notify();
        }
    }

    /// Close the menu but remember its app list: a click on "Open With ▸ Zed"
    /// closes the menu first and then needs to know what "Zed" was.
    fn close_menu_keeping_apps(&mut self) {
        if let Some(menu) = self.menu.take() {
            self.menu_apps = menu.apps;
        }
    }

    /// Open the selected files with `app`.
    fn open_selection_with(&mut self, app: AppInfo, cx: &mut Context<Self>) {
        let files = self.open_with_files(cx);
        self.with_local_copies(files, cx, move |workspace, files, _| {
            if let Err(message) = open_with::open_with(&app.path, &files) {
                workspace.banner = Some((message, std::time::Instant::now()));
            }
        });
    }

    /// The selected files (not folders), wherever they live.
    fn open_with_files(&self, cx: &mut Context<Self>) -> Vec<VfsUri> {
        self.focused_pane()
            .read(cx)
            .selection_or_cursor()
            .iter()
            .filter(|entry| !entry.is_dir())
            .map(|entry| entry.uri.clone())
            .collect()
    }

    /// "Other…": let the user pick any application.
    fn open_selection_with_other(&mut self, cx: &mut Context<Self>) {
        let files = self.open_with_files(cx);
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(app) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |workspace, cx| {
                workspace.with_local_copies(files, cx, move |workspace, files, _| {
                    if let Err(message) = open_with::open_with(&app, &files) {
                        workspace.banner = Some((message, std::time::Instant::now()));
                    }
                });
            })
            .ok();
        })
        .detach();
    }

    fn show_info(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.focused_pane().read(cx).cursor_entry().cloned() else {
            return;
        };
        let path = entry.uri.as_local_path();
        let is_dir = entry.is_dir();
        let id = entry.id();
        self.info = Some(InfoState {
            entry,
            folder_bytes: None,
        });
        cx.notify();
        if let (true, Some(path)) = (is_dir, path) {
            // Folder sizes need a full walk, so they arrive after the window opens.
            cx.spawn(async move |this, cx| {
                let (bytes, _) = cx
                    .background_executor()
                    .spawn(async move { vfs_core::ops::measure_tree(&path) })
                    .await;
                this.update(cx, |workspace, cx| {
                    if let Some(info) = workspace.info.as_mut().filter(|i| i.entry.id() == id) {
                        info.folder_bytes = Some(bytes);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    fn copy_text_to_clipboard(paths: &[PathBuf], cx: &mut Context<Self>) {
        let text = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    /// Replace the file clipboard and dim whatever was cut, in both panes.
    fn set_clipboard(&mut self, clipboard: FileClipboard, cx: &mut Context<Self>) {
        self.clipboard = clipboard;
        let cut = self.clipboard.cut_paths().to_vec();
        for pane in [self.left.clone(), self.right.clone()] {
            pane.update(cx, |pane, cx| pane.set_cut_paths(cut.clone(), cx));
        }
        cx.notify();
    }

    /// Paste into `dir`: moves after Cut, copies after Copy.
    fn paste_into(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let ops = self.clipboard.plan_paste(&dir);
        if ops.is_empty() {
            return;
        }
        self.focused_pane()
            .update(cx, |pane, cx| pane.apply_paste(ops, cx));
        let clipboard = std::mem::take(&mut self.clipboard).after_paste();
        self.set_clipboard(clipboard, cx);
    }

    /// One entry point for the menu rows and their keyboard shortcuts, so both
    /// always do the same thing.
    fn run_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        match action {
            MenuAction::FavOpen
            | MenuAction::FavOpenInNewTab
            | MenuAction::FavReveal
            | MenuAction::FavCopyPath
            | MenuAction::FavOpenInTerminal
            | MenuAction::FavRemove => self.run_favorite_action(action, cx),
            MenuAction::Nothing => {}
            MenuAction::Open => pane.update(cx, |pane, cx| pane.open_selection(cx)),
            MenuAction::OpenWith(index) => {
                if let Some(app) = self.menu_apps.get(index).cloned() {
                    self.open_selection_with(app, cx);
                }
            }
            MenuAction::OpenWithOther => self.open_selection_with_other(cx),
            MenuAction::OpenInNewTab => {
                if let Some(entry) = pane.read(cx).cursor_entry().cloned() {
                    pane.update(cx, |pane, cx| pane.open_in_new_tab(entry.uri, cx));
                }
            }
            MenuAction::ShowPackageContents => {
                pane.update(cx, |pane, cx| pane.show_package_contents(cx));
            }
            MenuAction::OpenInTerminal => {
                let target = {
                    let pane = pane.read(cx);
                    match pane.cursor_entry() {
                        Some(entry) if entry.is_dir() && !pane.listing.selected.is_empty() => {
                            entry.uri.as_local_path()
                        }
                        _ => pane.current_uri().as_local_path(),
                    }
                };
                if let Some(dir) = target {
                    let app = cx.global::<AppSettings>().terminal_app.clone();
                    Self::open_terminal_at(&dir, &app);
                }
            }
            MenuAction::Preview => self.show_inspector(cx),
            MenuAction::Compress(format) => {
                pane.update(cx, |pane, cx| pane.compress_selection(format, cx))
            }
            MenuAction::ExtractHere => pane.update(cx, |pane, cx| pane.extract_selection(true, cx)),
            MenuAction::ExtractTo => pane.update(cx, |pane, cx| pane.extract_selection(false, cx)),
            MenuAction::CopyPath => {
                let entries = pane.read(cx).selection_or_cursor();
                if entries.iter().any(|entry| !entry.uri.scheme.is_local()) {
                    // A server path is only meaningful with its connection, so copy
                    // the whole uri (`s3://1/images/a.png`).
                    let text: Vec<String> =
                        entries.iter().map(|entry| entry.uri.to_string()).collect();
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.join("\n")));
                } else {
                    let paths = pane.read(cx).selection_paths();
                    Self::copy_text_to_clipboard(&paths, cx);
                }
            }
            MenuAction::Cut => {
                let paths = pane.read(cx).selection_paths();
                Self::copy_text_to_clipboard(&paths, cx);
                self.set_clipboard(FileClipboard::cut(paths), cx);
            }
            MenuAction::Copy => {
                let paths = pane.read(cx).selection_paths();
                Self::copy_text_to_clipboard(&paths, cx);
                self.set_clipboard(FileClipboard::copy(paths), cx);
            }
            MenuAction::Paste => {
                if let Some(dir) = pane.read(cx).current_uri().as_local_path() {
                    self.paste_into(dir, cx);
                }
            }
            MenuAction::PasteInto => {
                let dir = pane
                    .read(cx)
                    .cursor_entry()
                    .filter(|entry| entry.is_dir())
                    .and_then(|entry| entry.uri.as_local_path());
                if let Some(dir) = dir {
                    self.paste_into(dir, cx);
                }
            }
            MenuAction::CreateShortcut => pane.update(cx, |pane, cx| pane.shortcut_selection(cx)),
            MenuAction::Delete => {
                // 回收站操作几乎瞬间完成，不值得为它弹出任务面板。
                self.delete_selection(cx);
            }
            MenuAction::Rename => {
                if let Some(entry) = pane.read(cx).cursor_entry().cloned() {
                    self.start_prompt(InlinePrompt::Rename, entry.name.clone(), window, cx);
                }
            }
            MenuAction::ToggleFavorite => self.toggle_favorite_of_cursor(cx),
            MenuAction::Properties => self.show_info(cx),
            MenuAction::NewFolder => self.start_prompt(
                InlinePrompt::NewFolder,
                "New folder".to_string(),
                window,
                cx,
            ),
            MenuAction::NewFile => self.start_prompt(
                InlinePrompt::NewFile,
                "untitled.txt".to_string(),
                window,
                cx,
            ),
            MenuAction::SortBy(key) => {
                let spec = vfs_core::SortSpec {
                    key,
                    ..pane.read(cx).sort
                };
                pane.update(cx, |pane, cx| pane.set_sort(spec, cx));
            }
            MenuAction::SortDescending(descending) => {
                let spec = vfs_core::SortSpec {
                    descending,
                    ..pane.read(cx).sort
                };
                pane.update(cx, |pane, cx| pane.set_sort(spec, cx));
            }
            MenuAction::SortFoldersFirst => {
                let current = pane.read(cx).sort;
                let spec = vfs_core::SortSpec {
                    dirs_first: !current.dirs_first,
                    ..current
                };
                pane.update(cx, |pane, cx| pane.set_sort(spec, cx));
            }
            MenuAction::SetView(view) => self.set_view(view, cx),
            MenuAction::ToggleSplit => self.toggle_split(window, cx),
            MenuAction::TogglePreviewPanel => self.on_toggle_preview(&TogglePreview, window, cx),
            MenuAction::ToggleTasks => self.on_toggle_tasks(&ToggleTasks, window, cx),
            MenuAction::CommandPalette => self.open_palette(PaletteMode::Commands, window, cx),
            MenuAction::OpenSettings => crate::settings_window::open(cx),
            MenuAction::ToggleHidden => {
                // One setting for both panes; `on_settings_changed` applies it.
                settings_apply::commit(cx, |settings| settings.show_hidden = !settings.show_hidden);
            }
            MenuAction::Refresh => pane.update(cx, |pane, cx| pane.refresh(cx)),
        }
        cx.notify();
    }

    /// Open the configured terminal application (Settings → Files) in `dir`.
    /// macOS only for now; elsewhere there is no single "the terminal" to ask for,
    /// so this is a no-op rather than a wrong guess.
    fn open_terminal_at(dir: &std::path::Path, app: &str) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg("-a")
                .arg(app)
                .arg(dir)
                .spawn();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (dir, app);
    }

    fn pane(&self, side: PaneSide) -> Entity<Pane> {
        match side {
            PaneSide::Left => self.left.clone(),
            PaneSide::Right => self.right.clone(),
        }
    }

    fn focused_pane(&self) -> Entity<Pane> {
        self.pane(self.focused)
    }

    fn other_pane(&self) -> Entity<Pane> {
        self.pane(self.focused.other())
    }

    fn focus_pane(&mut self, side: PaneSide, window: &mut Window, cx: &mut Context<Self>) {
        // Without the split there is only the left pane to focus.
        let side = if self.split { side } else { PaneSide::Left };
        self.focused = side;
        let handle = self.pane(side).read(cx).focus_handle.clone();
        window.focus(&handle);
        self.sync_preview(cx);
        self.retarget_watcher(cx);
        cx.notify();
    }

    fn switch_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.split {
            return;
        }
        self.focus_pane(self.focused.other(), window, cx);
    }

    /// Mirror the focused pane's cursor entry into the inspector. Nothing is
    /// loaded while the inspector is closed: previews can spawn external tools.
    fn sync_preview(&mut self, cx: &mut Context<Self>) {
        if !self.focused_pane().read(cx).preview_open {
            return;
        }
        let entry = self.focused_pane().read(cx).cursor_entry().cloned();
        self.preview
            .update(cx, |preview, cx| preview.show(entry, cx));
    }

    /// Open the inspector (never toggles) and load the cursor entry into it.
    fn show_inspector(&mut self, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| {
            if !pane.preview_open {
                pane.toggle_preview(cx);
            }
        });
        self.sync_preview(cx);
        self.save_layout(cx);
        cx.notify();
    }

    /// Point the filesystem watcher at the focused pane's directory.
    fn retarget_watcher(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.focused_pane().read(cx).current_uri().as_local_path() else {
            return;
        };
        if self.watched_dir.as_deref() == Some(dir.as_path()) {
            return;
        }
        let left = self.left.clone();
        let right = self.right.clone();
        let changed: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let changed_sink = changed.clone();

        let watcher = FsWatcher::new(move |event| {
            if let WatchEvent::DirectoryChanged(path) = event {
                changed_sink.lock().unwrap().push(path);
            }
        });

        match watcher {
            Ok(mut watcher) => {
                if watcher.watch(&dir).is_ok() {
                    self.watcher = Some(Arc::new(Mutex::new(watcher)));
                    self.watched_dir = Some(dir);
                    // Debounced refresh: coalesce a burst of events into one reload.
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(250))
                                .await;
                            let pending = {
                                let mut queue = changed.lock().unwrap();
                                let has = !queue.is_empty();
                                queue.clear();
                                has
                            };
                            if !pending {
                                continue;
                            }
                            let alive = this
                                .update(cx, |workspace, cx| {
                                    let side = workspace.focused;
                                    let pane = workspace.pane(side);
                                    pane.update(cx, |pane, cx| pane.refresh(cx));
                                    let other = workspace.pane(side.other());
                                    let other_dir = other.read(cx).current_uri().as_local_path();
                                    if other_dir.as_deref() == workspace.watched_dir.as_deref() {
                                        other.update(cx, |pane, cx| pane.refresh(cx));
                                    }
                                    let _ = (&left, &right);
                                    cx.notify();
                                })
                                .is_err();
                            if alive {
                                break;
                            }
                        }
                    })
                    .detach();
                }
            }
            Err(error) => {
                self.banner = Some((error.to_string(), std::time::Instant::now()));
            }
        }
    }

    fn run_palette_choice(
        &mut self,
        choice: PaletteChoice,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match choice {
            PaletteChoice::File(path) => {
                let pane = self.focused_pane();
                pane.update(cx, |pane, cx| {
                    if path.is_dir() {
                        pane.navigate_to(VfsUri::local(&path), cx);
                    } else {
                        if let Some(parent) = path.parent() {
                            pane.navigate_to(VfsUri::local(parent), cx);
                        }
                        cx.emit(PaneEvent::OpenExternally(VfsUri::local(&path)));
                    }
                });
            }
            PaletteChoice::Goto { path, is_dir } => {
                let pane = self.focused_pane();
                pane.update(cx, |pane, cx| {
                    if is_dir {
                        pane.navigate_to(VfsUri::local(&path), cx);
                    } else {
                        pane.reveal_file(&path, cx);
                    }
                });
            }
            PaletteChoice::Command(id) => {
                let pane = self.focused_pane();
                match id {
                    crate::palette::CommandId::NewFolder => {
                        pane.update(cx, |pane, cx| pane.create_dir("New Folder", cx));
                    }
                    crate::palette::CommandId::NewTab => {
                        pane.update(cx, |pane, cx| pane.new_tab(cx));
                    }
                    crate::palette::CommandId::TogglePreview => {
                        pane.update(cx, |pane, cx| pane.toggle_preview(cx));
                    }
                    crate::palette::CommandId::ToggleTasks => {
                        self.tasks_open = !self.tasks_open;
                        self.tasks.update(cx, |panel, cx| panel.toggle(cx));
                    }
                    crate::palette::CommandId::CopyToOtherPane => {
                        self.copy_to_other_pane(cx);
                    }
                    crate::palette::CommandId::MoveToOtherPane => {
                        self.move_to_other_pane(cx);
                    }
                    crate::palette::CommandId::TrashSelection => {
                        self.delete_selection(cx);
                    }
                    crate::palette::CommandId::Refresh => {
                        pane.update(cx, |pane, cx| pane.refresh(cx));
                    }
                    crate::palette::CommandId::CycleSort => {
                        pane.update(cx, |pane, cx| pane.cycle_sort(cx));
                    }
                    crate::palette::CommandId::TogglePane => {
                        if self.split {
                            self.focused = self.focused.other();
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    fn toggle_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.split = !self.split;
        if self.split {
            // Start the new pane where the user already is, ready as a copy target.
            let uri = self.left.read(cx).current_uri();
            self.right.update(cx, |pane, cx| pane.navigate_to(uri, cx));
        }
        self.focus_pane(PaneSide::Left, window, cx);
        self.save_layout(cx);
    }

    /// Copy/move need a destination pane; say so instead of silently doing nothing.
    fn require_split(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.split {
            self.banner = Some((
                tr("Open the second pane first (⌘\\) to choose a destination").to_string(),
                std::time::Instant::now(),
            ));
            cx.notify();
        }
        self.split
    }

    fn copy_to_other_pane(&mut self, cx: &mut Context<Self>) {
        self.send_to_other_pane(false, cx);
    }

    fn move_to_other_pane(&mut self, cx: &mut Context<Self>) {
        self.send_to_other_pane(true, cx);
    }

    /// Copy or move the focused pane's selection into the other pane's folder,
    /// whichever backends the two panes are showing.
    fn send_to_other_pane(&mut self, remove_source: bool, cx: &mut Context<Self>) {
        if !self.require_split(cx) {
            return;
        }
        let dir = self.other_pane().read(cx).current_uri();
        self.focused_pane().update(cx, |pane, cx| {
            if remove_source {
                pane.move_selection_to(&dir, cx)
            } else {
                pane.copy_selection_to(&dir, cx)
            }
        });
        // Remote folders have no watcher, so the destination would otherwise keep
        // showing its old contents.
        self.other_pane()
            .update(cx, |pane, cx| pane.reload_when_idle(cx));
    }

    fn open_palette(&mut self, mode: PaletteMode, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.focused_pane().read(cx).current_uri().as_local_path();
        self.palette
            .update(cx, |palette, cx| palette.show(mode, root, window, cx));
    }

    fn close_overlays(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() || self.info.take().is_some() {
            cx.notify();
            return;
        }
        if self.palette.read(cx).open {
            self.palette.update(cx, |palette, cx| palette.close(cx));
            return;
        }
        if self.prompt.is_some() {
            self.prompt = None;
            cx.notify();
        }
    }

    fn start_prompt(
        &mut self,
        kind: InlinePrompt,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| TextInput::new("name", cx));
        input.update(cx, |input, cx| input.set_content(initial, cx));
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle);
        let subscription =
            cx.subscribe(
                &input,
                |workspace, _, event: &TextInputEvent, cx| match event {
                    TextInputEvent::Submit(value) => workspace.commit_prompt(value.clone(), cx),
                    TextInputEvent::Cancel => {
                        workspace.prompt = None;
                        cx.notify();
                    }
                    TextInputEvent::Change(_) => {}
                },
            );
        self.subscriptions.push(subscription);
        self.prompt = Some((kind, input));
        cx.notify();
    }

    pub(crate) fn commit_prompt(&mut self, value: String, cx: &mut Context<Self>) {
        let Some((kind, _)) = self.prompt.take() else {
            return;
        };
        let value = value.trim().to_string();
        if value.is_empty() {
            cx.notify();
            return;
        }
        let pane = self.focused_pane();
        match kind {
            InlinePrompt::NewFolder => {
                pane.update(cx, |pane, cx| pane.create_dir(&value, cx));
            }
            InlinePrompt::NewFile => {
                pane.update(cx, |pane, cx| pane.create_file(&value, cx));
            }
            InlinePrompt::Rename => {
                pane.update(cx, |pane, cx| pane.rename_cursor(&value, cx));
            }
        }
        cx.notify();
    }
}

// ---------------------------------------------------------------------------
// actions
// ---------------------------------------------------------------------------

impl Workspace {
    fn on_move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.move_vertical(true, cx));
        self.sync_preview(cx);
    }

    fn on_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.move_vertical(false, cx));
        self.sync_preview(cx);
    }

    fn on_move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.move_horizontal(false, cx));
        self.sync_preview(cx);
    }

    fn on_move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.move_horizontal(true, cx));
        self.sync_preview(cx);
    }

    fn set_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.set_view(view, cx));
        cx.notify();
    }

    fn on_view_icons(&mut self, _: &ViewIcons, _: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewMode::Icons, cx);
    }

    fn on_toggle_hidden(&mut self, _: &ToggleHidden, w: &mut Window, cx: &mut Context<Self>) {
        self.run_menu_action(MenuAction::ToggleHidden, w, cx);
    }

    fn on_view_list(&mut self, _: &ViewList, _: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewMode::List, cx);
    }

    fn on_view_columns(&mut self, _: &ViewColumns, _: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewMode::Columns, cx);
    }

    fn on_page_down(&mut self, _: &MovePageDown, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.move_page(true, cx));
        self.sync_preview(cx);
    }

    fn on_page_up(&mut self, _: &MovePageUp, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.move_page(false, cx));
        self.sync_preview(cx);
    }

    fn on_jump_top(&mut self, _: &JumpTop, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.maybe_jump_top(cx));
        self.sync_preview(cx);
    }

    fn on_jump_bottom(&mut self, _: &JumpBottom, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| {
            pane.jump_to(crate::pane::JumpTarget::Bottom, cx)
        });
        self.sync_preview(cx);
    }

    fn on_go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.go_back(cx));
        self.sync_preview(cx);
    }

    fn on_go_forward(&mut self, _: &GoForward, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.go_forward(cx));
        self.sync_preview(cx);
    }

    fn on_go_parent(&mut self, _: &GoParent, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.go_parent(cx));
        self.sync_preview(cx);
    }

    fn on_open_entry(&mut self, _: &OpenEntry, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.activate_cursor(cx));
        self.sync_preview(cx);
    }

    fn on_focus_left(&mut self, _: &FocusLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(PaneSide::Left, window, cx);
    }

    fn on_focus_right(&mut self, _: &FocusRight, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(PaneSide::Right, window, cx);
    }

    fn on_switch_pane(&mut self, _: &SwitchPane, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_pane(window, cx);
    }

    fn on_extend_down(&mut self, _: &ExtendDown, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.extend_selection(1, cx));
        self.sync_preview(cx);
    }

    fn on_extend_up(&mut self, _: &ExtendUp, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.extend_selection(-1, cx));
        self.sync_preview(cx);
    }

    fn on_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.select_all(cx));
    }

    fn on_clear_selection(&mut self, _: &ClearSelection, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.clear_selection(cx));
    }

    fn on_new_tab(&mut self, _: &NewTab, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.new_tab(cx));
    }

    fn on_close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| {
            let index = pane.active_tab;
            pane.close_tab(index, cx);
        });
    }

    fn on_next_tab(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.cycle_tab(true, cx));
    }

    fn on_prev_tab(&mut self, _: &PrevTab, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.cycle_tab(false, cx));
    }

    fn on_toggle_split(&mut self, _: &ToggleSplit, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_split(window, cx);
    }

    fn on_open_selection(&mut self, _: &OpenSelection, w: &mut Window, cx: &mut Context<Self>) {
        self.run_menu_action(MenuAction::Open, w, cx);
    }

    fn on_copy_items(&mut self, _: &CopyItems, w: &mut Window, cx: &mut Context<Self>) {
        self.run_menu_action(MenuAction::Copy, w, cx);
    }

    fn on_paste_items(&mut self, _: &PasteItems, w: &mut Window, cx: &mut Context<Self>) {
        self.run_menu_action(MenuAction::Paste, w, cx);
    }

    fn on_cut_items(&mut self, _: &CutItems, w: &mut Window, cx: &mut Context<Self>) {
        self.run_menu_action(MenuAction::Cut, w, cx);
    }

    fn on_copy_as_path(&mut self, _: &CopyAsPath, w: &mut Window, cx: &mut Context<Self>) {
        self.run_menu_action(MenuAction::CopyPath, w, cx);
    }

    fn on_delete_items(&mut self, _: &DeleteItems, w: &mut Window, cx: &mut Context<Self>) {
        self.run_menu_action(MenuAction::Delete, w, cx);
    }

    fn on_properties(&mut self, _: &Properties, w: &mut Window, cx: &mut Context<Self>) {
        self.run_menu_action(MenuAction::Properties, w, cx);
    }

    fn on_start_filter(&mut self, _: &StartFilter, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.start_filter(window, cx));
    }

    fn on_open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(PaletteMode::Files, window, cx);
    }

    fn on_open_commands(&mut self, _: &OpenCommands, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(PaletteMode::Commands, window, cx);
    }

    fn on_toggle_preview(&mut self, _: &TogglePreview, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.toggle_preview(cx));
        // Opening it should show the current entry straight away.
        self.sync_preview(cx);
        self.save_layout(cx);
        cx.notify();
    }

    /// Drag a divider: the pointer's x position sets the width it controls.
    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(divider) = self.drag else {
            return;
        };
        // A button released outside the window never reaches `on_mouse_up`.
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        let x = f32::from(event.position.x);
        let window_width = f32::from(window.viewport_size().width);
        let inspector_open = self.focused_pane().read(cx).preview_open;
        match divider {
            Divider::Sidebar => {
                self.sidebar_width = sidebar_from_pointer(
                    x,
                    window_width,
                    self.inspector_width,
                    inspector_open,
                    self.split,
                );
            }
            Divider::Inspector => {
                self.inspector_width =
                    inspector_from_pointer(x, window_width, self.sidebar_width, self.split);
            }
            Divider::Panes => {
                let area = panes_area(
                    window_width,
                    self.sidebar_width,
                    self.inspector_width,
                    inspector_open,
                    true,
                );
                self.split_ratio = ratio_from_pointer(x, self.sidebar_width, area);
            }
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            // Save once the drag ends, not on every pointer move.
            self.save_layout(cx);
            cx.notify();
        }
    }

    fn on_copy_to_other(&mut self, _: &CopyToOtherPane, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_to_other_pane(cx);
    }

    fn on_move_to_other(&mut self, _: &MoveToOtherPane, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to_other_pane(cx);
    }

    fn on_trash(&mut self, _: &TrashSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_selection(cx);
    }

    /// Local items go to the Trash. Remote ones have no Trash, so they are deleted
    /// for good, straight away, like the local delete is.
    fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        let remote: Vec<VfsUri> = pane
            .read(cx)
            .selection_or_cursor()
            .into_iter()
            .map(|entry| entry.uri)
            .filter(|uri| !uri.scheme.is_local())
            .collect();
        if remote.is_empty() {
            pane.update(cx, |pane, cx| pane.trash_selection(cx));
        } else {
            pane.update(cx, |pane, cx| pane.delete_remote(remote, cx));
        }
    }

    fn on_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        self.start_prompt(
            InlinePrompt::NewFolder,
            "New Folder".to_string(),
            window,
            cx,
        );
    }

    fn on_rename(&mut self, _: &RenameEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.focused_pane().read(cx).cursor_entry().cloned() else {
            return;
        };
        self.start_prompt(InlinePrompt::Rename, entry.name.clone(), window, cx);
    }

    fn on_toggle_tasks(&mut self, _: &ToggleTasks, _: &mut Window, cx: &mut Context<Self>) {
        self.tasks_open = !self.tasks_open;
        self.tasks.update(cx, |panel, cx| panel.toggle(cx));
        self.save_layout(cx);
        cx.notify();
    }

    fn on_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.refresh(cx));
    }

    fn on_cycle_sort(&mut self, _: &CycleSort, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.cycle_sort(cx));
    }

    /// Esc is the universal "back out": overlay → filter → selection.
    fn on_close_overlay(&mut self, _: &CloseOverlay, _: &mut Window, cx: &mut Context<Self>) {
        if self.palette.read(cx).open
            || self.prompt.is_some()
            || self.menu.is_some()
            || self.info.is_some()
        {
            self.close_overlays(cx);
            return;
        }
        // Esc also cancels a pending Cut, like Explorer.
        if self.clipboard.is_cut() {
            self.set_clipboard(FileClipboard::default(), cx);
            return;
        }
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| {
            if pane.filter_active {
                pane.stop_filter(cx);
            } else if !pane.listing.selected.is_empty() {
                pane.clear_selection(cx);
            }
        });
    }

    fn on_quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    /// Route keys to the palette while it is open, before the global keymap runs.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let palette_open = self.palette.read(cx).open;
        let prompt_open = self.prompt.is_some();
        if !palette_open && !prompt_open {
            return;
        }
        let key = event.keystroke.key.as_str();
        match key {
            "escape" => {
                if palette_open {
                    self.palette.update(cx, |palette, cx| palette.close(cx));
                } else {
                    self.prompt = None;
                }
                cx.stop_propagation();
            }
            "down" | "up" if palette_open => {
                let delta = if key == "down" { 1 } else { -1 };
                self.palette
                    .update(cx, |palette, cx| palette.move_cursor(delta, cx));
                cx.stop_propagation();
            }
            "tab" if palette_open => {
                self.palette.update(cx, |palette, cx| palette.complete(cx));
                cx.stop_propagation();
            }
            "enter" if palette_open => {
                let choice = self.palette.read(cx).activated();
                self.palette.update(cx, |palette, cx| palette.close(cx));
                if let Some(choice) = choice {
                    self.run_palette_choice(choice, window, cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
        let _ = window;
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focused;
        let left_focused = focused == PaneSide::Left;
        let right_focused = focused == PaneSide::Right;

        let preview_open = self.focused_pane().read(cx).preview_open;
        let window_width = f32::from(window.viewport_size().width);
        let (left_width, right_width) = pane_widths(
            window_width,
            self.sidebar_width,
            self.inspector_width,
            preview_open,
            self.split,
            self.split_ratio,
        );
        let inspector_width = self.inspector_width;
        self.preview
            .update(cx, |preview, _| preview.width = inspector_width);
        let split = self.split;
        let left_pane = self.left.update(cx, |pane, cx| {
            pane.avail_width = left_width;
            pane.show_folder_name = split;
            pane.render_pane(left_focused, window, cx)
        });
        // The left pane takes its share as a fixed width; the right one flexes.
        let left = if split {
            div()
                .w(px(left_width))
                .flex_shrink_0()
                .h_full()
                .flex()
                .child(left_pane)
                .into_any_element()
        } else {
            left_pane
        };
        let right = split.then(|| {
            self.right.update(cx, |pane, cx| {
                pane.avail_width = right_width;
                pane.show_folder_name = true;
                pane.render_pane(right_focused, window, cx)
            })
        });
        let preview = preview_open.then(|| self.preview.clone());
        let palette_open = self.palette.read(cx).open;
        let prompt = self.prompt.as_ref().map(|(kind, input)| {
            let title = match kind {
                InlinePrompt::NewFolder => tr("New folder name").to_string(),
                InlinePrompt::NewFile => tr("New file name").to_string(),
                InlinePrompt::Rename => tr("Rename to").to_string(),
            };
            (title, input.clone())
        });
        let banner = self
            .banner
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(6))
            .map(|(message, _)| message.clone());
        let tasks = self.tasks.clone();
        let tasks_open = self.tasks_open;
        let palette = self.palette.clone();
        let sidebar = self.render_sidebar(cx);
        let menu = self.render_context_menu(window, cx);
        let info = self.render_info(cx);

        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_move_down))
            .on_action(cx.listener(Self::on_move_up))
            .on_action(cx.listener(Self::on_move_left))
            .on_action(cx.listener(Self::on_move_right))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_action(cx.listener(Self::on_view_icons))
            .on_action(cx.listener(Self::on_view_list))
            .on_action(cx.listener(Self::on_toggle_hidden))
            .on_action(cx.listener(Self::on_view_columns))
            .on_action(cx.listener(Self::on_page_down))
            .on_action(cx.listener(Self::on_page_up))
            .on_action(cx.listener(Self::on_jump_top))
            .on_action(cx.listener(Self::on_jump_bottom))
            .on_action(cx.listener(Self::on_go_back))
            .on_action(cx.listener(Self::on_go_forward))
            .on_action(cx.listener(Self::on_go_parent))
            .on_action(cx.listener(Self::on_open_entry))
            .on_action(cx.listener(Self::on_focus_left))
            .on_action(cx.listener(Self::on_focus_right))
            .on_action(cx.listener(Self::on_switch_pane))
            .on_action(cx.listener(Self::on_extend_down))
            .on_action(cx.listener(Self::on_extend_up))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_clear_selection))
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_toggle_split))
            .on_action(cx.listener(Self::on_open_selection))
            .on_action(cx.listener(Self::on_copy_items))
            .on_action(cx.listener(Self::on_paste_items))
            .on_action(cx.listener(Self::on_cut_items))
            .on_action(cx.listener(Self::on_copy_as_path))
            .on_action(cx.listener(Self::on_delete_items))
            .on_action(cx.listener(Self::on_properties))
            .on_action(cx.listener(Self::on_start_filter))
            .on_action(cx.listener(Self::on_open_palette))
            .on_action(cx.listener(Self::on_open_commands))
            .on_action(cx.listener(Self::on_toggle_preview))
            .on_action(cx.listener(Self::on_copy_to_other))
            .on_action(cx.listener(Self::on_move_to_other))
            .on_action(cx.listener(Self::on_trash))
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_rename))
            .on_action(cx.listener(Self::on_toggle_tasks))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_cycle_sort))
            .on_action(cx.listener(Self::on_close_overlay))
            .on_action(cx.listener(Self::on_quit))
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_row()
            .size_full()
            .bg(theme::bg())
            .text_color(theme::text())
            .child(sidebar)
            .child(resize_handle(
                "handle-sidebar",
                Divider::Sidebar,
                theme::bg(),
                theme::bg_pane(),
                cx,
            ))
            .child(
                // Everything right of the sidebar: toolbar, then the panes and the
                // inspector side by side, then the task panel.
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_toolbar(cx))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h_0()
                            .child(left)
                            .when_some(right, |this, right| {
                                this.child(resize_handle(
                                    "handle-panes",
                                    Divider::Panes,
                                    theme::bg_pane(),
                                    theme::bg_pane(),
                                    cx,
                                ))
                                .child(right)
                            })
                            .when_some(preview, |this, preview| {
                                this.child(resize_handle(
                                    "handle-inspector",
                                    Divider::Inspector,
                                    theme::bg_pane(),
                                    theme::bg(),
                                    cx,
                                ))
                                .child(preview)
                            }),
                    )
                    .when(tasks_open, |this| {
                        this.child(div().h(px(1.)).bg(theme::border())).child(tasks)
                    }),
            )
            .when_some(prompt, |this, (title, input)| {
                this.child(render_prompt(&title, input, cx))
            })
            .when(palette_open, |this| this.child(palette))
            .when_some(info, |this, info| this.child(info))
            // Last, so a menu opened over the inspector is drawn above it.
            .when_some(menu, |this, menu| this.child(menu))
            .when_some(banner, |this, message| this.child(render_banner(&message)))
    }
}

impl Workspace {
    fn render_info(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let info = self.info.as_ref()?;
        let rows = info_rows(&info.entry, info.folder_bytes);
        let name = info.entry.name.clone();
        let icon = crate::pane::entry_icon(&info.entry, 40.0);
        let entity = cx.entity();

        Some(
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .right(px(0.))
                .bottom(px(0.))
                .occlude()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(100.))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    entity.update(cx, |workspace, cx| {
                        workspace.info = None;
                        cx.notify();
                    });
                })
                .child(
                    div()
                        .w(px(380.))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .rounded_lg()
                        .bg(theme::bg_raised())
                        .border_1()
                        .border_color(theme::border())
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .child(icon)
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_color(theme::text())
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .truncate()
                                        .child(format!("{name} Properties")),
                                ),
                        )
                        .child(div().h(px(1.)).bg(theme::border_soft()))
                        .children(rows.into_iter().map(|(label, value)| {
                            div()
                                .flex()
                                .flex_row()
                                .gap_3()
                                .text_sm()
                                .child(
                                    div()
                                        .w(px(84.))
                                        .flex_shrink_0()
                                        .text_color(theme::text_faint())
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(theme::text())
                                        .child(value),
                                )
                        })),
                )
                .into_any_element(),
        )
    }

    /// The toolbar over the content area, laid out like Finder's: back/forward and
    /// the folder name on the left; view, sort, our own panel toggles, a "more"
    /// menu and search as round icon buttons on the right.
    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (title, view, sort, can_back, can_forward) = {
            let pane = self.focused_pane().read(cx);
            let uri = pane.current_uri();
            let name = uri.file_name().to_string();
            (
                if name.is_empty() {
                    "/".to_string()
                } else {
                    name
                },
                pane.view,
                pane.sort,
                pane.can_go_back(),
                pane.can_go_forward(),
            )
        };
        let preview_open = self.focused_pane().read(cx).preview_open;
        let active_tasks = self.tasks.read(cx).active_count();
        let entity = cx.entity();

        let view_icon = match view {
            ViewMode::Icons => icons::VIEW_ICONS,
            ViewMode::List => icons::VIEW_LIST,
            ViewMode::Columns => icons::VIEW_COLUMNS,
        };

        // Back / forward: one pill, two halves.
        let nav = {
            let back = entity.clone();
            let forward = entity.clone();
            pill()
                .child(
                    icon_button("nav-back", icons::CHEVRON_LEFT, can_back, false).on_click(
                        move |_, window, cx| {
                            back.update(cx, |workspace, cx| {
                                workspace.on_go_back(&GoBack, window, cx)
                            });
                        },
                    ),
                )
                .child(
                    icon_button("nav-forward", icons::CHEVRON_RIGHT, can_forward, false).on_click(
                        move |_, window, cx| {
                            forward.update(cx, |workspace, cx| {
                                workspace.on_go_forward(&GoForward, window, cx)
                            });
                        },
                    ),
                )
        };

        let view_menu_button = {
            let entity = entity.clone();
            dropdown_button("toolbar-view", view_icon).on_click(move |event, _, cx| {
                let at = event.position();
                entity.update(cx, |workspace, cx| {
                    workspace.open_toolbar_menu(ToolbarMenu::View, at, cx)
                });
            })
        };
        let sort_menu_button = {
            let entity = entity.clone();
            dropdown_button("toolbar-sort", icons::SORT).on_click(move |event, _, cx| {
                let at = event.position();
                entity.update(cx, |workspace, cx| {
                    workspace.open_toolbar_menu(ToolbarMenu::Sort, at, cx)
                });
            })
        };
        let _ = sort;

        // Our own switches: split view, preview panel, task panel.
        let panels = {
            let split = entity.clone();
            let preview = entity.clone();
            let tasks = entity.clone();
            pill()
                .child(
                    icon_button("toolbar-split", icons::SPLIT, true, self.split).on_click(
                        move |_, window, cx| {
                            split.update(cx, |workspace, cx| workspace.toggle_split(window, cx));
                        },
                    ),
                )
                .child(
                    icon_button("toolbar-preview", icons::EYE, true, preview_open).on_click(
                        move |_, window, cx| {
                            preview.update(cx, |workspace, cx| {
                                workspace.on_toggle_preview(&TogglePreview, window, cx)
                            });
                        },
                    ),
                )
                .child(
                    icon_button(
                        "toolbar-tasks",
                        icons::TASKS,
                        true,
                        self.tasks_open || active_tasks > 0,
                    )
                    .on_click(move |_, _, cx| {
                        tasks.update(cx, |workspace, cx| {
                            workspace.tasks_open = !workspace.tasks_open;
                            let panel = workspace.tasks.clone();
                            panel.update(cx, |panel, cx| panel.toggle(cx));
                            workspace.save_layout(cx);
                            cx.notify();
                        });
                    }),
                )
        };

        let more = {
            let entity = entity.clone();
            pill().child(
                icon_button("toolbar-more", icons::ELLIPSIS, true, false).on_click(
                    move |event, _, cx| {
                        let at = event.position();
                        entity.update(cx, |workspace, cx| {
                            workspace.open_toolbar_menu(ToolbarMenu::More, at, cx)
                        });
                    },
                ),
            )
        };

        let search = {
            let entity = entity.clone();
            pill().child(
                icon_button("toolbar-search", icons::SEARCH, true, false).on_click(
                    move |_, window, cx| {
                        entity.update(cx, |workspace, cx| {
                            workspace.open_palette(PaletteMode::Files, window, cx);
                        });
                    },
                ),
            )
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(52.0))
            .flex_shrink_0()
            .px_3()
            .bg(theme::bg_pane())
            .border_b_1()
            .border_color(theme::border_soft())
            .child(nav)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_base()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(title),
            )
            // Empty space drags the window, as the title bar would.
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(gpui::WindowControlArea::Drag),
            )
            .child(view_menu_button)
            .child(sort_menu_button)
            .child(panels)
            .child(more)
            .child(search)
            .into_any_element()
    }

    /// Open one of the toolbar's dropdown menus under the click.
    fn open_toolbar_menu(
        &mut self,
        which: ToolbarMenu,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let (view, sort, show_hidden, preview_open) = {
            let pane = self.focused_pane().read(cx);
            (
                pane.view,
                pane.sort,
                pane.listing.show_hidden,
                pane.preview_open,
            )
        };
        let items = match which {
            ToolbarMenu::View => view_menu(view),
            ToolbarMenu::Sort => sort_menu(sort),
            ToolbarMenu::More => more_menu(MoreState {
                show_hidden,
                split: self.split,
                preview_open,
                tasks_open: self.tasks_open,
            }),
        };
        self.menu_counter += 1;
        self.menu = Some(OpenMenu {
            id: self.menu_counter,
            // Hang the menu from the button's left side.
            origin: (f32::from(at.x) - 24.0, f32::from(at.y) + 18.0),
            items,
            open_sub: None,
            hovered: None,
            target: MenuTarget::Background,
            ctx: MenuContext::default(),
            apps: Vec::new(),
            open_with_only: false,
        });
        cx.notify();
    }

    /// Favorites list. Clicking navigates whichever pane has focus, which is
    /// the dual-pane analogue of Finder's single content area.
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let here = self.focused_pane().read(cx).current_uri();
        // 远程目录的路径可能和本地某个位置同名，只有本地目录才能高亮本地条目。
        let local_here = here.scheme.is_local();
        let current = here.path.clone();
        let mut favorites = self.places.clone();
        favorites.extend(favorite_rows(&self.favorites, &self.places));
        let sections = vec![
            (tr("Favorites"), favorites),
            (tr("Locations"), location_places()),
        ];

        // Room for the window's traffic lights, which sit in the sidebar.
        let mut column = div().flex().flex_col().gap_0p5().px_2().pt(px(46.));
        let mut index = 0;
        for (title, places) in sections {
            column = column.child(
                div()
                    .px_2()
                    .pt_3()
                    .pb_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text_faint())
                    .child(title),
            );
            for place in places {
                let active = local_here && !place.is_file && place_is_current(&place, &current);
                let target = place.path.clone();
                let is_file = place.is_file;
                let starred = place.starred;
                let entity = cx.entity();
                let menu_entity = cx.entity();
                let menu_path = place.path.clone();
                column = column.child(
                    sidebar_row(format!("place-{index}"), active, place.icon, place.label)
                        .on_mouse_down(MouseButton::Right, move |event, _, cx| {
                            let path = menu_path.clone();
                            let position = event.position;
                            menu_entity.update(cx, |workspace, cx| {
                                workspace.open_favorite_menu(path, !is_file, starred, position, cx)
                            });
                        })
                        .on_click(move |_, window, cx| {
                            let target = target.clone();
                            entity.update(cx, |workspace, cx| {
                                let side = workspace.focused;
                                workspace.pane(side).update(cx, |pane, cx| {
                                    if is_file {
                                        pane.reveal_file(&target, cx)
                                    } else {
                                        pane.navigate_to(VfsUri::local(&target), cx)
                                    }
                                });
                                workspace.focus_pane(side, window, cx);
                            });
                        }),
                );
                index += 1;
            }
        }

        if !self.connections.is_empty() {
            column = column.child(
                div()
                    .px_2()
                    .pt_3()
                    .pb_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text_faint())
                    .child(tr("Connections")),
            );
            let here = self.focused_pane().read(cx).current_uri();
            for connection in &self.connections {
                let id = connection.id;
                let active = here.connection_id.as_deref() == Some(&remote::uri_id(connection))
                    && !here.scheme.is_local();
                let entity = cx.entity();
                column = column.child(
                    sidebar_row(
                        format!("connection-{id}"),
                        active,
                        icons::DISK,
                        connection.name.clone(),
                    )
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |workspace, cx| {
                            workspace.open_connection(id, window, cx)
                        });
                    }),
                );
            }
        }

        div()
            .flex()
            .flex_col()
            .w(px(self.sidebar_width))
            .h_full()
            .flex_shrink_0()
            .bg(theme::bg())
            .child(column)
            .into_any_element()
    }
}

/// One clickable row in the sidebar: icon and label, highlighted when `active`.
fn sidebar_row(
    id: String,
    active: bool,
    icon: &'static str,
    label: String,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(26.))
        .px_2()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .when(active, |this| {
            this.bg(theme::bg_hover()).text_color(theme::text())
        })
        .when(!active, |this| {
            this.text_color(theme::text_muted())
                .hover(|this| this.bg(theme::bg_raised()).text_color(theme::text()))
        })
        .child(
            svg()
                .path(icon)
                .w(px(15.))
                .h(px(15.))
                .flex_shrink_0()
                .text_color(if active {
                    theme::accent()
                } else {
                    theme::text_faint()
                }),
        )
        .child(div().truncate().min_w_0().child(label))
}

/// A draggable divider between two regions.
///
/// The hit area is wider than the 1px line, but it must not look like a stripe of
/// foreign background: each side is therefore filled with the colour of the
/// region it borders (`left_bg` / `right_bg`), so the line sits exactly on the
/// seam and nothing appears to overflow.
fn resize_handle(
    id: &'static str,
    divider: Divider,
    left_bg: gpui::Hsla,
    right_bg: gpui::Hsla,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let entity = cx.entity();
    let side = (HANDLE_WIDTH - 1.0) / 2.0;
    div()
        .id(id)
        .group(id)
        .w(px(HANDLE_WIDTH))
        .h_full()
        .flex_shrink_0()
        .flex()
        .flex_row()
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            entity.update(cx, |workspace, cx| {
                workspace.drag = Some(divider);
                cx.notify();
            });
        })
        .child(div().w(px(side)).h_full().bg(left_bg))
        .child(
            div()
                .w(px(1.))
                .h_full()
                .bg(theme::border_soft())
                .group_hover(id, |this| this.bg(theme::accent())),
        )
        .child(div().w(px(side)).h_full().bg(right_bg))
        .into_any_element()
}

/// Which toolbar dropdown to open.
#[derive(Clone, Copy)]
enum ToolbarMenu {
    View,
    Sort,
    More,
}

/// A rounded capsule that groups toolbar buttons, as in Finder.
fn pill() -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(32.))
        .px_0p5()
        .gap_0p5()
        .rounded_full()
        .bg(theme::bg_raised())
        .border_1()
        .border_color(theme::border_soft())
}

/// A round icon button inside a pill. `enabled` greys it out; `active` tints it.
fn icon_button(
    id: &'static str,
    icon: &'static str,
    enabled: bool,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    let color = if !enabled {
        theme::text_faint()
    } else if active {
        theme::accent()
    } else {
        theme::text_muted()
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(30.))
        .h(px(28.))
        .rounded_full()
        .when(active, |this| this.bg(theme::bg_hover()))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(theme::bg_hover()))
        })
        .child(svg().path(icon).w(px(16.)).h(px(16.)).text_color(color))
}

/// A pill with an icon and a chevron: opens a dropdown menu.
fn dropdown_button(id: &'static str, icon: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .h(px(32.))
        .pl_3()
        .pr_2()
        .rounded_full()
        .bg(theme::bg_raised())
        .border_1()
        .border_color(theme::border_soft())
        .cursor_pointer()
        .hover(|this| this.bg(theme::bg_hover()))
        .child(
            svg()
                .path(icon)
                .w(px(16.))
                .h(px(16.))
                .text_color(theme::text_muted()),
        )
        .child(
            svg()
                .path(icons::CHEVRON_DOWN)
                .w(px(10.))
                .h(px(10.))
                .text_color(theme::text_faint()),
        )
}

fn render_prompt(title: &str, input: Entity<TextInput>, cx: &mut Context<Workspace>) -> AnyElement {
    let close_left = cx.entity();
    let close_right = cx.entity();
    // 与命令面板一致：整窗背板，点卡片外面取消，且不穿透到下面。
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .bottom(px(0.))
        .occlude()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(120.))
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
            close_left.update(cx, |workspace, cx| {
                workspace.prompt = None;
                cx.notify();
            });
        })
        .on_mouse_down(gpui::MouseButton::Right, move |_, _, cx| {
            close_right.update(cx, |workspace, cx| {
                workspace.prompt = None;
                cx.notify();
            });
        })
        .child(
            div()
                .w(px(420.))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_lg()
                .bg(theme::bg_raised())
                .border_1()
                .border_color(theme::border())
                .shadow_lg()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(26.))
                        .px_2()
                        .rounded_sm()
                        .bg(theme::bg())
                        .border_1()
                        .border_color(theme::accent_dim())
                        .child(input),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_faint())
                        .child(tr("enter to confirm · esc to cancel")),
                ),
        )
        .into_any_element()
}

fn render_banner(message: &str) -> AnyElement {
    div()
        .absolute()
        .bottom(px(40.))
        .left(px(0.))
        .right(px(0.))
        .flex()
        .justify_center()
        .child(
            div()
                .max_w(px(520.))
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme::red())
                .text_color(gpui::white())
                .text_xs()
                .child(message.to_string()),
        )
        .into_any_element()
}

/// Build the workspace entity.
pub fn new_workspace(persist: Persist, startup: SavedStartup, cx: &mut App) -> Entity<Workspace> {
    cx.new(|cx| Workspace::new(persist, startup, cx))
}

/// Kept so the module compiles if mouse handlers are added later.
fn _unused(_: MouseButton, _: &mut Window, _: &mut App, _: &Subscription) {}
