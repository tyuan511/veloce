//! Drawing the right-click menu and its flyout.

use gpui::{AnyElement, Context, Entity, MouseButton, Window};

use super::*;
use crate::context_menu::{ITEM_HEIGHT, SUBMENU_WIDTH, SubMenu, submenu_origin};

/// Which of the two stacked panels a row belongs to. Only top-level rows steer
/// the flyout; rows inside it just run their action.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Main,
    Flyout,
}

impl Workspace {
    pub(super) fn render_context_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let viewport = (
            f32::from(window.viewport_size().width),
            f32::from(window.viewport_size().height),
        );
        let origin = clamp_origin(
            menu.origin,
            (MENU_WIDTH, menu_height(&menu.items)),
            viewport,
        );

        let flyout = menu.open_sub.and_then(|index| match menu.items.get(index) {
            Some(MenuItem::Submenu(sub)) => Some((index, sub)),
            _ => None,
        });

        let main = render_panel(
            &menu.items,
            origin,
            MENU_WIDTH,
            Level::Main,
            menu.open_sub,
            menu.hovered,
            cx,
        );
        let flyout = flyout.map(|(index, sub)| {
            let size = (SUBMENU_WIDTH, menu_height(&sub.items));
            let at = submenu_origin(&menu.items, index, origin, size, viewport);
            render_flyout(sub, at, menu.hovered, cx)
        });

        let left = cx.entity();
        let right = cx.entity();
        Some(
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .right(px(0.))
                .bottom(px(0.))
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| dismiss(&left, cx))
                .on_mouse_down(MouseButton::Right, move |_, _, cx| dismiss(&right, cx))
                .child(main)
                .children(flyout)
                .into_any_element(),
        )
    }
}

fn dismiss(entity: &Entity<Workspace>, cx: &mut gpui::App) {
    entity.update(cx, |workspace, cx| {
        workspace.menu = None;
        cx.notify();
    });
}

fn render_flyout(
    sub: &SubMenu,
    at: (f32, f32),
    hovered: Option<(bool, usize)>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    render_panel(
        &sub.items,
        at,
        SUBMENU_WIDTH,
        Level::Flyout,
        None,
        hovered,
        cx,
    )
}

fn render_panel(
    items: &[MenuItem],
    at: (f32, f32),
    width: f32,
    level: Level,
    open_sub: Option<usize>,
    hovered: Option<(bool, usize)>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    // Menus with check marks reserve a column for them, so labels line up
    // whether or not a particular row is checked.
    let has_checks = items
        .iter()
        .any(|item| matches!(item, MenuItem::Action(a) if a.checked));
    let has_icons = items
        .iter()
        .any(|item| matches!(item, MenuItem::Action(a) if a.icon.is_some()));
    let rows: Vec<AnyElement> = items
        .iter()
        .enumerate()
        .map(|(index, item)| match item {
            MenuItem::Separator => div()
                .my(px(4.))
                .mx_2()
                .h(px(1.))
                .bg(theme::border_soft())
                .into_any_element(),
            MenuItem::Action(item) => {
                render_action_row(item, index, level, has_checks, has_icons, hovered, cx)
            }
            MenuItem::Submenu(sub) => render_submenu_row(
                sub,
                index,
                level,
                open_sub == Some(index),
                has_checks,
                hovered,
                cx,
            ),
        })
        .collect();

    div()
        .absolute()
        .left(px(at.0))
        .top(px(at.1))
        .w(px(width))
        .flex()
        .flex_col()
        .p(px(MENU_PADDING))
        .rounded_lg()
        .bg(theme::bg_raised())
        .border_1()
        .border_color(theme::border())
        .shadow_lg()
        // Clicks inside a menu must not reach the dismissing backdrop.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .children(rows)
        .into_any_element()
}

fn row_shell(
    id: String,
    label: &str,
    has_checks: bool,
    checked: Option<gpui::Hsla>,
    // `Some(None)` reserves the icon column for a row without an icon.
    icon: Option<Option<&std::path::Path>>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .h(px(ITEM_HEIGHT))
        .px_2()
        .rounded_sm()
        .text_sm()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .min_w_0()
                .when(has_checks, |this| {
                    this.child(div().w(px(14.)).flex_shrink_0().when_some(
                        checked,
                        |this, color| {
                            // SVG 不继承行的文字色（不显式指定就画不出来），颜色由调用方
                            // 按悬停态传入，悬停变白时勾号跟着变。
                            this.child(
                                svg()
                                    .path(icons::CHECK)
                                    .w(px(12.))
                                    .h(px(12.))
                                    .text_color(color),
                            )
                        },
                    ))
                })
                .when_some(icon, |this, icon| {
                    this.child(match icon {
                        Some(path) => gpui::img(path.to_path_buf())
                            .w(px(16.))
                            .h(px(16.))
                            .flex_shrink_0()
                            .into_any_element(),
                        None => div().w(px(16.)).flex_shrink_0().into_any_element(),
                    })
                })
                .child(div().truncate().min_w_0().child(label.to_string())),
        )
}

