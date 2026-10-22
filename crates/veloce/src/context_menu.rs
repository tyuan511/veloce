//! The right-click menu: what it contains for a given target, and where it goes.
//!
//! The content follows Finder's menus: Open / Open With ▸ / Compress ▸ / Copy /
//! Rename / Properties ..., with the long lists (formats, applications, sort keys)
//! folded into submenus. Only the model and the geometry live here; the workspace
//! draws it and runs the chosen action.

use crate::i18n::{tr, trf};
use std::path::PathBuf;

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
    /// Enter an application bundle like a folder.
    ShowPackageContents,
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
    /// Star or unstar the item in the sidebar.
    ToggleFavorite,
    // The sidebar's starred items act on that item, not on the list's selection.
    FavOpen,
    FavOpenInNewTab,
    FavReveal,
    FavCopyPath,
    FavOpenInTerminal,
    FavRemove,
    Delete,
    Rename,
    Properties,
    NewFolder,
    NewFile,
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
    /// A small PNG drawn before the label (an application's icon).
    pub icon: Option<PathBuf>,
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
        /// A single application bundle is selected: it launches, but can be entered.
        app_bundle: bool,
        /// Nothing but files is selected (Open With only makes sense then).
        files_only: bool,
        /// A single archive is selected: the folder name "Extract to" would create.
        archive_stem: Option<String>,
    },
}

/// One row of the Open With list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    pub name: String,
    /// PNG of the application's icon, when one could be produced.
    pub icon: Option<PathBuf>,
}

/// Applications that can open the selection, which arrive asynchronously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenWith {
    Loading,
    /// The first is the default application.
    Apps(Vec<AppEntry>),
}

/// State the menu content depends on besides what was clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuContext {
    pub can_paste: bool,
    pub view: ViewMode,
    pub show_hidden: bool,
    pub sort: SortSpec,
    pub open_with: OpenWith,
    /// Whether the single selected item is starred; `None` when it cannot be (several
    /// items, or not on a local disk).
    pub favorite: Option<bool>,
}

