use super::*;

pub(super) fn set_input_value_if_changed(
    input: &mut DateComponentField,
    value: String,
    _window: &mut Window,
    cx: &mut Context<DateComponentField>,
) {
    if input.value() != value {
        input.set_value(value, cx);
    }
}

pub(crate) fn popover_field(
    id: &'static str,
    input: &gpui::Entity<DateComponentField>,
    width: f32,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let focus_handle = input.read(cx).focus_handle.clone();
    let input_for_mouse_down = input.clone();
    let input_for_key_down = input.clone();

    div()
        .id(id)
        .w(px(width))
        .h(px(DATE_FIELD_HEIGHT))
        .flex_shrink_0()
        .track_focus(&focus_handle)
        .overflow_hidden()
        .cursor(CursorStyle::IBeam)
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            input_for_mouse_down.update(cx, |input, cx| {
                input.on_mouse_down(event, window, cx);
            });
            cx.stop_propagation();
        })
        .on_key_down(move |event, window, cx| {
            input_for_key_down.update(cx, |input, cx| {
                input.on_key_down(event, window, cx);
            });
        })
        .on_click(|_: &ClickEvent, _w, cx| cx.stop_propagation())
        .font_family(FONT_MONO)
        .text_size(px(DATE_FIELD_TEXT_SIZE))
        .text_color(token_hsla(t.text_highlight))
        .font_weight(gpui::FontWeight::NORMAL)
        .child(DateFieldElement {
            field: input.clone(),
        })
        .into_any_element()
}

pub(crate) fn date_group(children: Vec<gpui::AnyElement>, t: Theme) -> gpui::AnyElement {
    div()
        .h(px(26.0))
        .flex()
        .flex_shrink_0()
        .items_center()
        .px(px(DATE_GROUP_PAD_X))
        .rounded(px(3.0))
        .bg(token_rgba(t.bg_hint))
        .border_1()
        .border_color(token_rgba(t.caret_color))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(children)
        .into_any_element()
}

pub(super) fn date_time_with_meridiem_group(
    hour_input: &gpui::Entity<DateComponentField>,
    minute_input: &gpui::Entity<DateComponentField>,
    hour_is_pm: Option<bool>,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let mut children = vec![
        popover_field("hour-field", hour_input, 20.0, t, cx),
        time_separator(t),
        popover_field("minute-field", minute_input, 20.0, t, cx),
    ];

    if hour_is_pm.is_some() {
        children.push(div().w(px(1.0)).into_any_element());
        let hour_is_pm = hour_is_pm.unwrap_or(false);
        children.push(meridiem_button("date-am", "AM", !hour_is_pm, false, t, cx));
        children.push(meridiem_button("date-pm", "PM", hour_is_pm, true, t, cx));
    }

    div()
        .h(px(26.0))
        .flex()
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .items_center()
        .px(px(DATE_GROUP_PAD_X))
        .rounded(px(3.0))
        .bg(token_rgba(t.bg_hint))
        .border_1()
        .border_color(token_rgba(t.caret_color))
        .children(children)
        .into_any_element()
}

pub(super) fn date_time_group(
    hour_input: &gpui::Entity<DateComponentField>,
    minute_input: &gpui::Entity<DateComponentField>,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    date_time_with_meridiem_group(hour_input, minute_input, None, t, cx)
}

pub(super) fn meridiem_button(
    id: &'static str,
    label: &'static str,
    active: bool,
    is_pm: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id(id)
        .w(px(27.0))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .font_family(FONT_MONO)
        .text_size(px(FONT_SIZE_CAPTION2))
        .font_weight(if active {
            gpui::FontWeight::SEMIBOLD
        } else {
            gpui::FontWeight::NORMAL
        })
        .text_color(token_hsla(if active {
            t.text_highlight
        } else {
            t.text_muted
        }))
        .bg(token_rgba(if active { t.row_selected } else { 0x00000000 }))
        .cursor_pointer()
        .hover({
            let hover = t.button_hover;
            move |s| s.bg(token_rgba(hover))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            if let Some(popup) = this.date_popover.as_mut() {
                popup.hour_is_pm = is_pm;
            }
            this.apply_date_popover_inputs(cx);
            cx.stop_propagation();
            cx.notify();
        }))
        .child(label)
        .into_any_element()
}

pub(super) fn component_separator(label: &'static str, t: Theme) -> gpui::AnyElement {
    div()
        .w(px(5.0))
        .flex_shrink_0()
        .text_center()
        .text_size(px(FONT_SIZE_CAPTION2))
        .text_color(token_hsla(t.text_dim))
        .child(label)
        .into_any_element()
}

pub(super) fn time_separator(t: Theme) -> gpui::AnyElement {
    div()
        .w(px(4.0))
        .flex_shrink_0()
        .text_center()
        .text_size(px(FONT_SIZE_CAPTION2))
        .text_color(token_hsla(t.text_dim))
        .child(":")
        .into_any_element()
}

pub(crate) fn popover_hour_value(time_format: TimeFormat, hour: u32) -> String {
    match time_format {
        TimeFormat::TwelveHour => format!("{:02}", hour_12(hour)),
        TimeFormat::TwentyFourHour => format!("{hour:02}"),
    }
}
