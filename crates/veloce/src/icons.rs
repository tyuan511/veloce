//! Inline SVG icons, embedded in the binary.
//!
//! Every glyph is monochrome: GPUI tints an SVG with the element's text color, so
//! one asset serves every state (idle / hover / selected / accent).

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub const FOLDER: &str = "icons/folder.svg";
pub const FILE: &str = "icons/file.svg";
pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
pub const ARROW_UP: &str = "icons/arrow-up.svg";
pub const SEARCH: &str = "icons/search.svg";
pub const CLOSE: &str = "icons/close.svg";
pub const PLUS: &str = "icons/plus.svg";
pub const TERMINAL: &str = "icons/terminal.svg";
pub const TASKS: &str = "icons/tasks.svg";
pub const EYE: &str = "icons/eye.svg";
pub const LINK: &str = "icons/link.svg";

const FOLDER_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7.5A2 2 0 0 1 5 5.5h4l2 2.5h8a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>"#;

const FILE_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3.5h7l5 5v12a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-16a1 1 0 0 1 1-1z"/><path d="M13 3.5V9h5"/></svg>"#;

const CHEVRON_RIGHT_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 5l7 7-7 7"/></svg>"#;

const CHEVRON_DOWN_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 9l7 7 7-7"/></svg>"#;

const ARROW_UP_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5"/><path d="M6 11l6-6 6 6"/></svg>"#;

const SEARCH_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="6.5"/><path d="M16 16l4.5 4.5"/></svg>"#;

const CLOSE_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 6l12 12M18 6L6 18"/></svg>"#;

const PLUS_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14M5 12h14"/></svg>"#;

const TERMINAL_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4.5" width="18" height="15" rx="2"/><path d="M7 10l2.5 2.5L7 15"/><path d="M12.5 15H17"/></svg>"#;

const TASKS_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h9"/><path d="M4 12h9"/><path d="M4 17h6"/><path d="M17 13l3 3 3-3" transform="translate(-2 0)"/></svg>"#;

const EYE_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/></svg>"#;

const LINK_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7 0l2-2a5 5 0 0 0-7-7l-1 1"/><path d="M14 11a5 5 0 0 0-7 0l-2 2a5 5 0 0 0 7 7l1-1"/></svg>"#;

const ICONS: &[(&str, &str)] = &[
    (FOLDER, FOLDER_SVG),
    (FILE, FILE_SVG),
    (CHEVRON_RIGHT, CHEVRON_RIGHT_SVG),
    (CHEVRON_DOWN, CHEVRON_DOWN_SVG),
    (ARROW_UP, ARROW_UP_SVG),
    (SEARCH, SEARCH_SVG),
    (CLOSE, CLOSE_SVG),
    (PLUS, PLUS_SVG),
    (TERMINAL, TERMINAL_SVG),
    (TASKS, TASKS_SVG),
    (EYE, EYE_SVG),
    (LINK, LINK_SVG),
];

/// Serves the embedded icons to GPUI's SVG renderer.
pub struct EmbeddedAssets;

impl AssetSource for EmbeddedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, svg)| Cow::Borrowed(svg.as_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::new_static(name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_every_icon() {
        let assets = EmbeddedAssets;
        for (name, _) in ICONS {
            let data = assets.load(name).unwrap().expect("icon present");
            assert!(data.starts_with(b"<svg"));
        }
    }

    #[test]
    fn unknown_asset_is_none() {
        assert!(EmbeddedAssets.load("icons/nope.svg").unwrap().is_none());
    }
}
