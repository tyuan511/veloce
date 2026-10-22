//! The right-click menu: what it contains for a given target, and where it goes.
//!
//! The content follows Finder's menus: Open / Open With ▸ / Compress ▸ / Copy /
//! Rename / Properties ..., with the long lists (formats, applications, sort keys)
//! folded into submenus. Only the model and the geometry live here; the workspace
//! draws it and runs the chosen action.

use vfs_core::{SortKey, SortSpec, archive::CompressFormat};

use crate::view::ViewMode;

/// Everything a menu row can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// A row that does nothing (the "Loading…" placeholder).
    Nothing,
    Open,
    /// Open with the n-th application of the Open With list.
    OpenWith(usize),
    /// Pick any application from a file dialog.
    OpenWithOther,
    OpenInNewTab,
    OpenInTerminal,
    Preview,
    Compress(CompressFormat),
    ExtractHere,
    ExtractTo,
    CopyPath,
    Cut,
    Copy,
    /// Paste into the folder being viewed.
    Paste,
    /// Paste into the right-clicked folder.
    PasteInto,
    CreateShortcut,
    Delete,
    Rename,
    Properties,
    NewFolder,
    Refresh,
    SortBy(SortKey),
    SortDescending(bool),
    SortFoldersFirst,
    SetView(ViewMode),
    ToggleHidden,
    ToggleSplit,
    TogglePreviewPanel,
    ToggleTasks,
    CommandPalette,
    OpenSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItem {
    pub action: MenuAction,
    pub label: String,
    pub shortcut: Option<&'static str>,
    pub enabled: bool,
    /// Draws a check mark (current sort key, current view ...).
    pub checked: bool,
}

/// A row that opens a flyout to its right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubMenu {
    pub label: String,
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Action(ActionItem),
    Submenu(SubMenu),
    Separator,
}

/// What was right-clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuTarget {
    /// Empty space in the list.
    Background,
    /// One or more entries (the selection).
    Items {
        count: usize,
        /// A single folder is selected.
        is_dir: bool,
        /// Nothing but files is selected (Open With only makes sense then).
        files_only: bool,
        /// A single archive is selected: the folder name "Extract to" would create.
        archive_stem: Option<String>,
    },
}

/// Applications that can open the selection, which arrive asynchronously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenWith {
    Loading,
    /// Display names; the first is the default application.
    Apps(Vec<String>),
}

/// State the menu content depends on besides what was clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuContext {
    pub can_paste: bool,
    pub view: ViewMode,
    pub show_hidden: bool,
    pub sort: SortSpec,
    pub open_with: OpenWith,
}

impl Default for MenuContext {
    fn default() -> Self {
        Self {
            can_paste: false,
            view: ViewMode::default(),
            show_hidden: false,
            sort: SortSpec::default(),
            open_with: OpenWith::Loading,
        }
    }
}

fn action(
    action: MenuAction,
    label: &str,
    shortcut: Option<&'static str>,
    enabled: bool,
) -> MenuItem {
    MenuItem::Action(ActionItem {
        action,
        label: label.to_string(),
        shortcut,
        enabled,
        checked: false,
    })
}

fn submenu(label: &str, items: Vec<MenuItem>) -> MenuItem {
    MenuItem::Submenu(SubMenu {
        label: label.to_string(),
        items,
    })
}

fn checked(item: MenuItem, on: bool) -> MenuItem {
    match item {
        MenuItem::Action(mut item) => {
            item.checked = on;
            MenuItem::Action(item)
        }
        other => other,
    }
}

/// Finder's wording for sort keys ("Date Modified", not "Modified").
pub fn sort_key_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "Name",
        SortKey::Kind => "Kind",
        SortKey::Modified => "Date Modified",
        SortKey::Created => "Date Created",
        SortKey::Size => "Size",
    }
}

/// The "Sort By" menu, with the current choice checked.
pub fn sort_menu(spec: SortSpec) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = SortKey::ALL
        .iter()
        .map(|key| {
            checked(
                action(MenuAction::SortBy(*key), sort_key_label(*key), None, true),
                spec.key == *key,
            )
        })
        .collect();
    items.push(MenuItem::Separator);
    items.push(checked(
        action(MenuAction::SortDescending(false), "Ascending", None, true),
        !spec.descending,
    ));
    items.push(checked(
        action(MenuAction::SortDescending(true), "Descending", None, true),
        spec.descending,
    ));
    items.push(MenuItem::Separator);
    items.push(checked(
        action(
            MenuAction::SortFoldersFirst,
            "Keep Folders on Top",
            None,
            true,
        ),
        spec.dirs_first,
    ));
    items
}

