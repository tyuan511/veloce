//! The visual language: a dark and a light palette plus the spacing scale.
//!
//! The palette follows the OS appearance: the workspace calls [`set_appearance`]
//! at start-up and whenever the system switches, and every color accessor below
//! reads the active palette at render time. The UI stays mostly neutral greys and
//! only the focus ring, selection and active tab are tinted.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Hsla, WindowAppearance};

struct Palette {
    bg: Hsla,
    bg_pane: Hsla,
    bg_raised: Hsla,
    bg_hover: Hsla,
    bg_selected: Hsla,
    bg_selected_inactive: Hsla,
    border: Hsla,
    border_soft: Hsla,
    text: Hsla,
    text_muted: Hsla,
    text_faint: Hsla,
    accent: Hsla,
    accent_dim: Hsla,
    green: Hsla,
    red: Hsla,
    blue: Hsla,
    purple: Hsla,
}

const DARK: Palette = Palette {
    bg: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.086,
        a: 1.0,
    },
    bg_pane: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.106,
        a: 1.0,
    },
    bg_raised: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.145,
        a: 1.0,
    },
    bg_hover: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.19,
        a: 1.0,
    },
    bg_selected: Hsla {
        h: 0.58,
        s: 0.55,
        l: 0.36,
        a: 1.0,
    },
    bg_selected_inactive: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.26,
        a: 1.0,
    },
    border: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.2,
        a: 1.0,
    },
    border_soft: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.16,
        a: 1.0,
    },
    text: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.93,
        a: 1.0,
    },
    text_muted: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.62,
        a: 1.0,
    },
    text_faint: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.42,
        a: 1.0,
    },
    accent: Hsla {
        h: 0.58,
        s: 0.85,
        l: 0.62,
        a: 1.0,
    },
    accent_dim: Hsla {
        h: 0.58,
        s: 0.5,
        l: 0.45,
        a: 1.0,
    },
    green: Hsla {
        h: 0.36,
        s: 0.6,
        l: 0.55,
        a: 1.0,
    },
    red: Hsla {
        h: 0.0,
        s: 0.62,
        l: 0.6,
        a: 1.0,
    },
    blue: Hsla {
        h: 0.56,
        s: 0.7,
        l: 0.62,
        a: 1.0,
    },
    purple: Hsla {
        h: 0.75,
        s: 0.5,
        l: 0.65,
        a: 1.0,
    },
};

const LIGHT: Palette = Palette {
    bg: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.93,
        a: 1.0,
    },
    bg_pane: Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: 1.0,
    },
    bg_raised: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.955,
        a: 1.0,
    },
    bg_hover: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.9,
        a: 1.0,
    },
    bg_selected: Hsla {
        h: 0.58,
        s: 0.8,
        l: 0.5,
        a: 1.0,
    },
    bg_selected_inactive: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.82,
        a: 1.0,
    },
    border: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.82,
        a: 1.0,
    },
    border_soft: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.88,
        a: 1.0,
    },
    text: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.12,
        a: 1.0,
    },
    text_muted: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.38,
        a: 1.0,
    },
    text_faint: Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.55,
        a: 1.0,
    },
    accent: Hsla {
        h: 0.58,
        s: 0.85,
        l: 0.45,
        a: 1.0,
    },
    accent_dim: Hsla {
        h: 0.58,
        s: 0.6,
        l: 0.6,
        a: 1.0,
    },
    green: Hsla {
        h: 0.36,
        s: 0.6,
        l: 0.33,
        a: 1.0,
    },
    red: Hsla {
        h: 0.0,
        s: 0.65,
        l: 0.48,
        a: 1.0,
    },
    blue: Hsla {
        h: 0.58,
        s: 0.75,
        l: 0.5,
        a: 1.0,
    },
    purple: Hsla {
        h: 0.75,
        s: 0.45,
        l: 0.5,
        a: 1.0,
    },
};

static IS_DARK: AtomicBool = AtomicBool::new(true);

/// Adopt the OS appearance. Returns `true` if the active palette changed.
pub fn set_appearance(appearance: WindowAppearance) -> bool {
    let dark = is_dark_appearance(appearance);
    IS_DARK.swap(dark, Ordering::Relaxed) != dark
}

/// Whether the dark palette is active.
pub fn is_dark() -> bool {
    IS_DARK.load(Ordering::Relaxed)
}

/// Pure mapping from the OS appearance to "use the dark palette".
pub fn is_dark_appearance(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

fn palette() -> &'static Palette {
    if IS_DARK.load(Ordering::Relaxed) {
        &DARK
    } else {
        &LIGHT
    }
}

pub fn bg() -> Hsla {
    palette().bg
}

pub fn bg_pane() -> Hsla {
    palette().bg_pane
}

pub fn bg_raised() -> Hsla {
    palette().bg_raised
}

pub fn bg_hover() -> Hsla {
    palette().bg_hover
}

pub fn bg_selected() -> Hsla {
    palette().bg_selected
}

pub fn bg_selected_inactive() -> Hsla {
    palette().bg_selected_inactive
}

pub fn border() -> Hsla {
    palette().border
}

pub fn border_soft() -> Hsla {
    palette().border_soft
}

pub fn text() -> Hsla {
    palette().text
}

pub fn text_muted() -> Hsla {
    palette().text_muted
}

pub fn text_faint() -> Hsla {
    palette().text_faint
}

pub fn accent() -> Hsla {
    palette().accent
}

pub fn accent_dim() -> Hsla {
    palette().accent_dim
}

pub fn green() -> Hsla {
    palette().green
}

pub fn red() -> Hsla {
    palette().red
}

pub fn blue() -> Hsla {
    palette().blue
}

pub fn purple() -> Hsla {
    palette().purple
}

/// Row height for the file list. Fixed so `uniform_list` can virtualize on it.
pub const ROW_HEIGHT: f32 = 28.0;
pub const STATUS_HEIGHT: f32 = 24.0;
pub const SIDEBAR_WIDTH: f32 = 176.0;

pub const MONO_FAMILY: &str = "Menlo";

// Contrast invariants, checked at compile time for both palettes: a theme edit
// that makes text unreadable must fail the build, not a test someone forgot to run.
const fn readable(text: f32, bg: f32, dark: bool) -> bool {
    if dark { text > bg } else { text < bg }
}

const fn contrast_ok(p: &Palette, dark: bool) -> bool {
    readable(p.text.l, p.bg.l, dark)
        && readable(p.text.l, p.bg_pane.l, dark)
        && readable(p.text.l, p.bg_raised.l, dark)
        && readable(p.text.l, p.bg_hover.l, dark)
        && readable(p.text.l, p.bg_selected_inactive.l, dark)
        && readable(p.text_muted.l, p.bg_pane.l, dark)
        && readable(p.text_muted.l, p.bg_hover.l, dark)
}

const _: () = {
    assert!(contrast_ok(&DARK, true));
    assert!(contrast_ok(&LIGHT, false));
    assert!(ROW_HEIGHT.fract() == 0.0, "uniform_list needs whole rows");
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dark_appearances_pick_the_dark_palette() {
        assert!(is_dark_appearance(WindowAppearance::Dark));
        assert!(is_dark_appearance(WindowAppearance::VibrantDark));
        assert!(!is_dark_appearance(WindowAppearance::Light));
        assert!(!is_dark_appearance(WindowAppearance::VibrantLight));
    }
}
