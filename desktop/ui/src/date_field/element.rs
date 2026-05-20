use gpui::{
    relative, Bounds, DispatchPhase, Element, ElementId, ElementInputHandler, Entity,
    GlobalElementId, IntoElement, LayoutId, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels,
    Style, Window,
};

use super::paint::paint_date_field;
use super::DateComponentField;

pub struct DateFieldElement {
    pub field: Entity<DateComponentField>,
}

impl IntoElement for DateFieldElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DateFieldElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        cx: &mut gpui::App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut gpui::App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        if self.field.read(cx).is_selecting {
            let field = self.field.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && event.dragging() {
                    field.update(cx, |field, cx| {
                        field.drag_to_position(event.position, window, cx);
                    });
                }
            });

            let field = self.field.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    field.update(cx, |field, cx| {
                        field.is_selecting = false;
                        cx.notify();
                    });
                }
            });
        }

        let focus_handle = self.field.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        self.field.update(cx, |field, _cx| {
            field.last_bounds = Some(bounds);
        });
        paint_date_field(&self.field, bounds, window, cx);
    }
}
