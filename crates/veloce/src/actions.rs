//! Every user-triggerable command, plus the keymap that binds them.
//!
//! Bindings use the `Workspace && !TextInput` context so a focused text field
//! keeps its keystrokes: GPUI would otherwise fire both the action and the
//! character insertion.

use gpui::{App, KeyBinding, actions};

actions!(
    veloce,
    [
        // navigation
        MoveDown,
        MoveUp,
        MovePageDown,
        MovePageUp,
        JumpTop,
        JumpBottom,
        GoParent,
        OpenEntry,
        FocusLeft,
        FocusRight,
        MoveLeft,
        MoveRight,
        // selection
        ExtendDown,
        ExtendUp,
        SelectAll,
        ClearSelection,
        // panes & tabs
        SwitchPane,
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        ToggleSplit,
        // filtering & palette
        StartFilter,
        OpenPalette,
        OpenCommands,
        // preview
        TogglePreview,
        QuickLook,
        ViewIcons,
        ViewList,
        ViewColumns,
        ToggleHidden,
        // transfers
        CopyToOtherPane,
        MoveToOtherPane,
        TrashSelection,
        NewFolder,
        RenameEntry,
        // file menu (Finder parity)
        OpenSelection,
        CopyItems,
        PasteItems,
        CutItems,
        CopyAsPath,
        DeleteItems,
        Properties,
        // panels
        ToggleTasks,
        // misc
        Quit,
        CloseOverlay,
        Refresh,
        CycleSort,
    ]
);

/// The keymap context every list binding is scoped to.
pub const CONTEXT: &str = "Workspace && !TextInput";

/// Install the default keymap. Called once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        // ---- navigation (Vim-first, arrows as an alias) ----
        KeyBinding::new("j", MoveDown, Some(CONTEXT)),
        KeyBinding::new("down", MoveDown, Some(CONTEXT)),
        KeyBinding::new("k", MoveUp, Some(CONTEXT)),
        KeyBinding::new("up", MoveUp, Some(CONTEXT)),
        KeyBinding::new("ctrl-d", MovePageDown, Some(CONTEXT)),
        KeyBinding::new("pagedown", MovePageDown, Some(CONTEXT)),
        KeyBinding::new("ctrl-u", MovePageUp, Some(CONTEXT)),
        KeyBinding::new("pageup", MovePageUp, Some(CONTEXT)),
        KeyBinding::new("g g", JumpTop, Some(CONTEXT)),
        KeyBinding::new("home", JumpTop, Some(CONTEXT)),
        KeyBinding::new("shift-g", JumpBottom, Some(CONTEXT)),
        KeyBinding::new("end", JumpBottom, Some(CONTEXT)),
        KeyBinding::new("h", GoParent, Some(CONTEXT)),
        KeyBinding::new("left", MoveLeft, Some(CONTEXT)),
        KeyBinding::new("backspace", GoParent, Some(CONTEXT)),
        KeyBinding::new("cmd-up", GoParent, Some(CONTEXT)),
        KeyBinding::new("l", OpenEntry, Some(CONTEXT)),
        KeyBinding::new("right", MoveRight, Some(CONTEXT)),
        KeyBinding::new("enter", OpenEntry, Some(CONTEXT)),
        // ---- selection ----
        KeyBinding::new("shift-j", ExtendDown, Some(CONTEXT)),
        KeyBinding::new("shift-down", ExtendDown, Some(CONTEXT)),
        KeyBinding::new("shift-k", ExtendUp, Some(CONTEXT)),
        KeyBinding::new("shift-up", ExtendUp, Some(CONTEXT)),
        KeyBinding::new("cmd-a", SelectAll, Some(CONTEXT)),
        // ---- panes & tabs ----
        KeyBinding::new("tab", SwitchPane, Some(CONTEXT)),
        KeyBinding::new("cmd-t", NewTab, Some(CONTEXT)),
        KeyBinding::new("cmd-w", CloseTab, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-]", NextTab, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-[", PrevTab, Some(CONTEXT)),
        KeyBinding::new("cmd-\\", ToggleSplit, Some(CONTEXT)),
        KeyBinding::new("ctrl-1", FocusLeft, Some(CONTEXT)),
        KeyBinding::new("ctrl-2", FocusRight, Some(CONTEXT)),
        // ---- views (Finder: ⌘1 icons, ⌘2 list, ⌘3 columns) ----
        KeyBinding::new("cmd-1", ViewIcons, Some(CONTEXT)),
        KeyBinding::new("cmd-2", ViewList, Some(CONTEXT)),
        KeyBinding::new("cmd-3", ViewColumns, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-.", ToggleHidden, Some(CONTEXT)),
        // ---- filtering & palette ----
        KeyBinding::new("/", StartFilter, Some(CONTEXT)),
        KeyBinding::new("cmd-p", OpenPalette, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-p", OpenCommands, Some(CONTEXT)),
        // ---- preview ----
        KeyBinding::new("space", QuickLook, Some(CONTEXT)),
        KeyBinding::new("cmd-alt-i", TogglePreview, Some(CONTEXT)),
        // ---- transfers ----
        KeyBinding::new("f5", CopyToOtherPane, Some(CONTEXT)),
        KeyBinding::new("f6", MoveToOtherPane, Some(CONTEXT)),
        KeyBinding::new("cmd-backspace", TrashSelection, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-n", NewFolder, Some(CONTEXT)),
        KeyBinding::new("shift-r", RenameEntry, Some(CONTEXT)),
        KeyBinding::new("cmd-o", OpenSelection, Some(CONTEXT)),
        KeyBinding::new("cmd-c", CopyItems, Some(CONTEXT)),
        KeyBinding::new("cmd-v", PasteItems, Some(CONTEXT)),
        KeyBinding::new("cmd-x", CutItems, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-c", CopyAsPath, Some(CONTEXT)),
        KeyBinding::new("delete", DeleteItems, Some(CONTEXT)),
        KeyBinding::new("f2", RenameEntry, Some(CONTEXT)),
        KeyBinding::new("alt-enter", Properties, Some(CONTEXT)),
        // ---- panels ----
        KeyBinding::new("cmd-j", ToggleTasks, Some(CONTEXT)),
        KeyBinding::new("cmd-r", Refresh, Some(CONTEXT)),
        KeyBinding::new("shift-s", CycleSort, Some(CONTEXT)),
        // ---- misc ----
        KeyBinding::new("cmd-q", Quit, None),
        // Esc is the universal "back out": overlays first, then selection.
        KeyBinding::new("escape", CloseOverlay, None),
    ]);
}

/// Human-readable cheat sheet shown in the empty preview state and the help row.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("j / k", "move down / up"),
    ("h / l", "parent / open"),
    ("space", "quick look"),
    ("/", "filter this folder"),
    ("⌘P", "fuzzy jump"),
    ("⌘⇧P", "commands"),
    ("tab", "switch pane"),
    ("F5 / F6", "copy / move to other pane"),
    ("⌘⌫", "move to trash"),
    ("⌘⇧N", "new folder"),
    ("⌘J", "task panel"),
];