/// The toolbar's view switcher: Finder's "as Icons / as List / as Columns".
pub fn view_menu(view: ViewMode) -> Vec<MenuItem> {
    use MenuAction::SetView;
    vec![
        checked(
            action(SetView(ViewMode::Icons), "as Icons", Some("⌘1"), true),
            view == ViewMode::Icons,
        ),
        checked(
            action(SetView(ViewMode::List), "as List", Some("⌘2"), true),
            view == ViewMode::List,
        ),
        checked(
            action(SetView(ViewMode::Columns), "as Columns", Some("⌘3"), true),
            view == ViewMode::Columns,
        ),
    ]
}

fn compress_menu() -> Vec<MenuItem> {
    CompressFormat::ALL
        .iter()
        .map(|format| action(MenuAction::Compress(*format), &format.label(), None, true))
        .collect()
}

/// Open With submenu: the default app first, the rest alphabetical, then "Other…".
pub fn open_with_menu(open_with: &OpenWith) -> Vec<MenuItem> {
    let mut items = match open_with {
        OpenWith::Loading => vec![action(MenuAction::Nothing, "Loading…", None, false)],
        OpenWith::Apps(names) if names.is_empty() => {
            vec![action(MenuAction::Nothing, "No applications", None, false)]
        }
        OpenWith::Apps(names) => {
            let mut rows: Vec<MenuItem> = names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let label = if index == 0 {
                        format!("{name} (default)")
                    } else {
                        name.clone()
                    };
                    action(MenuAction::OpenWith(index), &label, None, true)
                })
                .collect();
            if rows.len() > 1 {
                rows.insert(1, MenuItem::Separator);
            }
            rows
        }
    };
    items.push(MenuItem::Separator);
    items.push(action(MenuAction::OpenWithOther, "Other…", None, true));
    items
}

/// Which of the toolbar's toggles are on, for the "more" menu's check marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoreState {
    pub show_hidden: bool,
    pub split: bool,
    pub preview_open: bool,
    pub tasks_open: bool,
}

/// The toolbar's "…" menu: the switches and commands that are not worth a button
/// of their own.
pub fn more_menu(state: MoreState) -> Vec<MenuItem> {
    use MenuAction::*;
    vec![
        action(NewFolder, "New folder", Some("⇧⌘N"), true),
        MenuItem::Separator,
        checked(
            action(ToggleHidden, "Show hidden files", Some("⇧⌘."), true),
            state.show_hidden,
        ),
        checked(
            action(ToggleSplit, "Split view", Some("⌘\\"), true),
            state.split,
        ),
        checked(
            action(TogglePreviewPanel, "Preview panel", Some("Space"), true),
            state.preview_open,
        ),
        checked(
            action(ToggleTasks, "Task panel", Some("⌘J"), true),
            state.tasks_open,
        ),
        MenuItem::Separator,
        action(Refresh, "Refresh", Some("⌘R"), true),
        action(OpenInTerminal, "Open in Terminal", None, true),
        action(CommandPalette, "Command palette…", Some("⇧⌘P"), true),
        MenuItem::Separator,
        action(OpenSettings, "Settings…", Some("⌘,"), true),
    ]
}

