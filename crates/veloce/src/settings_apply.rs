//! Changing settings at run time: update the global value, push the parts other
//! code caches (theme, row height, folder defaults), save, and repaint.
//!
//! Every window edits through [`commit`], so the main window and the Settings
//! window can never disagree about what is set.

use gpui::App;

use crate::{persist::Persist, settings::AppSettings, theme};

/// Make the visual parts of `settings` take effect.
fn apply_visuals(settings: &AppSettings) {
    theme::set_preference(settings.theme);
    theme::set_row_height(settings.density.row_height());
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
    if let Some(persist) = cx.try_global::<Persist>().cloned() {
        persist.set_defaults(after.default_view, after.default_sort);
        persist.save_settings(cx, &after);
        // A smaller cache limit takes effect now, not at the next launch.
        if after.thumbnail_cache_mb < before.thumbnail_cache_mb {
            persist.prune_thumbnails(cx, after.cache_limit_bytes());
        }
    }
    cx.set_global(after);
    cx.refresh_windows();
}
