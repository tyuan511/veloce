//! The Settings window: a second OS window with a module list on the left and the
//! selected module's controls on the right.
//!
//! Controls edit the global settings through [`settings_apply::commit`], so a change
//! is live in the main window at once. Connection management (SSH, S3, WebDAV) also
//! lives here: new kinds of remote just add a module section and a form.

use std::sync::Arc;

use gpui::{
    AnyElement, AnyWindowHandle, App, Context, Entity, Global, MouseButton, Render, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use veloce_store::{SavedConnection, Store};

use crate::{
    connections::{self, Draft, Field, Kind},
    persist::Persist,
    settings::{
        AppSettings, CACHE_CHOICES_MB, Density, OnConflict, Startup, ThemePref, clean_terminal_name,
    },
    settings_apply,
    text_input::{TextInput, TextInputEvent},
    theme,
    view::ViewMode,
};

/// The open Settings window, so a second request focuses it instead of opening
/// another.
struct SettingsWindow(AnyWindowHandle);

impl Global for SettingsWindow {}

/// Open Settings, or bring the open one to the front.
pub fn open(cx: &mut App) {
    if let Some(existing) = cx.try_global::<SettingsWindow>().map(|window| window.0)
        && existing
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let persist = cx
        .try_global::<Persist>()
        .cloned()
        .unwrap_or_else(Persist::disabled);
    let bounds = gpui::Bounds::centered(None, size(px(820.0), px(580.0)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(640.0), px(420.0))),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Settings".into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(18.0), px(12.0))),
            }),
            ..Default::default()
        },
        move |window, cx| {
            theme::set_appearance(window.appearance());
            // Follow the OS light/dark switch like the main window does.
            window
                .observe_window_appearance(|window, _| {
                    if theme::set_appearance(window.appearance()) {
                        window.refresh();
                    }
                })
                .detach();
            cx.new(|cx| SettingsView::new(persist, cx))
        },
    );
    if let Ok(handle) = opened {
        cx.set_global(SettingsWindow(handle.into()));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Module {
    General,
    Appearance,
    Files,
    Preview,
    Search,
    Shortcuts,
    Connections,
    Storage,
    About,
}

impl Module {
    const ALL: [Module; 9] = [
        Module::General,
        Module::Appearance,
        Module::Files,
        Module::Preview,
        Module::Search,
        Module::Shortcuts,
        Module::Connections,
        Module::Storage,
        Module::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Module::General => "General",
            Module::Appearance => "Appearance",
            Module::Files => "Files",
            Module::Preview => "Preview",
            Module::Search => "Search",
            Module::Shortcuts => "Shortcuts",
            Module::Connections => "Connections",
            Module::Storage => "Storage",
            Module::About => "About",
        }
    }
}

/// An "add connection" form in progress.
struct ConnectionForm {
    kind: Kind,
    name: Entity<TextInput>,
    fields: Vec<(Field, Entity<TextInput>)>,
    error: Option<String>,
}

struct SettingsView {
    module: Module,
    persist: Persist,
    connections: Vec<SavedConnection>,
    form: Option<ConnectionForm>,
    terminal_input: Entity<TextInput>,
    terminal_error: bool,
    thumbnail_bytes: Option<u64>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl SettingsView {
    fn new(persist: Persist, cx: &mut Context<Self>) -> Self {
        let terminal_input = cx.new(|cx| TextInput::new("Terminal", cx));
        let current = cx.global::<AppSettings>().terminal_app.clone();
        terminal_input.update(cx, |input, cx| input.set_content(current, cx));
        let subscription = cx.subscribe(&terminal_input, |view, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Change(text) = event {
                // Only a usable name is saved; the field flags anything else.
                match clean_terminal_name(text) {
                    Some(name) => {
                        view.terminal_error = false;
                        settings_apply::commit(cx, |settings| settings.terminal_app = name);
                    }
                    None => view.terminal_error = true,
                }
                cx.notify();
            }
        });
        let mut view = Self {
            module: Module::General,
            persist,
            connections: Vec::new(),
            form: None,
            terminal_input,
            terminal_error: false,
            thumbnail_bytes: None,
            _subscriptions: vec![subscription],
        };
        view.reload_connections(cx);
        view.reload_cache_size(cx);
        view
    }