/// Build the menu for `target`.
pub fn build_menu(target: &MenuTarget, ctx: &MenuContext) -> Vec<MenuItem> {
    use MenuAction::*;
    match target {
        MenuTarget::Background => vec![
            action(Refresh, "Refresh", Some("⌘R"), true),
            MenuItem::Separator,
            action(Paste, "Paste", Some("⌘V"), ctx.can_paste),
            MenuItem::Separator,
            action(NewFolder, "New folder", Some("⇧⌘N"), true),
            action(OpenInTerminal, "Open in Terminal", None, true),
            MenuItem::Separator,
            submenu("Sort By", sort_menu(ctx.sort)),
            submenu("View", view_menu(ctx.view)),
            checked(
                action(ToggleHidden, "Show hidden files", Some("⇧⌘."), true),
                ctx.show_hidden,
            ),
        ],
        MenuTarget::Items {
            count,
            is_dir,
            files_only,
            archive_stem,
        } => {
            let single = *count == 1;
            let folder = single && *is_dir;
            let mut items = vec![action(Open, "Open", Some("⌘O"), true)];
            if *files_only {
                items.push(submenu("Open With", open_with_menu(&ctx.open_with)));
            }
            if folder {
                items.push(action(OpenInNewTab, "Open in new tab", None, true));
                items.push(action(OpenInTerminal, "Open in Terminal", None, true));
            }
            if single && !*is_dir {
                items.push(action(Preview, "Preview", Some("Space"), true));
            }
            items.push(MenuItem::Separator);
            if let (true, Some(stem)) = (single, archive_stem) {
                items.push(action(ExtractHere, "Extract Here", None, true));
                items.push(action(
                    ExtractTo,
                    &format!("Extract to “{stem}/”"),
                    None,
                    true,
                ));
            }
            items.push(submenu("Compress to", compress_menu()));
            items.push(action(CopyPath, "Copy as path", Some("⇧⌘C"), true));
            items.push(MenuItem::Separator);
            items.push(action(Cut, "Cut", Some("⌘X"), true));
            items.push(action(Copy, "Copy", Some("⌘C"), true));
            if folder {
                items.push(action(PasteInto, "Paste", Some("⌘V"), ctx.can_paste));
            }
            items.push(MenuItem::Separator);
            items.push(action(CreateShortcut, "Create shortcut", None, true));
            items.push(action(Delete, "Delete", Some("Del"), true));
            items.push(action(Rename, "Rename", Some("F2"), single));
            items.push(MenuItem::Separator);
            items.push(action(Properties, "Properties", Some("⌥↵"), single));
            items
        }
    }
}

pub const MENU_WIDTH: f32 = 248.0;
pub const SUBMENU_WIDTH: f32 = 208.0;
pub const ITEM_HEIGHT: f32 = 24.0;
pub const SEPARATOR_HEIGHT: f32 = 9.0;
pub const MENU_PADDING: f32 = 4.0;

/// Total height of a rendered menu, for edge clamping.
pub fn menu_height(items: &[MenuItem]) -> f32 {
    let rows: f32 = items
        .iter()
        .map(|item| match item {
            MenuItem::Action(_) | MenuItem::Submenu(_) => ITEM_HEIGHT,
            MenuItem::Separator => SEPARATOR_HEIGHT,
        })
        .sum();
    rows + MENU_PADDING * 2.0
}

/// Keep the menu inside the window: flip it left/up of the cursor when it would
/// overflow, and never past the top/left edge.
pub fn clamp_origin(cursor: (f32, f32), menu: (f32, f32), window: (f32, f32)) -> (f32, f32) {
    let fit = |at: f32, size: f32, limit: f32| {
        if at + size <= limit {
            at
        } else {
            (at - size).max(0.0)
        }
    };
    (
        fit(cursor.0, menu.0, window.0),
        fit(cursor.1, menu.1, window.1),
    )
}

