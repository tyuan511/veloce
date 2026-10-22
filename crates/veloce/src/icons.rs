//! UI glyphs: the Lucide set (ISC, see `assets/icons/LICENSE-lucide.txt`), embedded
//! in the binary.
//!
//! Every glyph is monochrome: GPUI tints an SVG with the element's text color, so
//! one asset serves every state (idle / hover / selected / accent).

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub const FOLDER: &str = "icons/folder.svg";
pub const FILE: &str = "icons/file.svg";
pub const LINK: &str = "icons/link.svg";
pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
pub const ARROW_UP: &str = "icons/arrow-up.svg";
pub const SEARCH: &str = "icons/search.svg";
pub const CLOSE: &str = "icons/x.svg";
pub const PLUS: &str = "icons/plus.svg";
pub const TERMINAL: &str = "icons/terminal.svg";
pub const TASKS: &str = "icons/list-checks.svg";
pub const APPS: &str = "icons/app-window.svg";
pub const SPLIT: &str = "icons/columns-2.svg";
pub const EYE: &str = "icons/eye.svg";
pub const HOME: &str = "icons/house.svg";
pub const DESKTOP: &str = "icons/monitor.svg";
pub const DOWNLOAD: &str = "icons/download.svg";
pub const DISK: &str = "icons/hard-drive.svg";

const ICONS: &[(&str, &str)] = &[
    (FOLDER, include_str!("../assets/icons/folder.svg")),
    (FILE, include_str!("../assets/icons/file.svg")),
    (LINK, include_str!("../assets/icons/link.svg")),
    (
        CHEVRON_RIGHT,
        include_str!("../assets/icons/chevron-right.svg"),
    ),
    (
        CHEVRON_DOWN,
        include_str!("../assets/icons/chevron-down.svg"),
    ),
    (ARROW_UP, include_str!("../assets/icons/arrow-up.svg")),
    (SEARCH, include_str!("../assets/icons/search.svg")),
    (CLOSE, include_str!("../assets/icons/x.svg")),
    (PLUS, include_str!("../assets/icons/plus.svg")),
    (TERMINAL, include_str!("../assets/icons/terminal.svg")),
    (TASKS, include_str!("../assets/icons/list-checks.svg")),
    (APPS, include_str!("../assets/icons/app-window.svg")),
    (SPLIT, include_str!("../assets/icons/columns-2.svg")),
    (EYE, include_str!("../assets/icons/eye.svg")),
    (HOME, include_str!("../assets/icons/house.svg")),
    (DESKTOP, include_str!("../assets/icons/monitor.svg")),
    (DOWNLOAD, include_str!("../assets/icons/download.svg")),
    (DISK, include_str!("../assets/icons/hard-drive.svg")),
];

/// Serves the embedded icons to GPUI's SVG renderer.
pub struct EmbeddedAssets;

impl AssetSource for EmbeddedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = crate::file_icons::asset_bytes(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
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
            let text = std::str::from_utf8(&data).unwrap();
            assert!(text.contains("<svg"), "{name}");
        }
    }

    #[test]
    fn unknown_asset_is_none() {
        assert!(EmbeddedAssets.load("icons/nope.svg").unwrap().is_none());
    }
}
