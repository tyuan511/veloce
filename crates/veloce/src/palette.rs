//! The command palette (`⌘P`): fuzzy jump to any file in the workspace, or run a
//! command.
//!
//! Search runs on a background thread through `vfs_core::search_recursive`
//! (`ignore` + `nucleo`); results stream back in batches so the list fills while
//! the walk is still going. A new keystroke cancels the previous walk.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{
    AnyElement, Context, Entity, Focusable, Render, StatefulInteractiveElement, Window, div,
    prelude::*, px, svg,
};
use vfs_core::{
    SearchHit,
    search::{SearchOptions, rank, search_recursive},
};

use crate::{
    icons,
    text_input::{TextInput, TextInputEvent},
    theme,
};

/// What the palette is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    /// Fuzzy file jump (`⌘P`).
    Files,
    /// Command runner (`⌘⇧P`).
    Commands,
}

/// A runnable command shown in `Commands` mode.
#[derive(Debug, Clone)]
pub struct Command {
    pub label: &'static str,
    pub hint: &'static str,
    /// Action id the workspace dispatches when this row is chosen.
    pub id: CommandId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    NewFolder,
    NewTab,
    TogglePreview,
    ToggleTasks,
    CopyToOtherPane,
    MoveToOtherPane,
    TrashSelection,
    Refresh,
    CycleSort,
    TogglePane,
}

/// Every command the palette can run.
pub const COMMANDS: &[Command] = &[
    Command {
        label: "New Folder",
        hint: "⌘⇧N",
        id: CommandId::NewFolder,
    },
    Command {
        label: "New Tab",
        hint: "⌘T",
        id: CommandId::NewTab,
    },
    Command {
        label: "Toggle Preview",
        hint: "Space",
        id: CommandId::TogglePreview,
    },
    Command {
        label: "Toggle Task Panel",
        hint: "⌘J",
        id: CommandId::ToggleTasks,
    },
    Command {
        label: "Copy to Other Pane",
        hint: "F5",
        id: CommandId::CopyToOtherPane,
    },
    Command {
        label: "Move to Other Pane",
        hint: "F6",
        id: CommandId::MoveToOtherPane,
    },
    Command {
        label: "Move Selection to Trash",
        hint: "⌘⌫",
        id: CommandId::TrashSelection,
    },
    Command {
        label: "Refresh",
        hint: "⌘R",
        id: CommandId::Refresh,
    },
    Command {
        label: "Cycle Sort Order",
        hint: "S",
        id: CommandId::CycleSort,
    },
    Command {
        label: "Switch Pane",
        hint: "Tab",
        id: CommandId::TogglePane,
    },
];

pub struct Palette {
    pub open: bool,
    pub mode: PaletteMode,
    pub input: Entity<TextInput>,
    /// Root the file search walks (the focused pane's directory).
    pub root: Option<PathBuf>,
    pub hits: Vec<SearchHit>,
    pub cursor: usize,
    pub searching: bool,
    /// Command rows filtered by the current query.
    pub commands: Vec<&'static Command>,
    query: String,
    cancel: Arc<AtomicBool>,
    focus_handle: gpui::FocusHandle,
    _subscription: gpui::Subscription,
}