/// Where a flyout opens: level with its parent row, to the right of the parent
/// menu, flipping to the left when the right side has no room and sliding up if
/// it would run off the bottom.
pub fn submenu_origin(
    parent_items: &[MenuItem],
    row: usize,
    parent_origin: (f32, f32),
    sub: (f32, f32),
    window: (f32, f32),
) -> (f32, f32) {
    let rows_above: f32 = parent_items
        .iter()
        .take(row)
        .map(|item| match item {
            MenuItem::Separator => SEPARATOR_HEIGHT,
            _ => ITEM_HEIGHT,
        })
        .sum();
    // Overlap the parent by a few pixels so the pointer never crosses a gap.
    let overlap = 4.0;
    let right = parent_origin.0 + MENU_WIDTH - overlap;
    let x = if right + sub.0 <= window.0 {
        right
    } else {
        (parent_origin.0 - sub.0 + overlap).max(0.0)
    };
    let top = parent_origin.1 + MENU_PADDING + rows_above - MENU_PADDING;
    let y = if top + sub.1 <= window.1 {
        top
    } else {
        (window.1 - sub.1).max(0.0)
    };
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> MenuContext {
        MenuContext::default()
    }

    /// Every action in the menu, descending into submenus.
    fn flatten(items: &[MenuItem]) -> Vec<&ActionItem> {
        let mut out = Vec::new();
        for item in items {
            match item {
                MenuItem::Action(a) => out.push(a),
                MenuItem::Submenu(sub) => out.extend(flatten(&sub.items)),
                MenuItem::Separator => {}
            }
        }
        out
    }

    fn top_labels(items: &[MenuItem]) -> Vec<&str> {
        items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action(a) => Some(a.label.as_str()),
                MenuItem::Submenu(s) => Some(s.label.as_str()),
                MenuItem::Separator => None,
            })
            .collect()
    }

    fn find(items: &[MenuItem], wanted: MenuAction) -> &ActionItem {
        flatten(items)
            .into_iter()
            .find(|a| a.action == wanted)
            .expect("menu has the action")
    }

    fn has(items: &[MenuItem], wanted: MenuAction) -> bool {
        flatten(items).iter().any(|a| a.action == wanted)
    }

    fn sub<'a>(items: &'a [MenuItem], label: &str) -> &'a SubMenu {
        items
            .iter()
            .find_map(|item| match item {
                MenuItem::Submenu(s) if s.label == label => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no submenu {label}"))
    }

    fn checked_actions(items: &[MenuItem]) -> Vec<MenuAction> {
        flatten(items)
            .into_iter()
            .filter(|a| a.checked)
            .map(|a| a.action)
            .collect()
    }

    fn files(count: usize) -> MenuTarget {
        MenuTarget::Items {
            count,
            is_dir: false,
            files_only: true,
            archive_stem: None,
        }
    }

    fn folder() -> MenuTarget {
        MenuTarget::Items {
            count: 1,
            is_dir: true,
            files_only: false,
            archive_stem: None,
        }
    }

    fn archive() -> MenuTarget {
        MenuTarget::Items {
            count: 1,
            is_dir: false,
            files_only: true,
            archive_stem: Some("photos".to_string()),
        }
    }

    #[test]
    fn file_menu_has_the_explorer_core_in_order() {
        let items = build_menu(&files(1), &ctx());
        let labels = top_labels(&items);
        let at = |label: &str| labels.iter().position(|l| *l == label).unwrap();
        assert_eq!(labels[0], "Open");
        assert!(at("Cut") < at("Copy"));
        assert!(at("Copy") < at("Delete"));
        assert!(at("Delete") < at("Rename"));
        assert_eq!(*labels.last().unwrap(), "Properties");
    }

    #[test]
    fn no_finder_only_commands_remain() {
        for target in [files(1), folder(), MenuTarget::Background] {
            let items = build_menu(&target, &ctx());
            let labels: Vec<String> = flatten(&items).iter().map(|a| a.label.clone()).collect();
            for banned in ["Duplicate", "Make Alias", "Get Info", "Move to Trash"] {
                assert!(!labels.iter().any(|l| l == banned), "{banned}");
            }
        }
    }

    #[test]
    fn several_items_disable_rename_and_properties_but_not_delete() {
        let items = build_menu(&files(3), &ctx());
        assert!(!find(&items, MenuAction::Rename).enabled);
        assert!(!find(&items, MenuAction::Properties).enabled);
        assert!(find(&items, MenuAction::Delete).enabled);
        assert!(find(&items, MenuAction::Cut).enabled);
    }

    #[test]
    fn folders_gain_tab_terminal_and_paste_into() {
        let with_clipboard = MenuContext {
            can_paste: true,
            ..ctx()
        };
        let items = build_menu(&folder(), &with_clipboard);
        assert!(has(&items, MenuAction::OpenInNewTab));
        assert!(has(&items, MenuAction::OpenInTerminal));
        assert!(find(&items, MenuAction::PasteInto).enabled);
        assert!(!find(&build_menu(&folder(), &ctx()), MenuAction::PasteInto).enabled);
        assert!(!has(
            &build_menu(&files(1), &with_clipboard),
            MenuAction::PasteInto
        ));
    }

    #[test]
    fn single_files_can_be_previewed_but_folders_cannot() {
        assert!(has(&build_menu(&files(1), &ctx()), MenuAction::Preview));
        assert!(!has(&build_menu(&folder(), &ctx()), MenuAction::Preview));
        assert!(!has(&build_menu(&files(2), &ctx()), MenuAction::Preview));
    }

    #[test]
    fn compress_formats_live_in_a_submenu_not_the_top_level() {
        for target in [files(1), files(3), folder(), archive()] {
            let items = build_menu(&target, &ctx());
            let compress = sub(&items, "Compress to");
            assert_eq!(compress.items.len(), CompressFormat::ALL.len());
            for format in CompressFormat::ALL {
                assert!(
                    has(&compress.items, MenuAction::Compress(format)),
                    "{format:?}"
                );
            }
            assert!(
                !items
                    .iter()
                    .any(|i| matches!(i, MenuItem::Action(a) if matches!(a.action, MenuAction::Compress(_)))),
                "no format sits at the top level"
            );
        }
        let items = build_menu(&files(1), &ctx());
        let labels: Vec<&str> = sub(&items, "Compress to")
            .items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Action(a) => Some(a.label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(labels[0], "ZIP file");
        assert!(labels.contains(&"TAR.GZ"));
    }

    #[test]
    fn archives_offer_extraction_named_after_their_stem() {
        let items = build_menu(&archive(), &ctx());
        assert!(has(&items, MenuAction::ExtractHere));
        assert!(top_labels(&items).contains(&"Extract to “photos/”"));
    }

    #[test]
    fn only_a_single_archive_offers_extraction() {
        assert!(!has(
            &build_menu(&files(1), &ctx()),
            MenuAction::ExtractHere
        ));
        assert!(!has(&build_menu(&folder(), &ctx()), MenuAction::ExtractTo));
        let many = MenuTarget::Items {
            count: 2,
            is_dir: false,
            files_only: true,
            archive_stem: Some("a".into()),
        };
        assert!(!has(&build_menu(&many, &ctx()), MenuAction::ExtractHere));
    }

    #[test]
    fn open_with_appears_for_files_only() {
        assert!(top_labels(&build_menu(&files(1), &ctx())).contains(&"Open With"));
        assert!(top_labels(&build_menu(&files(3), &ctx())).contains(&"Open With"));
        assert!(!top_labels(&build_menu(&folder(), &ctx())).contains(&"Open With"));
        let mixed = MenuTarget::Items {
            count: 2,
            is_dir: false,
            files_only: false,
            archive_stem: None,
        };
        assert!(!top_labels(&build_menu(&mixed, &ctx())).contains(&"Open With"));
    }

    #[test]
    fn open_with_shows_loading_until_the_app_list_arrives() {
        let items = build_menu(&files(1), &ctx());
        let open_with = sub(&items, "Open With");
        let loading = find(&open_with.items, MenuAction::Nothing);
        assert_eq!(loading.label, "Loading…");
        assert!(!loading.enabled);
        assert!(has(&open_with.items, MenuAction::OpenWithOther));
    }

    #[test]
    fn open_with_lists_apps_with_the_default_first_and_marked() {
        let ready = MenuContext {
            open_with: OpenWith::Apps(vec!["Preview".into(), "Pixelmator".into()]),
            ..ctx()
        };
        let items = build_menu(&files(1), &ready);
        let open_with = sub(&items, "Open With");
        assert_eq!(
            find(&open_with.items, MenuAction::OpenWith(0)).label,
            "Preview (default)"
        );
        assert_eq!(
            find(&open_with.items, MenuAction::OpenWith(1)).label,
            "Pixelmator"
        );
        assert!(!has(&open_with.items, MenuAction::Nothing));
        assert!(find(&open_with.items, MenuAction::OpenWithOther).enabled);
    }

    #[test]
    fn open_with_with_no_apps_still_offers_other() {
        let none = MenuContext {
            open_with: OpenWith::Apps(Vec::new()),
            ..ctx()
        };
        let items = build_menu(&files(1), &none);
        let open_with = sub(&items, "Open With");
        assert_eq!(
            find(&open_with.items, MenuAction::Nothing).label,
            "No applications"
        );
        assert!(has(&open_with.items, MenuAction::OpenWithOther));
    }

    #[test]
    fn background_menu_pastes_only_with_a_clipboard() {
        let empty = build_menu(&MenuTarget::Background, &ctx());
        assert!(!find(&empty, MenuAction::Paste).enabled);
        let full = MenuContext {
            can_paste: true,
            ..ctx()
        };
        assert!(
            find(
                &build_menu(&MenuTarget::Background, &full),
                MenuAction::Paste
            )
            .enabled
        );
        assert!(has(&empty, MenuAction::NewFolder));
    }

    #[test]
    fn background_menu_folds_sort_and_view_into_submenus() {
        let items = build_menu(&MenuTarget::Background, &ctx());
        let sort = sub(&items, "Sort By");
        for key in SortKey::ALL {
            assert!(has(&sort.items, MenuAction::SortBy(key)), "{key:?}");
        }
        let view = sub(&items, "View");
        assert_eq!(flatten(&view.items).len(), 3);
    }

    #[test]
    fn background_submenus_check_the_current_sort_and_view() {
        for mode in [ViewMode::Icons, ViewMode::List, ViewMode::Columns] {
            let items = build_menu(
                &MenuTarget::Background,
                &MenuContext {
                    view: mode,
                    ..ctx()
                },
            );
            let view = sub(&items, "View");
            assert_eq!(
                checked_actions(&view.items),
                [MenuAction::SetView(mode)],
                "{mode:?}"
            );
        }
        let items = build_menu(
            &MenuTarget::Background,
            &MenuContext {
                sort: SortSpec::new(SortKey::Size, true),
                ..ctx()
            },
        );
        let checked = checked_actions(&sub(&items, "Sort By").items);
        assert!(checked.contains(&MenuAction::SortBy(SortKey::Size)));
        assert!(checked.contains(&MenuAction::SortDescending(true)));
    }

    #[test]
    fn background_menu_reflects_whether_hidden_files_are_shown() {
        let off = build_menu(&MenuTarget::Background, &ctx());
        assert!(!checked_actions(&off).contains(&MenuAction::ToggleHidden));
        let on = build_menu(
            &MenuTarget::Background,
            &MenuContext {
                show_hidden: true,
                ..ctx()
            },
        );
        assert!(checked_actions(&on).contains(&MenuAction::ToggleHidden));
    }

    #[test]
    fn no_menu_starts_or_ends_with_a_separator_or_doubles_one() {
        for target in [
            MenuTarget::Background,
            files(1),
            files(2),
            folder(),
            archive(),
        ] {
            let items = build_menu(&target, &ctx());
            assert_ne!(items.first(), Some(&MenuItem::Separator));
            assert_ne!(items.last(), Some(&MenuItem::Separator));
            assert!(
                !items
                    .windows(2)
                    .any(|w| w[0] == MenuItem::Separator && w[1] == MenuItem::Separator)
            );
        }
    }

    #[test]
    fn sort_menu_lists_every_key_and_checks_the_current_one() {
        let items = sort_menu(SortSpec::new(SortKey::Size, false));
        let checked = checked_actions(&items);
        assert!(checked.contains(&MenuAction::SortBy(SortKey::Size)));
        assert!(!checked.contains(&MenuAction::SortBy(SortKey::Name)));
        for key in SortKey::ALL {
            assert!(has(&items, MenuAction::SortBy(key)), "{key:?}");
        }
    }

    #[test]
    fn sort_menu_checks_exactly_one_direction() {
        let asc = checked_actions(&sort_menu(SortSpec::new(SortKey::Name, false)));
        assert!(asc.contains(&MenuAction::SortDescending(false)));
        assert!(!asc.contains(&MenuAction::SortDescending(true)));
        let desc = checked_actions(&sort_menu(SortSpec::new(SortKey::Name, true)));
        assert!(desc.contains(&MenuAction::SortDescending(true)));
        assert!(!desc.contains(&MenuAction::SortDescending(false)));
    }

    #[test]
    fn sort_menu_reflects_folders_on_top() {
        let on = SortSpec::new(SortKey::Name, false);
        assert!(checked_actions(&sort_menu(on)).contains(&MenuAction::SortFoldersFirst));
        let off = SortSpec {
            dirs_first: false,
            ..on
        };
        assert!(!checked_actions(&sort_menu(off)).contains(&MenuAction::SortFoldersFirst));
    }

    #[test]
    fn sort_labels_follow_finder() {
        assert_eq!(sort_key_label(SortKey::Modified), "Date Modified");
        assert_eq!(sort_key_label(SortKey::Created), "Date Created");
    }

    #[test]
    fn more_menu_reflects_every_toggle() {
        let off = more_menu(MoreState::default());
        assert!(checked_actions(&off).is_empty());
        let on = more_menu(MoreState {
            show_hidden: true,
            split: true,
            preview_open: true,
            tasks_open: true,
        });
        let checked = checked_actions(&on);
        for action in [
            MenuAction::ToggleHidden,
            MenuAction::ToggleSplit,
            MenuAction::TogglePreviewPanel,
            MenuAction::ToggleTasks,
        ] {
            assert!(checked.contains(&action), "{action:?}");
        }
    }

    #[test]
    fn more_menu_keeps_the_everyday_commands() {
        let items = more_menu(MoreState::default());
        for action in [
            MenuAction::NewFolder,
            MenuAction::Refresh,
            MenuAction::OpenInTerminal,
            MenuAction::CommandPalette,
        ] {
            assert!(has(&items, action), "{action:?}");
        }
        assert_ne!(items.first(), Some(&MenuItem::Separator));
        assert_ne!(items.last(), Some(&MenuItem::Separator));
    }

    #[test]
    fn view_menu_checks_the_current_view_and_offers_all_three() {
        for mode in [ViewMode::Icons, ViewMode::List, ViewMode::Columns] {
            let items = view_menu(mode);
            assert_eq!(items.len(), 3);
            assert_eq!(checked_actions(&items), [MenuAction::SetView(mode)]);
        }
    }

    #[test]
    fn menu_height_counts_rows_separators_submenus_and_padding() {
        let items = vec![
            action(MenuAction::Open, "Open", None, true),
            MenuItem::Separator,
            submenu(
                "More",
                vec![action(MenuAction::Delete, "Delete", None, true)],
            ),
        ];
        assert_eq!(
            menu_height(&items),
            ITEM_HEIGHT * 2.0 + SEPARATOR_HEIGHT + MENU_PADDING * 2.0
        );
    }

    #[test]
    fn menu_stays_put_when_it_fits() {
        assert_eq!(
            clamp_origin((100.0, 100.0), (200.0, 300.0), (1000.0, 800.0)),
            (100.0, 100.0)
        );
    }

    #[test]
    fn menu_flips_toward_the_cursor_near_right_and_bottom_edges() {
        let (x, y) = clamp_origin((950.0, 700.0), (200.0, 300.0), (1000.0, 800.0));
        assert_eq!((x, y), (750.0, 400.0));
    }

    #[test]
    fn menu_larger_than_the_window_pins_to_the_origin() {
        assert_eq!(
            clamp_origin((10.0, 10.0), (500.0, 500.0), (300.0, 300.0)),
            (0.0, 0.0)
        );
    }

    fn rows() -> Vec<MenuItem> {
        vec![
            action(MenuAction::Open, "Open", None, true),
            MenuItem::Separator,
            submenu("Compress to", Vec::new()),
        ]
    }

    #[test]
    fn flyout_opens_level_with_its_row_to_the_right() {
        let (x, y) = submenu_origin(&rows(), 2, (100.0, 50.0), (200.0, 100.0), (1000.0, 800.0));
        assert_eq!(x, 100.0 + MENU_WIDTH - 4.0);
        assert_eq!(y, 50.0 + ITEM_HEIGHT + SEPARATOR_HEIGHT);
    }

    #[test]
    fn flyout_flips_left_when_the_right_edge_is_too_close() {
        let (x, _) = submenu_origin(&rows(), 2, (800.0, 50.0), (200.0, 100.0), (1000.0, 800.0));
        assert_eq!(x, 800.0 - 200.0 + 4.0);
    }

    #[test]
    fn flyout_slides_up_to_stay_inside_the_window() {
        let (_, y) = submenu_origin(&rows(), 2, (100.0, 700.0), (200.0, 300.0), (1000.0, 800.0));
        assert_eq!(y, 500.0);
    }
}
