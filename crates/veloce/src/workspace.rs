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
use vfs_core::{FsWatcher, VfsUri, WatchEvent};

use crate::{
    actions::*,
    file_icons, icons,
    listing::{Place, expand_home, favorite_places, place_is_current},
    palette::{Palette, PaletteChoice, PaletteEvent, PaletteMode},
    pane::{Pane, PaneEvent, PaneSide, Services},
    preview::PreviewView,
    tasks_panel::TaskPanel,
    text_input::{TextInput, TextInputEvent},
    theme,
    tree::{TreeState, list_subdirs},
};

/// Files created by the inline "new folder / rename" prompt.
pub(crate) enum InlinePrompt {
    NewFolder,
    Rename,
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
    /// Finder-style shortcuts down the left edge.
    places: Vec<Place>,
    /// Lazy folder tree under the favorites, rooted at home.
    tree: TreeState,
    /// Folder the tree was last opened down to, so a refresh of the same
    /// folder does not re-expand what the user collapsed.
    revealed: Option<PathBuf>,
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
        let tasks = cx.new(|cx| TaskPanel::new(services.tasks.clone(), cx));
        let palette = cx.new(Palette::new);

        let mut workspace = Self {
            left: left.clone(),
            right: right.clone(),
            focused: PaneSide::Left,
            preview,
            tasks,
            palette: palette.clone(),
            tasks_open: false,
            split: false,
            places: favorite_places(&home_dir),
            tree: TreeState::new(home_dir.clone()),
            revealed: None,
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
        self.reveal_in_tree(cx);
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
                            workspace.reveal_in_tree(cx);
                        }
                        PaneEvent::OpenExternally(uri) => {
                            if let Some(path) = uri.as_local_path() {
                                let _ = open::that_detached(path);
                            }
                        }
                        PaneEvent::OpenTerminal(dir) => {
                            let _ = open::that_detached(dir);
                        }
                    }
                    let _ = pane;
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

    /// Read one folder's sub-directories off the UI thread and cache them.
    fn load_tree_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let target = dir.clone();
            let result = cx
                .background_executor()
                .spawn(async move { list_subdirs(&target) })
                .await;
            this.update(cx, |workspace, cx| {
                // An unreadable folder just shows no children rather than an error.
                workspace.tree.set_children(dir, result.unwrap_or_default());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open the tree down to the focused pane's folder so the user can see where they are.
    fn reveal_in_tree(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.focused_pane().read(cx).current_uri().as_local_path() else {
            return;
        };
        if self.revealed.as_ref() == Some(&path) {
            return;
        }
        self.revealed = Some(path.clone());
        for dir in self.tree.reveal(&path) {
            self.load_tree_dir(dir, cx);
        }
    }

    fn toggle_tree_dir(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.tree.toggle(&path) {
            self.load_tree_dir(path, cx);
        }
        cx.notify();
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
        self.preview
            .update(cx, |preview, cx| preview.show(entry, cx));
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
                                    pane.update(cx, |pane, cx| pane.reload(cx));
                                    let other = workspace.pane(side.other());
                                    let other_dir = other.read(cx).current_uri().as_local_path();
                                    if other_dir.as_deref() == workspace.watched_dir.as_deref() {
                                        other.update(cx, |pane, cx| pane.reload(cx));
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
                        pane.update(cx, |pane, cx| pane.reload(cx));
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
    fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.move_cursor(delta, extend, cx));
        self.sync_preview(cx);
    }

    fn on_move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(1, false, cx);
    }

    fn on_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(-1, false, cx);
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
        pane.update(cx, |pane, cx| pane.reload(cx));
    }

    fn on_cycle_sort(&mut self, _: &CycleSort, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        pane.update(cx, |pane, cx| pane.cycle_sort(cx));
    }

    /// Esc is the universal "back out": overlay → filter → selection.
    fn on_close_overlay(&mut self, _: &CloseOverlay, _: &mut Window, cx: &mut Context<Self>) {
        if self.palette.read(cx).open || self.prompt.is_some() {
            self.close_overlays(cx);
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

        let left = self
            .left
            .update(cx, |pane, cx| pane.render_pane(left_focused, window, cx));
        let split = self.split;
        let right = split.then(|| {
            self.right
                .update(cx, |pane, cx| pane.render_pane(right_focused, window, cx))
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

        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_move_down))
            .on_action(cx.listener(Self::on_move_up))
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
            .when_some(banner, |this, message| this.child(render_banner(&message)))
    }
}

impl Workspace {
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
            .child(div().flex_1())
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
        let mut list = div().flex().flex_col().gap_0p5().px_2().child(
            div()
                .px_2()
                .pb_1()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text_faint())
                .child("Favorites"),
        );

        for (index, place) in self.places.iter().enumerate() {
            let active = place_is_current(place, &current);
            let target = place.path.clone();
            let entity = cx.entity();
            list = list.child(
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
        }

        let mut tree = div().flex().flex_col().gap_0p5().px_2().pb_2().child(
            div()
                .px_2()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text_faint())
                .child("Folders"),
        );
        for (index, row) in self.tree.rows().into_iter().enumerate() {
            let active =
                row.path.to_string_lossy().trim_end_matches('/') == current.trim_end_matches('/');
            let toggle_entity = cx.entity();
            let open_entity = cx.entity();
            let toggle_path = row.path.clone();
            let open_path = row.path.clone();
            let icon_id = file_icons::folder_icon_id(&row.name, row.expanded);
            tree = tree.child(
                div()
                    .id(gpui::ElementId::Name(format!("tree-{index}").into()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(24.))
                    .pl(px(4. + row.depth as f32 * 12.))
                    .pr_2()
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
                        let target = open_path.clone();
                        open_entity.update(cx, |workspace, cx| {
                            let side = workspace.focused;
                            workspace.pane(side).update(cx, |pane, cx| {
                                pane.navigate_to(VfsUri::local(&target), cx)
                            });
                            workspace.focus_pane(side, window, cx);
                        });
                    })
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("tree-{index}-toggle").into()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(14.))
                            .h(px(14.))
                            .flex_shrink_0()
                            .rounded_sm()
                            .hover(|this| this.bg(theme::bg_hover()))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let path = toggle_path.clone();
                                toggle_entity.update(cx, |workspace, cx| {
                                    workspace.toggle_tree_dir(path, cx)
                                });
                            })
                            .child(
                                svg()
                                    .path(if row.expanded {
                                        icons::CHEVRON_DOWN
                                    } else {
                                        icons::CHEVRON_RIGHT
                                    })
                                    .w(px(10.))
                                    .h(px(10.))
                                    .text_color(theme::text_faint()),
                            ),
                    )
                    .child(
                        gpui::img(file_icons::asset_path(icon_id))
                            .w(px(15.))
                            .h(px(15.))
                            .flex_shrink_0(),
                    )
                    .child(div().truncate().min_w_0().child(row.name)),
            );
        }

        div()
            .flex()
            .flex_col()
            .w(px(theme::SIDEBAR_WIDTH))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .pt_3()
            .bg(theme::bg())
            .border_r_1()
            .border_color(theme::border_soft())
            .child(list)
            .child(
                div()
                    .id("sidebar-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(tree),
            )
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
