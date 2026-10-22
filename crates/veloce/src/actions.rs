//! Every user-triggerable command, plus the keymap that binds them.
//!
//! Bindings use the `Workspace && !TextInput` context so a focused text field
//! keeps its keystrokes: GPUI would otherwise fire both the action and the
//! character insertion.

use gpui::{App, KeyBinding, KeyBindingContextPredicate, actions};

use crate::{
    keymap::{self, Overrides},
    text_input,
};

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
        GoBack,
        GoForward,
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
        OpenSettings,
        CloseOverlay,
        Refresh,
        CycleSort,
    ]
);

/// The keymap context every list binding is scoped to.
pub const CONTEXT: &str = "Workspace && !TextInput";

/// Replace every key binding: text-field editing keys, the few fixed system keys,
/// and the rebindable commands with the user's overrides applied. Safe to call
/// again when the shortcuts change, because it starts from a cleared keymap.
pub fn install_keymap(cx: &mut App, overrides: &Overrides) {
    cx.clear_key_bindings();
    text_input::init(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        // Esc is the universal "back out": overlays first, then selection.
        KeyBinding::new("escape", CloseOverlay, None),
    ]);
    let predicate = KeyBindingContextPredicate::parse(CONTEXT)
        .ok()
        .map(std::rc::Rc::new);
    let bindings: Vec<KeyBinding> = keymap::all_bindings(overrides)
        .into_iter()
        .filter_map(|(action, binding)| {
            let built = cx.build_action(&format!("veloce::{action}"), None).ok()?;
            KeyBinding::load(
                &binding,
                built,
                predicate.clone(),
                false,
                None,
                cx.keyboard_mapper().as_ref(),
            )
            .ok()
        })
        .collect();
    cx.bind_keys(bindings);
}

/// Human-readable cheat sheet shown in the empty preview state and the help row.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("j / k", "move down / up"),
    ("h / l", "parent / open"),
    ("space", "preview"),
    ("/", "filter this folder"),
    ("⌘P", "fuzzy jump"),
    ("⌘⇧P", "commands"),
    ("tab", "switch pane"),
    ("F5 / F6", "copy / move to other pane"),
    ("⌘⌫", "move to trash"),
    ("⌘⇧N", "new folder"),
    ("⌘J", "task panel"),
];
