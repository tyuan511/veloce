//! One pane: a directory listing with its own tabs, history, selection, filter
//! and scroll state, plus all of its rendering.
//!
//! Panes are independent — that is what makes dual-pane usable — so each owns its
//! own cancellation tokens and text inputs. The workspace owns the shared services
//! (task manager, provider) and routes actions to whichever pane has focus.

mod views;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, MouseButton,
    Subscription, UniformListScrollHandle, Window, div, img, prelude::*, px, svg, uniform_list,
};
use vfs_core::{
    FileEntry, FileKind, Filter, LocalProvider, SortKey, SortSpec, VfsProvider, VfsUri,
    archive::{
        self, ArchiveKind, CommandSpec, CompressFormat, compress_archive_path, compress_command,
        extract_command, extract_dir_path,
    },
    entry::{format_relative, format_size},
    ops::{ConflictPolicy, TaskManager, shortcut_path},
};

use crate::{
    clipboard::PasteOp,
    file_icons,
    history::History,
    icons,
    listing::{
        Listing, breadcrumbs, cycle_index, index_after_close, parse_path_input, visible_crumbs,
    },
    persist::Persist,
    text_input::{TextInput, TextInputEvent},
    theme::{self, ROW_HEIGHT},
    view::{GridMove, ViewMode, grid_move},
};

/// Icon / size / time column widths, shared by the header and every row so they
/// cannot drift apart.
pub const ICON_WIDTH: f32 = 22.0;
pub const SIZE_WIDTH: f32 = 84.0;
pub const TIME_WIDTH: f32 = 128.0;
pub const KIND_WIDTH: f32 = 112.0;

/// Rows moved by a page-down / page-up.
const PAGE_ROWS: isize = 15;

/// Which pane has focus; determines where keyboard actions land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneSide {
    Left,
    Right,
}

impl PaneSide {
    pub fn other(self) -> Self {
        match self {
            PaneSide::Left => PaneSide::Right,
            PaneSide::Right => PaneSide::Left,
        }
    }

    pub fn index(self) -> u8 {
        match self {
            PaneSide::Left => 0,
            PaneSide::Right => 1,
        }
    }
}

/// One tab inside a pane.
pub struct Tab {
    pub uri: VfsUri,
    pub title: String,
    pub history: History<VfsUri>,
}

/// What the icon view knows about one file's thumbnail.
#[derive(Clone)]
pub struct ThumbEntry {
    /// The file version this answer is for; a changed file is asked again.
    pub stamp: crate::thumbs::Stamp,
    pub state: ThumbState,
}

#[derive(Clone)]
pub enum ThumbState {
    Pending,
    Ready(Arc<gpui::Image>),
    /// No thumbnail could be made: keep the ordinary icon.
    Unavailable,
}

/// Shared services the panes borrow.
#[derive(Clone)]
pub struct Services {
    pub provider: Arc<dyn VfsProvider>,
    pub tasks: Arc<TaskManager>,
    /// Saved per-folder views, and the queue for writing new ones.
    pub persist: Persist,
}

impl Services {
    pub fn new(persist: Persist) -> Self {
        Self {
            provider: Arc::new(LocalProvider::new()),
            tasks: Arc::new(TaskManager::with_defaults()),
            persist,
        }
    }
}

/// Events a pane emits so the workspace can react.
#[derive(Debug, Clone)]
pub enum PaneEvent {
    /// The listing changed (status bar, other pane).
    ListingChanged,
    /// The user asked to open something with the OS.
    OpenExternally(VfsUri),
    /// The user asked to reveal something in a terminal (`⌘⇧O`, milestone 2).
    #[allow(dead_code)]
    OpenTerminal(PathBuf),
    /// Right-click: `on_entry` says whether a row (vs. empty space) was hit.
    ContextMenu {
        position: gpui::Point<gpui::Pixels>,
        on_entry: bool,
    },
    /// A background operation failed; the workspace shows it as a banner.
    Notice(String),
    /// The pane moved to a folder (not emitted for Back/Forward replays of a
    /// reload, only for real changes of folder).
    Navigated(VfsUri),
    /// Tabs were opened, closed, switched or pointed elsewhere.
    TabsChanged,
}

/// Jump targets for `gg` / `G`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpTarget {
    Top,
    Bottom,
}

