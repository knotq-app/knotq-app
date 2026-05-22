#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assets;
mod notifications;
mod theme_gpui;
mod views;

use std::borrow::Cow;

use gpui::prelude::*;
use gpui::{
    actions, div, px, App, Application, Context, InteractiveElement, IntoElement, KeyBinding, Menu,
    MenuItem, OsAction, Render, TitlebarOptions, Window, WindowBounds, WindowDecorations,
    WindowOptions,
};
use gpui_component::{
    input::{IndentInline, MoveDown, MoveUp, OutdentInline},
    Root,
};

use crate::app::{
    initial_window_bounds, load_or_default_settings, KnotQApp, View, DAILY_QUEUE_TITLE,
    MIN_WINDOW_WIDTH,
};
use crate::assets::AppAssets;
use crate::theme_gpui::{token_hsla, token_rgba, FONT_UI};

actions!(
    knotq,
    [
        ToggleSearch,
        CloseSearch,
        NavWeekPrev,
        NavWeekNext,
        QuitApp,
        OpenSettingsView,
        OpenCalendarView,
        OpenDailyQueueView,
        NewItem,
        NewFolder,
        AppUndo,
        AppRedo,
        RenameSelectedNode,
        SubmitEventPopup,
    ]
);

const NAVIGATOR_W: f32 = 166.0;
const LEFT_PANEL_GAP: f32 = 8.0;
const UPCOMING_W: f32 = 258.0;
impl Render for KnotQApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._appearance_subscription.is_none() {
            self._appearance_subscription = Some(cx.observe_window_appearance(
                window,
                |this: &mut KnotQApp, window, cx| {
                    this.sync_system_theme(window);
                    cx.notify();
                },
            ));
        }
        if self._window_bounds_subscription.is_none() {
            self._window_bounds_subscription = Some(cx.observe_window_bounds(
                window,
                |this: &mut KnotQApp, window, _cx| {
                    let bounds = window.bounds();
                    this.remember_window_bounds(
                        f32::from(bounds.origin.x),
                        f32::from(bounds.origin.y),
                        f32::from(bounds.size.width),
                        f32::from(bounds.size.height),
                    );
                },
            ));
        }
        self.sync_system_theme(window);
        let view = self.selection.view;
        let t = self.theme();
        let current_scheme_title = self
            .current_scheme()
            .map(|s| (s.id, self.scheme_display_name(s), s.color_index));
        let title = match view {
            View::Union => "Calendar".to_string(),
            View::DailyQueue => DAILY_QUEUE_TITLE.to_string(),
            View::Scheme => current_scheme_title
                .as_ref()
                .map(|(_, name, _)| name.clone())
                .unwrap_or_else(|| "Workspace".to_string()),
            View::Settings => "Settings".to_string(),
        };
        let title_bar = self.render_title_bar(window, view, title, current_scheme_title, t, cx);

        let sidebar = self.render_sidebar(window, cx);
        let upcoming = self.render_upcoming(cx);
        let panel_bg = token_hsla(t.bg_app);
        let left_panel = div()
            .relative()
            .w(px(NAVIGATOR_W + LEFT_PANEL_GAP + UPCOMING_W))
            .h_full()
            .flex_shrink_0()
            .bg(panel_bg)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(NAVIGATOR_W + LEFT_PANEL_GAP))
                    .right_0()
                    .overflow_hidden()
                    .child(upcoming),
            )
            .child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .bottom(px(8.0))
                    .left(px(8.0))
                    .child(sidebar),
            );

        let main_available_w = (f32::from(window.viewport_size().width)
            - (NAVIGATOR_W + LEFT_PANEL_GAP + UPCOMING_W + 1.0))
            .max(0.0);

        let main: gpui::AnyElement = match view {
            View::Union => self
                .render_calendar(main_available_w, cx)
                .into_any_element(),
            View::DailyQueue => self.render_daily_queue(main_available_w, window, cx),
            View::Scheme => self.render_scheme_view(window, cx),
            View::Settings => self.render_settings(cx),
        };

        let mut root =
            div()
                .key_context("KnotQApp")
                .track_focus(&self.editor_focus_handle)
                .relative()
                .flex()
                .flex_col()
                .w_full()
                .h_full()
                .bg(token_hsla(t.bg_app))
                .text_color(token_hsla(t.text_primary))
                .font_family(FONT_UI)
                .on_action(cx.listener(|this, _: &OpenSettingsView, window, cx| {
                    this.open_settings();
                    this.focus_app_root(window);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &OpenCalendarView, window, cx| {
                    this.open_union();
                    this.focus_app_root(window);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &OpenDailyQueueView, window, cx| {
                    this.open_daily_queue(cx);
                    this.focus_current_editor(window, cx);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &NewItem, window, cx| {
                    if this.search_open {
                        this.close_search(window, cx);
                    }
                    this.sidebar_context_menu = None;
                    let parent = this.new_item_parent_folder();
                    this.open_new_node_prompt(parent, app::NewNodeKind::Scheme, window, cx);
                }))
                .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
                    if this.search_open {
                        this.close_search(window, cx);
                    }
                    this.sidebar_context_menu = None;
                    let root = this.workspace.root;
                    this.open_new_node_prompt(root, app::NewNodeKind::Folder, window, cx);
                }))
                .on_action(cx.listener(|this, _: &NavWeekPrev, _window, cx| {
                    this.shift_calendar_period(-1);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &NavWeekNext, _window, cx| {
                    this.shift_calendar_period(1);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &AppUndo, _window, cx| {
                    this.undo(cx);
                }))
                .on_action(cx.listener(|this, _: &AppRedo, _window, cx| {
                    this.redo(cx);
                }))
                .on_action(cx.listener(|this, _: &RenameSelectedNode, window, cx| {
                    this.start_renaming_current_scheme(window, cx);
                }))
                .on_action(cx.listener(|this, _: &ToggleSearch, window, cx| {
                    this.open_search(window, cx);
                }))
                .on_action(cx.listener(|this, _: &CloseSearch, window, cx| {
                    this.cancel_current_action(window, cx);
                }))
                .on_action(cx.listener(|this, _: &SubmitEventPopup, window, cx| {
                    if this.rename_node.is_some() {
                        this.finish_renaming_node(true, window, cx);
                        return;
                    }
                    if this.date_popover.is_some() {
                        cx.propagate();
                        return;
                    }
                    if this.event_popup.is_some() {
                        this.close_event_popup(cx);
                        this.focus_app_root(window);
                        cx.notify();
                        return;
                    }
                    cx.propagate();
                }))
                .on_action(cx.listener(|this, _: &MoveDown, _window, cx| {
                    this.select_next_search_result(cx);
                }))
                .on_action(cx.listener(|this, _: &IndentInline, _window, cx| {
                    this.select_next_search_result(cx);
                }))
                .on_action(cx.listener(|this, _: &MoveUp, _window, cx| {
                    this.select_previous_search_result(cx);
                }))
                .on_action(cx.listener(|this, _: &OutdentInline, _window, cx| {
                    this.select_previous_search_result(cx);
                }))
                .child(title_bar)
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(left_panel)
                        .child(div().w(px(1.0)).h_full().flex_shrink_0().bg(token_rgba(
                            if t.is_dark {
                                0xffffff08
                            } else {
                                t.divider_tiny
                            },
                        )))
                        .child(main),
                );

        if let Some(popover) = self.render_date_popover(window, cx) {
            root = root.child(popover);
        }
        if let Some(popover) = self.render_repeat_popover(window, cx) {
            root = root.child(popover);
        }
        if self.search_open {
            root = root.child(self.render_search(window, cx));
        }
        if let Some(menu) = self.render_sidebar_context_menu(window, cx) {
            root = root.child(menu);
        }
        if let Some(menu) = self.render_editor_context_menu(window, cx) {
            root = root.child(menu);
        }
        if let Some(confirm) = self.render_delete_confirmation(cx) {
            root = root.child(confirm);
        }
        if let Some(popup) = self.render_event_popup(window, cx) {
            root = root.child(popup);
        }
        if let Some(onboarding) = self.render_onboarding(window, cx) {
            root = root.child(onboarding);
        }
        root
    }
}

