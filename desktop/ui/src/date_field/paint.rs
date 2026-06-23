use gpui::{
    fill, point, px, size, App, Bounds, Entity, Pixels, Point, SharedString, TextRun, Window,
};
use gpui_component::ActiveTheme;

use crate::theme::token_rgba;

use super::{DateComponentField, DATE_FIELD_SELECTION_BG};
use crate::CURSOR_WIDTH;

pub(super) fn paint_date_field(
    field: &Entity<DateComponentField>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let (focused, selection, value, placeholder, cursor_blink_state) = {
        let state = field.read(cx);
        (
            state.focus_handle.is_focused(window),
            state.selection,
            state.value.clone(),
            state.placeholder.to_string(),
            state.cursor_blink_state,
        )
    };

    let line_height = window.line_height();
    let origin = date_field_text_origin(bounds, line_height);

    if focused && !selection.is_empty() {
        let (start, end) = selection.ordered();
        if start != end {
            let start_x = date_field_prefix_width(&value, start, window);
            let end_x = date_field_prefix_width(&value, end, window).max(start_x + px(1.0));
            window.paint_quad(fill(
                Bounds::new(
                    point(origin.x + start_x, origin.y),
                    size(end_x - start_x, line_height),
                ),
                token_rgba(DATE_FIELD_SELECTION_BG),
            ));
        }
    }

    let display_text = if value.is_empty() {
        placeholder.clone()
    } else if !focused && placeholder == "hh" && value.len() == 1 {
        format!("0{value}")
    } else {
        value.clone()
    };
    let style = window.text_style();
    let color = if value.is_empty() {
        cx.theme().muted_foreground
    } else {
        style.color
    };
    let run = TextRun {
        len: display_text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::new(display_text),
        style.font_size.to_pixels(window.rem_size()),
        &[run],
        None,
    );
    let _ = line.paint(origin, line_height, window, cx);

    if focused && selection.is_empty() && cursor_blink_state {
        let cursor_x = date_field_prefix_width(&value, selection.head, window);
        let cursor_height = (line_height - px(4.0)).max(px(12.0));
        window.paint_quad(fill(
            Bounds::new(
                point(
                    origin.x + cursor_x,
                    origin.y + ((line_height - cursor_height) / 2.0),
                ),
                size(px(CURSOR_WIDTH), cursor_height),
            ),
            style.color,
        ));
    }
}

pub(super) fn date_field_text_origin(bounds: Bounds<Pixels>, line_height: Pixels) -> Point<Pixels> {
    point(
        bounds.left(),
        bounds.top() + ((bounds.size.height - line_height) / 2.0).max(px(0.0)),
    )
}

pub(super) fn date_field_prefix_width(value: &str, offset: usize, window: &mut Window) -> Pixels {
    let offset = offset.min(value.len());
    measure_date_field_text(&value[..offset], window)
}

pub(super) fn date_field_index_for_x(value: &str, x: Pixels, window: &mut Window) -> usize {
    if x <= px(0.0) {
        return 0;
    }
    for index in 0..value.len() {
        let mid = (date_field_prefix_width(value, index, window)
            + date_field_prefix_width(value, index + 1, window))
            / 2.0;
        if x < mid {
            return index;
        }
    }
    value.len()
}

fn measure_date_field_text(text: &str, window: &mut Window) -> Pixels {
    if text.is_empty() {
        return px(0.0);
    }
    let style = window.text_style();
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color: style.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(
            SharedString::new(text.to_string()),
            style.font_size.to_pixels(window.rem_size()),
            &[run],
            None,
        )
        .width
}