impl Default for MenuContext {
    fn default() -> Self {
        Self {
            can_paste: false,
            view: ViewMode::default(),
            show_hidden: false,
            sort: SortSpec::default(),
            open_with: OpenWith::Loading,
            favorite: None,
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
        icon: None,
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
        SortKey::Name => tr("Name"),
        SortKey::Kind => tr("Kind"),
        SortKey::Modified => tr("Date Modified"),
        SortKey::Created => tr("Date Created"),
        SortKey::Size => tr("Size"),
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
        action(
            MenuAction::SortDescending(false),
            tr("Ascending"),
            None,
            true,
        ),
        !spec.descending,
    ));
    items.push(checked(
        action(
            MenuAction::SortDescending(true),
            tr("Descending"),
            None,
            true,
        ),
        spec.descending,
    ));
    items.push(MenuItem::Separator);
    items.push(checked(
        action(
            MenuAction::SortFoldersFirst,
            tr("Keep Folders on Top"),
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
            action(SetView(ViewMode::Icons), tr("as Icons"), Some("⌘1"), true),
            view == ViewMode::Icons,
        ),
        checked(
            action(SetView(ViewMode::List), tr("as List"), Some("⌘2"), true),
            view == ViewMode::List,
        ),
        checked(
            action(
                SetView(ViewMode::Columns),
                tr("as Columns"),
                Some("⌘3"),
                true,
            ),
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
        OpenWith::Loading => vec![action(MenuAction::Nothing, tr("Loading…"), None, false)],
        OpenWith::Apps(apps) if apps.is_empty() => {
            vec![action(
                MenuAction::Nothing,
                tr("No applications"),
                None,
                false,
            )]
        }
        OpenWith::Apps(apps) => {
            let mut rows: Vec<MenuItem> = apps
                .iter()
                .enumerate()
                .map(|(index, app)| {
                    let label = if index == 0 {
                        trf("{} (default)", &[&app.name])
                    } else {
                        app.name.clone()
                    };
                    let mut row = action(MenuAction::OpenWith(index), &label, None, true);
                    if let MenuItem::Action(item) = &mut row {
                        item.icon = app.icon.clone();
                    }
                    row
                })
                .collect();
            if rows.len() > 1 {
                rows.insert(1, MenuItem::Separator);
            }
            rows
        }
    };
    items.push(MenuItem::Separator);
    items.push(action(MenuAction::OpenWithOther, tr("Other…"), None, true));
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
        action(NewFolder, tr("New folder"), Some("⇧⌘N"), true),
        action(NewFile, tr("New file"), None, true),
        MenuItem::Separator,
        checked(
            action(ToggleHidden, tr("Show hidden files"), Some("⇧⌘."), true),
            state.show_hidden,
        ),
        checked(
            action(ToggleSplit, tr("Split view"), Some("⌘\\"), true),
            state.split,
        ),
        checked(
            action(TogglePreviewPanel, tr("Preview panel"), Some("Space"), true),
            state.preview_open,
        ),
        checked(
            action(ToggleTasks, tr("Task panel"), Some("⌘J"), true),
            state.tasks_open,
        ),
        MenuItem::Separator,
        action(Refresh, tr("Refresh"), Some("⌘R"), true),
        action(OpenInTerminal, tr("Open in Terminal"), None, true),
        action(CommandPalette, tr("Command palette…"), Some("⇧⌘P"), true),
        MenuItem::Separator,
        action(OpenSettings, tr("Settings…"), Some("⌘,"), true),
    ]
}

/// The menu for an item in the sidebar. Built-in places (Desktop, the disk ...)
/// cannot be un-starred, so they get no "Remove" row.
pub fn favorite_menu(is_dir: bool, removable: bool) -> Vec<MenuItem> {
    use MenuAction::*;
    let mut items = vec![action(FavOpen, tr("Open"), None, true)];
    if is_dir {
        items.push(action(FavOpenInNewTab, tr("Open in new tab"), None, true));
    }
    items.push(action(FavReveal, tr("Show in folder"), None, true));
    items.push(MenuItem::Separator);
    items.push(action(FavCopyPath, tr("Copy as path"), None, true));
    if is_dir {
        items.push(action(
            FavOpenInTerminal,
            tr("Open in Terminal"),
            None,
            true,
        ));
    }
    if removable {
        items.push(MenuItem::Separator);
        items.push(action(FavRemove, tr("Remove from Favorites"), None, true));
    }
    items
}

/// Build the menu for `target`.
pub fn build_menu(target: &MenuTarget, ctx: &MenuContext) -> Vec<MenuItem> {
    use MenuAction::*;
    match target {
        MenuTarget::Background => vec![
            action(Refresh, tr("Refresh"), Some("⌘R"), true),
            MenuItem::Separator,
            action(Paste, tr("Paste"), Some("⌘V"), ctx.can_paste),
            MenuItem::Separator,
            action(NewFolder, tr("New folder"), Some("⇧⌘N"), true),
            action(NewFile, tr("New file"), None, true),
            action(OpenInTerminal, tr("Open in Terminal"), None, true),
            MenuItem::Separator,
            submenu(tr("Sort By"), sort_menu(ctx.sort)),
            submenu(tr("View"), view_menu(ctx.view)),
            checked(
                action(ToggleHidden, tr("Show hidden files"), Some("⇧⌘."), true),
                ctx.show_hidden,
            ),
        ],
        MenuTarget::Items {
            count,
            is_dir,
            app_bundle,
            files_only,
            archive_stem,
        } => {
            let single = *count == 1;
            let folder = single && *is_dir;
            // 按用途分五组：打开 / 剪贴板 / 文件变换 / 删除 / 位置与属性。
            // 删除单独成组，避免和重命名挨在一起误点。
            let mut items = vec![action(Open, tr("Open"), Some("⌘O"), true)];
            if *files_only {
                items.push(submenu(tr("Open With"), open_with_menu(&ctx.open_with)));
            }
            if folder {
                items.push(action(OpenInNewTab, tr("Open in new tab"), None, true));
            }
            if *app_bundle {
                items.push(action(
                    ShowPackageContents,
                    "Show Package Contents",
                    None,
                    true,
                ));
            }
            if single && !*is_dir {
                items.push(action(Preview, tr("Preview"), Some("Space"), true));
            }
            items.push(MenuItem::Separator);
            items.push(action(Cut, tr("Cut"), Some("⌘X"), true));
            items.push(action(Copy, tr("Copy"), Some("⌘C"), true));
            if folder {
                items.push(action(PasteInto, tr("Paste"), Some("⌘V"), ctx.can_paste));
            }
            items.push(action(CopyPath, tr("Copy as path"), Some("⇧⌘C"), true));
            items.push(MenuItem::Separator);
            items.push(action(Rename, tr("Rename"), Some("F2"), single));
            items.push(action(CreateShortcut, tr("Create shortcut"), None, true));
            if let Some(starred) = ctx.favorite {
                let label = if starred {
                    tr("Remove from Favorites")
                } else {
                    tr("Add to Favorites")
                };
                items.push(action(ToggleFavorite, label, None, true));
            }
            if let (true, Some(stem)) = (single, archive_stem) {
                items.push(action(ExtractHere, tr("Extract Here"), None, true));
                items.push(action(
                    ExtractTo,
                    &trf("Extract to “{}/”", &[stem]),
                    None,
                    true,
                ));
            }
            items.push(submenu(tr("Compress to"), compress_menu()));
            items.push(MenuItem::Separator);
            items.push(action(Delete, tr("Delete"), Some("Del"), true));
            items.push(MenuItem::Separator);
            if folder {
                items.push(action(OpenInTerminal, tr("Open in Terminal"), None, true));
            }
            items.push(action(Properties, tr("Properties"), Some("⌥↵"), single));
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

    fn app(name: &str) -> AppEntry {
        AppEntry {
            name: name.into(),
            icon: None,
        }
    }

    fn ctx() -> MenuContext {
        MenuContext::default()
    }

    #[test]
    fn favorite_row_appears_only_when_it_applies_and_names_the_next_state() {
        let label = |favorite| {
            let ctx = MenuContext { favorite, ..ctx() };
            let items = build_menu(&folder(), &ctx);
            flatten(&items)
                .into_iter()
                .find(|a| a.action == MenuAction::ToggleFavorite)
                .map(|a| a.label.clone())
        };
        assert_eq!(label(None), None);
        assert_eq!(label(Some(false)).as_deref(), Some("Add to Favorites"));
        assert_eq!(label(Some(true)).as_deref(), Some("Remove from Favorites"));
    }

    #[test]
    fn favorite_menu_offers_folder_only_rows_for_folders_and_always_remove() {
        let actions = |is_dir| {
            flatten(&favorite_menu(is_dir, true))
                .into_iter()
                .map(|a| a.action)
                .collect::<Vec<_>>()
        };
        let folder = actions(true);
        let file = actions(false);
        for only_folders in [MenuAction::FavOpenInNewTab, MenuAction::FavOpenInTerminal] {
            assert!(folder.contains(&only_folders));
            assert!(!file.contains(&only_folders));
        }
        for always in [
            MenuAction::FavOpen,
            MenuAction::FavReveal,
            MenuAction::FavCopyPath,
            MenuAction::FavRemove,
        ] {
            assert!(folder.contains(&always) && file.contains(&always));
        }
    }

    #[test]
    fn built_in_places_have_no_remove_row() {
        let builtin = favorite_menu(true, false);
        assert!(
            !flatten(&builtin)
                .iter()
                .any(|a| a.action == MenuAction::FavRemove)
        );
        assert!(
            flatten(&builtin)
                .iter()
                .any(|a| a.action == MenuAction::FavOpenInTerminal)
        );
        // No dangling separator where the Remove row would have been.
        assert!(!matches!(builtin.last(), Some(MenuItem::Separator)));
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
            app_bundle: false,
            archive_stem: None,
        }
    }

    fn folder() -> MenuTarget {
        MenuTarget::Items {
            count: 1,
            is_dir: true,
            files_only: false,
            app_bundle: false,
            archive_stem: None,
        }
    }

    fn archive() -> MenuTarget {
        MenuTarget::Items {
            count: 1,
            is_dir: false,
            files_only: true,
            app_bundle: false,
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
        assert!(at("Copy") < at("Rename"));
        assert!(at("Rename") < at("Delete"));
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
            app_bundle: false,
            archive_stem: Some("a".into()),
        };
        assert!(!has(&build_menu(&many, &ctx()), MenuAction::ExtractHere));
    }

    #[test]
    fn groups_are_separated_by_exactly_one_separator() {
        for target in [files(1), files(3), MenuTarget::Background] {
            let items = build_menu(&target, &ctx());
            assert!(!matches!(items.first(), Some(MenuItem::Separator)));
            assert!(!matches!(items.last(), Some(MenuItem::Separator)));
            assert!(
                !items
                    .windows(2)
                    .any(|w| matches!(w, [MenuItem::Separator, MenuItem::Separator]))
            );
        }
    }

    #[test]
    fn delete_stands_alone_in_its_own_group() {
        let items = build_menu(&files(1), &ctx());
        let at = items
            .iter()
            .position(|i| matches!(i, MenuItem::Action(a) if a.action == MenuAction::Delete))
            .unwrap();
        assert!(matches!(items[at - 1], MenuItem::Separator));
        assert!(matches!(items[at + 1], MenuItem::Separator));
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
            app_bundle: false,
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
            open_with: OpenWith::Apps(vec![app("Preview"), app("Pixelmator")]),
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
    fn open_with_rows_carry_the_app_icon() {
        let ready = MenuContext {
            open_with: OpenWith::Apps(vec![AppEntry {
                name: "Preview".into(),
                icon: Some(PathBuf::from("/cache/Preview.png")),
            }]),
            ..ctx()
        };
        let items = build_menu(&files(1), &ready);
        let open_with = sub(&items, "Open With");
        assert_eq!(
            find(&open_with.items, MenuAction::OpenWith(0)).icon,
            Some(PathBuf::from("/cache/Preview.png"))
        );
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
        assert!(has(&empty, MenuAction::NewFile));
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
