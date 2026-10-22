//! The list of rebindable commands, their default keys, and the pure logic for
//! user overrides: parsing, conflict checks and display strings.
//!
//! Only data and string handling live here, so every rule is unit-tested. Turning
//! the result into live GPUI bindings is `actions::install_keymap`.

use crate::i18n::{tr, trf};
use std::collections::BTreeMap;

use gpui::Keystroke;

/// One rebindable command.
pub struct Command {
    /// The GPUI action's name inside the `veloce` namespace.
    pub action: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    /// Space-separated keystrokes make a sequence (`g g`).
    pub defaults: &'static [&'static str],
}

const fn cmd(
    group: &'static str,
    action: &'static str,
    label: &'static str,
    defaults: &'static [&'static str],
) -> Command {
    Command {
        action,
        label,
        group,
        defaults,
    }
}

pub const COMMANDS: &[Command] = &[
    cmd("Navigation", "MoveDown", "Move down", &["j", "down"]),
    cmd("Navigation", "MoveUp", "Move up", &["k", "up"]),
    cmd(
        "Navigation",
        "MovePageDown",
        "Page down",
        &["ctrl-d", "pagedown"],
    ),
    cmd("Navigation", "MovePageUp", "Page up", &["ctrl-u", "pageup"]),
    cmd("Navigation", "JumpTop", "Jump to top", &["g g", "home"]),
    cmd(
        "Navigation",
        "JumpBottom",
        "Jump to bottom",
        &["shift-g", "end"],
    ),
    cmd(
        "Navigation",
        "GoParent",
        "Parent folder",
        &["h", "backspace", "cmd-up"],
    ),
    cmd("Navigation", "OpenEntry", "Open or enter", &["l", "enter"]),
    cmd("Navigation", "OpenSelection", "Open", &["cmd-o"]),
    cmd("Navigation", "MoveLeft", "Column left", &["left"]),
    cmd("Navigation", "MoveRight", "Column right", &["right"]),
    cmd("Navigation", "GoBack", "Back", &["cmd-["]),
    cmd("Navigation", "GoForward", "Forward", &["cmd-]"]),
    cmd(
        "Selection",
        "ExtendDown",
        "Extend selection down",
        &["shift-j", "shift-down"],
    ),
    cmd(
        "Selection",
        "ExtendUp",
        "Extend selection up",
        &["shift-k", "shift-up"],
    ),
    cmd("Selection", "SelectAll", "Select all", &["cmd-a"]),
    cmd("Panes and tabs", "SwitchPane", "Switch pane", &["tab"]),
    cmd(
        "Panes and tabs",
        "FocusLeft",
        "Focus left pane",
        &["ctrl-1"],
    ),
    cmd(
        "Panes and tabs",
        "FocusRight",
        "Focus right pane",
        &["ctrl-2"],
    ),
    cmd("Panes and tabs", "ToggleSplit", "Split view", &["cmd-\\"]),
    cmd("Panes and tabs", "NewTab", "New tab", &["cmd-t"]),
    cmd("Panes and tabs", "CloseTab", "Close tab", &["cmd-w"]),
    cmd("Panes and tabs", "NextTab", "Next tab", &["cmd-shift-]"]),
    cmd(
        "Panes and tabs",
        "PrevTab",
        "Previous tab",
        &["cmd-shift-["],
    ),
    cmd("View", "ViewIcons", "Icon view", &["cmd-1"]),
    cmd("View", "ViewList", "List view", &["cmd-2"]),
    cmd("View", "ViewColumns", "Column view", &["cmd-3"]),
    cmd(
        "View",
        "ToggleHidden",
        "Show hidden files",
        &["cmd-shift-."],
    ),
    cmd(
        "View",
        "TogglePreview",
        "Preview panel",
        &["space", "cmd-alt-i"],
    ),
    cmd("View", "ToggleTasks", "Task panel", &["cmd-j"]),
    cmd("View", "CycleSort", "Cycle sort order", &["shift-s"]),
    cmd("View", "Refresh", "Refresh", &["cmd-r"]),
    cmd("Search", "StartFilter", "Filter this folder", &["/"]),
    cmd("Search", "OpenPalette", "Search and go to path", &["cmd-p"]),
    cmd(
        "Search",
        "OpenCommands",
        "Command palette",
        &["cmd-shift-p"],
    ),
    cmd("Files", "CopyItems", "Copy", &["cmd-c"]),
    cmd("Files", "CutItems", "Cut", &["cmd-x"]),
    cmd("Files", "PasteItems", "Paste", &["cmd-v"]),
    cmd("Files", "CopyAsPath", "Copy path", &["cmd-shift-c"]),
    cmd("Files", "CopyToOtherPane", "Copy to other pane", &["f5"]),
    cmd("Files", "MoveToOtherPane", "Move to other pane", &["f6"]),
    cmd("Files", "RenameEntry", "Rename", &["shift-r", "f2"]),
    cmd("Files", "NewFolder", "New folder", &["cmd-shift-n"]),
    cmd(
        "Files",
        "TrashSelection",
        "Move to Trash",
        &["cmd-backspace"],
    ),
    cmd(
        "Files",
        "DeleteItems",
        "Move to Trash (Delete key)",
        &["delete"],
    ),
    cmd("Files", "Properties", "Properties", &["alt-enter"]),
];