pub struct Pane {
    pub side: PaneSide,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    /// Sorted entries, filter view, selection and cursor — all pure logic.
    pub listing: Listing,
    pub sort: SortSpec,
    pub filter: Filter,
    pub filter_active: bool,
    pub view: ViewMode,
    /// Width the workspace grants this pane; drives the icon grid's column count.
    pub avail_width: f32,
    pub grid_cols: usize,
    /// Side-column folder contents (columns view), loaded lazily.
    pub columns: HashMap<PathBuf, Arc<Vec<FileEntry>>>,
    /// Thumbnails for the icon view, generated lazily for visible files.
    pub thumbs: HashMap<PathBuf, ThumbEntry>,
    thumbs_in_flight: usize,
    columns_loading: HashSet<PathBuf>,
    column_scrolls: HashMap<PathBuf, UniformListScrollHandle>,
    columns_scrolled: HashSet<PathBuf>,
    /// Files cut to the clipboard; drawn dimmed until pasted.
    pub cut_paths: Vec<PathBuf>,
    pub preview_open: bool,
    pub scroll: UniformListScrollHandle,
    pub services: Services,
    pub focus_handle: FocusHandle,
    pub loading: bool,
    pub error: Option<String>,
    /// `true` while the path bar shows an editable field.
    pub editing_path: bool,
    pub path_input: Entity<TextInput>,
    pub filter_input: Entity<TextInput>,
    pub listed_from: Option<VfsUri>,
    pub status: String,
    /// Cancel token of the in-flight listing; flipping it abandons the stream.
    listing_token: Arc<AtomicBool>,
    /// Timestamp of the last `g`, for the `gg` chord.
    last_g: Option<Instant>,
    /// Directory to put the cursor on once the next listing lands (set by `h`).
    pending_focus: Option<VfsUri>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaneEvent> for Pane {}

impl Focusable for Pane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Pane {
    pub fn new(side: PaneSide, uri: VfsUri, services: Services, cx: &mut Context<Self>) -> Self {
        let path_input = cx.new(|cx| TextInput::new("Path", cx).monospace());
        let filter_input = cx.new(|cx| TextInput::new("Filter this folder…", cx));

        let subscriptions = vec![
            cx.subscribe(
                &path_input,
                |pane, _, event: &TextInputEvent, cx| match event {
                    TextInputEvent::Submit(_) => pane.commit_path_edit(cx),
                    TextInputEvent::Cancel => {
                        pane.editing_path = false;
                        cx.notify();
                    }
                    TextInputEvent::Change(_) => {}
                },
            ),
            cx.subscribe(
                &filter_input,
                |pane, _, event: &TextInputEvent, cx| match event {
                    TextInputEvent::Change(text) => pane.set_filter_query(text.clone(), cx),
                    TextInputEvent::Submit(_) => {
                        // Enter in the filter keeps the filtered list, closes the caret.
                        cx.notify();
                    }
                    TextInputEvent::Cancel => pane.stop_filter(cx),
                },
            ),
        ];

        let title = uri.file_name().to_string();
        Self {
            side,
            tabs: vec![Tab {
                history: History::new(uri.clone()),
                uri: uri.clone(),
                title,
            }],
            active_tab: 0,
            listing: Listing::new(),
            sort: SortSpec::default(),
            filter: Filter::new(),
            filter_active: false,
            view: ViewMode::default(),
            avail_width: 800.0,
            grid_cols: 1,
            columns: HashMap::new(),
            thumbs: HashMap::new(),
            thumbs_in_flight: 0,
            columns_loading: HashSet::new(),
            column_scrolls: HashMap::new(),
            columns_scrolled: HashSet::new(),
            cut_paths: Vec::new(),
            preview_open: false,
            scroll: UniformListScrollHandle::new(),
            services,
            focus_handle: cx.focus_handle(),
            loading: false,
            error: None,
            editing_path: false,
            path_input,
            filter_input,
            listed_from: None,
            status: String::new(),
            listing_token: Arc::new(AtomicBool::new(false)),
            last_g: None,
            pending_focus: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn current_uri(&self) -> VfsUri {
        self.tabs[self.active_tab].uri.clone()
    }

    pub fn cursor_entry(&self) -> Option<&FileEntry> {
        self.listing.cursor_entry()
    }

    /// Selected entries, falling back to the cursor entry.
    pub fn selection_or_cursor(&self) -> Vec<FileEntry> {
        self.listing.selection_or_cursor()
    }

    // ---- listing ----------------------------------------------------------

    /// Stream the current directory in. The first chunk paints, the rest appends.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let uri = self.current_uri();
        self.listed_from = Some(uri.clone());

        // Abandon the previous stream.
        self.listing_token.store(true, Ordering::Relaxed);
        let token = Arc::new(AtomicBool::new(false));
        self.listing_token = token.clone();

        self.loading = true;
        self.error = None;
        // Drop the previous directory's rows immediately: an empty directory would
        // otherwise keep showing them, since no chunk ever arrives to clear them.
        self.listing.clear();

        let provider = self.services.provider.clone();
        cx.spawn(async move |this, cx| {
            let stream = match provider.list_stream(&uri).await {
                Ok(stream) => stream,
                Err(error) => {
                    let message = error.to_string();
                    this.update(cx, |pane, cx| {
                        if token.load(Ordering::Relaxed) {
                            return;
                        }
                        pane.loading = false;
                        pane.error = Some(message);
                        pane.listing.clear();
                        pane.refresh_status();
                        cx.emit(PaneEvent::ListingChanged);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            use futures::StreamExt as _;
            let mut stream = stream;
            let mut first = true;
            while let Some(chunk) = stream.next().await {
                if token.load(Ordering::Relaxed) {
                    return;
                }
                match chunk {
                    Ok(chunk) => {
                        let first_chunk = first;
                        first = false;
                        this.update(cx, |pane, cx| {
                            if first_chunk {
                                pane.listing.clear();
                            }
                            pane.listing
                                .extend_entries(chunk, &pane.sort, &mut pane.filter);
                            if first_chunk {
                                pane.loading = false;
                                pane.refresh_status();
                                cx.emit(PaneEvent::ListingChanged);
                            }
                            cx.notify();
                        })
                        .ok();
                    }
                    Err(error) => {
                        let message = error.to_string();
                        this.update(cx, |pane, cx| {
                            pane.loading = false;
                            pane.error = Some(message);
                            pane.refresh_status();
                            cx.emit(PaneEvent::ListingChanged);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                }
            }

            this.update(cx, |pane, cx| {
                if token.load(Ordering::Relaxed) {
                    return;
                }
                pane.loading = false;
                pane.recompute_visible();
                // `h` remembers which folder it came from; restore the cursor onto it.
                if let Some(target) = pane.pending_focus.take() {
                    pane.listing.focus_uri(&target);
                }
                pane.refresh_status();
                cx.emit(PaneEvent::ListingChanged);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Re-apply sort and filter, keeping the cursor inside bounds.
    pub fn recompute_visible(&mut self) {
        self.listing.resync(&self.sort, &mut self.filter);
    }

    pub fn refresh_status(&mut self) {
        self.status = match self.error.as_ref() {
            Some(error) => error.clone(),
            None => self.listing.status(),
        };
    }

    // ---- navigation -------------------------------------------------------

    /// Go to `uri`, recording the visit so Back can return here.
    pub fn navigate_to(&mut self, uri: VfsUri, cx: &mut Context<Self>) {
        self.tabs[self.active_tab].history.push(uri.clone());
        self.show_uri(uri, cx);
    }

    pub fn can_go_back(&self) -> bool {
        self.tabs[self.active_tab].history.can_go_back()
    }

    pub fn can_go_forward(&self) -> bool {
        self.tabs[self.active_tab].history.can_go_forward()
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(uri) = self.tabs[self.active_tab].history.back() {
            self.show_uri(uri, cx);
        }
    }

    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(uri) = self.tabs[self.active_tab].history.forward() {
            self.show_uri(uri, cx);
        }
    }

    /// The view and sort saved for this folder (or Finder's default for one never
    /// customised). Only local folders are remembered.
    fn apply_folder_prefs(&mut self, uri: &VfsUri) {
        let Some(path) = uri.as_local_path() else {
            return;
        };
        let (view, sort) = self.services.persist.folder_view(&path.to_string_lossy());
        self.view = view;
        self.sort = sort;
        self.recompute_visible();
        self.scroll = UniformListScrollHandle::new();
    }

    /// Save the current view and sort as this folder's.
    fn remember_folder_prefs(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.current_uri().as_local_path() {
            self.services.persist.remember_folder(
                cx,
                &path.to_string_lossy(),
                self.view,
                self.sort,
            );
        }
    }

    /// Local paths of the open tabs and the active index, for the saved session.
    pub fn tab_paths(&self) -> (Vec<String>, usize) {
        let mut paths = Vec::new();
        let mut active = 0;
        for (index, tab) in self.tabs.iter().enumerate() {
            if let Some(path) = tab.uri.as_local_path() {
                if index == self.active_tab {
                    active = paths.len();
                }
                paths.push(path.to_string_lossy().into_owned());
            }
        }
        (paths, active)
    }

    /// Replace the tabs with a restored session. Call before the first `reload`.
    pub fn restore_tabs(&mut self, uris: Vec<VfsUri>, active: usize) {
        if uris.is_empty() {
            // Nothing saved, but the starting folder may have its own view.
            let uri = self.current_uri();
            self.apply_folder_prefs(&uri);
            return;
        }
        self.tabs = uris
            .into_iter()
            .map(|uri| Tab {
                title: uri.file_name().to_string(),
                history: History::new(uri.clone()),
                uri,
            })
            .collect();
        self.active_tab = active.min(self.tabs.len() - 1);
        let uri = self.current_uri();
        self.apply_folder_prefs(&uri);
    }

    /// Show a folder without touching the history (Back/Forward and `navigate_to`
    /// both end here).
    fn show_uri(&mut self, uri: VfsUri, cx: &mut Context<Self>) {
        self.apply_folder_prefs(&uri);
        self.tabs[self.active_tab].uri = uri.clone();
        self.tabs[self.active_tab].title = uri.file_name().to_string();
        self.listing.clear_selection();
        self.filter.clear();
        self.filter_active = false;
        self.filter_input.update(cx, |input, cx| input.clear(cx));
        self.scroll = UniformListScrollHandle::new();
        self.reload(cx);
        cx.emit(PaneEvent::Navigated(uri));
        cx.emit(PaneEvent::TabsChanged);
    }

    pub fn set_cursor_position(&mut self, position: usize, extend: bool, cx: &mut Context<Self>) {
        self.listing.set_cursor(position, extend);
        self.refresh_status();
        cx.notify();
    }

    /// Keep the cursor visible. The icon grid scrolls by row, not by item.
    fn scroll_to_cursor(&self) {
        let position = self.listing.cursor;
        let row = match self.view {
            ViewMode::Icons => position / self.grid_cols.max(1),
            _ => position,
        };
        self.scroll
            .scroll_to_item(row, gpui::ScrollStrategy::Center);
    }

    /// Up/Down: one row in lists and columns, one grid row in the icon view.
    pub fn move_vertical(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.view != ViewMode::Icons {
            self.move_cursor(if down { 1 } else { -1 }, false, cx);
            return;
        }
        let direction = if down { GridMove::Down } else { GridMove::Up };
        self.move_grid(direction, cx);
    }

    fn move_grid(&mut self, direction: GridMove, cx: &mut Context<Self>) {
        let target = grid_move(
            self.listing.cursor,
            self.listing.len(),
            self.grid_cols,
            direction,
        );
        self.listing.set_cursor(target, false);
        self.scroll_to_cursor();
        self.refresh_status();
        cx.notify();
    }

    pub fn set_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        if self.view == view {
            return;
        }
        self.view = view;
        self.scroll = UniformListScrollHandle::new();
        self.scroll_to_cursor();
        self.remember_folder_prefs(cx);
        cx.notify();
    }

    /// Show or hide dot-files in every view of this pane.
    pub fn set_show_hidden(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.listing.show_hidden == show {
            return;
        }
        self.listing.show_hidden = show;
        self.recompute_visible();
        self.invalidate_columns();
        self.refresh_status();
        cx.emit(PaneEvent::ListingChanged);
        cx.notify();
    }

    pub fn set_sort(&mut self, sort: SortSpec, cx: &mut Context<Self>) {
        self.sort = sort;
        self.recompute_visible();
        self.invalidate_columns();
        self.remember_folder_prefs(cx);
        cx.notify();
    }

    /// Re-read the folder and drop cached side columns (they may be stale too).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.invalidate_columns();
        self.reload(cx);
    }

    /// Left/right in the icon grid move one item; everywhere else they navigate.
    pub fn move_horizontal(&mut self, forward: bool, cx: &mut Context<Self>) {
        match self.view {
            ViewMode::Icons => self.move_grid(
                if forward {
                    GridMove::Right
                } else {
                    GridMove::Left
                },
                cx,
            ),
            _ if forward => self.activate_cursor(cx),
            _ => self.go_parent(cx),
        }
    }

    pub fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        self.listing.move_cursor(delta, extend);
        self.scroll_to_cursor();
        self.refresh_status();
        cx.notify();
    }

    pub fn move_page(&mut self, down: bool, cx: &mut Context<Self>) {
        self.move_cursor(if down { PAGE_ROWS } else { -PAGE_ROWS }, false, cx);
    }

    pub fn jump_to(&mut self, target: JumpTarget, cx: &mut Context<Self>) {
        self.listing.jump(target);
        self.scroll_to_cursor();
        self.refresh_status();
        cx.notify();
    }

    /// Handle the `g` prefix so `gg` jumps to the top like Vim.
    pub fn maybe_jump_top(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let repeated = self
            .last_g
            .is_some_and(|at| now.duration_since(at) < Duration::from_millis(600));
        if repeated {
            self.last_g = None;
            self.jump_to(JumpTarget::Top, cx);
        } else {
            self.last_g = Some(now);
        }
    }

    /// Visual-mode style: move the cursor one row and grow the selection.
    ///
    /// The cursor row is always part of the selection (arrow keys select), so a
    /// plain "toggle" would deselect the row you are looking at. Extending is the
    /// behaviour a keyboard user actually wants from `shift-j`.
    pub fn extend_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.listing.extend_selection(delta);
        self.scroll_to_cursor();
        self.refresh_status();
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.listing.select_all();
        self.refresh_status();
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.filter_active {
            self.stop_filter(cx);
            return;
        }
        self.listing.clear_selection();
        self.refresh_status();
        cx.notify();
    }

    /// Enter / `l`: descend into directories, ask the OS to open files.
    pub fn activate_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor_entry().cloned() else {
            return;
        };
        if entry.is_dir() {
            self.navigate_to(entry.uri.clone(), cx);
        } else {
            cx.emit(PaneEvent::OpenExternally(entry.uri));
        }
    }

    /// Go up one level, then put the cursor back on the folder we came from so
    /// `h` followed by `l` returns exactly where you were.
    pub fn go_parent(&mut self, cx: &mut Context<Self>) {
        let current = self.current_uri();
        let Some(parent) = current.parent() else {
            return;
        };
        self.navigate_to(parent, cx);
        self.pending_focus = Some(current);
    }

    // ---- filtering --------------------------------------------------------

    pub fn start_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_active = true;
        self.filter.clear();
        self.filter_input.update(cx, |input, cx| input.clear(cx));
        self.recompute_visible();
        self.refresh_status();
        let handle = self.filter_input.read(cx).focus_handle(cx);
        window.focus(&handle);
        cx.notify();
    }

    pub fn stop_filter(&mut self, cx: &mut Context<Self>) {
        self.filter_active = false;
        self.filter.clear();
        self.filter_input.update(cx, |input, cx| input.clear(cx));
        self.recompute_visible();
        self.refresh_status();
        cx.notify();
    }

    pub fn set_filter_query(&mut self, query: impl Into<String>, cx: &mut Context<Self>) {
        if self.filter.set_query(query) {
            self.listing.cursor = 0;
            self.recompute_visible();
            self.refresh_status();
            cx.notify();
        }
    }

    // ---- path editing -----------------------------------------------------

    pub fn begin_path_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.current_uri().path.clone();
        self.editing_path = true;
        self.path_input
            .update(cx, |input, cx| input.set_content(path, cx));
        let handle = self.path_input.read(cx).focus_handle(cx);
        window.focus(&handle);
        cx.notify();
    }

    pub fn commit_path_edit(&mut self, cx: &mut Context<Self>) {
        self.editing_path = false;
        let value = self.path_input.read(cx).content().to_string();
        match parse_path_input(&value, &self.current_uri()) {
            Some(uri) => self.navigate_to(uri, cx),
            None => cx.notify(),
        }
    }

    // ---- sorting ----------------------------------------------------------

    pub fn cycle_sort(&mut self, cx: &mut Context<Self>) {
        self.set_sort(self.sort.toggle(self.sort.key.next()), cx);
    }

    pub fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.set_sort(self.sort.toggle(key), cx);
    }

    // ---- preview ----------------------------------------------------------

    pub fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_open = !self.preview_open;
        cx.notify();
    }

    // ---- file operations --------------------------------------------------

    pub fn copy_selection_to(&mut self, target_dir: &std::path::Path, cx: &mut Context<Self>) {
        for entry in self.selection_or_cursor() {
            let Some(from) = entry.uri.as_local_path() else {
                continue;
            };
            let to = target_dir.join(&entry.name);
            self.services.tasks.copy(from, to, ConflictPolicy::Rename);
        }
        self.reload_when_idle(cx);
        cx.notify();
    }

    pub fn move_selection_to(&mut self, target_dir: &std::path::Path, cx: &mut Context<Self>) {
        for entry in self.selection_or_cursor() {
            let Some(from) = entry.uri.as_local_path() else {
                continue;
            };
            let to = target_dir.join(&entry.name);
            self.services
                .tasks
                .move_path(from, to, ConflictPolicy::Rename);
        }
        self.listing.clear_selection();
        self.reload_when_idle(cx);
    }

    /// Send the selection to the OS trash: recoverable, never a hard delete.
    ///
    /// Rows disappear immediately (optimistic UI) and the listing is re-read once
    /// the trash task actually completes, so the two never disagree.
    pub fn trash_selection(&mut self, cx: &mut Context<Self>) {
        let targets: Vec<String> = self
            .selection_or_cursor()
            .into_iter()
            .map(|entry| entry.id())
            .collect();
        for entry in self.selection_or_cursor() {
            if let Some(path) = entry.uri.as_local_path() {
                self.services.tasks.trash(path);
            }
        }
        let remaining: Vec<FileEntry> = self
            .listing
            .entries
            .iter()
            .filter(|entry| !targets.contains(&entry.id()))
            .cloned()
            .collect();
        self.listing.clear_selection();
        self.listing
            .set_entries(remaining, &self.sort, &mut self.filter);
        self.refresh_status();
        self.reload_when_idle(cx);
        cx.notify();
    }

    /// Re-read the directory once every queued transfer has finished.
    ///
    /// The task manager is fire-and-forget, so without this the listing would show
    /// pre-operation state until some unrelated event refreshed it.
    pub fn reload_when_idle(&mut self, cx: &mut Context<Self>) {
        let tasks = self.services.tasks.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
                let idle = tasks.active_count() == 0;
                if idle {
                    break;
                }
            }
            this.update(cx, |pane, cx| pane.refresh(cx)).ok();
        })
        .detach();
    }