    fn store(&self) -> Option<Arc<Store>> {
        self.persist.store()
    }

    fn reload_connections(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let list = cx
                .background_executor()
                .spawn(async move { store.list_connections().unwrap_or_default() })
                .await;
            this.update(cx, |view, cx| {
                view.connections = list;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn reload_cache_size(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_executor()
                .spawn(async move { store.thumbnail_bytes().ok() })
                .await;
            this.update(cx, |view, cx| {
                view.thumbnail_bytes = bytes;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_form(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let name = cx.new(|cx| TextInput::new("Name, e.g. Production server", cx));
        let fields = connections::fields_for(kind)
            .iter()
            .map(|field| {
                let input = cx.new(|cx| TextInput::new(field.placeholder, cx));
                (*field, input)
            })
            .collect();
        self.form = Some(ConnectionForm {
            kind,
            name,
            fields,
            error: None,
        });
        cx.notify();
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let store = self.store();
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let draft = Draft {
            kind: form.kind,
            name: form.name.read(cx).content().to_string(),
            values: form
                .fields
                .iter()
                .map(|(field, input)| (field.key.to_string(), input.read(cx).content().to_string()))
                .collect(),
        };
        let config = match connections::validate(&draft) {
            Ok(config) => config,
            Err(message) => {
                form.error = Some(message);
                cx.notify();
                return;
            }
        };
        let Some(store) = store else {
            form.error =
                Some("Saving is unavailable: the state database could not be opened".into());
            cx.notify();
            return;
        };
        let kind = draft.kind.key();
        let name = draft.name.trim().to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.add_connection(kind, &name, &config) })
                .await;
            this.update(cx, |view, cx| {
                match result {
                    Ok(_) => {
                        view.form = None;
                        view.reload_connections(cx);
                    }
                    Err(_) => {
                        if let Some(form) = view.form.as_mut() {
                            form.error =
                                Some("A connection with this name already exists".to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn delete_connection(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(store) = self.store() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    let _ = store.delete_connection(id);
                })
                .await;
            this.update(cx, |view, cx| view.reload_connections(cx)).ok();
        })
        .detach();
    }
}

// ---- small building blocks ---------------------------------------------------

fn page(title: &str, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .text_xl()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(title.to_string()),
        )
        .children(children)
        .into_any_element()
}

/// A titled group of rows in a bordered card.
fn section(title: &str, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text_faint())
                .child(title.to_uppercase()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(theme::border_soft())
                .bg(theme::bg_raised())
                .children(rows.into_iter().enumerate().map(|(index, row)| {
                    div()
                        .when(index > 0, |this| {
                            this.border_t_1().border_color(theme::border_soft())
                        })
                        .child(row)
                })),
        )
        .into_any_element()
}

/// One setting: label and hint on the left, its control on the right.
fn row(label: &str, hint: Option<&str>, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .px_4()
        .py_3()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::text())
                        .child(label.to_string()),
                )
                .when_some(hint.map(str::to_string), |this, hint| {
                    this.child(div().text_xs().text_color(theme::text_faint()).child(hint))
                }),
        )
        .child(div().flex_shrink_0().child(control))
        .into_any_element()
}

fn note(text: &str) -> AnyElement {
    div()
        .px_4()
        .py_3()
        .text_xs()
        .text_color(theme::text_faint())
        .child(text.to_string())
        .into_any_element()
}

/// An on/off switch.
fn toggle(id: &'static str, on: bool, change: impl Fn(bool, &mut App) + 'static) -> AnyElement {
    div()
        .id(id)
        .w(px(38.))
        .h(px(22.))
        .rounded_full()
        .p(px(2.))
        .cursor_pointer()
        .flex()
        .items_center()
        .when(on, |this| this.bg(theme::accent()).justify_end())
        .when(!on, |this| this.bg(theme::border()).justify_start())
        .on_click(move |_, _, cx| change(!on, cx))
        .child(
            div()
                .w(px(18.))
                .h(px(18.))
                .rounded_full()
                .bg(gpui::white())
                .shadow_sm(),
        )
        .into_any_element()
}

/// A row of mutually exclusive choices.
fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, String)],
    selected: T,
    pick: impl Fn(T, &mut App) + Clone + 'static,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .p_0p5()
        .gap_0p5()
        .rounded_md()
        .bg(theme::bg_hover())
        .children(options.iter().enumerate().map(|(index, (value, label))| {
            let value = *value;
            let pick = pick.clone();
            let active = value == selected;
            div()
                .id(gpui::ElementId::Name(format!("{id}-{index}").into()))
                .px_3()
                .h(px(24.))
                .flex()
                .items_center()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .when(active, |this| {
                    this.bg(theme::bg_raised()).text_color(theme::text())
                })
                .when(!active, |this| {
                    this.text_color(theme::text_muted())
                        .hover(|this| this.text_color(theme::text()))
                })
                .on_click(move |_, _, cx| pick(value, cx))
                .child(label.clone())
        }))
        .into_any_element()
}

