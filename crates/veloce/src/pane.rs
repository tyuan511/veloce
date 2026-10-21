//! One pane: a directory listing with its own tabs, history, selection, filter
//! and scroll state, plus all of its rendering.
//!
//! Panes are independent — that is what makes dual-pane usable — so each owns its
//! own cancellation tokens and text inputs. The workspace owns the shared services
//! (task manager, provider) and routes actions to whichever pane has focus.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, MouseButton,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, svg, uniform_list,
};
use vfs_core::{
    FileEntry, FileKind, Filter, LocalProvider, SortKey, SortSpec, VfsProvider, VfsUri,
    entry::{format_relative, format_size},
    ops::{ConflictPolicy, TaskManager},
};

use crate::{
    icons,
    listing::{Listing, cycle_index, index_after_close, parse_path_input},
    text_input::{TextInput, TextInputEvent},
    theme::{self, ROW_HEIGHT},
};

/// Icon / size / time column widths, shared by the header and every row so they
/// cannot drift apart.
pub const ICON_WIDTH: f32 = 22.0;
pub const SIZE_WIDTH: f32 = 92.0;
pub const TIME_WIDTH: f32 = 132.0;

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
}

/// Shared services the panes borrow.
#[derive(Clone)]
pub struct Services {
    pub provider: Arc<dyn VfsProvider>,
    pub tasks: Arc<TaskManager>,
}

impl Services {
    pub fn new() -> Self {
        Self {
            provider: Arc::new(LocalProvider::new()),
            tasks: Arc::new(TaskManager::with_defaults()),
        }
    }
}

