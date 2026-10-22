//! The transfer / task panel: everything slower than a frame lands here.
//!
//! Snapshots are polled on a timer rather than pushed from worker threads: the
//! workers publish into atomics, the panel samples them ~10x a second, which keeps
//! the UI thread free and the cost flat no matter how many tasks run.

use crate::i18n::{tr, trf};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, Context, Render, StatefulInteractiveElement, Window, div, prelude::*, px, svg,
};
use vfs_core::{TaskSnapshot, TaskState, entry::format_size, ops::TaskManager};

use crate::{icons, theme};

/// How often the panel refreshes while tasks are running.
const TICK: Duration = Duration::from_millis(120);

pub struct TaskPanel {
    tasks: Arc<TaskManager>,
    pub open: bool,
    snapshots: Vec<TaskSnapshot>,
    /// Kept so the panel can stop ticking when nothing is running.
    ticking: bool,
    last_tick: Option<Instant>,
    focus_handle: gpui::FocusHandle,
}

impl gpui::Focusable for TaskPanel {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl TaskPanel {
    pub fn new(tasks: Arc<TaskManager>, cx: &mut Context<Self>) -> Self {
        Self {
            tasks,
            open: false,
            snapshots: Vec::new(),
            ticking: false,
            last_tick: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        if self.open {
            self.refresh(cx);
            self.start_ticking(cx);
        }
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.snapshots = self.tasks.snapshots();
        self.last_tick = Some(Instant::now());
        cx.notify();
    }

    /// Poll until nothing is left running, then stop.
    fn start_ticking(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this
                    .update(cx, |panel, cx| {
                        panel.snapshots = panel.tasks.snapshots();
                        panel.last_tick = Some(Instant::now());
                        let active = panel.snapshots.iter().any(|task| !task.is_terminal());
                        let visible = panel.open;
                        cx.notify();
                        active && visible
                    })
                    .unwrap_or(false);
                if !alive {
                    this.update(cx, |panel, _| {
                        panel.ticking = false;
                    })
                    .ok();
                    break;
                }
            }
        })
        .detach();
    }

    pub fn active_count(&self) -> usize {
        self.snapshots
            .iter()
            .filter(|task| !task.is_terminal())
            .count()
    }

    pub fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        self.tasks.cancel(id);
        self.refresh(cx);
    }

    pub fn clear_finished(&mut self, cx: &mut Context<Self>) {
        self.tasks.clear_finished();
        self.refresh(cx);
    }
}

impl Render for TaskPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        render_panel(self, cx).into_any_element()
    }
}

fn render_panel(panel: &mut TaskPanel, cx: &mut Context<TaskPanel>) -> AnyElement {
    let active = panel.active_count();
    let entity = cx.entity();

    let header =
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(28.0))
            .flex_shrink_0()
            .px_2()
            .bg(theme::bg())
            .border_b_1()
            .border_color(theme::border_soft())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(svg().path(icons::TASKS).w(px(12.)).h(px(12.)).text_color(
                        if active > 0 {
                            theme::accent()
                        } else {
                            theme::text_faint()
                        },
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(if active > 0 {
                                trf("Tasks · {} running", &[&active.to_string()])
                            } else {
                                tr("Tasks").to_string()
                            }),
                    ),
            )
            .child({
                let entity = entity.clone();
                div()
                    .id("tasks-clear")
                    .px_2()
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .text_xs()
                    .text_color(theme::text_faint())
                    .hover(|this| this.bg(theme::bg_hover()).text_color(theme::text()))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |panel, cx| panel.clear_finished(cx));
                    })
                    .child(tr("Clear finished"))
            });

    let body: AnyElement = if panel.snapshots.is_empty() {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_faint())
                    .child(tr("No transfers yet")),
            )
            .into_any_element()
    } else {
        div()
            .id("tasks-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(panel.snapshots.iter().rev().map(|task| {
                let entity = entity.clone();
                render_task(task, entity)
            }))
            .into_any_element()
    };

    div()
        .flex()
        .flex_col()
        .w_full()
        .flex_shrink_0()
        .h(px(200.0))
        .bg(theme::bg_pane())
        .border_t_1()
        .border_color(theme::border())
        .child(header)
        .child(body)
        .into_any_element()
}