fn labels<T: Copy>(items: &[T], label: impl Fn(T) -> &'static str) -> Vec<(T, String)> {
    items
        .iter()
        .map(|item| (*item, label(*item).to_string()))
        .collect()
}

fn button(
    id: &'static str,
    text: &str,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::bg_hover())
        .text_sm()
        .text_color(theme::text())
        .cursor_pointer()
        .hover(|this| this.bg(theme::border()))
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(text.to_string())
        .into_any_element()
}

fn field_box(input: &Entity<TextInput>, width: f32) -> AnyElement {
    div()
        .w(px(width))
        .h(px(26.))
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::bg())
        .border_1()
        .border_color(theme::border())
        .text_sm()
        .child(input.clone())
        .into_any_element()
}

fn human_bytes(bytes: u64) -> String {
    vfs_core::entry::format_size(bytes)
}

// ---- the modules ---------------------------------------------------------------

impl SettingsView {
    fn render_module(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let settings = cx.global::<AppSettings>().clone();
        match self.module {
            Module::General => self.general(&settings),
            Module::Appearance => appearance(&settings),
            Module::Files => self.files(&settings),
            Module::Preview => preview(&settings),
            Module::Search => search(&settings),
            Module::Shortcuts => shortcuts(),
            Module::Connections => self.connections_page(cx),
            Module::Storage => self.storage(cx),
            Module::About => about(),
        }
    }

    fn general(&self, s: &AppSettings) -> AnyElement {
        let sort_options: Vec<(vfs_core::SortKey, String)> = vfs_core::SortKey::ALL
            .iter()
            .map(|key| {
                (
                    *key,
                    crate::context_menu::sort_key_label(*key).replace("Date ", ""),
                )
            })
            .collect();
        page(
            "General",
            vec![
                section(
                    "Startup",
                    vec![row(
                        "When Veloce starts",
                        Some("The command-line folder, if you pass one, always wins."),
                        segmented(
                            "startup",
                            &labels(&Startup::ALL, Startup::label),
                            s.startup,
                            |value, cx| settings_apply::commit(cx, |s| s.startup = value),
                        ),
                    )],
                ),
                section(
                    "New folders",
                    vec![
                        row(
                            "Default view",
                            Some("For folders you have not customised, as in Finder."),
                            segmented(
                                "view",
                                &[
                                    (ViewMode::Icons, "Icons".to_string()),
                                    (ViewMode::List, "List".to_string()),
                                    (ViewMode::Columns, "Columns".to_string()),
                                ],
                                s.default_view,
                                |value, cx| settings_apply::commit(cx, |s| s.default_view = value),
                            ),
                        ),
                        row(
                            "Sort by",
                            None,
                            segmented(
                                "sort-key",
                                &sort_options,
                                s.default_sort.key,
                                |value, cx| {
                                    settings_apply::commit(cx, |s| s.default_sort.key = value)
                                },
                            ),
                        ),
                        row(
                            "Order",
                            None,
                            segmented(
                                "sort-order",
                                &[
                                    (false, "Ascending".to_string()),
                                    (true, "Descending".to_string()),
                                ],
                                s.default_sort.descending,
                                |value, cx| {
                                    settings_apply::commit(cx, |s| {
                                        s.default_sort.descending = value
                                    })
                                },
                            ),
                        ),
                        row(
                            "Keep folders on top",
                            None,
                            toggle("folders-first", s.default_sort.dirs_first, |on, cx| {
                                settings_apply::commit(cx, |s| s.default_sort.dirs_first = on)
                            }),
                        ),
                        row(
                            "Show hidden files",
                            Some("Files whose names start with a dot. Also ⇧⌘."),
                            toggle("show-hidden", s.show_hidden, |on, cx| {
                                settings_apply::commit(cx, |s| s.show_hidden = on)
                            }),
                        ),
                    ],
                ),
            ],
        )
    }