impl Default for Services {
    fn default() -> Self {
        Self::new()
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
                uri: uri.clone(),
                title,
            }],
            active_tab: 0,
            listing: Listing::new(),
            sort: SortSpec::default(),
            filter: Filter::new(),
            filter_active: false,
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

    pub fn navigate_to(&mut self, uri: VfsUri, cx: &mut Context<Self>) {
        self.tabs[self.active_tab].uri = uri.clone();
        self.tabs[self.active_tab].title = uri.file_name().to_string();
        self.listing.clear_selection();
        self.filter.clear();
        self.filter_active = false;
        self.filter_input.update(cx, |input, cx| input.clear(cx));
        self.scroll = UniformListScrollHandle::new();
        self.reload(cx);
    }

    pub fn set_cursor_position(&mut self, position: usize, extend: bool, cx: &mut Context<Self>) {
        self.listing.set_cursor(position, extend);
        self.refresh_status();
        cx.notify();
    }

    pub fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        self.listing.move_cursor(delta, extend);
        let position = self.listing.cursor;
        self.scroll
            .scroll_to_item(position, gpui::ScrollStrategy::Center);
        self.refresh_status();
        cx.notify();
    }

    pub fn move_page(&mut self, down: bool, cx: &mut Context<Self>) {
        self.move_cursor(if down { PAGE_ROWS } else { -PAGE_ROWS }, false, cx);
    }

    pub fn jump_to(&mut self, target: JumpTarget, cx: &mut Context<Self>) {
        self.listing.jump(target);
        let position = self.listing.cursor;
        self.scroll
            .scroll_to_item(position, gpui::ScrollStrategy::Center);
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
        let position = self.listing.cursor;
        self.scroll
            .scroll_to_item(position, gpui::ScrollStrategy::Center);
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
        self.sort = self.sort.toggle(self.sort.key.next());
        self.recompute_visible();
        cx.notify();
    }

    pub fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.sort = self.sort.toggle(key);
        self.recompute_visible();
        cx.notify();
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
            this.update(cx, |pane, cx| pane.reload(cx)).ok();
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

    // ---- tabs -------------------------------------------------------------

    pub fn new_tab(&mut self, cx: &mut Context<Self>) {
        let uri = self.current_uri();
        let title = uri.file_name().to_string();
        self.tabs.push(Tab { uri, title });
        self.active_tab = self.tabs.len() - 1;
        self.reload(cx);
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
        cx.notify();
    }

    pub fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.active_tab = index;
        self.listing.clear_selection();
        self.reload(cx);
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
        let tabs = self.render_tabs(focused, cx);
        let path_bar = self.render_path_bar(focused, window, cx);
        let filter_bar = self.filter_active.then(|| self.render_filter_bar(cx));
        let header = self.render_header(cx);
        let body = self.render_body(focused, cx);
        let status = self.render_status();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme::BG_PANE)
            .when(focused, |this| {
                this.border_t_1().border_color(theme::ACCENT_DIM)
            })
            .child(tabs)
            .child(path_bar)
            .children(filter_bar)
            .child(header)
            .child(body)
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
            .bg(theme::BG)
            .border_b_1()
            .border_color(theme::BORDER_SOFT)
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
                    this.bg(theme::BG_PANE)
                        .text_color(theme::TEXT)
                        .when(focused, |this| {
                            this.border_b_1().border_color(theme::ACCENT)
                        })
                })
                .when(!active, |this| this.text_color(theme::TEXT_MUTED))
                .hover(|this| this.bg(theme::BG_HOVER))
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
                            theme::BLUE
                        } else {
                            theme::TEXT_FAINT
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
                        .text_color(theme::TEXT_FAINT)
                        .hover(|this| this.bg(theme::BG_RAISED).text_color(theme::TEXT))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            entity.update(cx, |pane, cx| pane.close_tab(index, cx));
                        })
                        .child(svg().path(icons::CLOSE).w(px(8.)).h(px(8.))),
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
                .text_color(theme::TEXT_FAINT)
                .hover(|this| this.bg(theme::BG_HOVER).text_color(theme::TEXT))
                .on_click(move |_, _, cx| {
                    entity.update(cx, |pane, cx| pane.new_tab(cx));
                })
                .child(svg().path(icons::PLUS).w(px(10.)).h(px(10.))),
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
                    theme::TEXT_MUTED
                } else {
                    theme::TEXT_FAINT
                })
                .when(can_go_up, |this| {
                    this.hover(|this| this.bg(theme::BG_HOVER).text_color(theme::TEXT))
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |pane, cx| pane.go_parent(cx));
                        })
                })
                .child(svg().path(icons::ARROW_UP).w(px(11.)).h(px(11.)))
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
                .bg(theme::BG)
                .border_1()
                .border_color(theme::ACCENT_DIM)
                .text_color(theme::TEXT)
                .child(self.path_input.clone())
                .into_any_element()
        } else {
            let path_text = self.current_uri().path.clone();
            let entity = cx.entity();
            div()
                .id(gpui::ElementId::Name(
                    format!("pane{}-path", self.side.index()).into(),
                ))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(21.))
                .px_2()
                .rounded_sm()
                .text_color(theme::TEXT_MUTED)
                .hover(|this| this.bg(theme::BG_HOVER).text_color(theme::TEXT))
                .on_click(move |_, window, cx| {
                    entity.update(cx, |pane, cx| pane.begin_path_edit(window, cx));
                })
                .child(
                    svg()
                        .path(icons::FOLDER)
                        .w(px(11.))
                        .h(px(11.))
                        .flex_shrink_0()
                        .text_color(theme::TEXT_FAINT),
                )
                .child(
                    div()
                        .text_xs()
                        .truncate()
                        .min_w_0()
                        .font_family(theme::MONO_FAMILY)
                        .child(path_text),
                )
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
                    theme::ACCENT
                } else {
                    theme::TEXT_FAINT
                })
                .hover(|this| this.bg(theme::BG_HOVER).text_color(theme::TEXT))
                .on_click(move |_, window, cx| {
                    entity.update(cx, |pane, cx| {
                        if pane.filter_active {
                            pane.stop_filter(cx);
                        } else {
                            pane.start_filter(window, cx);
                        }
                    });
                })
                .child(svg().path(icons::SEARCH).w(px(11.)).h(px(11.)))
        };

        let preview_button = {
            let entity = cx.entity();
            let open = self.preview_open;
            div()
                .id(gpui::ElementId::Name(
                    format!("pane{}-preview", self.side.index()).into(),
                ))
                .flex()
                .items_center()
                .justify_center()
                .w(px(21.))
                .h(px(20.))
                .rounded_sm()
                .text_color(if open {
                    theme::ACCENT
                } else {
                    theme::TEXT_FAINT
                })
                .hover(|this| this.bg(theme::BG_HOVER).text_color(theme::TEXT))
                .on_click(move |_, _, cx| {
                    entity.update(cx, |pane, cx| pane.toggle_preview(cx));
                })
                .child(svg().path(icons::EYE).w(px(12.)).h(px(12.)))
        };

        let _ = (focused, window);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(31.0))
            .flex_shrink_0()
            .px_2()
            .bg(theme::BG)
            .border_b_1()
            .border_color(theme::BORDER_SOFT)
            .child(up)
            .child(path_area)
            .child(filter_button)
            .child(preview_button)
            .into_any_element()
    }

    fn render_filter_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let matched = self.listing.len();
        let total = self.listing.entries.len();
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
            .bg(theme::BG_RAISED)
            .border_b_1()
            .border_color(theme::BORDER_SOFT)
            .child(
                svg()
                    .path(icons::SEARCH)
                    .w(px(10.))
                    .h(px(10.))
                    .flex_shrink_0()
                    .text_color(theme::ACCENT),
            )
            .child(div().flex_1().min_w_0().child(input))
            .child(
                div()
                    .text_xs()
                    .flex_shrink_0()
                    .text_color(theme::TEXT_FAINT)
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
            .h(px(21.0))
            .flex_shrink_0()
            .bg(theme::BG_PANE)
            .border_b_1()
            .border_color(theme::BORDER_SOFT)
            .child(div().w(px(ICON_WIDTH + 8.0)).flex_shrink_0());

        for (key, label, width) in [
            (SortKey::Name, "Name", None),
            (SortKey::Size, "Size", Some(SIZE_WIDTH)),
            (SortKey::Modified, "Modified", Some(TIME_WIDTH)),
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
                    .when_some(width, |this, width| this.w(px(width)).justify_end())
                    .when(width.is_none(), |this| this.flex_1().min_w_0())
                    .text_color(if active {
                        theme::TEXT
                    } else {
                        theme::TEXT_FAINT
                    })
                    .hover(|this| this.text_color(theme::TEXT_MUTED))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |pane, cx| pane.sort_by(key, cx));
                    })
                    .child(format!("{label}{arrow}")),
            );
        }
        row.into_any_element()
    }

    fn render_body(&mut self, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        if let Some(error) = self.error.clone() {
            return render_message(icons::CLOSE, "Cannot open this folder", &error, theme::RED);
        }
        if self.loading && self.listing.entries.is_empty() {
            return render_message(icons::FOLDER, "Loading…", "", theme::TEXT_MUTED);
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
            return render_message(icons::FOLDER, &title, &detail, theme::TEXT_FAINT);
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
                            let is_cursor = position == cursor && focused;
                            rows.push(render_row(
                                pane.side.index(),
                                position,
                                entry,
                                is_selected,
                                is_cursor,
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
        let uri = self.current_uri();
        let scheme = uri.scheme.to_string();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(theme::STATUS_HEIGHT))
            .flex_shrink_0()
            .px_2()
            .bg(theme::BG)
            .border_t_1()
            .border_color(theme::BORDER_SOFT)
            .text_xs()
            .text_color(theme::TEXT_FAINT)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .child(format!("{} · {}", scheme, self.sort.key.label()))
                    .when(self.filter.is_active(), |this| {
                        this.child(
                            div()
                                .text_color(theme::ACCENT)
                                .child(format!("filter: {}", self.filter.query())),
                        )
                    }),
            )
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
    selected: bool,
    cursor: bool,
    entity: Entity<Pane>,
) -> AnyElement {
    let color = theme::kind_color(entry);
    let is_dir = entry.is_dir();
    let name = entry.name.clone();
    let size = if is_dir {
        "—".to_string()
    } else {
        format_size(entry.size)
    };
    let modified = format_relative(entry.modified);
    let icon = if is_dir {
        icons::FOLDER
    } else if entry.kind == FileKind::Symlink {
        icons::LINK
    } else {
        icons::FILE
    };

    let click_entity = entity.clone();
    let right_entity = entity;

    div()
        .id(gpui::ElementId::Name(
            format!("{side}-row-{position}").into(),
        ))
        .flex()
        .flex_row()
        .items_center()
        .h(px(ROW_HEIGHT))
        .w_full()
        .cursor_pointer()
        .border_l_2()
        .border_color(if cursor {
            theme::ACCENT
        } else {
            Hsla::transparent_black()
        })
        .when(selected, |this| {
            this.bg(theme::BG_SELECTED).text_color(gpui::white())
        })
        .when(!selected, |this| this.text_color(theme::TEXT))
        .hover(|this| {
            if selected {
                this.bg(theme::BG_SELECTED)
            } else {
                this.bg(theme::BG_HOVER)
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
        .on_mouse_down(MouseButton::Right, move |_, _, cx| {
            right_entity.update(cx, |pane, cx| {
                pane.set_cursor_position(position, false, cx);
            });
        })
        .child(
            div()
                .w(px(ICON_WIDTH))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(
                    svg()
                        .path(icon)
                        .w(px(13.))
                        .h(px(13.))
                        .text_color(if selected { gpui::white() } else { color }),
                ),
        )
        .child(div().flex_1().min_w_0().text_sm().truncate().child(name))
        .child(
            div()
                .w(px(SIZE_WIDTH))
                .px_2()
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_xs()
                .when(!selected, |this| this.text_color(theme::TEXT_MUTED))
                .child(size),
        )
        .child(
            div()
                .w(px(TIME_WIDTH))
                .px_2()
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_xs()
                .when(!selected, |this| this.text_color(theme::TEXT_FAINT))
                .child(modified),
        )
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
                .text_color(theme::TEXT_MUTED)
                .child(title.to_string()),
        )
        .when(!detail.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .max_w(px(300.))
                    .text_color(theme::TEXT_FAINT)
                    .child(detail.to_string()),
            )
        })
        .into_any_element()
}