    pub fn create_dir(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(dir) = self.current_uri().as_local_path() else {
            return;
        };
        self.services.tasks.create_dir(dir.join(name));
        self.reload_when_idle(cx);
    }

    pub fn rename_cursor(&mut self, new_name: &str, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor_entry().cloned() else {
            return;
        };
        let Some(path) = entry.uri.as_local_path() else {
            return;
        };
        let Some(parent) = path.parent().map(|parent| parent.to_path_buf()) else {
            return;
        };
        self.services.tasks.rename(path, parent.join(new_name));
        self.reload_when_idle(cx);
    }

    // ---- context-menu operations -------------------------------------------

    /// Right-click bookkeeping: a click inside the selection keeps it (so the menu
    /// acts on all of it); a click elsewhere selects just that row, like Finder.
    pub fn context_click(
        &mut self,
        row: Option<usize>,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        match row {
            Some(row) => {
                let inside = self
                    .listing
                    .visible
                    .get(row)
                    .is_some_and(|index| self.listing.selected.contains(index));
                if inside {
                    self.listing.cursor = row;
                } else {
                    self.listing.set_cursor(row, false);
                }
            }
            None => self.listing.clear_selection(),
        }
        cx.emit(PaneEvent::ContextMenu {
            position,
            on_entry: row.is_some(),
        });
        cx.emit(PaneEvent::ListingChanged);
        cx.notify();
    }