    fn files(&self, s: &AppSettings) -> AnyElement {
        page(
            "Files",
            vec![
                section(
                    "Copy and move",
                    vec![row(
                        "When a file with the same name exists",
                        Some("Applies to paste, F5 and F6."),
                        segmented(
                            "conflict",
                            &labels(&OnConflict::ALL, OnConflict::label),
                            s.on_conflict,
                            |value, cx| settings_apply::commit(cx, |s| s.on_conflict = value),
                        ),
                    )],
                ),
                section(
                    "Terminal",
                    vec![
                        row(
                            "Open in Terminal uses",
                            Some("An application name, such as Terminal, iTerm or Ghostty."),
                            field_box(&self.terminal_input, 200.0),
                        ),
                        if self.terminal_error {
                            div()
                                .px_4()
                                .pb_3()
                                .text_xs()
                                .text_color(theme::red())
                                .child("Enter just the application's name: no slashes, no leading dash.")
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        },
                    ],
                ),
                section(
                    "Deleting",
                    vec![note(
                        "Delete always moves items to the Trash. Veloce has no way to delete permanently.",
                    )],
                ),
            ],
        )
    }

    fn connections_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let has_store = self.store().is_some();
        let list: Vec<AnyElement> = if self.connections.is_empty() {
            vec![note(if has_store {
                "No connections yet. Add one to reach a server from the file list."
            } else {
                "Connections cannot be saved: the state database could not be opened."
            })]
        } else {
            self.connections
                .iter()
                .map(|connection| {
                    let kind = Kind::from_key(&connection.kind);
                    let id = connection.id;
                    let entity = entity.clone();
                    row(
                        &connection.name,
                        Some(&format!(
                            "{} · {}",
                            kind.map(Kind::label).unwrap_or("Unknown"),
                            kind.map(|k| connections::summary(k, &connection.config))
                                .unwrap_or_default()
                        )),
                        button(
                            Box::leak(format!("del-{id}").into_boxed_str()),
                            "Remove",
                            move |_, cx| {
                                entity.update(cx, |view, cx| view.delete_connection(id, cx));
                            },
                        ),
                    )
                })
                .collect()
        };