impl gpui::Focusable for Palette {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Palette {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Search files, or type a command…", cx));
        let subscription = cx.subscribe(
            &input,
            |palette, _, event: &TextInputEvent, cx| match event {
                TextInputEvent::Change(text) => palette.set_query(text.clone(), cx),
                TextInputEvent::Submit(_) => palette.activate(cx),
                TextInputEvent::Cancel => palette.close(cx),
            },
        );
        Self {
            open: false,
            mode: PaletteMode::Files,
            input,
            root: None,
            hits: Vec::new(),
            cursor: 0,
            searching: false,
            commands: COMMANDS.iter().collect(),
            query: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    pub fn show(
        &mut self,
        mode: PaletteMode,
        root: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open = true;
        self.mode = mode;
        self.root = root;
        self.hits.clear();
        self.cursor = 0;
        self.query.clear();
        self.commands = COMMANDS.iter().collect();
        self.input.update(cx, |input, cx| input.clear(cx));
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.cancel.store(true, Ordering::Relaxed);
        self.searching = false;
        cx.notify();
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query.clone();
        self.cursor = 0;
        match self.mode {
            PaletteMode::Commands => {
                let needle = query.to_lowercase();
                self.commands = COMMANDS
                    .iter()
                    .filter(|command| {
                        needle.is_empty() || command.label.to_lowercase().contains(&needle)
                    })
                    .collect();
                cx.notify();
            }
            PaletteMode::Files => self.start_search(query, cx),
        }
    }

    fn start_search(&mut self, query: String, cx: &mut Context<Self>) {
        // Cancel the previous walk; a new keystroke always wins.
        self.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.hits.clear();
        self.searching = true;

        let Some(root) = self.root.clone() else {
            self.searching = false;
            cx.notify();
            return;
        };
        if query.trim().is_empty() {
            self.searching = false;
            cx.notify();
            return;
        }

        cx.notify();
        let options = SearchOptions::default();
        cx.spawn(async move |this, cx| {
            let (tx, rx) = async_channel::bounded::<Vec<SearchHit>>(8);
            // The walk runs on the background executor; the channel carries hits
            // back. Dropping the task handle is fine because the walker owns its
            // own copy of the sender and finishes on its own.
            cx.background_spawn({
                let cancel = cancel.clone();
                let query = query.clone();
                let root = root.clone();
                async move {
                    let sink = tx.clone();
                    let _ = search_recursive(&root, &query, &options, cancel, move |batch| {
                        // A full channel means the UI is behind; dropping the batch
                        // is better than blocking the walker.
                        let _ = sink.try_send(batch);
                    });
                    drop(tx);
                }
            })
            .detach();

            while let Ok(batch) = rx.recv().await {
                let done = this
                    .update(cx, |palette, cx| {
                        palette.hits.extend(batch);
                        rank(&mut palette.hits);
                        palette.hits.truncate(200);
                        palette.cursor = palette.cursor.min(palette.hits.len().saturating_sub(1));
                        cx.notify();
                    })
                    .is_err();
                if done {
                    return;
                }
            }

            this.update(cx, |palette, cx| {
                palette.searching = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.row_count();
        if len == 0 {
            return;
        }
        let max = len as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
        cx.notify();
    }

    pub fn row_count(&self) -> usize {
        match self.mode {
            PaletteMode::Files => self.hits.len(),
            PaletteMode::Commands => self.commands.len(),
        }
    }

    /// What the current row means; the workspace turns this into a real action.
    pub fn activated(&self) -> Option<PaletteChoice> {
        match self.mode {
            PaletteMode::Files => self
                .hits
                .get(self.cursor)
                .map(|hit| PaletteChoice::File(hit.path.clone())),
            PaletteMode::Commands => self
                .commands
                .get(self.cursor)
                .map(|command| PaletteChoice::Command(command.id)),
        }
    }

    fn activate(&mut self, cx: &mut Context<Self>) {
        if let Some(choice) = self.activated() {
            cx.emit(PaletteEvent::Chosen(choice));
            self.close(cx);
        }
    }
}

/// What a palette row resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteChoice {
    File(PathBuf),
    Command(CommandId),
}

#[derive(Debug, Clone)]
pub enum PaletteEvent {
    Chosen(PaletteChoice),
}

impl gpui::EventEmitter<PaletteEvent> for Palette {}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        render_palette(self, cx).into_any_element()
    }
}

fn render_palette(palette: &mut Palette, cx: &mut Context<Palette>) -> AnyElement {
    let entity = cx.entity();
    let title = match palette.mode {
        PaletteMode::Files => "Jump to file",
        PaletteMode::Commands => "Run command",
    };
    let rows: AnyElement = match palette.mode {
        PaletteMode::Files => {
            if palette.hits.is_empty() {
                div()
                    .p_3()
                    .text_xs()
                    .text_color(theme::text_faint())
                    .child(if palette.searching {
                        "Searching…".to_string()
                    } else if palette.query.is_empty() {
                        "Type to search this folder".to_string()
                    } else {
                        "No matches".to_string()
                    })
                    .into_any_element()
            } else {
                div()
                    .id("palette-hits")
                    .flex()
                    .flex_col()
                    .max_h(px(340.))
                    .overflow_y_scroll()
                    .children(palette.hits.iter().enumerate().map(|(index, hit)| {
                        let selected = index == palette.cursor;
                        let entity = entity.clone();
                        let parent = hit
                            .path
                            .parent()
                            .map(|path| path.to_string_lossy().to_string())
                            .unwrap_or_default();
                        div()
                            .id(gpui::ElementId::Name(format!("hit-{index}").into()))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .h(px(26.))
                            .px_2()
                            .when(selected, |this| {
                                this.bg(theme::bg_selected()).text_color(gpui::white())
                            })
                            .hover(|this| this.bg(theme::bg_hover()))
                            .on_click(move |_, _, cx| {
                                entity.update(cx, |palette, cx| {
                                    palette.cursor = index;
                                    palette.activate(cx);
                                });
                            })
                            .child(
                                svg()
                                    .path(if hit.is_dir {
                                        icons::FOLDER
                                    } else {
                                        icons::FILE
                                    })
                                    .w(px(12.))
                                    .h(px(12.))
                                    .flex_shrink_0()
                                    .text_color(if hit.is_dir {
                                        theme::blue()
                                    } else {
                                        theme::text_muted()
                                    }),
                            )
                            .child(div().text_xs().flex_shrink_0().child(hit.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .when(!selected, |this| this.text_color(theme::text_faint()))
                                    .child(parent),
                            )
                    }))
                    .into_any_element()
            }
        }
        PaletteMode::Commands => div()
            .id("palette-commands")
            .flex()
            .flex_col()
            .max_h(px(340.))
            .overflow_y_scroll()
            .children(palette.commands.iter().enumerate().map(|(index, command)| {
                let selected = index == palette.cursor;
                let entity = entity.clone();
                div()
                    .id(gpui::ElementId::Name(format!("cmd-{index}").into()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(26.))
                    .px_2()
                    .when(selected, |this| {
                        this.bg(theme::bg_selected()).text_color(gpui::white())
                    })
                    .hover(|this| this.bg(theme::bg_hover()))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |palette, cx| {
                            palette.cursor = index;
                            palette.activate(cx);
                        });
                    })
                    .child(div().text_xs().child(command.label))
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme::MONO_FAMILY)
                            .when(!selected, |this| this.text_color(theme::text_faint()))
                            .child(command.hint),
                    )
            }))
            .into_any_element(),
    };