    /// Local paths of the selection (or cursor entry).
    pub fn selection_paths(&self) -> Vec<PathBuf> {
        self.selection_or_cursor()
            .iter()
            .filter_map(|entry| entry.uri.as_local_path())
            .collect()
    }

    /// `Open`: one entry behaves like Enter; several open every file (folders
    /// cannot all be entered at once, so they are skipped).
    pub fn open_selection(&mut self, cx: &mut Context<Self>) {
        let entries = self.selection_or_cursor();
        if entries.len() <= 1 {
            self.activate_cursor(cx);
            return;
        }
        for entry in entries.into_iter().filter(|entry| !entry.is_dir()) {
            cx.emit(PaneEvent::OpenExternally(entry.uri));
        }
    }

    pub fn open_in_new_tab(&mut self, uri: VfsUri, cx: &mut Context<Self>) {
        self.new_tab(cx);
        self.navigate_to(uri, cx);
    }

    /// Run a planned paste. Copies that collide keep both (`name 2`), so pasting
    /// never silently overwrites.
    pub fn apply_paste(&mut self, ops: Vec<PasteOp>, cx: &mut Context<Self>) {
        for op in ops {
            match op {
                PasteOp::Copy { from, to } => {
                    self.services.tasks.copy(from, to, ConflictPolicy::Rename);
                }
                PasteOp::Move { from, to } => {
                    self.services
                        .tasks
                        .move_path(from, to, ConflictPolicy::Rename);
                }
            }
        }
        self.reload_when_idle(cx);
    }

