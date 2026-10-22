//! The workspace: two panes, the inspector, the task panel, the palette and the
//! status bar, plus every keyboard action that ties them together.
//!
//! All actions arrive here and are forwarded to the focused pane. That single
//! choke point is what keeps the keymap honest: the keymap binds actions, and this
//! file is the only place that decides what each one means.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, Render, Subscription, Window, div, prelude::*, px, svg,
};
mod menu_view;

use vfs_core::{FsWatcher, VfsUri, WatchEvent};

use crate::{
    actions::*,
    clipboard::FileClipboard,
    context_menu::{
        ActionItem, MENU_PADDING, MENU_WIDTH, MenuAction, MenuContext, MenuItem, MenuTarget,
        OpenWith, build_menu, clamp_origin, menu_height, open_with_menu, sort_menu,
    },
    icons,
    info::info_rows,
    listing::{Place, expand_home, favorite_places, location_places, place_is_current},
    open_with::{self, AppInfo},
    palette::{Palette, PaletteChoice, PaletteEvent, PaletteMode},
    pane::{Pane, PaneEvent, PaneSide, Services},
    preview::PreviewView,
    tasks_panel::TaskPanel,
    text_input::{TextInput, TextInputEvent},
    theme,
    view::{ViewMode, pane_width},
};

