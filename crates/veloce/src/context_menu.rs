//! The right-click menu: what it contains for a given target, and where it goes.
//!
//! The content follows Finder's menus (Open / Move to Trash / Get Info / Rename /
//! Compress / Duplicate / Make Alias / Quick Look / Copy / Paste Item ...). Only
//! the model lives here; the workspace draws it and runs the chosen action.

use vfs_core::{SortKey, SortSpec, archive::CompressFormat};

use crate::view::ViewMode;

/// Everything a menu row can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Open,
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
        /// A single archive is selected: the folder name "Extract to" would create.
        archive_stem: Option<String>,
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
        checked: false,
    })
}

fn checked(item: MenuItem, on: bool) -> MenuItem {
    match item {
        MenuItem::Action(mut item) => {
            item.checked = on;
            MenuItem::Action(item)
        }
        separator => separator,
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

/// The toolbar's "Sort By" menu, with the current choice checked.
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

/// Build the menu for `target`, laid out like Explorer's classic context menu:
/// open actions, then Cut / Copy / Paste, then shortcut / Delete / Rename, then
/// Properties. `can_paste` is whether the file clipboard holds anything.
pub fn build_menu(
    target: &MenuTarget,
    can_paste: bool,
    view: ViewMode,
    show_hidden: bool,
) -> Vec<MenuItem> {
    use MenuAction::*;
    match target {
        MenuTarget::Background => vec![
            action(Refresh, "Refresh", Some("⌘R"), true),
            MenuItem::Separator,
            action(Paste, "Paste", Some("⌘V"), can_paste),
            MenuItem::Separator,
            action(NewFolder, "New folder", Some("⇧⌘N"), true),
            action(OpenInTerminal, "Open in Terminal", None, true),
            MenuItem::Separator,
            checked(
                action(SetView(ViewMode::Icons), "View as Icons", Some("⌘1"), true),
                view == ViewMode::Icons,
            ),
            checked(
                action(SetView(ViewMode::List), "View as List", Some("⌘2"), true),
                view == ViewMode::List,
            ),
            checked(
                action(
                    SetView(ViewMode::Columns),
                    "View as Columns",
                    Some("⌘3"),
                    true,
                ),
                view == ViewMode::Columns,
            ),
            MenuItem::Separator,
            checked(
                action(ToggleHidden, "Show hidden files", Some("⇧⌘."), true),
                show_hidden,
            ),
        ],
        MenuTarget::Items {
            count,
            is_dir,
            archive_stem,
        } => {
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
            if let (true, Some(stem)) = (single, archive_stem) {
                items.push(action(ExtractHere, "Extract Here", None, true));
                items.push(action(
                    ExtractTo,
                    &format!("Extract to “{stem}/”"),
                    None,
                    true,
                ));
            }
            for format in CompressFormat::ALL {
                items.push(action(
                    Compress(format),
                    &format!("Compress to {}", format.label()),
                    None,
                    true,
                ));
            }
            items.push(MenuItem::Separator);
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
            archive_stem: None,
        }
    }

    fn folder() -> MenuTarget {
        MenuTarget::Items {
            count: 1,
            is_dir: true,
            archive_stem: None,
        }
    }

    fn archive() -> MenuTarget {
        MenuTarget::Items {
            count: 1,
            is_dir: false,
            archive_stem: Some("photos".to_string()),
        }
    }

    #[test]
    fn file_menu_has_the_explorer_core_in_order() {
        let items = build_menu(&files(1), false, ViewMode::List, false);
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
            let labels = labels(&build_menu(&target, true, ViewMode::List, false))
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
        let items = build_menu(&files(3), false, ViewMode::List, false);
        assert!(!find(&items, MenuAction::Rename).enabled);
        assert!(!find(&items, MenuAction::Properties).enabled);
        assert!(find(&items, MenuAction::Delete).enabled);
        assert!(find(&items, MenuAction::Cut).enabled);
    }

    #[test]
    fn folders_gain_tab_terminal_and_paste_into() {
        let items = build_menu(&folder(), true, ViewMode::List, false);
        assert!(has(&items, MenuAction::OpenInNewTab));
        assert!(has(&items, MenuAction::OpenInTerminal));
        assert!(find(&items, MenuAction::PasteInto).enabled);
        assert!(
            !find(
                &build_menu(&folder(), false, ViewMode::List, false),
                MenuAction::PasteInto
            )
            .enabled
        );
        assert!(!has(
            &build_menu(&files(1), true, ViewMode::List, false),
            MenuAction::PasteInto
        ));
    }

    #[test]
    fn every_compress_format_is_offered_for_any_selection() {
        for target in [files(1), files(3), folder(), archive()] {
            let items = build_menu(&target, false, ViewMode::List, false);
            for format in CompressFormat::ALL {
                assert!(has(&items, MenuAction::Compress(format)), "{format:?}");
            }
        }
        assert!(
            labels(&build_menu(&files(1), false, ViewMode::List, false))
                .contains(&"Compress to ZIP file")
        );
        assert!(
            labels(&build_menu(&files(1), false, ViewMode::List, false))
                .contains(&"Compress to TAR.GZ")
        );
    }

    #[test]
    fn archives_offer_extraction_named_after_their_stem() {
        let items = build_menu(&archive(), false, ViewMode::List, false);
        assert!(has(&items, MenuAction::ExtractHere));
        assert!(labels(&items).contains(&"Extract to “photos/”"));
    }

    #[test]
    fn only_a_single_archive_offers_extraction() {
        assert!(!has(
            &build_menu(&files(1), false, ViewMode::List, false),
            MenuAction::ExtractHere
        ));
        assert!(!has(
            &build_menu(&folder(), false, ViewMode::List, false),
            MenuAction::ExtractTo
        ));
        let many = MenuTarget::Items {
            count: 2,
            is_dir: false,
            archive_stem: Some("a".into()),
        };
        assert!(!has(
            &build_menu(&many, false, ViewMode::List, false),
            MenuAction::ExtractHere
        ));
    }

    #[test]
    fn single_files_can_be_previewed_but_folders_cannot() {
        assert!(has(
            &build_menu(&files(1), false, ViewMode::List, false),
            MenuAction::Preview
        ));
        assert!(!has(
            &build_menu(&folder(), false, ViewMode::List, false),
            MenuAction::Preview
        ));
        assert!(!has(
            &build_menu(&files(2), false, ViewMode::List, false),
            MenuAction::Preview
        ));
    }

    #[test]
    fn background_menu_pastes_only_with_a_clipboard() {
        let empty = build_menu(&MenuTarget::Background, false, ViewMode::List, false);
        assert!(!find(&empty, MenuAction::Paste).enabled);
        assert!(
            find(
                &build_menu(&MenuTarget::Background, true, ViewMode::List, false),
                MenuAction::Paste
            )
            .enabled
        );
        assert!(has(&empty, MenuAction::NewFolder));
    }

    #[test]
    fn no_menu_starts_or_ends_with_a_separator_or_doubles_one() {
        for target in [MenuTarget::Background, files(1), files(2), folder()] {
            let items = build_menu(&target, true, ViewMode::List, false);
            assert_ne!(items.first(), Some(&MenuItem::Separator));
            assert_ne!(items.last(), Some(&MenuItem::Separator));
            assert!(
                !items
                    .windows(2)
                    .any(|w| w[0] == MenuItem::Separator && w[1] == MenuItem::Separator)
            );
        }
    }

    fn checked_actions(items: &[MenuItem]) -> Vec<MenuAction> {
        items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action(a) if a.checked => Some(a.action),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sort_menu_lists_every_key_and_checks_the_current_one() {
        let items = build_sort_for_test(SortKey::Size, false);
        let checked = checked_actions(&items);
        assert!(checked.contains(&MenuAction::SortBy(SortKey::Size)));
        assert!(!checked.contains(&MenuAction::SortBy(SortKey::Name)));
        for key in SortKey::ALL {
            assert!(has(&items, MenuAction::SortBy(key)), "{key:?}");
        }
    }

    #[test]
    fn sort_menu_checks_exactly_one_direction() {
        let asc = checked_actions(&build_sort_for_test(SortKey::Name, false));
        assert!(asc.contains(&MenuAction::SortDescending(false)));
        assert!(!asc.contains(&MenuAction::SortDescending(true)));
        let desc = checked_actions(&build_sort_for_test(SortKey::Name, true));
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

    fn build_sort_for_test(key: SortKey, descending: bool) -> Vec<MenuItem> {
        sort_menu(SortSpec::new(key, descending))
    }

    #[test]
    fn background_menu_checks_the_current_view() {
        for mode in [ViewMode::Icons, ViewMode::List, ViewMode::Columns] {
            let items = build_menu(&MenuTarget::Background, false, mode, false);
            assert_eq!(
                checked_actions(&items),
                [MenuAction::SetView(mode)],
                "{mode:?}"
            );
        }
    }

    #[test]
    fn background_menu_reflects_whether_hidden_files_are_shown() {
        let off = build_menu(&MenuTarget::Background, false, ViewMode::List, false);
        assert!(!checked_actions(&off).contains(&MenuAction::ToggleHidden));
        let on = build_menu(&MenuTarget::Background, false, ViewMode::List, true);
        assert!(checked_actions(&on).contains(&MenuAction::ToggleHidden));
        assert!(has(&off, MenuAction::ToggleHidden));
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