    /// Which paths to draw dimmed (files waiting to be moved by a Paste).
    pub fn set_cut_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.cut_paths != paths {
            self.cut_paths = paths;
            cx.notify();
        }
    }

    /// `Create shortcut`: a symlink named `name - Shortcut`. Off the UI thread
    /// like every other filesystem write.
    pub fn shortcut_selection(&mut self, cx: &mut Context<Self>) {
        let pairs: Vec<(PathBuf, PathBuf)> = self
            .selection_paths()
            .into_iter()
            .map(|from| {
                let to = shortcut_path(&from);
                (from, to)
            })
            .collect();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    for (from, to) in pairs {
                        std::os::unix::fs::symlink(&from, &to).map_err(|error| {
                            format!("Cannot create a shortcut to {}: {error}", from.display())
                        })?;
                    }
                    Ok::<(), String>(())
                })
                .await;
            this.update(cx, |pane, cx| {
                if let Err(message) = result {
                    cx.emit(PaneEvent::Notice(message));
                }
                pane.reload(cx);
            })
            .ok();
        })
        .detach();
    }

    /// `Compress to ...`: an archive next to the first item, written by the
    /// system tools so symlinks and permissions survive.
    pub fn compress_selection(&mut self, format: CompressFormat, cx: &mut Context<Self>) {
        let sources = self.selection_paths();
        let Some(archive) = compress_archive_path(&sources, format) else {
            return;
        };
        let names: Vec<std::ffi::OsString> = sources
            .iter()
            .filter_map(|source| source.file_name().map(|name| name.to_os_string()))
            .collect();
        let spec = compress_command(format, &archive, &names);
        self.run_archive_job(
            spec,
            None,
            format!("Compress to {} failed", format.label()),
            cx,
        );
    }

    /// `Extract Here` / `Extract to "name/"`: unpack the cursor's archive.
    pub fn extract_selection(&mut self, here: bool, cx: &mut Context<Self>) {
        let Some(archive) = self
            .cursor_entry()
            .and_then(|entry| entry.uri.as_local_path())
        else {
            return;
        };
        let Some(kind) = archive
            .file_name()
            .and_then(|name| ArchiveKind::detect(&name.to_string_lossy()))
        else {
            cx.emit(PaneEvent::Notice("This is not a supported archive".into()));
            return;
        };
        let dest = if here {
            archive.parent().map(|parent| parent.to_path_buf())
        } else {
            extract_dir_path(&archive)
        };
        let Some(dest) = dest else {
            return;
        };
        let spec = extract_command(kind, &archive, &dest);
        self.run_archive_job(spec, Some(dest), "Extract failed".to_string(), cx);
    }

    /// Run an archive tool off the UI thread; failures surface as a banner and the
    /// listing refreshes either way.
    fn run_archive_job(
        &mut self,
        spec: CommandSpec,
        create_dir: Option<PathBuf>,
        failure: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(dir) = create_dir {
                        std::fs::create_dir_all(&dir)
                            .map_err(|error| format!("Cannot create {}: {error}", dir.display()))?;
                    }
                    archive::run(&spec)
                })
                .await;
            this.update(cx, |pane, cx| {
                if let Err(message) = result {
                    cx.emit(PaneEvent::Notice(format!("{failure}: {message}")));
                }
                pane.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    // ---- tabs -------------------------------------------------------------

    pub fn new_tab(&mut self, cx: &mut Context<Self>) {
        let uri = self.current_uri();
        let title = uri.file_name().to_string();
        self.tabs.push(Tab {
            history: History::new(uri.clone()),
            uri,
            title,
        });
        self.active_tab = self.tabs.len() - 1;
        self.reload(cx);
        cx.emit(PaneEvent::TabsChanged);
        cx.notify();
    }

    pub fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            return;
        }
        let len = self.tabs.len();
        self.active_tab = index_after_close(self.active_tab, index, len);
        self.tabs.remove(index);
        self.listing.clear_selection();
        self.reload(cx);
        cx.emit(PaneEvent::TabsChanged);
        cx.notify();
    }

    pub fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.active_tab = index;
        self.listing.clear_selection();
        self.reload(cx);
        cx.emit(PaneEvent::TabsChanged);
        cx.notify();
    }

    pub fn cycle_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.tabs.len() < 2 {
            return;
        }
        let next = cycle_index(self.active_tab, self.tabs.len(), forward);
        self.select_tab(next, cx);
    }
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