/// Files created by the inline "new folder / rename" prompt.
pub(crate) enum InlinePrompt {
    NewFolder,
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
    target: MenuTarget,
    ctx: MenuContext,
    /// Applications behind the Open With rows, in menu order.
    apps: Vec<AppInfo>,
    /// The menu is just the Open With list (Quick Look's Open button).
    open_with_only: bool,
}

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
    /// Quick Look: a large preview opened with Space.
    quick_look: Entity<PreviewView>,
    quick_look_open: bool,
    pub tasks: Entity<TaskPanel>,
    pub palette: Entity<Palette>,
    pub tasks_open: bool,
    /// Second pane visible. Off by default, like Finder; ⌘\ turns it on.
    pub split: bool,
    /// Paths copied with Copy / "Copy Item", pasted by Paste Item.
    clipboard: FileClipboard,
    menu: Option<OpenMenu>,
    menu_counter: u64,
    /// Open With applications of the menu that was just closed by a click.
    menu_apps: Vec<AppInfo>,
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
    pub fn new(cx: &mut Context<Self>) -> Self {
        let services = Services::new();

        let home_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let home = home_dir.clone();
        let start = std::env::args()
            .nth(1)
            .map(|arg| expand_home(&arg))
            .filter(|path| path.is_dir())
            .unwrap_or(home);

        let left =
            cx.new(|cx| Pane::new(PaneSide::Left, VfsUri::local(&start), services.clone(), cx));
        let right =
            cx.new(|cx| Pane::new(PaneSide::Right, VfsUri::local(&start), services.clone(), cx));

        let preview = cx.new(PreviewView::new);
        let quick_look = cx.new(PreviewView::quick_look);
        let tasks = cx.new(|cx| TaskPanel::new(services.tasks.clone(), cx));
        let palette = cx.new(Palette::new);

        let mut workspace = Self {
            left: left.clone(),
            right: right.clone(),
            focused: PaneSide::Left,
            preview,
            quick_look,
            quick_look_open: false,
            tasks,
            palette: palette.clone(),
            tasks_open: false,
            split: false,
            clipboard: FileClipboard::default(),
            menu: None,
            menu_counter: 0,
            menu_apps: Vec::new(),
            home: home_dir.clone(),
            info: None,
            places: favorite_places(&home_dir),
            prompt: None,
            focus_handle: cx.focus_handle(),
            watcher: None,
            watched_dir: None,
            subscriptions: Vec::new(),
            banner: None,
        };

        workspace.subscriptions = workspace.wire(cx);
        workspace
    }

    /// Load both panes and start watching the focused one.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                        PaneEvent::OpenExternally(uri) => {
                            if let Some(path) = uri.as_local_path() {
                                let _ = open::that_detached(path);
                            }
                        }
                        PaneEvent::OpenTerminal(dir) => {
                            let _ = open::that_detached(dir);
                        }
                        PaneEvent::ContextMenu { position, on_entry } => {
                            workspace.open_context_menu(&pane, *position, *on_entry, cx);
                        }
                        PaneEvent::Notice(message) => {
                            workspace.banner = Some((message.clone(), std::time::Instant::now()));
                        }
                    }
                    cx.notify();
                }),
            );
        }

        // Palette → workspace: run the chosen row.
        subscriptions.push(cx.subscribe(
            &self.palette,
            |workspace, _, event: &PaletteEvent, cx| {
                let PaletteEvent::Chosen(choice) = event;
                workspace.run_palette_choice(choice.clone(), cx);
            },
        ));

        subscriptions
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
                is_dir: selection.len() == 1 && first.is_dir(),
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
        let ctx = MenuContext {
            can_paste: !self.clipboard.is_empty(),
            view,
            show_hidden,
            sort,
            open_with: OpenWith::Loading,
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
            target,
            ctx,
            apps: Vec::new(),
            open_with_only: false,
        });
        cx.notify();
        if wants_apps {
            let first = selection.iter().find_map(|entry| entry.uri.as_local_path());
            if let Some(path) = first {
                self.load_open_with(id, path, cx);
            }
        }
    }

    /// Ask LaunchServices which apps can open `path`, off the UI thread, and fill
    /// the Open With submenu when the answer lands (if that menu is still open).
    fn load_open_with(&mut self, id: u64, path: PathBuf, cx: &mut Context<Self>) {
        let home = self.home.clone();
        cx.spawn(async move |this, cx| {
            let apps = cx
                .background_executor()
                .spawn(async move { open_with::list_apps(&path, &home) })
                .await
                .unwrap_or_default();
            this.update(cx, |workspace, cx| {
                let Some(menu) = workspace.menu.as_mut().filter(|menu| menu.id == id) else {
                    return;
                };
                menu.ctx.open_with =
                    OpenWith::Apps(apps.iter().map(|app| app.name.clone()).collect());
                menu.apps = apps;
                menu.items = if menu.open_with_only {
                    open_with_menu(&menu.ctx.open_with)
                } else {
                    build_menu(&menu.target, &menu.ctx)
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The toolbar's "Sort By" menu, dropped just below the click.
    fn open_sort_menu(&mut self, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let spec = self.focused_pane().read(cx).sort;
        self.menu_counter += 1;
        self.menu = Some(OpenMenu {
            id: self.menu_counter,
            origin: (f32::from(at.x), f32::from(at.y) + 14.0),
            items: sort_menu(spec),
            open_sub: None,
            target: MenuTarget::Background,
            ctx: MenuContext::default(),
            apps: Vec::new(),
            open_with_only: false,
        });
        cx.notify();
    }

    /// Quick Look's Open button: the same application list as the context menu's
    /// "Open With", dropped under the button.
    fn open_quick_look_apps(&mut self, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let Some(entry) = self.focused_pane().read(cx).cursor_entry().cloned() else {
            return;
        };
        let Some(path) = entry.uri.as_local_path() else {
            return;
        };
        let ctx = MenuContext::default();
        self.menu_counter += 1;
        let id = self.menu_counter;
        self.menu = Some(OpenMenu {
            id,
            origin: (f32::from(at.x) - MENU_WIDTH + 24.0, f32::from(at.y) + 14.0),
            items: open_with_menu(&ctx.open_with),
            open_sub: None,
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
        if let Err(message) = open_with::open_with(&app.path, &files) {
            self.banner = Some((message, std::time::Instant::now()));
        }
    }

    fn open_with_files(&self, cx: &mut Context<Self>) -> Vec<PathBuf> {
        self.focused_pane()
            .read(cx)
            .selection_or_cursor()
            .iter()
            .filter(|entry| !entry.is_dir())
            .filter_map(|entry| entry.uri.as_local_path())
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
            if let Err(message) = open_with::open_with(&app, &files) {
                this.update(cx, |workspace, cx| {
                    workspace.banner = Some((message, std::time::Instant::now()));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn show_tasks(&mut self, cx: &mut Context<Self>) {
        self.tasks_open = true;
        self.tasks.update(cx, |panel, cx| {
            if !panel.open {
                panel.toggle(cx);
            }
        });
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
        self.show_tasks(cx);
    }

    /// One entry point for the menu rows and their keyboard shortcuts, so both
    /// always do the same thing.
    fn run_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        match action {
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
                    Self::open_terminal_at(&dir);
                }
            }
            MenuAction::Preview => {
                if !self.quick_look_open {
                    self.toggle_quick_look(cx);
                }
            }
            MenuAction::Compress(format) => {
                pane.update(cx, |pane, cx| pane.compress_selection(format, cx))
            }
            MenuAction::ExtractHere => pane.update(cx, |pane, cx| pane.extract_selection(true, cx)),
            MenuAction::ExtractTo => pane.update(cx, |pane, cx| pane.extract_selection(false, cx)),
            MenuAction::CopyPath => {
                let paths = pane.read(cx).selection_paths();
                Self::copy_text_to_clipboard(&paths, cx);
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
                pane.update(cx, |pane, cx| pane.trash_selection(cx));
                self.show_tasks(cx);
            }
            MenuAction::Rename => {
                if let Some(entry) = pane.read(cx).cursor_entry().cloned() {
                    self.start_prompt(InlinePrompt::Rename, entry.name.clone(), window, cx);
                }
            }
            MenuAction::Properties => self.show_info(cx),
            MenuAction::NewFolder => self.start_prompt(
                InlinePrompt::NewFolder,
                "New folder".to_string(),
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
            MenuAction::ToggleHidden => {
                let show = !pane.read(cx).listing.show_hidden;
                pane.update(cx, |pane, cx| pane.set_show_hidden(show, cx));
            }
            MenuAction::Refresh => pane.update(cx, |pane, cx| pane.refresh(cx)),
        }
        cx.notify();
    }

    /// Open Terminal in `dir`. macOS only for now; elsewhere there is no single
    /// "the terminal" to ask for, so this is a no-op rather than a wrong guess.
    fn open_terminal_at(dir: &std::path::Path) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .args(["-a", "Terminal"])
                .arg(dir)
                .spawn();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = dir;
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

    /// Mirror the focused pane's cursor entry into the inspector.
    fn sync_preview(&mut self, cx: &mut Context<Self>) {
        let entry = self.focused_pane().read(cx).cursor_entry().cloned();
        if self.quick_look_open {
            let entry = entry.clone();
            self.quick_look
                .update(cx, |preview, cx| preview.show(entry, cx));
        }
        self.preview
            .update(cx, |preview, cx| preview.show(entry, cx));
    }

    /// Space: open Quick Look on the cursor entry, or close it.
    fn toggle_quick_look(&mut self, cx: &mut Context<Self>) {
        if self.quick_look_open {
            self.quick_look_open = false;
            cx.notify();
            return;
        }
        if self.focused_pane().read(cx).cursor_entry().is_none() {
            return;
        }
        self.quick_look_open = true;
        self.sync_preview(cx);
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

    fn run_palette_choice(&mut self, choice: PaletteChoice, cx: &mut Context<Self>) {
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
                        pane.update(cx, |pane, cx| pane.trash_selection(cx));
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
    }

    /// Copy/move need a destination pane; say so instead of silently doing nothing.
    fn require_split(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.split {
            self.banner = Some((
                "Open the second pane first (⌘\\) to choose a destination".to_string(),
                std::time::Instant::now(),
            ));
            cx.notify();
        }
        self.split
    }

    fn copy_to_other_pane(&mut self, cx: &mut Context<Self>) {
        if !self.require_split(cx) {
            return;
        }
        let other_dir = self.other_pane().read(cx).current_uri().as_local_path();
        let Some(dir) = other_dir else {
            self.banner = Some((
                "Copying into a remote backend arrives with the SFTP milestone".to_string(),
                std::time::Instant::now(),
            ));
            return;
        };
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.copy_selection_to(&dir, cx));
        self.tasks_open = true;
        self.tasks.update(cx, |panel, cx| {
            if !panel.open {
                panel.toggle(cx);
            }
        });
    }

    fn move_to_other_pane(&mut self, cx: &mut Context<Self>) {
        if !self.require_split(cx) {
            return;
        }
        let other_dir = self.other_pane().read(cx).current_uri().as_local_path();
        let Some(dir) = other_dir else {
            return;
        };
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.move_selection_to(&dir, cx));
        self.tasks_open = true;
        self.tasks.update(cx, |panel, cx| {
            if !panel.open {
                panel.toggle(cx);
            }
        });
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
        if self.quick_look_open {
            self.quick_look_open = false;
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

    fn on_quick_look(&mut self, _: &QuickLook, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_quick_look(cx);
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
    }

    fn on_copy_to_other(&mut self, _: &CopyToOtherPane, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_to_other_pane(cx);
    }

    fn on_move_to_other(&mut self, _: &MoveToOtherPane, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to_other_pane(cx);
    }

    fn on_trash(&mut self, _: &TrashSelection, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.trash_selection(cx));
        self.tasks_open = true;
        self.tasks.update(cx, |panel, cx| {
            if !panel.open {
                panel.toggle(cx);
            }
        });
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
            || self.quick_look_open
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
            "enter" if palette_open => {
                let choice = self.palette.read(cx).activated();
                self.palette.update(cx, |palette, cx| palette.close(cx));
                if let Some(choice) = choice {
                    self.run_palette_choice(choice, cx);
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

        let inspector = if self.focused_pane().read(cx).preview_open {
            340.0
        } else {
            0.0
        };
        let width = pane_width(
            f32::from(window.viewport_size().width),
            theme::SIDEBAR_WIDTH,
            inspector,
            self.split,
        );
        let left = self.left.update(cx, |pane, cx| {
            pane.avail_width = width;
            pane.render_pane(left_focused, window, cx)
        });
        let split = self.split;
        let right = split.then(|| {
            self.right.update(cx, |pane, cx| {
                pane.avail_width = width;
                pane.render_pane(right_focused, window, cx)
            })
        });
        let preview_open = self.focused_pane().read(cx).preview_open;
        let preview = preview_open.then(|| self.preview.clone());
        let palette_open = self.palette.read(cx).open;
        let prompt = self.prompt.as_ref().map(|(kind, input)| {
            let title = match kind {
                InlinePrompt::NewFolder => "New folder name",
                InlinePrompt::Rename => "Rename to",
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
        let quick_look = self.render_quick_look(window, cx);

        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_move_down))
            .on_action(cx.listener(Self::on_move_up))
            .on_action(cx.listener(Self::on_move_left))
            .on_action(cx.listener(Self::on_move_right))
            .on_action(cx.listener(Self::on_quick_look))
            .on_action(cx.listener(Self::on_view_icons))
            .on_action(cx.listener(Self::on_view_list))
            .on_action(cx.listener(Self::on_toggle_hidden))
            .on_action(cx.listener(Self::on_view_columns))
            .on_action(cx.listener(Self::on_page_down))
            .on_action(cx.listener(Self::on_page_up))
            .on_action(cx.listener(Self::on_jump_top))
            .on_action(cx.listener(Self::on_jump_bottom))
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
            .flex_col()
            .size_full()
            .bg(theme::bg())
            .text_color(theme::text())
            .child(self.render_title_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
                    .child(left)
                    .when_some(right, |this, right| {
                        this.child(div().w(px(1.)).flex_shrink_0().bg(theme::border()))
                            .child(right)
                    })
                    .children(preview.map(|preview| preview.into_any_element())),
            )
            .when(tasks_open, |this| {
                this.child(div().h(px(1.)).bg(theme::border())).child(tasks)
            })
            .when_some(prompt, |this, (title, input)| {
                this.child(render_prompt(title, input, cx))
            })
            .when(palette_open, |this| this.child(palette))
            .when_some(info, |this, info| this.child(info))
            .when_some(quick_look, |this, ql| this.child(ql))
            // Last, so a menu opened from Quick Look is drawn above it.
            .when_some(menu, |this, menu| this.child(menu))
            .when_some(banner, |this, message| this.child(render_banner(&message)))
    }
}

impl Workspace {
    /// Quick Look: a floating window in Finder's style (title bar with a close
    /// button, the file's name and kind, an Open button) over the dimmed list.
    ///
    /// Every child of the rounded card is transparent. GPUI clips overflow to a
    /// rectangle, so any opaque child would poke out of the rounded corners.
    fn render_quick_look(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.quick_look_open {
            return None;
        }
        let entry = self.focused_pane().read(cx).cursor_entry().cloned()?;
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) * 0.7).clamp(420.0, 980.0);
        let height = (f32::from(viewport.height) * 0.78).max(320.0);
        let preview = self.quick_look.clone();
        let close = cx.entity();
        let backdrop = cx.entity();
        let open = cx.entity();
        let apps = cx.entity();
        let subtitle = {
            let kind = crate::info::kind_label(&entry);
            if entry.is_dir() {
                kind
            } else {
                format!("{kind} · {}", vfs_core::entry::format_size(entry.size))
            }
        };
        let icon = crate::pane::entry_icon(&entry, 22.0);

        Some(
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .right(px(0.))
                .bottom(px(0.))
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0.0, 0.0, 0.0, 0.32))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    backdrop.update(cx, |workspace, cx| {
                        workspace.quick_look_open = false;
                        cx.notify();
                    });
                })
                .child(
                    div()
                        .w(px(width))
                        .h(px(height))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(theme::border())
                        .shadow_lg()
                        .bg(theme::bg())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            // Title bar: transparent, separated by a hairline. The
                            // name sits in the middle; Open and close are on the right.
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .h(px(46.))
                                .flex_shrink_0()
                                .px_3()
                                .gap_3()
                                .border_b_1()
                                .border_color(theme::border_soft())
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(icon)
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                                        .text_color(theme::text())
                                                        .truncate()
                                                        .child(entry.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme::text_faint())
                                                        .truncate()
                                                        .child(subtitle),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_end()
                                        .gap_2()
                                        // Split button: the label opens with the default
                                        // app, the arrow lists every application.
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .h(px(24.))
                                                .rounded_md()
                                                .bg(theme::bg_hover())
                                                .text_sm()
                                                .text_color(theme::text())
                                                .child(
                                                    div()
                                                        .id("quick-look-open")
                                                        .h_full()
                                                        .px_3()
                                                        .flex()
                                                        .items_center()
                                                        .rounded_l_md()
                                                        .cursor_pointer()
                                                        .hover(|this| this.bg(theme::border()))
                                                        .on_click(move |_, window, cx| {
                                                            open.update(cx, |workspace, cx| {
                                                                workspace.quick_look_open = false;
                                                                workspace.run_menu_action(
                                                                    MenuAction::Open,
                                                                    window,
                                                                    cx,
                                                                );
                                                            });
                                                        })
                                                        .child("Open"),
                                                )
                                                .child(
                                                    div().w(px(1.)).h(px(14.)).bg(theme::border()),
                                                )
                                                .child(
                                                    div()
                                                        .id("quick-look-open-with")
                                                        .h_full()
                                                        .w(px(24.))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .rounded_r_md()
                                                        .cursor_pointer()
                                                        .hover(|this| this.bg(theme::border()))
                                                        .on_click(move |event, _, cx| {
                                                            let at = event.position();
                                                            apps.update(cx, |workspace, cx| {
                                                                workspace
                                                                    .open_quick_look_apps(at, cx)
                                                            });
                                                        })
                                                        .child(
                                                            svg()
                                                                .path(icons::CHEVRON_DOWN)
                                                                .w(px(10.))
                                                                .h(px(10.))
                                                                .text_color(theme::text_muted()),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id("quick-look-close")
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .w(px(24.))
                                                .h(px(24.))
                                                .flex_shrink_0()
                                                .rounded_md()
                                                .cursor_pointer()
                                                .hover(|this| this.bg(theme::bg_hover()))
                                                .on_click(move |_, _, cx| {
                                                    close.update(cx, |workspace, cx| {
                                                        workspace.quick_look_open = false;
                                                        cx.notify();
                                                    });
                                                })
                                                .child(
                                                    svg()
                                                        .path(icons::CLOSE)
                                                        .w(px(12.))
                                                        .h(px(12.))
                                                        .text_color(theme::text_muted()),
                                                ),
                                        ),
                                ),
                        )
                        .child(div().flex_1().min_h_0().child(preview)),
                )
                .into_any_element(),
        )
    }

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

    /// Unified toolbar, as in Finder: window controls on the left, one search
    /// field in the middle, view toggles on the right. The path lives in the
    /// panes' breadcrumbs, so it is not repeated here.
    fn render_title_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let active_tasks = self.tasks.read(cx).active_count();
        let tasks_open = self.tasks_open;
        let preview_open = self.focused_pane().read(cx).preview_open;
        let entity = cx.entity();

        let search = {
            let entity = entity.clone();
            div()
                .id("titlebar-palette")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w(px(300.))
                .h(px(24.))
                .px_2()
                .rounded_md()
                .bg(theme::bg_raised())
                .text_color(theme::text_faint())
                .hover(|this| this.bg(theme::bg_hover()).text_color(theme::text_muted()))
                .on_click(move |_, window, cx| {
                    entity.update(cx, |workspace, cx| {
                        workspace.open_palette(PaletteMode::Files, window, cx);
                    });
                })
                .child(
                    svg()
                        .path(icons::SEARCH)
                        .w(px(12.))
                        .h(px(12.))
                        .text_color(theme::text_faint()),
                )
                .child(div().flex_1().text_sm().child("Search"))
                .child(div().text_xs().child("⌘P"))
        };

        let preview_button = {
            let entity = entity.clone();
            toolbar_button(
                "titlebar-preview",
                icons::EYE,
                preview_open,
                theme::accent(),
            )
            .on_click(move |_, _, cx| {
                entity.update(cx, |workspace, cx| {
                    let pane = workspace.focused_pane();
                    pane.update(cx, |pane, cx| pane.toggle_preview(cx));
                    cx.notify();
                });
            })
        };

        let current_view = self.focused_pane().read(cx).view;
        let view_buttons = {
            let mut group = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_0p5()
                .p_0p5()
                .rounded_md()
                .bg(theme::bg_raised());
            for (mode, icon) in [
                (ViewMode::Icons, icons::VIEW_ICONS),
                (ViewMode::List, icons::VIEW_LIST),
                (ViewMode::Columns, icons::VIEW_COLUMNS),
            ] {
                let entity = entity.clone();
                let active = current_view == mode;
                group = group.child(
                    toolbar_button(
                        match mode {
                            ViewMode::Icons => "view-icons",
                            ViewMode::List => "view-list",
                            ViewMode::Columns => "view-columns",
                        },
                        icon,
                        false,
                        theme::text(),
                    )
                    .when(active, |this| this.bg(theme::bg_hover()))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |workspace, cx| workspace.set_view(mode, cx));
                    }),
                );
            }
            group
        };

        let sort_button = {
            let entity = entity.clone();
            toolbar_button("titlebar-sort", icons::SORT, false, theme::text()).on_click(
                move |event, _, cx| {
                    let at = event.position();
                    entity.update(cx, |workspace, cx| workspace.open_sort_menu(at, cx));
                },
            )
        };

        let split_button = {
            let entity = entity.clone();
            toolbar_button("titlebar-split", icons::SPLIT, self.split, theme::accent()).on_click(
                move |_, window, cx| {
                    entity.update(cx, |workspace, cx| workspace.toggle_split(window, cx));
                },
            )
        };

        let tasks_button = {
            let entity = entity.clone();
            toolbar_button(
                "titlebar-tasks",
                icons::TASKS,
                tasks_open || active_tasks > 0,
                theme::accent(),
            )
            .on_click(move |_, _, cx| {
                entity.update(cx, |workspace, cx| {
                    workspace.tasks_open = !workspace.tasks_open;
                    let tasks = workspace.tasks.clone();
                    tasks.update(cx, |panel, cx| panel.toggle(cx));
                    cx.notify();
                });
            })
            .when(active_tasks > 0, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme::accent())
                        .child(active_tasks.to_string()),
                )
            })
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(38.0))
            .flex_shrink_0()
            .pl(px(84.0))
            .pr_3()
            .bg(theme::bg())
            .border_b_1()
            .border_color(theme::border_soft())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(view_buttons)
                    .child(sort_button),
            )
            .child(search)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .child(split_button)
                    .child(preview_button)
                    .child(tasks_button),
            )
            .into_any_element()
    }

    /// Favorites list. Clicking navigates whichever pane has focus, which is
    /// the dual-pane analogue of Finder's single content area.
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.focused_pane().read(cx).current_uri().path.clone();
        let sections = [
            ("Favorites", self.places.clone()),
            ("Locations", location_places()),
        ];

        let mut column = div().flex().flex_col().gap_0p5().px_2();
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
                let active = place_is_current(&place, &current);
                let target = place.path.clone();
                let entity = cx.entity();
                column = column.child(
                    div()
                        .id(gpui::ElementId::Name(format!("place-{index}").into()))
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
                        .on_click(move |_, window, cx| {
                            let target = target.clone();
                            entity.update(cx, |workspace, cx| {
                                let side = workspace.focused;
                                workspace.pane(side).update(cx, |pane, cx| {
                                    pane.navigate_to(VfsUri::local(&target), cx)
                                });
                                workspace.focus_pane(side, window, cx);
                            });
                        })
                        .child(
                            svg()
                                .path(place.icon)
                                .w(px(15.))
                                .h(px(15.))
                                .flex_shrink_0()
                                .text_color(if active {
                                    theme::accent()
                                } else {
                                    theme::text_faint()
                                }),
                        )
                        .child(div().truncate().min_w_0().child(place.label)),
                );
                index += 1;
            }
        }

        div()
            .flex()
            .flex_col()
            .w(px(theme::SIDEBAR_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(theme::bg())
            .border_r_1()
            .border_color(theme::border_soft())
            .child(column)
            .into_any_element()
    }
}