fn render_action_row(
    item: &ActionItem,
    index: usize,
    level: Level,
    has_checks: bool,
    has_icons: bool,
    hovered: Option<(bool, usize)>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let action = item.action;
    let entity = cx.entity();
    let hover_entity = cx.entity();
    let key = (level == Level::Flyout, index);
    let on_row = item.enabled && hovered == Some(key);
    let prefix = if level == Level::Main { "m" } else { "f" };
    let row = row_shell(
        format!("menu-{prefix}-{index}"),
        &item.label,
        has_checks,
        item.checked.then(|| {
            if on_row {
                gpui::white()
            } else if item.enabled {
                theme::text()
            } else {
                theme::text_faint()
            }
        }),
        has_icons.then_some(item.icon.as_deref()),
    )
    .child(
        div()
            .flex_shrink_0()
            .text_xs()
            .opacity(0.7)
            .child(item.shortcut.unwrap_or("")),
    )
    .on_hover(move |hovered, _, cx| {
        hover_entity.update(cx, |workspace, cx| {
            workspace.set_menu_hover(key, *hovered, cx);
            // Moving onto any ordinary top-level row retracts an open flyout.
            if *hovered && !key.0 {
                workspace.set_open_sub(None, cx);
            }
        });
    });
    // 删除是唯一的破坏性菜单项：常态与悬停都用危险色，和普通项区分开。
    let danger = action == MenuAction::Delete;
    if item.enabled {
        // 悬停态由视图记录而不是用样式 `:hover`：文字颜色在布局阶段就定了，
        // 样式层的悬停改不动它。
        row.text_color(if on_row {
            gpui::white()
        } else if danger {
            theme::red()
        } else {
            theme::text()
        })
        .when(on_row, |this| {
            this.bg(if danger {
                // 白字要压得住红底：亮色 red 在深色主题里偏浅，悬停时压暗。
                let mut red = theme::red();
                red.l = 0.4;
                red
            } else {
                theme::bg_selected()
            })
        })
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            entity.update(cx, |workspace, cx| {
                workspace.close_menu_keeping_apps();
                workspace.run_menu_action(action, window, cx);
            });
        })
        .into_any_element()
    } else {
        row.text_color(theme::text_faint()).into_any_element()
    }
}

fn render_submenu_row(
    sub: &SubMenu,
    index: usize,
    level: Level,
    open: bool,
    has_checks: bool,
    hovered: Option<(bool, usize)>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let hover_entity = cx.entity();
    let click_entity = cx.entity();
    let key = (level == Level::Flyout, index);
    let lit = open || hovered == Some(key);
    let row = row_shell(
        format!("menu-sub-{index}"),
        &sub.label,
        has_checks,
        None,
        None,
    )
    .child(
        svg()
            .path(icons::CHEVRON_RIGHT)
            .w(px(14.))
            .h(px(14.))
            .flex_shrink_0()
            // SVG 不一定继承行的文字色，显式指定才能保证箭头可见。
            .text_color(if lit {
                gpui::white()
            } else {
                theme::text_muted()
            }),
    )
    .cursor_pointer()
    .text_color(if lit { gpui::white() } else { theme::text() })
    .when(lit, |this| this.bg(theme::bg_selected()));
    // Nested flyouts are not needed; a submenu row inside a flyout stays inert.
    if level == Level::Flyout {
        return row.into_any_element();
    }
    row.on_hover(move |hovered, _, cx| {
        hover_entity.update(cx, |workspace, cx| {
            workspace.set_menu_hover(key, *hovered, cx);
            if *hovered {
                workspace.set_open_sub(Some(index), cx);
            }
        });
    })
    .on_click(move |_, _, cx| {
        click_entity.update(cx, |workspace, cx| workspace.set_open_sub(Some(index), cx));
    })
    .into_any_element()
}