impl Pane {
    pub fn render_pane(
        &mut self,
        focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Finder only shows the tab strip once there is more than one tab.
        let tabs = (self.tabs.len() > 1).then(|| self.render_tabs(focused, cx));
        let path_bar = self.render_path_bar(focused, window, cx);
        let filter_bar = self.filter_active.then(|| self.render_filter_bar(cx));
        let header = (self.view == ViewMode::List).then(|| self.render_header(cx));
        let body = self.render_body(focused, cx);
        let status = self.render_status();
        let entity = cx.entity();
        let body_area = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .on_mouse_down(MouseButton::Right, move |event, _, cx| {
                let at = event.position;
                entity.update(cx, |pane, cx| pane.context_click(None, at, cx));
            })
            .child(body);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme::bg_pane())
            // Always reserve the 2px so focus changes never shift the layout.
            .border_t_2()
            .border_color(if focused {
                theme::accent_dim()
            } else {
                Hsla::transparent_black()
            })
            .children(tabs)
            .child(path_bar)
            .children(filter_bar)
            .children(header)
            .child(body_area)
            .child(status)
            .into_any_element()
    }

    fn render_tabs(&self, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let count = self.tabs.len();
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(30.0))
            .flex_shrink_0()
            .bg(theme::bg())
            .border_b_1()
            .border_color(theme::border_soft())
            .pl_1();

        for (index, tab) in self.tabs.iter().enumerate() {
            let active = index == self.active_tab;
            let title = tab.title.clone();
            let entity = cx.entity();
            let mut chip = div()
                .id(gpui::ElementId::Name(
                    format!("pane{}-tab{index}", self.side.index()).into(),
                ))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(23.0))
                .px_2()
                .max_w(px(180.0))
                .rounded_md()
                .when(active, |this| {
                    this.bg(theme::bg_pane())
                        .text_color(theme::text())
                        .when(focused, |this| {
                            this.border_b_1().border_color(theme::accent())
                        })
                })
                .when(!active, |this| this.text_color(theme::text_muted()))
                .hover(|this| this.bg(theme::bg_hover()))
                .on_click(move |_, _, cx| {
                    entity.update(cx, |pane, cx| pane.select_tab(index, cx));
                })
                .child(
                    svg()
                        .path(icons::FOLDER)
                        .w(px(12.))
                        .h(px(12.))
                        .flex_shrink_0()
                        .text_color(if active {
                            theme::blue()
                        } else {
                            theme::text_faint()
                        }),
                )
                .child(div().text_xs().truncate().min_w_0().child(title));

            if count > 1 {
                let entity = cx.entity();
                chip = chip.child(
                    div()
                        .id(gpui::ElementId::Name(
                            format!("pane{}-tab{index}-close", self.side.index()).into(),
                        ))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(14.))
                        .h(px(14.))
                        .rounded_sm()
                        .text_color(theme::text_faint())
                        .hover(|this| this.bg(theme::bg_raised()).text_color(theme::text()))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            entity.update(cx, |pane, cx| pane.close_tab(index, cx));
                        })
                        .child(
                            svg()
                                .path(icons::CLOSE)
                                .w(px(8.))
                                .h(px(8.))
                                .text_color(theme::text_faint()),
                        ),
                );
            }
            row = row.child(chip);
        }

        let entity = cx.entity();
        row.child(
            div()
                .id(gpui::ElementId::Name(
                    format!("pane{}-newtab", self.side.index()).into(),
                ))
                .flex()
                .items_center()
                .justify_center()
                .w(px(22.))
                .h(px(20.))
                .rounded_sm()
                .text_color(theme::text_faint())
                .hover(|this| this.bg(theme::bg_hover()).text_color(theme::text()))
                .on_click(move |_, _, cx| {
                    entity.update(cx, |pane, cx| pane.new_tab(cx));
                })
                .child(
                    svg()
                        .path(icons::PLUS)
                        .w(px(10.))
                        .h(px(10.))
                        .text_color(theme::text_faint()),
                ),
        )
        .child(div().flex_1())
        .into_any_element()
    }

    fn render_path_bar(
        &mut self,
        focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_go_up = !self.current_uri().is_root();
        let up = {
            let entity = cx.entity();
            div()
                .id(gpui::ElementId::Name(
                    format!("pane{}-up", self.side.index()).into(),
                ))
                .flex()
                .items_center()
                .justify_center()
                .w(px(21.))
                .h(px(20.))
                .rounded_sm()
                .text_color(if can_go_up {
                    theme::text_muted()
                } else {
                    theme::text_faint()
                })
                .when(can_go_up, |this| {
                    this.hover(|this| this.bg(theme::bg_hover()).text_color(theme::text()))
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |pane, cx| pane.go_parent(cx));
                        })
                })
                .child(
                    svg()
                        .path(icons::ARROW_UP)
                        .w(px(11.))
                        .h(px(11.))
                        .text_color(if can_go_up {
                            theme::text_muted()
                        } else {
                            theme::text_faint()
                        }),
                )
        };

        let path_area: AnyElement = if self.editing_path {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .px_2()
                .h(px(21.))
                .rounded_sm()
                .bg(theme::bg())
                .border_1()
                .border_color(theme::accent_dim())
                .text_color(theme::text())
                .child(self.path_input.clone())
                .into_any_element()
        } else {
            let path_text = self.current_uri().path.clone();
            let crumbs = breadcrumbs(&path_text);
            let (shown, cut) = visible_crumbs(&crumbs, 4);
            let last = shown.len() - 1;
            let side = self.side.index();

            let mut trail = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_0p5()
                .min_w_0()
                .overflow_hidden();
            if cut {
                trail = trail.child(div().text_xs().text_color(theme::text_faint()).child("… ›"));
            }
            for (index, crumb) in shown.iter().enumerate() {
                let entity = cx.entity();
                let target = crumb.path.clone();
                let is_last = index == last;
                if index > 0 {
                    trail = trail.child(
                        svg()
                            .path(icons::CHEVRON_RIGHT)
                            .w(px(9.))
                            .h(px(9.))
                            .flex_shrink_0()
                            .text_color(theme::text_faint()),
                    );
                }
                trail = trail.child(
                    div()
                        .id(gpui::ElementId::Name(
                            format!("pane{side}-crumb{index}").into(),
                        ))
                        .px_1()
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .rounded_sm()
                        .text_sm()
                        .text_color(if is_last {
                            theme::text()
                        } else {
                            theme::text_muted()
                        })
                        .when(is_last, |this| this.font_weight(gpui::FontWeight::MEDIUM))
                        .hover(|this| this.bg(theme::bg_hover()).text_color(theme::text()))
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |pane, cx| {
                                pane.navigate_to(VfsUri::local(&target), cx)
                            });
                        })
                        .child(crumb.label.clone()),
                );
            }

            // The empty space after the trail opens the editable path, like
            // clicking beside the breadcrumb in Nautilus / Explorer.
            let entity = cx.entity();
            div()
                .id(gpui::ElementId::Name(format!("pane{side}-path").into()))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .h(px(24.))
                .on_click(move |_, window, cx| {
                    entity.update(cx, |pane, cx| pane.begin_path_edit(window, cx));
                })
                .child(trail)
                .into_any_element()
        };

        let filter_button = {
            let entity = cx.entity();
            let active = self.filter_active;
            div()
                .id(gpui::ElementId::Name(
                    format!("pane{}-filter", self.side.index()).into(),
                ))
                .flex()
                .items_center()
                .justify_center()
                .w(px(21.))
                .h(px(20.))
                .rounded_sm()
                .text_color(if active {
                    theme::accent()
                } else {
                    theme::text_faint()
                })
                .hover(|this| this.bg(theme::bg_hover()).text_color(theme::text()))
                .on_click(move |_, window, cx| {
                    entity.update(cx, |pane, cx| {
                        if pane.filter_active {
                            pane.stop_filter(cx);
                        } else {
                            pane.start_filter(window, cx);
                        }
                    });
                })
                .child(
                    svg()
                        .path(icons::SEARCH)
                        .w(px(11.))
                        .h(px(11.))
                        .text_color(if active {
                            theme::accent()
                        } else {
                            theme::text_faint()
                        }),
                )
        };

        let _ = (focused, window);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(34.0))
            .flex_shrink_0()
            .px_2()
            .bg(theme::bg_pane())
            .border_b_1()
            .border_color(theme::border_soft())
            .child(up)
            .child(path_area)
            .child(filter_button)
            .into_any_element()
    }

    fn render_filter_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let matched = self.listing.len();
        let total = self.listing.shown_total();
        let input = self.filter_input.clone();
        let _ = cx;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(29.0))
            .flex_shrink_0()
            .px_2()
            .bg(theme::bg_raised())
            .border_b_1()
            .border_color(theme::border_soft())
            .child(
                svg()
                    .path(icons::SEARCH)
                    .w(px(10.))
                    .h(px(10.))
                    .flex_shrink_0()
                    .text_color(theme::accent()),
            )
            .child(div().flex_1().min_w_0().child(input))
            .child(
                div()
                    .text_xs()
                    .flex_shrink_0()
                    .text_color(theme::text_faint())
                    .child(format!("{matched}/{total}")),
            )
            .into_any_element()
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let sort = self.sort;
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(22.0))
            .flex_shrink_0()
            .px_1()
            .bg(theme::bg_pane())
            .border_b_1()
            .border_color(theme::border_soft())
            .child(div().w(px(ICON_WIDTH + 4.0)).flex_shrink_0());

        for (key, label, width) in [
            (SortKey::Name, "Name", None),
            (SortKey::Modified, "Date Modified", Some(TIME_WIDTH)),
            (SortKey::Size, "Size", Some(SIZE_WIDTH)),
            (SortKey::Kind, "Kind", Some(KIND_WIDTH)),
        ] {
            let active = sort.key == key;
            let arrow = if active {
                if sort.descending { " ↓" } else { " ↑" }
            } else {
                ""
            };
            let entity = cx.entity();
            row = row.child(
                div()
                    .id(gpui::ElementId::Name(
                        format!("hdr-{}-{label}", self.side.index()).into(),
                    ))
                    .flex()
                    .items_center()
                    .h_full()
                    .px_2()
                    .text_xs()
                    .flex_shrink_0()
                    .when_some(width, |this, width| this.w(px(width)).justify_start())
                    .when(width.is_none(), |this| this.flex_1().min_w_0())
                    .text_color(if active {
                        theme::text()
                    } else {
                        theme::text_faint()
                    })
                    .hover(|this| this.text_color(theme::text_muted()))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |pane, cx| pane.sort_by(key, cx));
                    })
                    .child(format!("{label}{arrow}")),
            );
        }
        row.into_any_element()
    }

    fn render_body(&mut self, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        match self.view {
            ViewMode::List => self.render_list_body(focused, cx),
            ViewMode::Icons => self
                .state_message()
                .unwrap_or_else(|| self.render_icons_body(focused, cx)),
            ViewMode::Columns => self.render_columns_body(focused, cx),
        }
    }

    /// Error / loading / empty placeholder, if the listing has nothing to draw.
    fn state_message(&self) -> Option<AnyElement> {
        if let Some(error) = self.error.clone() {
            return Some(render_message(
                icons::CLOSE,
                "Cannot open this folder",
                &error,
                theme::red(),
            ));
        }
        if self.loading && self.listing.entries.is_empty() {
            return Some(render_message(
                icons::FOLDER,
                "Loading…",
                "",
                theme::text_muted(),
            ));
        }
        if self.listing.is_empty() {
            let (title, detail) = if self.filter.is_active() {
                (
                    "No matches".to_string(),
                    format!("Nothing here matches “{}”", self.filter.query()),
                )
            } else {
                ("This folder is empty".to_string(), String::new())
            };
            return Some(render_message(
                icons::FOLDER,
                &title,
                &detail,
                theme::text_faint(),
            ));
        }
        None
    }

    fn render_list_body(&mut self, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        if let Some(message) = self.state_message() {
            return message;
        }

        let count = self.listing.len();
        let entity = cx.entity();
        let selected = self.listing.selected.clone();
        let cursor = self.listing.cursor;

        div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                uniform_list(
                    gpui::ElementId::Name(format!("pane{}-list", self.side.index()).into()),
                    count,
                    cx.processor(move |pane, range: std::ops::Range<usize>, _window, _cx| {
                        let mut rows: Vec<AnyElement> = Vec::with_capacity(range.len());
                        for position in range {
                            let Some(entry) = pane
                                .listing
                                .visible
                                .get(position)
                                .and_then(|index| pane.listing.entries.get(*index))
                            else {
                                continue;
                            };
                            let is_selected = pane
                                .listing
                                .visible
                                .get(position)
                                .is_some_and(|index| selected.contains(index));
                            let is_cursor = position == cursor;
                            let dimmed = !pane.cut_paths.is_empty()
                                && entry
                                    .uri
                                    .as_local_path()
                                    .is_some_and(|path| pane.cut_paths.contains(&path));
                            rows.push(render_row(
                                pane.side.index(),
                                position,
                                entry,
                                is_selected || is_cursor,
                                focused,
                                dimmed,
                                entity.clone(),
                            ));
                        }
                        rows
                    }),
                )
                .track_scroll(self.scroll.clone())
                .h_full(),
            )
            .into_any_element()
    }

    fn render_status(&self) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .h(px(theme::STATUS_HEIGHT))
            .flex_shrink_0()
            .px_2()
            .bg(theme::bg_pane())
            .border_t_1()
            .border_color(theme::border_soft())
            .text_xs()
            .text_color(theme::text_faint())
            .child(div().truncate().min_w_0().child(self.status.clone()))
            .when(self.filter.is_active(), |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_color(theme::accent())
                        .child(format!("filter: {}", self.filter.query())),
                )
            })
            .into_any_element()
    }
}