/// A square icon button for the toolbar; the caller attaches `on_click`.
fn toolbar_button(
    id: &'static str,
    icon: &'static str,
    active: bool,
    active_color: gpui::Hsla,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_1()
        .h(px(24.))
        .min_w(px(28.))
        .px_1()
        .rounded_md()
        .when(active, |this| this.bg(theme::bg_raised()))
        .text_color(if active {
            active_color
        } else {
            theme::text_muted()
        })
        .hover(|this| this.bg(theme::bg_hover()).text_color(theme::text()))
        .child(
            svg()
                .path(icon)
                .w(px(14.))
                .h(px(14.))
                .text_color(if active {
                    active_color
                } else {
                    theme::text_muted()
                }),
        )
}

fn render_prompt(
    title: &str,
    input: Entity<TextInput>,
    _cx: &mut Context<Workspace>,
) -> AnyElement {
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .bottom(px(0.))
        .flex()
        .items_start()
        .justify_center()
        .pt(px(120.))
        .child(
            div()
                .w(px(420.))
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
                        .child("enter to confirm · esc to cancel"),
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
pub fn new_workspace(cx: &mut App) -> Entity<Workspace> {
    cx.new(Workspace::new)
}

/// Kept so the module compiles if mouse handlers are added later.
fn _unused(_: MouseButton, _: &mut Window, _: &mut App, _: &Subscription) {}