        let mut children = vec![section("Saved connections", list)];
        match &self.form {
            None => {
                let adders: Vec<AnyElement> = Kind::ALL
                    .iter()
                    .map(|kind| {
                        let kind = *kind;
                        let entity = entity.clone();
                        button(
                            Box::leak(format!("add-{}", kind.key()).into_boxed_str()),
                            &format!("Add {}", kind.label()),
                            move |_, cx| {
                                entity.update(cx, |view, cx| view.open_form(kind, cx));
                            },
                        )
                    })
                    .collect();
                children.push(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .flex_wrap()
                        .children(adders)
                        .into_any_element(),
                );
                children.push(
                    div()
                        .text_xs()
                        .text_color(theme::text_faint())
                        .child("Passwords and secret keys are never saved here; they will be kept in the system keychain when the remote backends arrive.")
                        .into_any_element(),
                );
            }
            Some(form) => children.push(self.connection_form(form, &entity)),
        }
        page("Connections", children)
    }

    fn connection_form(&self, form: &ConnectionForm, entity: &Entity<SettingsView>) -> AnyElement {
        let mut rows = vec![row("Name", None, field_box(&form.name, 260.0))];
        for (field, input) in &form.fields {
            rows.push(row(
                &format!("{}{}", field.label, if field.required { " *" } else { "" }),
                None,
                field_box(input, 260.0),
            ));
        }
        if let Some(error) = &form.error {
            rows.push(
                div()
                    .px_4()
                    .py_2()
                    .text_xs()
                    .text_color(theme::red())
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        let save = entity.clone();
        let cancel = entity.clone();
        rows.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .px_4()
                .py_3()
                .child(button("form-cancel", "Cancel", move |_, cx| {
                    cancel.update(cx, |view, cx| {
                        view.form = None;
                        cx.notify();
                    });
                }))
                .child(button("form-save", "Save connection", move |_, cx| {
                    save.update(cx, |view, cx| view.save_form(cx));
                }))
                .into_any_element(),
        );
        section(&format!("New {} connection", form.kind.label()), rows)
    }

    fn storage(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let path = veloce_store::Store::default_path();
        let path_text = path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Unavailable".to_string());
        let reveal = path.and_then(|p| p.parent().map(|dir| dir.to_path_buf()));
        let persist = self.persist.clone();
        let clear_entity = entity.clone();
        page(
            "Storage",
            vec![
                section(
                    "Saved state",
                    vec![
                        row(
                            "Database",
                            Some(&path_text),
                            button("reveal-db", "Show in Finder", move |_, _| {
                                if let Some(dir) = &reveal {
                                    let _ = open::that_detached(dir);
                                }
                            }),
                        ),
                        note(
                            "Holds your settings, folder views, tabs, recent folders, connections and the thumbnail cache.",
                        ),
                    ],
                ),
                section(
                    "Thumbnail cache",
                    vec![row(
                        "Cached thumbnails",
                        Some(&match self.thumbnail_bytes {
                            Some(bytes) => format!("Using {}", human_bytes(bytes)),
                            None => "Size unknown".to_string(),
                        }),
                        button("clear-cache", "Clear cache", move |_, cx| {
                            persist.clear_thumbnails(cx);
                            // The delete runs in the background; show the empty state now.
                            clear_entity.update(cx, |view, cx| {
                                view.thumbnail_bytes = Some(0);
                                cx.notify();
                            });
                        }),
                    )],
                ),
                section(
                    "Reset",
                    vec![row(
                        "Restore default settings",
                        Some("Connections, folder views and tabs are kept."),
                        button("reset-settings", "Reset settings", |_, cx| {
                            settings_apply::commit(cx, |s| *s = AppSettings::default());
                        }),
                    )],
                ),
            ],
        )
    }
}

fn appearance(s: &AppSettings) -> AnyElement {
    page(
        "Appearance",
        vec![section(
            "Look",
            vec![
                row(
                    "Theme",
                    Some("System follows the macOS light/dark setting."),
                    segmented(
                        "theme",
                        &labels(&ThemePref::ALL, ThemePref::label),
                        s.theme,
                        |value, cx| settings_apply::commit(cx, |s| s.theme = value),
                    ),
                ),
                row(
                    "Row density",
                    Some("Height of rows in the list view."),
                    segmented(
                        "density",
                        &labels(&Density::ALL, Density::label),
                        s.density,
                        |value, cx| settings_apply::commit(cx, |s| s.density = value),
                    ),
                ),
            ],
        )],
    )
}

fn preview(s: &AppSettings) -> AnyElement {
    page(
        "Preview",
        vec![section(
            "Thumbnails",
            vec![
                row(
                    "Show thumbnails in the icon view",
                    Some(
                        "Pictures, PDFs, videos and documents show a preview instead of a type icon.",
                    ),
                    toggle("thumbnails", s.thumbnails, |on, cx| {
                        settings_apply::commit(cx, |s| s.thumbnails = on)
                    }),
                ),
                row(
                    "Cache size limit",
                    Some("Least recently used thumbnails are removed past this."),
                    segmented(
                        "cache",
                        &CACHE_CHOICES_MB
                            .iter()
                            .map(|mb| {
                                (
                                    *mb,
                                    if *mb >= 1024 {
                                        format!("{} GB", mb / 1024)
                                    } else {
                                        format!("{mb} MB")
                                    },
                                )
                            })
                            .collect::<Vec<_>>(),
                        s.thumbnail_cache_mb,
                        |value, cx| settings_apply::commit(cx, |s| s.thumbnail_cache_mb = value),
                    ),
                ),
            ],
        )],
    )
}

fn search(s: &AppSettings) -> AnyElement {
    page(
        "Search",
        vec![section(
            "Searching folders (⌘P)",
            vec![
                row(
                    "Include hidden files",
                    None,
                    toggle("search-hidden", s.search_hidden, |on, cx| {
                        settings_apply::commit(cx, |s| s.search_hidden = on)
                    }),
                ),
                row(
                    "Skip files ignored by .gitignore",
                    Some("Keeps build output and dependencies out of the results."),
                    toggle("search-git", s.search_respects_gitignore, |on, cx| {
                        settings_apply::commit(cx, |s| s.search_respects_gitignore = on)
                    }),
                ),
            ],
        )],
    )
}

