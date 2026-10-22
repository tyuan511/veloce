//! The icon grid and the column browser: the two Finder view modes besides the
//! list. They live beside `pane.rs` because they read the pane's private state.

use std::path::Path;

use gpui::{ClickEvent, Stateful};

use super::*;
use crate::{
    info::kind_label,
    view::{
        COLUMN_WIDTH, GRID_PADDING, ICON_CELL_H, ICON_CELL_W, MAX_COLUMN_DIRS, column_dirs,
        grid_columns, grid_rows,
    },
};

impl Pane {
    // ---- icon grid --------------------------------------------------------

    pub(super) fn render_icons_body(
        &mut self,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cols = grid_columns(self.avail_width);
        self.grid_cols = cols;
        let count = self.listing.len();
        let rows = grid_rows(count, cols);
        let entity = cx.entity();
        let selected = self.listing.selected.clone();
        let cursor = self.listing.cursor;

        div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                uniform_list(
                    gpui::ElementId::Name(format!("pane{}-grid", self.side.index()).into()),
                    rows,
                    cx.processor(move |pane, range: std::ops::Range<usize>, _window, _cx| {
                        range
                            .map(|row| {
                                let mut line = div()
                                    .flex()
                                    .flex_row()
                                    .h(px(ICON_CELL_H))
                                    .px(px(GRID_PADDING));
                                for position in row * cols..((row + 1) * cols).min(count) {
                                    let Some(index) = pane.listing.visible.get(position) else {
                                        continue;
                                    };
                                    let Some(entry) = pane.listing.entries.get(*index) else {
                                        continue;
                                    };
                                    let highlighted =
                                        selected.contains(index) || position == cursor;
                                    line = line.child(render_cell(
                                        pane.side.index(),
                                        position,
                                        entry,
                                        highlighted,
                                        focused,
                                        pane.is_cut(entry),
                                        entity.clone(),
                                    ));
                                }
                                line.into_any_element()
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.scroll.clone())
                .h_full(),
            )
            .into_any_element()
    }

    /// Whether `entry` is waiting to be moved by a Paste.
    pub(super) fn is_cut(&self, entry: &FileEntry) -> bool {
        !self.cut_paths.is_empty()
            && entry
                .uri
                .as_local_path()
                .is_some_and(|path| self.cut_paths.contains(&path))
    }

    // ---- columns ----------------------------------------------------------

    /// Load one folder for a side column, off the UI thread, once.
    pub(super) fn request_column(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.columns.contains_key(path) || self.columns_loading.contains(path) {
            return;
        }
        self.columns_loading.insert(path.to_path_buf());
        let key = path.to_path_buf();
        let uri = VfsUri::local(path);
        let provider = self.services.provider.clone();
        let sort = self.sort;
        cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            let mut entries: Vec<FileEntry> = Vec::new();
            // An unreadable folder shows as empty; caching that also stops retries.
            if let Ok(mut stream) = provider.list_stream(&uri).await {
                while let Some(Ok(chunk)) = stream.next().await {
                    entries.extend(chunk);
                }
            }
            sort.sort(&mut entries);
            this.update(cx, |pane, cx| {
                pane.columns_loading.remove(&key);
                pane.columns.insert(key, Arc::new(entries));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Forget cached side columns (after a refresh or a sort change).
    pub fn invalidate_columns(&mut self) {
        self.columns.clear();
        self.columns_scrolled.clear();
    }

    /// Click in a side column: go to that folder with the clicked entry selected,
    /// which is how Finder's columns move right.
    pub(super) fn open_column_entry(
        &mut self,
        dir: PathBuf,
        entry: FileEntry,
        double: bool,
        cx: &mut Context<Self>,
    ) {
        let here = self.current_uri().as_local_path();
        if here.as_deref() == Some(dir.as_path()) {
            if let Some(position) = self.listing.position_of(&entry.uri) {
                self.set_cursor_position(position, false, cx);
            }
        } else {
            self.pending_focus = Some(entry.uri.clone());
            self.navigate_to(VfsUri::local(&dir), cx);
        }
        if double {
            self.activate_cursor(cx);
        }
    }

    pub(super) fn render_columns_body(
        &mut self,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(current) = self.current_uri().as_local_path() else {
            return self.render_list_body(focused, cx);
        };
        let dirs = column_dirs(&current, MAX_COLUMN_DIRS);
        let side = self.side.index();
        let mut row = div().flex().flex_row().flex_1().min_h_0().overflow_hidden();

        // Ancestor columns, with the folder we came through highlighted.
        for (index, dir) in dirs.iter().enumerate() {
            if index + 1 == dirs.len() {
                break;
            }
            self.request_column(dir, cx);
            let through = dirs[index + 1].clone();
            row =
                row.child(self.render_side_column(index, dir.clone(), Some(through), focused, cx));
        }

        // The folder itself: the live, interactive listing.
        row = row.child(
            div()
                .w(px(COLUMN_WIDTH))
                .flex_shrink_0()
                .h_full()
                .border_r_1()
                .border_color(theme::border_soft())
                .child(self.render_active_column(focused, cx)),
        );

        // Detail column: the selected folder's contents, or the selected file.
        let cursor = self.cursor_entry().cloned();
        let detail: AnyElement = match cursor {
            Some(entry) if entry.is_dir() => match entry.uri.as_local_path() {
                Some(path) => {
                    self.request_column(&path, cx);
                    self.render_side_column(dirs.len(), path, None, focused, cx)
                }
                None => div().into_any_element(),
            },
            Some(entry) => render_file_detail(&entry),
            None => div().into_any_element(),
        };
        let _ = side;
        row.child(detail).into_any_element()
    }

    /// The interactive column: the pane's own listing in the compact row style.
    fn render_active_column(&mut self, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        if let Some(message) = self.state_message() {
            return message;
        }
        let count = self.listing.len();
        let side = self.side.index();
        let entity = cx.entity();
        let selected = self.listing.selected.clone();
        let cursor = self.listing.cursor;
        uniform_list(
            gpui::ElementId::Name(format!("pane{side}-activecol").into()),
            count,
            cx.processor(move |pane, range: std::ops::Range<usize>, _window, _cx| {
                range
                    .filter_map(|position| {
                        let index = *pane.listing.visible.get(position)?;
                        let entry = pane.listing.entries.get(index)?.clone();
                        let highlighted = selected.contains(&index) || position == cursor;
                        let (background, on_accent) = highlight_colors(highlighted, focused);
                        let click = entity.clone();
                        let right = entity.clone();
                        let dimmed = pane.is_cut(&entry);
                        Some(render_column_row(
                            format!("pane{side}-activecol-{position}"),
                            &entry,
                            background,
                            on_accent,
                            dimmed,
                            move |event: &ClickEvent, _, cx| {
                                let shift = event.modifiers().shift;
                                let double = event.click_count() >= 2;
                                click.update(cx, |pane, cx| {
                                    pane.set_cursor_position(position, shift, cx);
                                    if double {
                                        pane.activate_cursor(cx);
                                    }
                                });
                            },
                            Some((right, position)),
                        ))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.scroll.clone())
        .h_full()
        .into_any_element()
    }

    /// A read-only column fed from the cache. `through` is the child folder to
    /// highlight (grey, like Finder's inactive selection).
    fn render_side_column(
        &mut self,
        index: usize,
        dir: PathBuf,
        through: Option<PathBuf>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let side = self.side.index();
        let Some(data) = self.columns.get(&dir).cloned() else {
            return div()
                .w(px(COLUMN_WIDTH))
                .flex_shrink_0()
                .h_full()
                .border_r_1()
                .border_color(theme::border_soft())
                .p_3()
                .text_xs()
                .text_color(theme::text_faint())
                .child("Loading…")
                .into_any_element();
        };

        let handle = self.column_scrolls.entry(dir.clone()).or_default().clone();
        // Bring the highlighted folder into view once per load; afterwards the
        // user's own scrolling is left alone.
        if let (Some(through), false) = (&through, self.columns_scrolled.contains(&dir)) {
            if let Some(at) = data
                .iter()
                .position(|e| e.uri.as_local_path().as_deref() == Some(through.as_path()))
            {
                handle.scroll_to_item(at, gpui::ScrollStrategy::Center);
            }
            self.columns_scrolled.insert(dir.clone());
        }

        let entity = cx.entity();
        let count = data.len();
        let list_dir = dir.clone();
        let list = if count == 0 {
            div()
                .p_3()
                .text_xs()
                .text_color(theme::text_faint())
                .child("Empty")
                .into_any_element()
        } else {
            uniform_list(
                gpui::ElementId::Name(format!("pane{side}-col{index}").into()),
                count,
                cx.processor(move |pane, range: std::ops::Range<usize>, _window, _cx| {
                    let Some(data) = pane.columns.get(&list_dir).cloned() else {
                        return Vec::new();
                    };
                    range
                        .filter_map(|position| {
                            let entry = data.get(position)?.clone();
                            let is_through = through.as_ref().is_some_and(|through| {
                                entry.uri.as_local_path().as_deref() == Some(through.as_path())
                            });
                            let dir = list_dir.clone();
                            let entity = entity.clone();
                            let clicked = entry.clone();
                            Some(render_column_row(
                                format!("pane{side}-col{index}-{position}"),
                                &entry,
                                is_through.then(theme::bg_selected_inactive),
                                false,
                                pane.is_cut(&entry),
                                move |event: &ClickEvent, _, cx| {
                                    let double = event.click_count() >= 2;
                                    entity.update(cx, |pane, cx| {
                                        pane.open_column_entry(
                                            dir.clone(),
                                            clicked.clone(),
                                            double,
                                            cx,
                                        )
                                    });
                                },
                                None,
                            ))
                        })
                        .collect()
                }),
            )
            .track_scroll(handle)
            .h_full()
            .into_any_element()
        };
        let _ = focused;

        div()
            .w(px(COLUMN_WIDTH))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(theme::border_soft())
            .child(list)
            .into_any_element()
    }
}

// ---- free rendering helpers ------------------------------------------------

/// Click and right-click behaviour shared by every selectable item.
pub(super) fn attach_pointer(
    item: Stateful<gpui::Div>,
    position: usize,
    entity: Entity<Pane>,
) -> Stateful<gpui::Div> {
    let click_entity = entity.clone();
    item.on_click(move |event, _, cx| {
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
        // The list background also listens for right-clicks; the item owns this one.
        cx.stop_propagation();
        let at = event.position;
        entity.update(cx, |pane, cx| pane.context_click(Some(position), at, cx));
    })
}

fn highlight_colors(highlighted: bool, pane_focused: bool) -> (Option<Hsla>, bool) {
    let on_accent = highlighted && pane_focused;
    let background = highlighted.then(|| {
        if pane_focused {
            theme::bg_selected()
        } else {
            theme::bg_selected_inactive()
        }
    });
    (background, on_accent)
}

/// One icon-view cell: large icon over a two-line name.
fn render_cell(
    side: u8,
    position: usize,
    entry: &FileEntry,
    highlighted: bool,
    pane_focused: bool,
    dimmed: bool,
    entity: Entity<Pane>,
) -> AnyElement {
    let (background, on_accent) = highlight_colors(highlighted, pane_focused);
    let cell = div()
        .id(gpui::ElementId::Name(
            format!("{side}-cell-{position}").into(),
        ))
        .flex()
        .flex_col()
        .items_center()
        .w(px(ICON_CELL_W))
        .h(px(ICON_CELL_H))
        .flex_shrink_0()
        .pt_2()
        .px_1()
        .gap_1()
        .rounded_md()
        .cursor_pointer()
        .when(dimmed, |this| this.opacity(0.5))
        .when_some(background, |this, color| this.bg(color))
        .when(!highlighted, |this| {
            this.hover(|this| this.bg(theme::bg_raised()))
        })
        .text_color(if on_accent {
            gpui::white()
        } else {
            theme::text()
        })
        .child(entry_icon(entry, 48.0))
        .child(
            div()
                .w_full()
                .text_xs()
                .text_center()
                .line_clamp(2)
                .child(entry.name.clone()),
        );
    attach_pointer(cell, position, entity).into_any_element()
}

/// One row in a column: small icon, name, and a chevron for folders.
pub(super) fn render_column_row(
    id: String,
    entry: &FileEntry,
    background: Option<Hsla>,
    on_accent: bool,
    dimmed: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    right_click: Option<(Entity<Pane>, usize)>,
) -> AnyElement {
    let text = if on_accent {
        gpui::white()
    } else {
        theme::text()
    };
    div()
        .w_full()
        .h(px(26.))
        .px_1()
        .child(
            div()
                .id(gpui::ElementId::Name(id.into()))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .size_full()
                .px_2()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .text_color(text)
                .when(dimmed, |this| this.opacity(0.5))
                .when_some(background, |this, color| this.bg(color))
                .on_click(on_click)
                .when_some(right_click, |this, (entity, position)| {
                    this.on_mouse_down(MouseButton::Right, move |event, _, cx| {
                        cx.stop_propagation();
                        let at = event.position;
                        entity.update(cx, |pane, cx| pane.context_click(Some(position), at, cx));
                    })
                })
                .child(entry_icon(entry, 16.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(entry.name.clone()),
                )
                .when(entry.is_dir(), |this| {
                    this.child(
                        svg()
                            .path(icons::CHEVRON_RIGHT)
                            .w(px(10.))
                            .h(px(10.))
                            .flex_shrink_0()
                            .text_color(if on_accent {
                                gpui::white()
                            } else {
                                theme::text_faint()
                            }),
                    )
                }),
        )
        .into_any_element()
}

/// The rightmost column when a file is selected: icon plus basic facts.
fn render_file_detail(entry: &FileEntry) -> AnyElement {
    let rows = [
        ("Kind", kind_label(entry)),
        ("Size", format_size(entry.size)),
        ("Modified", format_relative(entry.modified)),
    ];
    div()
        .flex_1()
        .min_w(px(240.))
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .gap_3()
        .p_4()
        .child(entry_icon(entry, 96.0))
        .child(
            div()
                .text_sm()
                .text_center()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(entry.name.clone()),
        )
        .children(rows.into_iter().map(|(label, value)| {
            div()
                .w_full()
                .flex()
                .flex_row()
                .justify_between()
                .gap_3()
                .text_xs()
                .child(div().text_color(theme::text_faint()).child(label))
                .child(
                    div()
                        .text_color(theme::text_muted())
                        .truncate()
                        .child(value),
                )
        }))
        .into_any_element()
}
