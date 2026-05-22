use super::text::clamp_range_to_char_boundaries;
use super::{inline_selection_rgba, SingleLineEditor, CURSOR_WIDTH};
use gpui::{
    fill, point, px, relative, size, App, Bounds, ContentMask, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, GlobalElementId, IntoElement, LayoutId, MouseButton,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, ShapedLine, SharedString, Style, TextRun,
    Window,
};

pub(super) struct SingleLineTextElement {
    pub(super) editor: Entity<SingleLineEditor>,
}

pub(super) struct SingleLinePrepaintState {
    line: Option<ShapedLine>,
    selection: Option<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for SingleLineTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SingleLineTextElement {
    type RequestLayoutState = ();
    type PrepaintState = SingleLinePrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let editor = self.editor.read(cx);
        let selected_range = clamp_range_to_char_boundaries(&editor.value, {
            let (start, end) = editor.selection.ordered();
            start..end
        });
        let marked_range = editor
            .marked_range
            .as_ref()
            .map(|range| clamp_range_to_char_boundaries(&editor.value, range.clone()));
        let style = window.text_style();
        let mut color = style.color;
        let display_text = if editor.value.is_empty() {
            color.a *= 0.55;
            editor.placeholder.clone()
        } else {
            SharedString::from(editor.value.clone())
        };

        let base_run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = marked_range
            .as_ref()
            .map(|range| {
                [
                    TextRun {
                        len: range.start,
                        ..base_run.clone()
                    },
                    TextRun {
                        len: range.end - range.start,
                        underline: Some(gpui::UnderlineStyle {
                            color: Some(base_run.color),
                            thickness: px(1.0),
                            wavy: false,
                        }),
                        ..base_run.clone()
                    },
                    TextRun {
                        len: display_text.len().saturating_sub(range.end),
                        ..base_run.clone()
                    },
                ]
                .into_iter()
                .filter(|run| run.len > 0)
                .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![base_run]);

        let line = window.text_system().shape_line(
            display_text,
            style.font_size.to_pixels(window.rem_size()),
            &runs,
            None,
        );

        let selection = if selected_range.is_empty() {
            None
        } else {
            let start_x = line.x_for_index(selected_range.start);
            let end_x = line.x_for_index(selected_range.end).max(start_x + px(1.0));
            Some(fill(
                Bounds::new(
                    point(bounds.left() + start_x, bounds.top()),
                    size(end_x - start_x, bounds.size.height),
                ),
                inline_selection_rgba(cx),
            ))
        };

        let cursor = if selected_range.is_empty() {
            let cursor_x = line.x_for_index(editor.selection.head);
            Some(fill(
                Bounds::new(
                    point(bounds.left() + cursor_x, bounds.top()),
                    size(CURSOR_WIDTH, bounds.size.height),
                ),
                style.color,
            ))
        } else {
            None
        };

        SingleLinePrepaintState {
            line: Some(line),
            selection,
            cursor,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.editor.read(cx).is_selecting {
            let editor = self.editor.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && event.dragging() {
                    editor.update(cx, |editor, cx| {
                        editor.drag_to_position(event.position, window, cx);
                    });
                }
            });

            let editor = self.editor.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    editor.update(cx, |editor, cx| {
                        editor.is_selecting = false;
                        cx.notify();
                    });
                }
            });
        }

        let (focus_handle, cursor_visible) = {
            let editor = self.editor.read(cx);
            (editor.focus_handle.clone(), editor.cursor_visible)
        };
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection);
            }

            if let Some(line) = prepaint.line.take() {
                let _ = line.paint(bounds.origin, window.line_height(), window, cx);
                self.editor.update(cx, |editor, _cx| {
                    editor.last_layout = Some(line);
                    editor.last_bounds = Some(bounds);
                });
            }

            if focus_handle.is_focused(window) && cursor_visible {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
        });
    }
}
