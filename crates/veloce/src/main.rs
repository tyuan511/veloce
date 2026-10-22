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
mod connections;
mod context_menu;
mod file_icons;
#[rustfmt::skip]
mod file_icons_data;
mod history;
mod icons;
mod info;
mod keymap;
mod listing;
mod open_with;
mod palette;
mod pane;
mod path_query;
mod persist;
mod prefs;
mod preview;
mod settings;
mod settings_apply;
mod settings_window;
mod tasks_panel;
mod terminals;
mod text_input;
mod theme;
mod thumbs;
mod view;
mod workspace;

use gpui::{App, Application, Bounds, WindowBounds, WindowOptions, px, size};

use crate::{icons::EmbeddedAssets, workspace::new_workspace};

fn main() {
    let app = Application::new().with_assets(EmbeddedAssets);

    app.run(|cx: &mut App| {
        // Load everything saved before the window exists, so the first frame is
        // already the restored layout.
        let (persist, startup) = persist::Persist::open();
        actions::install_keymap(cx, &startup.settings.shortcuts);
        settings_apply::init(cx, startup.settings.clone());
        cx.set_global(persist.clone());
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
                |_, cx| new_workspace(persist, startup, cx),
            )
            .expect("open window");

        window
            .update(cx, |workspace, window, cx| {
                workspace.start(window, cx);
            })
            .ok();

        // The Settings window is its own window, so ⌘, and the menu bar are app-level
        // rather than handled by a workspace.
        cx.on_action(|_: &actions::OpenSettings, cx| settings_window::open(cx));
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.set_menus(vec![gpui::Menu {
            name: "Veloce".into(),
            items: vec![
                gpui::MenuItem::action("Settings…", actions::OpenSettings),
                gpui::MenuItem::separator(),
                gpui::MenuItem::action("Quit Veloce", actions::Quit),
            ],
        }]);
        // Closing the main window ends the app even if Settings is still open.
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        cx.activate(true);
    });
}