    div()
        .absolute()
        .top(px(72.))
        .left(px(0.))
        .right(px(0.))
        .flex()
        .justify_center()
        .child(
            div()
                .w(px(560.))
                .flex()
                .flex_col()
                .rounded_lg()
                .bg(theme::bg_raised())
                .border_1()
                .border_color(theme::border())
                .shadow_lg()
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .h(px(38.))
                        .px_3()
                        .border_b_1()
                        .border_color(theme::border_soft())
                        .child(
                            svg()
                                .path(if palette.mode == PaletteMode::Files {
                                    icons::SEARCH
                                } else {
                                    icons::TERMINAL
                                })
                                .w(px(13.))
                                .h(px(13.))
                                .text_color(theme::accent())
                                .flex_shrink_0(),
                        )
                        .child(div().flex_1().min_w_0().child(palette.input.clone()))
                        .child(div().text_xs().text_color(theme::text_faint()).child(title)),
                )
                .child(rows),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_are_unique() {
        let mut ids: Vec<_> = COMMANDS
            .iter()
            .map(|command| format!("{:?}", command.id))
            .collect();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "two commands share an id");
    }

    #[test]
    fn every_command_has_a_label_and_hint() {
        for command in COMMANDS {
            assert!(!command.label.is_empty());
            assert!(
                !command.hint.is_empty(),
                "{} has no shortcut",
                command.label
            );
        }
    }

    #[test]
    fn command_labels_are_unique() {
        let mut labels: Vec<&str> = COMMANDS.iter().map(|c| c.label).collect();
        labels.sort();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "duplicate command label");
    }

    #[test]
    fn palette_choice_distinguishes_files_from_commands() {
        let file = PaletteChoice::File(PathBuf::from("/tmp/a"));
        let command = PaletteChoice::Command(CommandId::NewTab);
        assert_ne!(file, command);
        assert_eq!(file, PaletteChoice::File(PathBuf::from("/tmp/a")));
    }

    #[test]
    fn command_list_covers_the_core_actions() {
        // These are the actions a user must be able to reach without the keymap.
        let ids: Vec<CommandId> = COMMANDS.iter().map(|c| c.id).collect();
        for required in [
            CommandId::NewFolder,
            CommandId::TogglePreview,
            CommandId::ToggleTasks,
            CommandId::CopyToOtherPane,
            CommandId::TrashSelection,
        ] {
            assert!(ids.contains(&required), "missing {required:?}");
        }
    }
}
