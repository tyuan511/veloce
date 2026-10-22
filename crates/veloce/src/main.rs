//! Veloce — a GPU-accelerated, keyboard-first unified file manager.
//!
//! Architecture (see `docs/research.md`):
//!
//! - `vfs-core` owns everything that is not pixels: URIs, providers, streaming
//!   listings, fuzzy search, background transfers. It has no GUI dependency and is
//!   fully unit-tested.
//! - this crate is the GPUI front-end. It never touches the filesystem directly
//!   except through `vfs-core`, which keeps the UI honest about what a backend can do.

mod actions;
mod clipboard;
mod context_menu;
mod file_icons;
#[rustfmt::skip]
mod file_icons_data;
mod icons;
mod info;
mod listing;
mod palette;
mod pane;
mod preview;
mod tasks_panel;
mod text_input;
mod theme;
mod view;
mod workspace;

use gpui::{App, Application, Bounds, WindowBounds, WindowOptions, px, size};

use crate::{icons::EmbeddedAssets, workspace::new_workspace};

fn main() {
    let app = Application::new().with_assets(EmbeddedAssets);

    app.run(|cx: &mut App| {
        text_input::init(cx);
        actions::bind_keys(cx);

        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(720.0), px(460.0))),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("Veloce".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(gpui::point(px(18.0), px(12.0))),
                    }),
                    ..Default::default()
                },
                |_, cx| new_workspace(cx),
            )
            .expect("open window");

        window
            .update(cx, |workspace, window, cx| {
                workspace.start(window, cx);
            })
            .ok();

        cx.activate(true);
    });
}