fn titlebar_options() -> TitlebarOptions {
    if cfg!(target_os = "macos") {
        TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(12.0), px(12.0))),
        }
    } else {
        TitlebarOptions {
            title: Some("KnotQ".into()),
            appears_transparent: false,
            traffic_light_position: None,
        }
    }
}

fn window_decorations() -> Option<WindowDecorations> {
    if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        Some(WindowDecorations::Server)
    } else {
        None
    }
}

fn main() {
    Application::new()
        .with_assets(AppAssets::new())
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            knotq_editor::scheme_editor::init(cx);

            let fonts = vec![
                Cow::Borrowed(include_bytes!("../font/SF-Pro-Text-Regular.otf") as &[u8]),
                Cow::Borrowed(include_bytes!("../font/SF-Pro-Text-Medium.otf") as &[u8]),
                Cow::Borrowed(include_bytes!("../font/SF-Pro-Text-Semibold.otf") as &[u8]),
                Cow::Borrowed(include_bytes!("../font/SF-Pro-Text-Bold.otf") as &[u8]),
                Cow::Borrowed(include_bytes!("../font/SF-Pro-Text-RegularItalic.otf") as &[u8]),
                Cow::Borrowed(include_bytes!("../font/SF-Pro-Display-Regular.otf") as &[u8]),
                Cow::Borrowed(include_bytes!("../font/SF-Pro-Display-Bold.otf") as &[u8]),
                Cow::Borrowed(include_bytes!("../font/SF-Mono-Regular.otf") as &[u8]),
                Cow::Borrowed(include_bytes!("../font/SF-Mono-Semibold.otf") as &[u8]),
                Cow::Borrowed(include_bytes!("../font/SF-Mono-Bold.otf") as &[u8]),
            ];
            let _ = cx.text_system().add_fonts(fonts);

            cx.bind_keys([
                KeyBinding::new("cmd-q", QuitApp, None),
                KeyBinding::new("cmd-,", OpenSettingsView, None),
                KeyBinding::new("cmd-z", AppUndo, Some("KnotQApp")),
                KeyBinding::new("cmd-shift-z", AppRedo, Some("KnotQApp")),
                KeyBinding::new("cmd-z", AppUndo, Some("SchemeEditor")),
                KeyBinding::new("cmd-shift-z", AppRedo, Some("SchemeEditor")),
                KeyBinding::new("cmd-n", NewItem, None),
                KeyBinding::new("cmd-shift-n", NewFolder, None),
                KeyBinding::new("cmd-f", ToggleSearch, None),
                KeyBinding::new("secondary-f", ToggleSearch, None),
                KeyBinding::new("f2", RenameSelectedNode, Some("KnotQApp")),
                KeyBinding::new("escape", CloseSearch, None),
                KeyBinding::new("enter", SubmitEventPopup, Some("KnotQApp")),
                KeyBinding::new("cmd-[", NavWeekPrev, Some("KnotQApp")),
                KeyBinding::new("cmd-]", NavWeekNext, Some("KnotQApp")),
                KeyBinding::new("cmd-u", OpenCalendarView, None),
                KeyBinding::new("cmd-d", OpenDailyQueueView, None),
            ]);

            cx.on_action(|_: &QuitApp, cx| cx.quit());
            cx.activate(true);
            cx.set_menus(vec![
                Menu {
                    name: "KnotQ".into(),
                    items: vec![
                        MenuItem::action("Settings", OpenSettingsView),
                        MenuItem::separator(),
                        MenuItem::action("Quit KnotQ", QuitApp),
                    ],
                },
                Menu {
                    name: "File".into(),
                    items: vec![
                        MenuItem::action("New Item", NewItem),
                        MenuItem::action("New Folder", NewFolder),
                    ],
                },
                Menu {
                    name: "Edit".into(),
                    items: vec![
                        MenuItem::os_action("Undo", AppUndo, OsAction::Undo),
                        MenuItem::os_action("Redo", AppRedo, OsAction::Redo),
                    ],
                },
                Menu {
                    name: "View".into(),
                    items: vec![
                        MenuItem::action("Calendar", OpenCalendarView),
                        MenuItem::action("Daily", OpenDailyQueueView),
                        MenuItem::action("Settings", OpenSettingsView),
                        MenuItem::separator(),
                        MenuItem::action("Previous Week", NavWeekPrev),
                        MenuItem::action("Next Week", NavWeekNext),
                    ],
                },
            ]);

            let settings = load_or_default_settings();
            let initial_bounds = initial_window_bounds(&settings, cx);
            let opts = WindowOptions {
                titlebar: Some(titlebar_options()),
                window_bounds: Some(WindowBounds::Windowed(initial_bounds)),
                window_min_size: Some(gpui::size(px(MIN_WINDOW_WIDTH), px(1.0))),
                window_decorations: window_decorations(),
                ..Default::default()
            };

            crate::notifications::configure_notification_handling();
            cx.open_window(opts, |window, cx| {
                let app = cx.new(KnotQApp::new);
                let weak_app = app.downgrade();
                window.on_window_should_close(cx, move |_window, cx| {
                    let _ = weak_app.update(cx, |app, _cx| {
                        app.flush_for_shutdown("window close");
                    });
                    true
                });
                app.update(cx, |app, _cx| app.focus_app_root(window));
                cx.new(|cx| Root::new(app, window, cx))
            })
            .unwrap();

            // Request notification authorization after the window is open and
            // the app is active — macOS requires this to show the permission
            // dialog.
            crate::notifications::request_authorization_nonblocking();
        });
}
