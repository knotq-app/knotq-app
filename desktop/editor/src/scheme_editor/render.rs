use super::*;

impl gpui::Focusable for SchemeEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl gpui::Render for SchemeEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.read_only {
            return div().w_full().child(SchemeTextElement {
                editor: cx.entity(),
            });
        }
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .w_full()
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|this, _: &Backspace, window, cx| this.backspace(window, cx)))
            .on_action(
                cx.listener(|this, _: &BackspaceWord, window, cx| this.backspace_word(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &BackspaceLine, window, cx| this.backspace_line(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DeleteForward, window, cx| this.delete_forward(window, cx)),
            )
            .on_action(cx.listener(|this, _: &DeleteWord, window, cx| this.delete_word(window, cx)))
            .on_action(cx.listener(|this, _: &DeleteLine, window, cx| this.delete_line(window, cx)))
            .on_action(cx.listener(|this, _: &Enter, window, cx| this.enter(window, cx)))
            .on_action(cx.listener(|this, _: &MoveLeft, _w, cx| this.move_left(false, cx)))
            .on_action(cx.listener(|this, _: &MoveRight, _w, cx| this.move_right(false, cx)))
            .on_action(cx.listener(|this, _: &MoveLeftWord, _w, cx| this.move_word_left(false, cx)))
            .on_action(
                cx.listener(|this, _: &MoveRightWord, _w, cx| this.move_word_right(false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MoveLineStart, _w, cx| this.move_line_start(false, cx)),
            )
            .on_action(cx.listener(|this, _: &MoveLineEnd, _w, cx| this.move_line_end(false, cx)))
            .on_action(cx.listener(|this, _: &MoveDocumentStart, _w, cx| {
                this.move_document_start(false, cx)
            }))
            .on_action(
                cx.listener(|this, _: &MoveDocumentEnd, _w, cx| this.move_document_end(false, cx)),
            )
            .on_action(cx.listener(|this, _: &MoveUp, _w, cx| this.move_vertical(-1, false, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _w, cx| this.move_vertical(1, false, cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _w, cx| this.move_left(true, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _w, cx| this.move_right(true, cx)))
            .on_action(
                cx.listener(|this, _: &SelectLeftWord, _w, cx| this.move_word_left(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectRightWord, _w, cx| this.move_word_right(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectLineStart, _w, cx| this.move_line_start(true, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectLineEnd, _w, cx| this.move_line_end(true, cx)))
            .on_action(cx.listener(|this, _: &SelectDocumentStart, _w, cx| {
                this.move_document_start(true, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectDocumentEnd, _w, cx| this.move_document_end(true, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectUp, _w, cx| this.move_vertical(-1, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _w, cx| this.move_vertical(1, true, cx)))
            .on_action(cx.listener(|this, _: &IndentLine, _w, cx| this.indent_current_line(1, cx)))
            .on_action(
                cx.listener(|this, _: &UnindentLine, _w, cx| this.indent_current_line(-1, cx)),
            )
            .on_action(cx.listener(|this, _: &InsertImage, window, cx| {
                this.insert_image_from_picker(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &InsertTable, window, cx| this.insert_table(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectAll, _w, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &Copy, _w, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Cut, window, cx| this.cut(Some(window), cx)))
            .on_action(cx.listener(|this, _: &Paste, window, cx| this.paste(Some(window), cx)))
            .on_action(
                cx.listener(|this, _: &PastePlain, window, cx| this.paste_plain(Some(window), cx)),
            )
            .on_action(cx.listener(|this, _: &SetMarkerBlank, _w, cx| {
                this.set_marker_for_selection(ItemMarker::Blank, cx)
            }))
            .on_action(cx.listener(|this, _: &SetMarkerCheckbox, _w, cx| {
                this.set_marker_for_selection(ItemMarker::Checkbox, cx)
            }))
            .on_action(cx.listener(|this, _: &SetMarkerBullet, _w, cx| {
                this.set_marker_for_selection(ItemMarker::Bullet, cx)
            }))
            .on_action(cx.listener(|this, _: &SetMarkerNumbered, _w, cx| {
                this.set_marker_for_selection(ItemMarker::Numbered, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleBold, _w, cx| this.toggle_bold(cx)))
            .on_action(cx.listener(|this, _: &ToggleItalic, _w, cx| this.toggle_italic(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleStrikethrough, _w, cx| this.toggle_strikethrough(cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleHeading, _w, cx| this.toggle_heading(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleStatus, _w, cx| {
                    this.toggle_status_for_current_line(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleStartDate, _w, cx| {
                this.open_date_for_current_line(DateKind::Start, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleEndDate, _w, cx| {
                this.open_date_for_current_line(DateKind::End, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleRepeat, _w, cx| {
                    this.toggle_repeat_for_current_line(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &RemoveStartDate, _w, cx| {
                this.remove_date_for_current_line(DateKind::Start, cx)
            }))
            .on_action(cx.listener(|this, _: &RemoveEndDate, _w, cx| {
                this.remove_date_for_current_line(DateKind::End, cx)
            }))
            .on_action(cx.listener(|_this, _: &ShowCharacterPalette, window, _cx| {
                window.show_character_palette();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(SchemeEditor::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(SchemeEditor::on_mouse_down))
            .on_mouse_move(cx.listener(SchemeEditor::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(SchemeEditor::on_mouse_up))
            .can_drop(|dragged, _window, _cx| {
                dragged
                    .downcast_ref::<ExternalPaths>()
                    .is_some_and(external_paths_have_supported_image)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                let position = window.mouse_position();
                this.drop_image_paths(paths, position, Some(window), cx);
            }))
            .child(SchemeTextElement {
                editor: cx.entity(),
            })
    }
}

struct SchemeTextElement {
    editor: Entity<SchemeEditor>,
}

impl IntoElement for SchemeTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SchemeTextElement {
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
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let editor = self.editor.update(cx, |editor, _cx| {
            editor.relayout_if_dirty(window);
            editor
                .estimated_height()
                .max(px(TEXT_LINE_HEIGHT + editor.top_pad))
        });
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = editor.into();
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
        self.editor.update(cx, |editor, cx| {
            let previous_width = editor.last_bounds.map(|bounds| bounds.size.width);
            editor.last_bounds = Some(bounds);
            if editor.line_map_dirty || previous_width != Some(bounds.size.width) {
                editor.relayout(bounds.size.width, window);
            }
            editor.apply_pending_scroll_to_cursor(cx);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.editor.read(cx).is_selecting {
            let editor = self.editor.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble && event.dragging() {
                    editor.update(cx, |editor, cx| {
                        editor.auto_scroll_last_mouse_position = Some(event.position);
                        editor.drag_to_position(event.position, cx);
                    });
                }
            });

            let editor = self.editor.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    editor.update(cx, |editor, cx| {
                        editor.is_selecting = false;
                        editor.stop_responding_to_mouse_movements();
                        cx.notify();
                    });
                }
            });
        }

        if !self.editor.read(cx).read_only {
            let focus_handle = self.editor.read(cx).focus_handle.clone();
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.editor.clone()),
                cx,
            );
        }
        self.editor.update(cx, |editor, cx| {
            editor.paint_editor(bounds, window, cx);
        });
    }
}