fn render_task(task: &TaskSnapshot, entity: gpui::Entity<TaskPanel>) -> AnyElement {
    let (state_color, state_label) = match &task.state {
        TaskState::Queued => (theme::text_faint(), tr("queued").to_string()),
        TaskState::Running => (theme::accent(), tr("running").to_string()),
        TaskState::Done => (theme::green(), tr("done").to_string()),
        TaskState::Cancelled => (theme::text_faint(), tr("cancelled").to_string()),
        TaskState::Failed(message) => (theme::red(), message.clone()),
    };

    let detail = match task.progress() {
        Some(progress) => {
            let speed = task
                .speed_bps
                .map(|bps| format!("{}/s", format_size(bps as u64)))
                .unwrap_or_default();
            format!(
                "{} / {} · {:.0}%{}",
                format_size(task.done_bytes),
                format_size(task.total_bytes),
                progress * 100.0,
                if speed.is_empty() {
                    String::new()
                } else {
                    format!(" · {speed}")
                }
            )
        }
        None => state_label.clone(),
    };

    let task_id = task.id;
    let cancel_button = (!task.is_terminal()).then(|| {
        let entity = entity.clone();
        div()
            .id(gpui::ElementId::Name(
                format!("task-cancel-{task_id}").into(),
            ))
            .flex()
            .items_center()
            .justify_center()
            .w(px(16.))
            .h(px(16.))
            .rounded_sm()
            .text_color(theme::text_faint())
            .hover(|this| this.bg(theme::bg_hover()).text_color(theme::red()))
            .on_click(move |_, _, cx| {
                entity.update(cx, |panel, cx| panel.cancel(task_id, cx));
            })
            .child(
                svg()
                    .path(icons::CLOSE)
                    .w(px(8.))
                    .h(px(8.))
                    .text_color(theme::text_faint()),
            )
    });

    let ratio = task.progress().unwrap_or(0.0);

    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(theme::border_soft())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text())
                        .truncate()
                        .min_w_0()
                        .flex_1()
                        .child(format!("{} {}", task.kind.verb(), task.label)),
                )
                .children(cancel_button),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    // Progress bar: two stacked divs, no canvas needed.
                    div()
                        .flex_1()
                        .h(px(3.))
                        .rounded_full()
                        .bg(theme::bg_raised())
                        .child(div().h_full().w(gpui::relative(ratio)).rounded_full().bg(
                            if task.state == TaskState::Done {
                                theme::green()
                            } else {
                                theme::accent()
                            },
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .flex_shrink_0()
                        .text_color(state_color)
                        .child(detail),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs_core::ops::ConflictPolicy;

    #[test]
    fn panel_reflects_finished_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.txt");
        std::fs::write(&source, b"x").unwrap();
        let manager = Arc::new(TaskManager::new(1));
        manager.copy(source, dir.path().join("b.txt"), ConflictPolicy::Overwrite);
        manager.wait_idle();
        let snapshots = manager.snapshots();
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0].is_terminal());
        assert_eq!(snapshots[0].progress(), Some(1.0));
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use vfs_core::ops::{ConflictPolicy, TaskKind};

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn finished_task_reports_full_progress() {
        let dir = tempdir();
        std::fs::write(dir.path().join("a.txt"), vec![0u8; 100]).unwrap();
        let manager = Arc::new(TaskManager::new(1));
        manager.copy(
            dir.path().join("a.txt"),
            dir.path().join("b.txt"),
            ConflictPolicy::Overwrite,
        );
        manager.wait_idle();

        let snapshot = manager.snapshots().pop().unwrap();
        assert_eq!(snapshot.kind, TaskKind::Copy);
        assert_eq!(snapshot.progress(), Some(1.0));
        assert!(snapshot.is_terminal());
    }

    #[test]
    fn failed_task_is_terminal_but_has_no_progress() {
        let dir = tempdir();
        let manager = Arc::new(TaskManager::new(1));
        manager.copy(
            dir.path().join("missing.txt"),
            dir.path().join("out.txt"),
            ConflictPolicy::Overwrite,
        );
        manager.wait_idle();

        let snapshot = manager.snapshots().pop().unwrap();
        assert!(snapshot.is_terminal());
        assert_eq!(snapshot.progress(), None, "nothing was transferred");
    }

    #[test]
    fn clearing_finished_tasks_leaves_nothing_behind() {
        let dir = tempdir();
        let manager = Arc::new(TaskManager::new(1));
        manager.create_dir(dir.path().join("x"));
        manager.wait_idle();
        assert_eq!(manager.snapshots().len(), 1);
        manager.clear_finished();
        assert!(manager.snapshots().is_empty());
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn cancel_by_unknown_id_reports_false() {
        let manager = Arc::new(TaskManager::new(1));
        assert!(!manager.cancel(424_242));
    }

    #[test]
    fn snapshots_are_ordered_oldest_first() {
        let dir = tempdir();
        let manager = Arc::new(TaskManager::new(1));
        manager.create_dir(dir.path().join("first"));
        manager.wait_idle();
        manager.create_dir(dir.path().join("second"));
        manager.wait_idle();

        let ids: Vec<u64> = manager.snapshots().iter().map(|task| task.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "the panel reverses them for display");
    }
}