impl gpui::Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.render_pane(true, window, cx)
    }
}

fn render_row(
    side: u8,
    position: usize,
    entry: &FileEntry,
    highlighted: bool,
    pane_focused: bool,
    dimmed: bool,
    entity: Entity<Pane>,
) -> AnyElement {
    let is_dir = entry.is_dir();
    let name = entry.name.clone();
    let size = if is_dir {
        "--".to_string()
    } else {
        format_size(entry.size)
    };
    let modified = format_relative(entry.modified);
    let kind = crate::info::kind_label(entry);
    let icon = entry_icon(entry, 16.0);

    // Finder semantics: blue highlight in the active pane, grey in the other.
    let on_accent = highlighted && pane_focused;
    let highlight = if pane_focused {
        theme::bg_selected()
    } else {
        theme::bg_selected_inactive()
    };

    let click_entity = entity.clone();
    let right_entity = entity;

    // The outer wrapper provides the 4px gutter so the highlight reads as an
    // inset rounded bar rather than a full-bleed stripe.
    div()
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_1()
        .child(
            div()
                .id(gpui::ElementId::Name(
                    format!("{side}-row-{position}").into(),
                ))
                .flex()
                .flex_row()
                .items_center()
                .size_full()
                .rounded_md()
                .cursor_pointer()
                .when(dimmed, |this| this.opacity(0.5))
                .when(highlighted, |this| this.bg(highlight))
                .when(on_accent, |this| this.text_color(gpui::white()))
                .when(!on_accent, |this| this.text_color(theme::text()))
                .hover(|this| {
                    if highlighted {
                        this
                    } else {
                        this.bg(theme::bg_raised())
                    }
                })
                .on_click(move |event, _, cx| {
                    let shift = event.modifiers().shift;
                    let double = event.click_count() >= 2;
                    click_entity.update(cx, |pane, cx| {
                        pane.set_cursor_position(position, shift, cx);
                        if double {
                            pane.activate_cursor(cx);
                        }
                    });
                })
                .on_mouse_down(MouseButton::Right, move |event, _, cx| {
                    // The list background also listens for right-clicks; the row
                    // owns this one.
                    cx.stop_propagation();
                    let at = event.position;
                    right_entity.update(cx, |pane, cx| pane.context_click(Some(position), at, cx));
                })
                .child(
                    div()
                        .w(px(ICON_WIDTH))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .child(icon),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pl_1()
                        .text_sm()
                        .truncate()
                        .child(name),
                )
                .child(
                    div()
                        .w(px(TIME_WIDTH))
                        .px_2()
                        .flex_shrink_0()
                        .text_xs()
                        .when(!on_accent, |this| this.text_color(theme::text_muted()))
                        .when(on_accent, |this| this.text_color(gpui::white()))
                        .child(modified),
                )
                .child(
                    div()
                        .w(px(SIZE_WIDTH))
                        .px_2()
                        .flex_shrink_0()
                        .text_xs()
                        .when(!on_accent, |this| this.text_color(theme::text_muted()))
                        .when(on_accent, |this| this.text_color(gpui::white()))
                        .child(size),
                )
                .child(
                    div()
                        .w(px(KIND_WIDTH))
                        .px_2()
                        .flex_shrink_0()
                        .truncate()
                        .text_xs()
                        .when(!on_accent, |this| this.text_color(theme::text_muted()))
                        .when(on_accent, |this| this.text_color(gpui::white()))
                        .child(kind),
                ),
        )
        .into_any_element()
}

/// Full-colour Material icon for an entry. Symlinks keep the Lucide link glyph so
/// they stay recognisable whatever they point at.
pub(crate) fn entry_icon(entry: &FileEntry, size: f32) -> AnyElement {
    if entry.kind == FileKind::Symlink {
        return svg()
            .path(icons::LINK)
            .w(px(size))
            .h(px(size))
            .flex_shrink_0()
            .text_color(theme::purple())
            .into_any_element();
    }
    let id = if entry.is_dir() {
        file_icons::folder_icon_id(&entry.name, false)
    } else {
        file_icons::file_icon_id(&entry.name)
    };
    img(file_icons::asset_path(id, size))
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .into_any_element()
}

/// Centered message block: empty folder, load failure, loading.
fn render_message(icon: &'static str, title: &str, detail: &str, tint: Hsla) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .child(svg().path(icon).w(px(24.)).h(px(24.)).text_color(tint))
        .child(
            div()
                .text_sm()
                .text_color(theme::text_muted())
                .child(title.to_string()),
        )
        .when(!detail.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .max_w(px(300.))
                    .text_color(theme::text_faint())
                    .child(detail.to_string()),
            )
        })
        .into_any_element()
}
