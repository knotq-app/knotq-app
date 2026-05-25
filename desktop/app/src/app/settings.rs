use gpui::{App, Bounds, Context, Pixels, Window, WindowAppearance};
use knotq_storage_json::{
    save_app_settings, settings_path, AppSettings, CalendarViewMode, CalendarWeekRange,
    NotificationDefaults, SavedWindowPosition, SavedWindowSize, ThemeMode, TimeFormat,
};

use super::{KnotQApp, DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH, MIN_WINDOW_WIDTH};
use crate::theme_gpui::{all_themes, Theme};

impl KnotQApp {
    pub fn theme(&self) -> Theme {
        let themes = all_themes();
        match self.theme_mode {
            ThemeMode::System => {
                if self.system_theme_dark {
                    themes[0]
                } else {
                    themes[1]
                }
            }
            ThemeMode::Dark => themes[0],
            ThemeMode::Light => themes[1],
        }
    }

    pub fn set_theme_mode(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        if self.theme_mode == mode {
            return;
        }
        self.theme_mode = mode;
        self.save_app_settings();
        cx.notify();
    }

    pub fn sync_system_theme(&mut self, window: &Window) {
        self.system_theme_dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
    }

    pub fn set_calendar_view(&mut self, view: CalendarViewMode, cx: &mut Context<Self>) {
        if self.calendar_view == view {
            return;
        }
        self.calendar_view = view;
        if view == CalendarViewMode::Week {
            self.cal_scroll_initialized = false;
        }
        self.dismiss_event_popup_if_hidden_context();
        self.save_app_settings();
        cx.notify();
    }

    pub fn set_calendar_week_range(&mut self, range: CalendarWeekRange, cx: &mut Context<Self>) {
        if self.calendar_week_range == range {
            return;
        }
        self.calendar_week_range = range;
        self.week_offset = 0;
        self.cal_scroll_initialized = false;
        self.save_app_settings();
        cx.notify();
    }

    pub fn set_time_format(&mut self, format: TimeFormat, cx: &mut Context<Self>) {
        if self.time_format == format {
            return;
        }
        self.time_format = format;
        self.save_app_settings();
        cx.notify();
    }

    pub fn set_notification_defaults(
        &mut self,
        defaults: NotificationDefaults,
        cx: &mut Context<Self>,
    ) {
        if self.notification_defaults == defaults {
            return;
        }
        self.notification_defaults = defaults;
        self.save_app_settings();
        self.reschedule_notifications();
        cx.notify();
    }

    pub fn remember_window_bounds(&mut self, x: f32, y: f32, width: f32, height: f32) {
        if !width.is_finite() || !height.is_finite() {
            return;
        }
        let next_size = SavedWindowSize {
            width: width.max(MIN_WINDOW_WIDTH).round(),
            height: height.max(1.0).round(),
        };
        let next_position = if x.is_finite() && y.is_finite() {
            Some(SavedWindowPosition {
                x: x.round(),
                y: y.round(),
            })
        } else {
            None
        };
        let changed = self.window_size != Some(next_size)
            || next_position.is_some_and(|position| self.window_position != Some(position));
        self.window_size = Some(next_size);
        if let Some(position) = next_position {
            self.window_position = Some(position);
        }
        if changed {
            self.save_app_settings();
        }
    }

    pub(crate) fn save_app_settings(&self) {
        let settings = AppSettings {
            replica_id: self.settings.replica_id,
            calendar_view: self.calendar_view,
            calendar_week_range: self.calendar_week_range,
            theme_mode: self.theme_mode,
            time_format: self.time_format,
            notification_defaults: self.notification_defaults,
            scheduled_notification_ids: self.scheduled_notification_ids.clone(),
            window_size: self.window_size,
            window_position: self.window_position,
            google_accounts: self.settings.google_accounts.clone(),
            onboarding_completed: self.settings.onboarding_completed,
        };
        if let Err(err) = save_app_settings(&settings_path(), &settings) {
            eprintln!("settings save failed: {err:#}");
        }
    }
}

pub fn initial_window_size(settings: &AppSettings) -> SavedWindowSize {
    let size = settings.window_size.unwrap_or(SavedWindowSize {
        width: DEFAULT_WINDOW_WIDTH,
        height: DEFAULT_WINDOW_HEIGHT,
    });
    SavedWindowSize {
        width: size.width.max(MIN_WINDOW_WIDTH).round(),
        height: size.height.max(1.0).round(),
    }
}

pub fn initial_window_bounds(settings: &AppSettings, cx: &App) -> Bounds<Pixels> {
    use gpui::{point, px, size};

    let initial_size = initial_window_size(settings);
    let size = size(px(initial_size.width), px(initial_size.height));
    if let Some(position) = settings.window_position {
        let bounds = Bounds::new(point(px(position.x), px(position.y)), size);
        if cx
            .displays()
            .iter()
            .any(|display| bounds.is_contained_within(&display.bounds()))
        {
            return bounds;
        }
    }
    Bounds::centered(None, size, cx)
}
