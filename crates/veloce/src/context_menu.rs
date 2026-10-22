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
    Trash,
    GetInfo,
    Rename,
    Compress,
    Duplicate,
    MakeAlias,
    QuickLook,
    Copy,
    CopyPath,
    Paste,
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
        /// Name shown in labels when `count == 1`.
        name: String,
        /// A single folder is selected.
        is_dir: bool,
    },
}

/// Finder quotes a single name (`Compress "notes.md"`) and counts several
/// (`Compress 3 Items`). Long names are shortened so the menu keeps its width.
pub fn subject(count: usize, name: &str) -> String {
    if count == 1 {
        let shown: String = if name.chars().count() > 24 {
            let head: String = name.chars().take(23).collect();
            format!("{head}…")
        } else {
            name.to_string()
        };
        format!("“{shown}”")
    } else {
        format!("{count} Items")
    }
}

fn action(
    action: MenuAction,
    label: impl Into<String>,
    shortcut: Option<&'static str>,
    enabled: bool,
) -> MenuItem {
    MenuItem::Action(ActionItem {
        action,
        label: label.into(),
        shortcut,
        enabled,
    })
}

/// Build the menu for `target`. `can_paste` is whether the in-app clipboard holds
/// anything.
pub fn build_menu(target: &MenuTarget, can_paste: bool) -> Vec<MenuItem> {
    use MenuAction::*;
    match target {
        MenuTarget::Background => vec![
            action(NewFolder, "New Folder", Some("⇧⌘N"), true),
            MenuItem::Separator,
            action(Paste, "Paste Item", Some("⌘V"), can_paste),
            MenuItem::Separator,
            action(OpenInTerminal, "New Terminal at Folder", None, true),
            action(Refresh, "Refresh", Some("⌘R"), true),
        ],
        MenuTarget::Items {
            count,
            name,
            is_dir,
        } => {
            let single = *count == 1;
            let who = subject(*count, name);
            let mut items = vec![action(Open, "Open", Some("⌘O"), true)];
            if single && *is_dir {
                items.push(action(OpenInNewTab, "Open in New Tab", None, true));
                items.push(action(OpenInTerminal, "New Terminal at Folder", None, true));
            }
            items.push(MenuItem::Separator);
            items.push(action(Trash, "Move to Trash", Some("⌘⌫"), true));
            items.push(MenuItem::Separator);
            items.push(action(GetInfo, "Get Info", Some("⌘I"), true));
            items.push(action(Rename, "Rename", Some("⇧R"), single));
            items.push(action(Compress, format!("Compress {who}"), None, true));
            items.push(action(Duplicate, "Duplicate", Some("⌘D"), true));
            items.push(action(MakeAlias, "Make Alias", Some("⌘L"), true));
            items.push(action(
                QuickLook,
                format!("Quick Look {who}"),
                Some("Space"),
                single,
            ));
            items.push(MenuItem::Separator);
            items.push(action(Copy, format!("Copy {who}"), Some("⌘C"), true));
            items.push(action(CopyPath, "Copy as Pathname", None, true));
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

    fn file(count: usize, name: &str) -> MenuTarget {
        MenuTarget::Items {
            count,
            name: name.to_string(),
            is_dir: false,
        }
    }

    #[test]
    fn single_file_menu_quotes_the_name_like_finder() {
        let items = build_menu(&file(1, "notes.md"), false);
        let labels = labels(&items);
        assert!(labels.contains(&"Compress “notes.md”"));
        assert!(labels.contains(&"Copy “notes.md”"));
        assert!(labels.contains(&"Quick Look “notes.md”"));
        assert_eq!(labels[0], "Open");
    }

    #[test]
    fn several_items_are_counted_and_rename_and_quick_look_disable() {
        let items = build_menu(&file(3, "a"), false);
        assert!(labels(&items).contains(&"Compress 3 Items"));
        assert!(!find(&items, MenuAction::Rename).enabled);
        assert!(!find(&items, MenuAction::QuickLook).enabled);
        assert!(find(&items, MenuAction::Trash).enabled);
    }

    #[test]
    fn folders_gain_open_in_new_tab_and_terminal() {
        let target = MenuTarget::Items {
            count: 1,
            name: "src".into(),
            is_dir: true,
        };
        let items = build_menu(&target, true);
        assert!(labels(&items).contains(&"Open in New Tab"));
        assert!(labels(&items).contains(&"New Terminal at Folder"));
        let plain = build_menu(&file(1, "a.txt"), true);
        assert!(!labels(&plain).contains(&"Open in New Tab"));
    }

    #[test]
    fn background_menu_offers_new_folder_and_paste_only_with_a_clipboard() {
        let empty = build_menu(&MenuTarget::Background, false);
        assert_eq!(labels(&empty)[0], "New Folder");
        assert!(!find(&empty, MenuAction::Paste).enabled);
        let full = build_menu(&MenuTarget::Background, true);
        assert!(find(&full, MenuAction::Paste).enabled);
    }

    #[test]
    fn no_menu_starts_or_ends_with_a_separator_or_doubles_one() {
        let targets = [
            MenuTarget::Background,
            file(1, "a"),
            file(2, "a"),
            MenuTarget::Items {
                count: 1,
                name: "d".into(),
                is_dir: true,
            },
        ];
        for target in targets {
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
    fn long_names_are_shortened() {
        let name = "a-very-long-file-name-that-keeps-going.txt";
        let shown = subject(1, name);
        assert!(shown.chars().count() <= 26, "{shown}");
        assert!(shown.contains('…'));
    }

    #[test]
    fn menu_height_counts_rows_separators_and_padding() {
        let items = vec![
            action(MenuAction::Open, "Open", None, true),
            MenuItem::Separator,
            action(MenuAction::Trash, "Trash", None, true),
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