/// Keys that always keep their meaning: no command may take them and they are not
/// listed as rebindable. Escape backs out of everything, so losing it would strand
/// the user inside an overlay.
pub const RESERVED: &[(&str, &str)] = &[
    ("escape", "Close overlay or clear selection"),
    ("cmd-q", "Quit"),
    ("cmd-,", "Settings"),
];

/// Group names in display order.
pub fn groups() -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for command in COMMANDS {
        if !seen.contains(&command.group) {
            seen.push(command.group);
        }
    }
    seen
}

pub fn command(action: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.action == action)
}

/// Overrides: for each customised action, its complete list of bindings. An empty
/// list means the user unbound it on purpose.
pub type Overrides = BTreeMap<String, Vec<String>>;

/// Normalise one chord or sequence (`Cmd-Shift-N`, `shift-cmd-n`) to the form GPUI
/// itself prints, so equal bindings compare equal. `None` for text GPUI rejects.
pub fn canonical(text: &str) -> Option<String> {
    let mut parts = Vec::new();
    for token in text.split_whitespace() {
        let keystroke = Keystroke::parse(token).ok()?;
        if !valid_key(&keystroke.key) {
            return None;
        }
        parts.push(keystroke.unparse());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// GPUI accepts any text as a key name; only a single character or a real named
/// key can ever be pressed, so anything else is a typo or a damaged value.
fn valid_key(key: &str) -> bool {
    const NAMED: &[&str] = &[
        "up",
        "down",
        "left",
        "right",
        "enter",
        "escape",
        "backspace",
        "delete",
        "tab",
        "space",
        "pageup",
        "pagedown",
        "home",
        "end",
        "insert",
    ];
    if key.chars().count() == 1 {
        return true;
    }
    if NAMED.contains(&key) {
        return true;
    }
    key.strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=20).contains(&n))
}

fn tokens(binding: &str) -> Vec<&str> {
    binding.split_whitespace().collect()
}

/// Two bindings clash when one is the other or its start: `g` would swallow the
/// first press of `g g`.
fn clashes(a: &str, b: &str) -> bool {
    let (a, b) = (tokens(a), tokens(b));
    let shared = a.len().min(b.len());
    a[..shared] == b[..shared]
}

/// The bindings in force for `command`.
pub fn effective(command: &Command, overrides: &Overrides) -> Vec<String> {
    match overrides.get(command.action) {
        Some(list) => list.clone(),
        None => command
            .defaults
            .iter()
            .filter_map(|d| canonical(d))
            .collect(),
    }
}

pub fn is_customised(action: &str, overrides: &Overrides) -> bool {
    overrides.contains_key(action)
}

/// Every `(action, binding)` pair in force, in table order.
pub fn all_bindings(overrides: &Overrides) -> Vec<(&'static str, String)> {
    COMMANDS
        .iter()
        .flat_map(|c| {
            effective(c, overrides)
                .into_iter()
                .map(move |binding| (c.action, binding))
        })
        .collect()
}

