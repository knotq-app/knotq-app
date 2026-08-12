use gpui::prelude::*;
use gpui::{deferred, div, px, ClickEvent, Context, IntoElement, MouseButton, Window};
use knotq_commands::Command;
use knotq_model::SchemeId;
use knotq_ui::{clamped_popover_left, popover_top_biased_below};

use crate::app::{KnotQApp, SchemeColorPicker, View};
use crate::theme_gpui::{palette_hsla, scheme_color, token_hsla, token_rgba, SCHEME_COLOR_ORDER};

const PICKER_PRIORITY: usize = 20_100;
const PICKER_WIDTH: f32 = 228.0;
const PICKER_HEIGHT: f32 = 120.0;
const SWATCH_SIZE: f32 = 26.0;

impl KnotQApp {
    pub(crate) fn toggle_scheme_color_picker(
        &mut self,
        scheme_id: SchemeId,
        anchor: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self
            .scheme_color_picker
            .is_some_and(|picker| picker.scheme_id == scheme_id)
        {
            self.scheme_color_picker = None;
        } else {
            self.scheme_color_picker = Some(SchemeColorPicker { scheme_id, anchor });
        }
        cx.notify();
    }

    pub(crate) fn render_scheme_color_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let picker = self.scheme_color_picker?;
        if self.selection.view != View::Scheme || self.selection.scheme_id != Some(picker.scheme_id)
        {
            self.scheme_color_picker = None;
            return None;
        }
        let active_color = self.workspace.scheme(picker.scheme_id)?.color_index;
        let t = self.theme();
        let viewport_width = px(f32::from(window.viewport_size().width));
        let viewport_height = px(f32::from(window.viewport_size().height));
        let left = clamped_popover_left(
            picker.anchor.x - px(PICKER_WIDTH),
            px(PICKER_WIDTH),
            viewport_width,
        );
        let top = popover_top_biased_below(
            picker.anchor.y + px(16.0),
            px(PICKER_HEIGHT),
            viewport_height,
        );

        let scrim = div()
            .id("scheme-color-picker-scrim")
            .absolute()
            .inset_0()
            .bg(token_rgba(0x00000001))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.scheme_color_picker = None;
                cx.notify();
                cx.stop_propagation();
            }));

        let mut grid = div().flex().flex_wrap().gap(px(8.0));
        for (position, color_index) in picker_color_indices().enumerate() {
            let selected = active_color == color_index;
            let fill = palette_hsla(scheme_color(color_index, t.is_dark), 1.0);
            let scheme_id = picker.scheme_id;
            grid = grid.child(
                div()
                    .id(("scheme-color-picker-swatch", position))
                    .w(px(SWATCH_SIZE))
                    .h(px(SWATCH_SIZE))
                    .rounded(px(5.0))
                    .bg(fill)
                    .border_2()
                    .border_color(token_hsla(if selected {
                        t.text_primary
                    } else {
                        t.bg_modal
                    }))
                    .cursor_pointer()
                    .hover(|swatch| swatch.opacity(0.82))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.apply(
                            Command::SetSchemeColor {
                                id: scheme_id,
                                color_index,
                            },
                            cx,
                        );
                        this.scheme_color_picker = None;
                        cx.notify();
                        cx.stop_propagation();
                    })),
            );
        }

        let card = div()
            .id("scheme-color-picker-card")
            .absolute()
            .left(left)
            .top(top)
            .w(px(PICKER_WIDTH))
            .bg(token_hsla(t.bg_modal))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .rounded(px(8.0))
            .shadow_lg()
            .p(px(12.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_: &ClickEvent, _, cx| cx.stop_propagation())
            .child(grid);

        Some(
            deferred(div().absolute().inset_0().child(scrim).child(card))
                .with_priority(PICKER_PRIORITY)
                .into_any_element(),
        )
    }
}

fn picker_color_indices() -> impl Iterator<Item = u8> {
    SCHEME_COLOR_ORDER.iter().copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_gpui::PALETTE;

    #[test]
    fn picker_exposes_every_persisted_palette_index_once() {
        let indices = picker_color_indices().collect::<Vec<_>>();
        assert_eq!(indices.len(), PALETTE.len());
        for index in 0..PALETTE.len() as u8 {
            assert_eq!(
                indices
                    .iter()
                    .filter(|candidate| **candidate == index)
                    .count(),
                1
            );
        }
    }
}
