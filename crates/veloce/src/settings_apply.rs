//! Changing settings at run time: update the global value, push the parts other
//! code caches (theme, row height, folder defaults), save, and repaint.
//!
//! Every window edits through [`commit`], so the main window and the Settings
//! window can never disagree about what is set.

use gpui::App;

use crate::{i18n::tr, persist::Persist, settings::AppSettings, theme};

/// Make the visual parts of `settings` take effect.
fn apply_visuals(settings: &AppSettings) {
    crate::i18n::set_lang(settings.language.resolve(crate::i18n::system_lang()));
    theme::set_preference(settings.theme);
    theme::set_row_height(settings.density.row_height());
}

/// The macOS menu bar. Rebuilt when the language changes, since its labels are
/// fixed text handed to the OS rather than something we render each frame.
pub fn install_menus(cx: &mut App) {
    cx.set_menus(vec![gpui::Menu {
        name: "Veloce".into(),
        items: vec![
            gpui::MenuItem::action(tr("Settings…"), crate::actions::OpenSettings),
            gpui::MenuItem::separator(),
            gpui::MenuItem::action(tr("Quit Veloce"), crate::actions::Quit),
        ],
    }]);
}

/// Install the settings loaded at start-up as the global value.
pub fn init(cx: &mut App, settings: AppSettings) {
    apply_visuals(&settings);
    cx.set_global(settings);
}

/// Apply `change` to the settings, then apply, save and repaint.
pub fn commit(cx: &mut App, change: impl FnOnce(&mut AppSettings)) {
    let before = cx.global::<AppSettings>().clone();
    let mut after = before.clone();
    change(&mut after);
    if after == before {
        return;
    }
    apply_visuals(&after);
    if after.shortcuts != before.shortcuts {
        crate::actions::install_keymap(cx, &after.shortcuts);
    }
    if let Some(persist) = cx.try_global::<Persist>().cloned() {
        persist.set_defaults(after.default_view, after.default_sort);
        persist.save_settings(cx, &after);
        // A smaller cache limit takes effect now, not at the next launch.
        if after.thumbnail_cache_mb < before.thumbnail_cache_mb {
            persist.prune_thumbnails(cx, after.cache_limit_bytes());
        }
    }
    let language_changed = after.language != before.language;
    cx.set_global(after);
    if language_changed {
        install_menus(cx);
    }
    cx.refresh_windows();
}