/// Why a binding cannot be added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not a keystroke GPUI understands, or only modifier keys.
    Invalid,
    /// Reserved (Escape, ⌘Q, ⌘,) — carries what it does.
    Reserved(&'static str),
    /// Already used by another command — carries that command's label.
    Taken(&'static str),
}

impl Refusal {
    pub fn message(&self, shown: &str) -> String {
        match self {
            Refusal::Invalid => tr("That is not a usable shortcut").to_string(),
            Refusal::Reserved(what) => trf("{} is reserved for “{}”", &[shown, tr(what)]),
            Refusal::Taken(label) => trf(
                "{} is already used by “{}”. Remove it there first.",
                &[shown, tr(label)],
            ),
        }
    }
}

/// Add `binding` to `action`, returning the new overrides or the reason not to.
pub fn add_binding(
    overrides: &Overrides,
    action: &str,
    binding: &str,
) -> Result<Overrides, Refusal> {
    let binding = canonical(binding).ok_or(Refusal::Invalid)?;
    let target = command(action).ok_or(Refusal::Invalid)?;
    if let Some((_, what)) = RESERVED.iter().find(|(key, _)| clashes(key, &binding)) {
        return Err(Refusal::Reserved(what));
    }
    for other in COMMANDS {
        if other.action == action {
            continue;
        }
        if effective(other, overrides)
            .iter()
            .any(|existing| clashes(existing, &binding))
        {
            return Err(Refusal::Taken(other.label));
        }
    }
    let mut list = effective(target, overrides);
    if list.iter().any(|existing| clashes(existing, &binding)) {
        // Adding the same key twice, or a prefix of one of its own sequences.
        return Err(Refusal::Taken(target.label));
    }
    list.push(binding);
    Ok(with_list(overrides, target, list))
}

/// Remove one binding from `action`.
pub fn remove_binding(overrides: &Overrides, action: &str, binding: &str) -> Overrides {
    let Some(target) = command(action) else {
        return overrides.clone();
    };
    let list: Vec<String> = effective(target, overrides)
        .into_iter()
        .filter(|existing| existing != binding)
        .collect();
    with_list(overrides, target, list)
}

/// Put `action` back to its default bindings.
pub fn reset_action(overrides: &Overrides, action: &str) -> Overrides {
    let mut next = overrides.clone();
    next.remove(action);
    next
}

/// Store `list`, or forget the override when it equals the defaults.
fn with_list(overrides: &Overrides, target: &Command, list: Vec<String>) -> Overrides {
    let defaults: Vec<String> = target
        .defaults
        .iter()
        .filter_map(|d| canonical(d))
        .collect();
    let mut next = overrides.clone();
    if list == defaults {
        next.remove(target.action);
    } else {
        next.insert(target.action.to_string(), list);
    }
    next
}

/// Serialise: one `Action=binding<TAB>binding` line per customised action.
pub fn serialize(overrides: &Overrides) -> String {
    overrides
        .iter()
        .map(|(action, list)| format!("{action}={}", list.join("\t")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse what [`serialize`] wrote. Lines for actions that no longer exist, bad
/// keystrokes, and bindings that would clash with each other are dropped, so a
/// hand-edited or older database never produces a broken keymap.
pub fn parse(text: &str) -> Overrides {
    let mut result = Overrides::new();
    for line in text.lines() {
        let Some((action, rest)) = line.split_once('=') else {
            continue;
        };
        let Some(target) = command(action) else {
            continue;
        };
        let list: Vec<String> = rest.split('\t').filter_map(canonical).collect();
        let defaults: Vec<String> = target
            .defaults
            .iter()
            .filter_map(|d| canonical(d))
            .collect();
        if list != defaults {
            result.insert(action.to_string(), list);
        }
    }
    // Re-validate as a whole: reserved keys and cross-command clashes are removed.
    let mut clean = Overrides::new();
    for target in COMMANDS {
        let Some(list) = result.get(target.action) else {
            continue;
        };
        let mut accepted: Vec<String> = Vec::new();
        for binding in list {
            let reserved = RESERVED.iter().any(|(key, _)| clashes(key, binding));
            let taken_elsewhere = COMMANDS.iter().any(|other| {
                other.action != target.action
                    && effective(other, &clean_or(&clean, &result, other))
                        .iter()
                        .any(|existing| clashes(existing, binding))
            });
            let repeated = accepted.iter().any(|existing| clashes(existing, binding));
            if !reserved && !taken_elsewhere && !repeated {
                accepted.push(binding.clone());
            }
        }
        clean.insert(target.action.to_string(), accepted);
    }
    // Overrides that ended up equal to the defaults are not overrides.
    COMMANDS.iter().for_each(|c| {
        if let Some(list) = clean.get(c.action) {
            let defaults: Vec<String> = c.defaults.iter().filter_map(|d| canonical(d)).collect();
            if *list == defaults {
                clean.remove(c.action);
            }
        }
    });
    clean
}

/// The override map as seen while validating `other`: already-cleaned entries win,
/// otherwise what was read (it may still be cleaned later).
fn clean_or(clean: &Overrides, read: &Overrides, other: &Command) -> Overrides {
    let mut view = Overrides::new();
    if let Some(list) = clean.get(other.action).or_else(|| read.get(other.action)) {
        view.insert(other.action.to_string(), list.clone());
    }
    view
}

/// `cmd-shift-n` → `⇧⌘N`; sequences join with a space.
pub fn display(binding: &str) -> String {
    tokens(binding)
        .into_iter()
        .map(display_one)
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_one(token: &str) -> String {
    let Ok(keystroke) = Keystroke::parse(token) else {
        return token.to_string();
    };
    let m = keystroke.modifiers;
    let mut out = String::new();
    // macOS order: control, option, shift, command.
    if m.control {
        out.push('⌃');
    }
    if m.alt {
        out.push('⌥');
    }
    if m.shift {
        out.push('⇧');
    }
    if m.platform {
        out.push('⌘');
    }
    if m.function {
        out.push_str("fn ");
    }
    out.push_str(&match keystroke.key.as_str() {
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "enter" => "↩".to_string(),
        "escape" => "⎋".to_string(),
        "backspace" => "⌫".to_string(),
        "delete" => "⌦".to_string(),
        "tab" => "⇥".to_string(),
        "space" => "Space".to_string(),
        "pageup" => "Page Up".to_string(),
        "pagedown" => "Page Down".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        key => key.to_uppercase(),
    });
    out
}

/// Whether a key event is only a modifier press, which cannot be a shortcut.
pub fn is_modifier_only(key: &str) -> bool {
    matches!(
        key,
        "shift" | "control" | "alt" | "platform" | "function" | "cmd" | "ctrl"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Overrides {
        Overrides::new()
    }

    #[test]
    fn every_default_parses_and_is_unique_across_commands() {
        let mut seen: Vec<(String, &str)> = Vec::new();
        for c in COMMANDS {
            for d in c.defaults {
                let binding = canonical(d).unwrap_or_else(|| panic!("{} default {d}", c.action));
                for (other, owner) in &seen {
                    assert!(
                        !clashes(other, &binding),
                        "{} clashes with {owner}",
                        c.action
                    );
                }
                seen.push((binding, c.action));
            }
        }
    }

    #[test]
    fn defaults_avoid_the_reserved_keys() {
        for (action, binding) in all_bindings(&none()) {
            assert!(
                !RESERVED.iter().any(|(key, _)| clashes(key, &binding)),
                "{action} uses a reserved key"
            );
        }
    }

    #[test]
    fn canonical_orders_modifiers_and_rejects_nonsense() {
        assert_eq!(canonical("shift-cmd-n").as_deref(), Some("cmd-shift-n"));
        assert_eq!(canonical("g  g").as_deref(), Some("g g"));
        assert_eq!(canonical(""), None);
        assert_eq!(canonical("cmd-@@@"), None, "not a key");
        assert_eq!(canonical("f13").as_deref(), Some("f13"));
        assert_eq!(canonical("f99"), None);
        assert_eq!(canonical("   "), None);
    }

    #[test]
    fn adding_a_free_key_keeps_the_defaults_and_appends() {
        let next = add_binding(&none(), "Refresh", "cmd-shift-r").unwrap();
        let refresh = command("Refresh").unwrap();
        assert_eq!(effective(refresh, &next), vec!["cmd-r", "cmd-shift-r"]);
        assert!(is_customised("Refresh", &next));
    }

    #[test]
    fn taking_another_commands_key_is_refused_with_its_name() {
        let refusal = add_binding(&none(), "Refresh", "cmd-c").unwrap_err();
        assert_eq!(refusal, Refusal::Taken("Copy"));
        assert!(refusal.message("⌘C").contains("Copy"));
    }

    #[test]
    fn a_key_that_starts_another_sequence_is_refused() {
        // `g g` is JumpTop, so a bare `g` would swallow its first press.
        assert_eq!(
            add_binding(&none(), "Refresh", "g").unwrap_err(),
            Refusal::Taken("Jump to top")
        );
        // ...and the other way round.
        let next = add_binding(&none(), "Refresh", "z").unwrap();
        assert_eq!(
            add_binding(&next, "NewTab", "z z").unwrap_err(),
            Refusal::Taken("Refresh")
        );
    }

    #[test]
    fn reserved_keys_cannot_be_taken() {
        assert!(matches!(
            add_binding(&none(), "Refresh", "escape"),
            Err(Refusal::Reserved(_))
        ));
        assert!(matches!(
            add_binding(&none(), "Refresh", "cmd-q"),
            Err(Refusal::Reserved(_))
        ));
    }

    #[test]
    fn unknown_actions_and_bad_keys_are_invalid() {
        assert_eq!(
            add_binding(&none(), "Nope", "x").unwrap_err(),
            Refusal::Invalid
        );
        assert_eq!(
            add_binding(&none(), "Refresh", "").unwrap_err(),
            Refusal::Invalid
        );
    }

    #[test]
    fn adding_the_same_key_twice_is_refused() {
        let next = add_binding(&none(), "Refresh", "cmd-r");
        assert!(next.is_err(), "already one of its own bindings");
    }

    #[test]
    fn removing_all_bindings_unbinds_the_command_and_frees_the_keys() {
        let mut state = none();
        state = remove_binding(&state, "CopyItems", "cmd-c");
        let copy = command("CopyItems").unwrap();
        assert!(effective(copy, &state).is_empty());
        assert!(is_customised("CopyItems", &state));
        // The freed key can go to another command.
        let state = add_binding(&state, "Refresh", "cmd-c").unwrap();
        assert!(effective(command("Refresh").unwrap(), &state).contains(&"cmd-c".to_string()));
    }

    #[test]
    fn restoring_the_defaults_drops_the_override() {
        let removed = remove_binding(&none(), "Refresh", "cmd-r");
        assert!(is_customised("Refresh", &removed));
        let back = add_binding(&removed, "Refresh", "cmd-r").unwrap();
        assert!(!is_customised("Refresh", &back), "equal to defaults");
        assert!(back.is_empty());
    }

    #[test]
    fn reset_action_only_touches_that_command() {
        let state = add_binding(&none(), "Refresh", "cmd-shift-r").unwrap();
        let state = remove_binding(&state, "NewTab", "cmd-t");
        let state = reset_action(&state, "Refresh");
        assert!(!is_customised("Refresh", &state));
        assert!(is_customised("NewTab", &state));
    }

    #[test]
    fn overrides_survive_a_round_trip_including_sequences_and_unbound() {
        let mut state = add_binding(&none(), "Refresh", "cmd-shift-r").unwrap();
        state = add_binding(&state, "NewTab", "x y").unwrap();
        state = remove_binding(&state, "CopyItems", "cmd-c");
        let again = parse(&serialize(&state));
        assert_eq!(again, state);
    }

    #[test]
    fn damaged_text_is_dropped_piece_by_piece() {
        let text = "Refresh=cmd-shift-r\nGone=cmd-k\nno equals sign\nNewTab=@@@\nCopyItems=escape";
        let parsed = parse(text);
        assert_eq!(parsed["Refresh"], vec!["cmd-shift-r"]);
        assert!(!parsed.contains_key("Gone"));
        assert_eq!(
            parsed["NewTab"],
            Vec::<String>::new(),
            "bad key leaves it unbound"
        );
        assert_eq!(
            parsed["CopyItems"],
            Vec::<String>::new(),
            "a reserved key is not honoured"
        );
    }

    #[test]
    fn a_hand_edited_clash_leaves_exactly_one_owner() {
        // Both want ⌘K; whichever way it resolves, the keymap stays unambiguous.
        let parsed = parse("NewTab=cmd-k\nCloseTab=cmd-k");
        let all = all_bindings(&parsed);
        let owners: Vec<&str> = all
            .iter()
            .filter(|(_, binding)| binding == "cmd-k")
            .map(|(action, _)| *action)
            .collect();
        assert_eq!(owners.len(), 1, "{owners:?}");
    }

    #[test]
    fn overrides_equal_to_defaults_vanish_on_load() {
        assert!(parse("Refresh=cmd-r").is_empty());
        assert!(parse("").is_empty());
    }

    #[test]
    fn bindings_display_in_the_mac_style() {
        assert_eq!(display("cmd-shift-n"), "⇧⌘N");
        assert_eq!(display("ctrl-alt-up"), "⌃⌥↑");
        assert_eq!(display("g g"), "G G");
        assert_eq!(display("space"), "Space");
        assert_eq!(display("cmd-backspace"), "⌘⌫");
    }

    #[test]
    fn modifier_presses_alone_are_not_shortcuts() {
        assert!(is_modifier_only("shift"));
        assert!(is_modifier_only("platform"));
        assert!(!is_modifier_only("a"));
    }

    #[test]
    fn groups_keep_table_order_without_repeats() {
        let groups = groups();
        assert_eq!(groups[0], "Navigation");
        let mut sorted = groups.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), groups.len());
    }
}
