use super::*;

pub(super) fn trigger_time(
    kind: ItemKind,
    start: Option<chrono::DateTime<Utc>>,
    end: Option<chrono::DateTime<Utc>>,
) -> Option<chrono::DateTime<Utc>> {
    match kind {
        ItemKind::Assignment => end,
        _ => start,
    }
}

pub(super) fn row_status_color(
    kind: ItemKind,
    start: Option<chrono::DateTime<Utc>>,
    end: Option<chrono::DateTime<Utc>>,
    default: Hsla,
) -> Hsla {
    match kind {
        ItemKind::Event => start
            .map(|start| {
                event_status_color(
                    start.with_timezone(&Local),
                    end.map(|end| end.with_timezone(&Local)),
                    default,
                )
            })
            .unwrap_or(default),
        ItemKind::Assignment => end
            .map(|end| date_status_color(end.with_timezone(&Local), default))
            .unwrap_or(default),
        ItemKind::Reminder => start
            .map(|start| date_status_color(start.with_timezone(&Local), default))
            .unwrap_or(default),
        ItemKind::Procedure => default,
    }
}

pub(super) fn when_label(
    time_format: knotq_storage_json::TimeFormat,
    kind: ItemKind,
    start: Option<chrono::DateTime<Utc>>,
    end: Option<chrono::DateTime<Utc>>,
) -> String {
    match kind {
        ItemKind::Assignment => end
            .map(|dt| {
                knotq_l10n::t_with(
                    "upcoming.when.due",
                    &[("when", &date_label(time_format, dt.with_timezone(&Local)))],
                )
            })
            .unwrap_or_default(),
        ItemKind::Reminder => start
            .map(|dt| {
                knotq_l10n::t_with(
                    "upcoming.when.at",
                    &[("when", &date_label(time_format, dt.with_timezone(&Local)))],
                )
            })
            .unwrap_or_default(),
        ItemKind::Event => match (start, end) {
            (Some(start), Some(end)) => {
                let start = start.with_timezone(&Local);
                let end = end.with_timezone(&Local);
                if start.date_naive() == end.date_naive() {
                    // The " → " separator is re-parsed verbatim by
                    // `when_label_element` below, so it is kept as a fixed
                    // internal delimiter rather than a translatable string.
                    format!(
                        "{} → {}",
                        date_label(time_format, start),
                        format_time(time_format, end)
                    )
                } else {
                    format!(
                        "{} → {}",
                        date_label(time_format, start),
                        date_label(time_format, end)
                    )
                }
            }
            (Some(start), None) => knotq_l10n::t_with(
                "upcoming.when.at",
                &[(
                    "when",
                    &date_label(time_format, start.with_timezone(&Local)),
                )],
            ),
            (None, Some(end)) => knotq_l10n::t_with(
                "upcoming.when.due",
                &[("when", &date_label(time_format, end.with_timezone(&Local)))],
            ),
            _ => String::new(),
        },
        ItemKind::Procedure => String::new(),
    }
}

fn date_label(time_format: knotq_storage_json::TimeFormat, dt: chrono::DateTime<Local>) -> String {
    let today = Local::now().date_naive();
    let dn = dt.date_naive();
    let day_part = if dn == today {
        String::new()
    } else if dn == today + chrono::Duration::days(1) {
        knotq_l10n::t("upcoming.date.tomorrow").to_string()
    } else if dn < today + chrono::Duration::days(7) && dn > today {
        knotq_date_util::weekday_short_name(dt.weekday()).to_string()
    } else {
        format!(
            "{} {:02}",
            knotq_date_util::month_short_name(dt.month()),
            dt.day()
        )
    };
    let time = format_time(time_format, dt);
    if day_part.is_empty() {
        time
    } else {
        format!("{day_part} {time}")
    }
}

pub(super) fn when_label_element(label: &str, color: Hsla) -> gpui::AnyElement {
    let mut parts = label.splitn(2, " \u{2192} ");
    let first = parts.next().unwrap_or_default().to_string();
    let second = parts.next().map(|end| format!("\u{2192} {end}"));

    let line = |text: String| {
        div()
            .w_full()
            .min_w_0()
            .whitespace_nowrap()
            .overflow_hidden()
            .text_right()
            .child(text)
            .into_any_element()
    };

    div()
        .flex()
        .flex_col()
        .items_end()
        .justify_end()
        .w(px(132.0))
        .min_w_0()
        .text_size(px(10.0))
        .line_height(px(11.0))
        .font_family(FONT_MONO)
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(color)
        .flex_shrink_0()
        .child(line(first))
        .children(second.map(line))
        .into_any_element()
}