/// Keyboard shortcuts, grouped. Read-only for now.
const SHORTCUT_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Navigation",
        &[
            ("↑ ↓ ← →  /  j k", "Move the selection"),
            ("Return  /  ⌘O", "Open"),
            ("h  /  ⌘↑", "Go to the parent folder"),
            ("⌘[  /  ⌘]", "Back / forward"),
            ("⌘P", "Search, or type a path to go there"),
            ("/", "Filter this folder"),
        ],
    ),
    (
        "View",
        &[
            ("⌘1  ⌘2  ⌘3", "Icons / list / columns"),
            ("Space", "Show or hide the preview panel"),
            ("⇧⌘.", "Show hidden files"),
            ("⌘\\", "Split view"),
            ("⌃1  ⌃2", "Focus the left / right pane"),
            ("⌘T  ⌘W", "New tab / close tab"),
        ],
    ),
    (
        "Files",
        &[
            ("⌘X  ⌘C  ⌘V", "Cut / copy / paste"),
            ("F2", "Rename"),
            ("⌘⌫", "Move to Trash"),
            ("⇧⌘N", "New folder"),
            ("⇧⌘C", "Copy path"),
            ("F5  F6", "Copy / move to the other pane"),
        ],
    ),
    (
        "App",
        &[("⌘,", "Settings"), ("⌘J", "Task panel"), ("⌘Q", "Quit")],
    ),
];

fn shortcuts() -> AnyElement {
    let mut children: Vec<AnyElement> = SHORTCUT_GROUPS
        .iter()
        .map(|(title, items)| {
            section(
                title,
                items
                    .iter()
                    .map(|(keys, what)| {
                        row(
                            what,
                            None,
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .bg(theme::bg_hover())
                                .font_family(theme::MONO_FAMILY)
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(keys.to_string())
                                .into_any_element(),
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    children.push(
        div()
            .text_xs()
            .text_color(theme::text_faint())
            .child("Custom shortcuts are not editable yet.")
            .into_any_element(),
    );
    page("Shortcuts", children)
}

fn about() -> AnyElement {
    page(
        "About",
        vec![section(
            "Veloce",
            vec![
                row(
                    "Version",
                    None,
                    div()
                        .text_sm()
                        .text_color(theme::text_muted())
                        .child(env!("CARGO_PKG_VERSION"))
                        .into_any_element(),
                ),
                note("A keyboard-first file manager built on Rust and GPUI."),
                note(
                    "Icons: Lucide (ISC) and Material Icon Theme (MIT). Highlighting: tree-sitter and syntect. Storage: SQLite.",
                ),
            ],
        )],
    )
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let sidebar = div()
            .flex()
            .flex_col()
            .w(px(188.))
            .h_full()
            .flex_shrink_0()
            .bg(theme::bg())
            .border_r_1()
            .border_color(theme::border_soft())
            // Room for the traffic lights.
            .pt(px(46.))
            .px_2()
            .gap_0p5()
            .children(Module::ALL.iter().map(|module| {
                let module = *module;
                let entity = entity.clone();
                let active = self.module == module;
                div()
                    .id(gpui::ElementId::Name(
                        format!("module-{}", module.label()).into(),
                    ))
                    .px_3()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .when(active, |this| {
                        this.bg(theme::bg_selected()).text_color(gpui::white())
                    })
                    .when(!active, |this| {
                        this.text_color(theme::text_muted())
                            .hover(|this| this.bg(theme::bg_hover()).text_color(theme::text()))
                    })
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |view, cx| {
                            view.module = module;
                            view.form = None;
                            if module == Module::Storage {
                                view.reload_cache_size(cx);
                            }
                            cx.notify();
                        });
                    })
                    .child(module.label())
            }));

        let body = self.render_module(cx);
        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(theme::bg_pane())
            .text_color(theme::text())
            .child(sidebar)
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px_6()
                    .pt(px(46.))
                    .pb_6()
                    // Clicking blank space must not leave a text field focused
                    // and swallowing the next shortcut.
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.blur())
                    .child(div().max_w(px(560.)).child(body)),
            )
    }
}
