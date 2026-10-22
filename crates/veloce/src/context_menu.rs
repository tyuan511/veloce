//! The right-click menu: what it contains for a given target, and where it goes.
//!
//! The content follows Finder's menus (Open / Move to Trash / Get Info / Rename /
//! Compress / Duplicate / Make Alias / Quick Look / Copy / Paste Item ...). Only
//! the model lives here; the workspace draws it and runs the chosen action.

/// Everything a menu row can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    OpenInNewTab,
    OpenInTerminal,
    Preview,
    CompressZip,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItem {
    pub action: MenuAction,
    pub label: String,
    pub shortcut: Option<&'static str>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Action(ActionItem),
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
    },
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
    })
}

/// Build the menu for `target`, laid out like Explorer's classic context menu:
/// open actions, then Cut / Copy / Paste, then shortcut / Delete / Rename, then
/// Properties. `can_paste` is whether the file clipboard holds anything.
pub fn build_menu(target: &MenuTarget, can_paste: bool) -> Vec<MenuItem> {
    use MenuAction::*;
    match target {
        MenuTarget::Background => vec![
            action(Refresh, "Refresh", Some("⌘R"), true),
            MenuItem::Separator,
            action(Paste, "Paste", Some("⌘V"), can_paste),
            MenuItem::Separator,
            action(NewFolder, "New folder", Some("⇧⌘N"), true),
            action(OpenInTerminal, "Open in Terminal", None, true),
        ],
        MenuTarget::Items { count, is_dir } => {
            let single = *count == 1;
            let folder = single && *is_dir;
            let mut items = vec![action(Open, "Open", Some("⌘O"), true)];
            if folder {
                items.push(action(OpenInNewTab, "Open in new tab", None, true));
                items.push(action(OpenInTerminal, "Open in Terminal", None, true));
            }
            if single && !*is_dir {
                items.push(action(Preview, "Preview", Some("Space"), true));
            }
            items.push(MenuItem::Separator);
            items.push(action(CompressZip, "Compress to ZIP file", None, true));
            items.push(action(CopyPath, "Copy as path", Some("⇧⌘C"), true));
            items.push(MenuItem::Separator);
            items.push(action(Cut, "Cut", Some("⌘X"), true));
            items.push(action(Copy, "Copy", Some("⌘C"), true));
            if folder {
                items.push(action(PasteInto, "Paste", Some("⌘V"), can_paste));
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
pub const ITEM_HEIGHT: f32 = 24.0;
pub const SEPARATOR_HEIGHT: f32 = 9.0;
pub const MENU_PADDING: f32 = 4.0;

/// Total height of a rendered menu, for edge clamping.
pub fn menu_height(items: &[MenuItem]) -> f32 {
    let rows: f32 = items
        .iter()
        .map(|item| match item {
            MenuItem::Action(_) => ITEM_HEIGHT,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action(a) => Some(a.label.as_str()),
                MenuItem::Separator => None,
            })
            .collect()
    }

    fn find(items: &[MenuItem], wanted: MenuAction) -> &ActionItem {
        items
            .iter()
            .find_map(|item| match item {
                MenuItem::Action(a) if a.action == wanted => Some(a),
                _ => None,
            })
            .expect("menu has the action")
    }

    fn has(items: &[MenuItem], wanted: MenuAction) -> bool {
        items
            .iter()
            .any(|item| matches!(item, MenuItem::Action(a) if a.action == wanted))
    }

    fn files(count: usize) -> MenuTarget {
        MenuTarget::Items {
            count,
            is_dir: false,
        }
    }

    fn folder() -> MenuTarget {
        MenuTarget::Items {
            count: 1,
            is_dir: true,
        }
    }

    #[test]
    fn file_menu_has_the_explorer_core_in_order() {
        let items = build_menu(&files(1), false);
        let labels = labels(&items);
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
            let labels = labels(&build_menu(&target, true))
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>();
            for banned in ["Duplicate", "Make Alias", "Get Info", "Move to Trash"] {
                assert!(!labels.iter().any(|l| l == banned), "{banned}");
            }
        }
    }

    #[test]
    fn several_items_disable_rename_and_properties_but_not_delete() {
        let items = build_menu(&files(3), false);
        assert!(!find(&items, MenuAction::Rename).enabled);
        assert!(!find(&items, MenuAction::Properties).enabled);
        assert!(find(&items, MenuAction::Delete).enabled);
        assert!(find(&items, MenuAction::Cut).enabled);
    }

    #[test]
    fn folders_gain_tab_terminal_and_paste_into() {
        let items = build_menu(&folder(), true);
        assert!(has(&items, MenuAction::OpenInNewTab));
        assert!(has(&items, MenuAction::OpenInTerminal));
        assert!(find(&items, MenuAction::PasteInto).enabled);
        assert!(!find(&build_menu(&folder(), false), MenuAction::PasteInto).enabled);
        assert!(!has(&build_menu(&files(1), true), MenuAction::PasteInto));
    }

    #[test]
    fn single_files_can_be_previewed_but_folders_cannot() {
        assert!(has(&build_menu(&files(1), false), MenuAction::Preview));
        assert!(!has(&build_menu(&folder(), false), MenuAction::Preview));
        assert!(!has(&build_menu(&files(2), false), MenuAction::Preview));
    }

    #[test]
    fn background_menu_pastes_only_with_a_clipboard() {
        let empty = build_menu(&MenuTarget::Background, false);
        assert!(!find(&empty, MenuAction::Paste).enabled);
        assert!(
            find(
                &build_menu(&MenuTarget::Background, true),
                MenuAction::Paste
            )
            .enabled
        );
        assert!(has(&empty, MenuAction::NewFolder));
    }

    #[test]
    fn no_menu_starts_or_ends_with_a_separator_or_doubles_one() {
        for target in [MenuTarget::Background, files(1), files(2), folder()] {
            let items = build_menu(&target, true);
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
    fn menu_height_counts_rows_separators_and_padding() {
        let items = vec![
            action(MenuAction::Open, "Open", None, true),
            MenuItem::Separator,
            action(MenuAction::Delete, "Delete", None, true),
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
}
