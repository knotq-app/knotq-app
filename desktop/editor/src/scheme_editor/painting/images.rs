use super::super::*;

/// Where an image was painted this frame, for hover and the resize handle.
#[derive(Clone, Copy)]
pub(in crate::scheme_editor) struct ImageHitbox {
    pub(in crate::scheme_editor) bounds: Bounds<Pixels>,
    pub(in crate::scheme_editor) item_id: ItemId,
    pub(in crate::scheme_editor) max_width: Pixels,
}

impl ImageHitbox {
    pub(in crate::scheme_editor) fn handle_bounds(&self) -> Bounds<Pixels> {
        let side = px(IMAGE_RESIZE_HANDLE)
            .min(self.bounds.size.width)
            .min(self.bounds.size.height);
        Bounds::new(
            point(self.bounds.right() - side, self.bounds.bottom() - side),
            size(side, side),
        )
    }
}

/// A drag of an image's corner. The line is resized live as the mouse moves and
/// committed as one command on release, so the whole drag is one undo step.
#[derive(Clone, Copy)]
pub(in crate::scheme_editor) struct ImageResize {
    item_id: ItemId,
    original: ImageInline,
    start_position: Point<Pixels>,
    start_size: gpui::Size<Pixels>,
    max_width: Pixels,
}

impl SchemeEditor {
    pub(in crate::scheme_editor) fn block_object_caret_bounds(
        &self,
        loc: TextLocation,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        if let Some(object) = self.table_object_range_for_row(loc.row) {
            if loc.col == object.start || loc.col == object.end {
                let (origin, grid_w, grid_h) = self.table_grid_geom(loc.row, bounds)?;
                let x = if loc.col == object.end {
                    origin.x + grid_w
                } else {
                    origin.x
                };
                return Some(Bounds::new(
                    point(x, origin.y),
                    size(px(1.5), grid_h.max(px(12.0))),
                ));
            }
        }
        self.image_object_caret_bounds(loc, bounds)
    }

    fn image_object_caret_bounds(
        &self,
        loc: TextLocation,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        let (object, image_index) = self.image_object_at_location(loc)?;
        let image = self.image_bounds_for_index(loc.row, image_index, bounds)?;
        let x = if loc.col == object.end {
            image.right()
        } else {
            image.left()
        };
        Some(Bounds::new(
            point(x, image.top()),
            size(px(1.5), image.size.height.max(px(12.0))),
        ))
    }

    pub(in crate::scheme_editor) fn image_object_at_location(
        &self,
        loc: TextLocation,
    ) -> Option<(Range<usize>, usize)> {
        let editor_row = self.rows.get(loc.row)?;
        if editor_row.path.is_cell() || editor_row.path.is_table_anchor() {
            return None;
        }
        let line = self
            .line_range(loc.row)
            .and_then(|range| self.text.get(range))?;
        let mut image_index = 0;
        let inlines = editor_row.item.content.to_inlines();
        for (block_index, object) in block_object_ranges(line).into_iter().enumerate() {
            let inline = inlines
                .iter()
                .filter(|inline| !inline.is_text())
                .nth(block_index)?;
            let is_here = loc.col == object.start || loc.col == object.end;
            match inline {
                Inline::Image(_) if is_here => return Some((object, image_index)),
                Inline::Image(_) => image_index += 1,
                Inline::Table(_) => {}
                Inline::Text { .. } => unreachable!(),
            }
        }
        None
    }

    fn image_bounds_for_index(
        &self,
        row: usize,
        target_image_index: usize,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        let image = self.image_bounds_for_index_content(row, target_image_index)?;
        Some(Bounds::new(
            self.content_to_window(image.origin, bounds),
            image.size,
        ))
    }

    pub(in crate::scheme_editor) fn image_bounds_for_index_content(
        &self,
        row: usize,
        target_image_index: usize,
    ) -> Option<Bounds<Pixels>> {
        let editor_row = self.rows.get(row)?;
        let item_line = self.line_map.item_line(row)?;
        let max_width = self.image_max_width_for_row(row, item_line);
        let line = self
            .line_range(row)
            .and_then(|range| self.text.get(range))?;
        let has_text = !clean_line_text(line).is_empty();
        let (base_x, base_y) = self.row_base_xy(row);
        let annotation_height = item_line
            .annotation
            .as_ref()
            .map(|annotation| annotation.height)
            .unwrap_or(px(0.0));
        let mut y = base_y
            + self.line_map.line_text_height(row)
            + annotation_height
            + if has_text { px(IMAGE_TOP_GAP) } else { px(0.0) };
        let text_left = base_x + self.first_text_x(row);
        let mut image_index = 0;
        for media in editor_row.item.images() {
            let media_size = self.media_size(media, max_width);
            if media_size.height <= px(0.0) {
                continue;
            }
            let image = Bounds::new(point(text_left, y), media_size);
            if image_index == target_image_index {
                return Some(image);
            }
            y += media_size.height + px(IMAGE_STACK_GAP);
            image_index += 1;
        }
        None
    }

    pub(in crate::scheme_editor) fn image_max_width_for_row(
        &self,
        row: usize,
        line: &SchemeItemLine,
    ) -> Pixels {
        let text_width = line.text.size(line.line_height()).width.max(px(120.0));
        (self
            .last_bounds
            .map(|bounds| bounds.size.width - px(TEXT_LEFT_PAD + 24.0) - self.row_indent_x(row))
            .unwrap_or(text_width))
        .max(px(120.0))
    }

    pub(in crate::scheme_editor) fn marker_left_for_text_left(
        &self,
        item: &Item,
        text_left: Pixels,
    ) -> Pixels {
        if item.marker == ItemMarker::Blank {
            text_left
        } else {
            text_left - px(CHECKBOX_SIZE + CHECKBOX_GAP)
        }
    }

    pub(in crate::scheme_editor) fn media_stack_height(
        &self,
        item: &Item,
        max_width: Pixels,
        has_text: bool,
    ) -> Pixels {
        let mut height = px(0.0);
        let mut count = 0;
        for media in item.images() {
            let media_size = self.media_size(media, max_width);
            if media_size.height <= px(0.0) {
                continue;
            }
            if count == 0 {
                // An image-only line has no text to separate from, so it sits
                // flush at the top of the (collapsed) line band.
                height += if has_text { px(IMAGE_TOP_GAP) } else { px(0.0) };
            } else {
                height += px(IMAGE_STACK_GAP);
            }
            height += media_size.height;
            count += 1;
        }
        height
    }

    pub(in crate::scheme_editor) fn image_for_media(
        &mut self,
        media: &ImageInline,
    ) -> Option<Arc<Image>> {
        if let Some(cached) = self.image_cache.get(&media.asset) {
            return cached.clone();
        }

        // Cache the result either way: a missing asset stays `None` so we don't
        // hit the disk again on every repaint.
        let image = load_image_for_media(media);
        self.image_cache.insert(media.asset, image.clone());
        image
    }

    /// Forget cached image-load failures so assets that have since appeared on
    /// disk (e.g. downloaded by sync) are retried on the next paint. Successful
    /// loads stay cached because assets are immutable (keyed by a fresh UUID).
    pub fn forget_missing_images(&mut self) {
        self.image_cache.retain(|_, image| image.is_some());
        self.image_intrinsic_sizes
            .borrow_mut()
            .retain(|_, size| size.is_some());
    }

    pub(in crate::scheme_editor) fn media_size(
        &self,
        media: &ImageInline,
        max_width: Pixels,
    ) -> gpui::Size<Pixels> {
        // Only an image taller than the cap is laid out differently for having
        // been resized, so only those cost a look at the file.
        let over_cap = media
            .height
            .is_some_and(|height| height as f32 > IMAGE_MAX_HEIGHT);
        let user_sized =
            over_cap && media_is_user_sized(media, self.image_intrinsic_size_cached(media));
        media_display_size(media, max_width, user_sized)
    }

    fn image_intrinsic_size_cached(&self, media: &ImageInline) -> Option<(u32, u32)> {
        *self
            .image_intrinsic_sizes
            .borrow_mut()
            .entry(media.asset)
            .or_insert_with(|| image_intrinsic_size(media))
    }

    pub(in crate::scheme_editor) fn image_hitbox_at(
        &self,
        position: Point<Pixels>,
    ) -> Option<ImageHitbox> {
        self.image_hitboxes
            .iter()
            .copied()
            .find(|hitbox| bounds_contains(hitbox.bounds, position))
    }

    pub(in crate::scheme_editor) fn is_resizing_image(&self) -> bool {
        self.image_resize.is_some()
    }

    pub(in crate::scheme_editor) fn image_shows_resize_handle(&self, item_id: ItemId) -> bool {
        !self.read_only
            && (self.hovered_image == Some(item_id)
                || self
                    .image_resize
                    .is_some_and(|resize| resize.item_id == item_id))
    }

    /// Begin a corner drag if `position` is on an image's resize handle.
    pub(in crate::scheme_editor) fn start_image_resize(&mut self, position: Point<Pixels>) -> bool {
        let Some(hitbox) = self
            .image_hitbox_at(position)
            .filter(|hitbox| bounds_contains(hitbox.handle_bounds(), position))
        else {
            return false;
        };
        let Some(original) = self
            .rows
            .iter()
            .find(|row| row.path.is_doc() && row.item.id == hitbox.item_id)
            .and_then(|row| row.item.content.image().copied())
        else {
            return false;
        };
        self.image_resize = Some(ImageResize {
            item_id: hitbox.item_id,
            original,
            start_position: position,
            start_size: hitbox.bounds.size,
            max_width: hitbox.max_width,
        });
        true
    }

    /// Resize the dragged image so its corner follows the mouse.
    pub(in crate::scheme_editor) fn drag_image_resize(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.image_resize else {
            return;
        };
        let start_width = resize.start_size.width.to_f64() as f32;
        let start_height = resize.start_size.height.to_f64() as f32;
        if start_width <= 0.0 || start_height <= 0.0 {
            return;
        }
        let dx = (position.x - resize.start_position.x).to_f64() as f32;
        let dy = (position.y - resize.start_position.y).to_f64() as f32;
        // Follow whichever axis the mouse has pulled further, so dragging
        // straight down or straight across both work.
        let scale = ((start_width + dx) / start_width).max((start_height + dy) / start_height);
        let max_scale = (resize.max_width.to_f64() as f32 / start_width).max(0.0);
        let min_scale = (IMAGE_MIN_SIDE / start_width.min(start_height)).min(max_scale);
        let width = start_width * scale.clamp(min_scale, max_scale);
        let resized = media_resized_to_width(
            &resize.original,
            self.image_intrinsic_size_cached(&resize.original),
            width,
        );
        let Some(row) = self
            .rows
            .iter_mut()
            .find(|row| row.path.is_doc() && row.item.id == resize.item_id)
        else {
            return;
        };
        if row.item.content.image() == Some(&resized) {
            return;
        }
        row.item.set_image(resized);
        self.line_map_dirty = true;
        cx.notify();
    }

    /// End the corner drag, committing the new size if it changed.
    pub(in crate::scheme_editor) fn finish_image_resize(&mut self, cx: &mut Context<Self>) {
        let Some(resize) = self.image_resize.take() else {
            return;
        };
        let item = self
            .rows
            .iter()
            .find(|row| row.path.is_doc() && row.item.id == resize.item_id)
            .map(|row| row.item.clone());
        if let Some(item) = item.filter(|item| item.content.image() != Some(&resize.original)) {
            cx.emit(EditorEvent::Command(Command::ReplaceItem {
                scheme: self.scheme_id,
                item,
            }));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};
    use knotq_model::ImageAssetFormat;
    use std::cell::RefCell;
    use std::rc::Rc;

    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;

    fn editor_with_an_image(
        cx: &mut TestAppContext,
    ) -> (
        gpui::Entity<SchemeEditor>,
        &mut VisualTestContext,
        Item,
        Rc<RefCell<Vec<Command>>>,
    ) {
        let mut line = Item::new("");
        line.set_image(ImageInline {
            asset: Uuid::from_u128(0x1a6e_u128),
            format: ImageAssetFormat::Png,
            width: Some(WIDTH),
            height: Some(HEIGHT),
        });
        let mut scheme = Scheme::new("Screenshots", 0);
        scheme.items = vec![Item::new("above"), line.clone(), Item::new("below")].into();

        let commands = Rc::new(RefCell::new(Vec::new()));
        let (editor, cx) = cx.add_window_view(|window, cx| {
            SchemeEditor::new(
                SchemeId::new(),
                scheme,
                knotq_theme::all_themes()[0],
                TimeFormat::TwentyFourHour,
                ScrollHandle::new(),
                window,
                cx,
            )
        });
        let sink = commands.clone();
        cx.update(|_, cx| {
            // Stand in for the file on disk, whose header is where this size is
            // normally read from.
            editor
                .read(cx)
                .image_intrinsic_sizes
                .borrow_mut()
                .insert(line.content.image().unwrap().asset, Some((WIDTH, HEIGHT)));
            cx.subscribe(&editor, move |_, event: &EditorEvent, _| {
                if let EditorEvent::Command(command) = event {
                    sink.borrow_mut().push(command.clone());
                }
            })
            .detach();
        });
        cx.run_until_parked();
        (editor, cx, line, commands)
    }

    fn painted_image(
        editor: &gpui::Entity<SchemeEditor>,
        cx: &mut VisualTestContext,
    ) -> ImageHitbox {
        cx.run_until_parked();
        cx.update(|_, cx| {
            let hitboxes = &editor.read(cx).image_hitboxes;
            assert_eq!(hitboxes.len(), 1, "the image line was not painted");
            hitboxes[0]
        })
    }

    fn image_on_line(
        editor: &gpui::Entity<SchemeEditor>,
        cx: &mut VisualTestContext,
    ) -> ImageInline {
        cx.update(|_, cx| {
            *editor.read(cx).rows[1]
                .item
                .content
                .image()
                .expect("an image")
        })
    }

    #[gpui::test]
    fn dragging_the_corner_resizes_the_image_and_commits_once(cx: &mut TestAppContext) {
        let (editor, cx, line, commands) = editor_with_an_image(cx);
        let before = painted_image(&editor, cx);
        assert_eq!(
            before.bounds.size,
            size(px(WIDTH as f32), px(HEIGHT as f32))
        );
        let below_before = cx.update(|_, cx| editor.read(cx).row_base_xy(2).1);

        let corner = point(
            before.bounds.right() - px(4.0),
            before.bounds.bottom() - px(4.0),
        );
        cx.simulate_mouse_move(corner, None, Modifiers::default());
        assert!(cx.update(|_, cx| editor.read(cx).hovered_image_handle));
        cx.simulate_mouse_down(corner, MouseButton::Left, Modifiers::default());
        assert!(cx.update(|_, cx| editor.read(cx).is_resizing_image()));

        // Pull the corner in by half the width; the height follows.
        let halfway = point(corner.x - px(60.0), corner.y - px(40.0));
        cx.simulate_mouse_move(halfway, MouseButton::Left, Modifiers::default());
        let target = point(corner.x - px(120.0), corner.y - px(80.0));
        cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        assert!(
            commands.borrow().is_empty(),
            "a drag in progress must not commit: {:?}",
            commands.borrow()
        );
        let during = painted_image(&editor, cx);
        assert_eq!(during.bounds.size, size(px(120.0), px(80.0)));
        assert_eq!(during.bounds.origin, before.bounds.origin);
        // The lines below move up with it rather than being painted over.
        let below_during = cx.update(|_, cx| editor.read(cx).row_base_xy(2).1);
        assert_eq!(below_before - below_during, px(80.0));

        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        assert!(!cx.update(|_, cx| editor.read(cx).is_resizing_image()));
        let resized = image_on_line(&editor, cx);
        assert_eq!((resized.width, resized.height), (Some(120), Some(80)));
        let commands = commands.borrow();
        let [Command::ReplaceItem { item, .. }] = commands.as_slice() else {
            panic!("expected one ReplaceItem, got {commands:?}");
        };
        assert_eq!(item.id, line.id);
        assert_eq!(item.content.image(), Some(&resized));
    }

    #[gpui::test]
    fn a_drag_stops_at_the_minimum_and_at_the_width_of_the_line(cx: &mut TestAppContext) {
        let (editor, cx, _, commands) = editor_with_an_image(cx);
        let image = painted_image(&editor, cx);
        let corner = point(
            image.bounds.right() - px(2.0),
            image.bounds.bottom() - px(2.0),
        );

        cx.simulate_mouse_down(corner, MouseButton::Left, Modifiers::default());
        let far_inside = point(corner.x - px(5000.0), corner.y - px(5000.0));
        cx.simulate_mouse_move(far_inside, MouseButton::Left, Modifiers::default());
        let smallest = painted_image(&editor, cx).bounds.size;
        assert_eq!(smallest.height, px(IMAGE_MIN_SIDE));
        assert_eq!(
            smallest.width,
            px(IMAGE_MIN_SIDE * WIDTH as f32 / HEIGHT as f32)
        );

        let far_outside = point(corner.x + px(5000.0), corner.y);
        cx.simulate_mouse_move(far_outside, MouseButton::Left, Modifiers::default());
        let largest = painted_image(&editor, cx);
        assert_eq!(largest.bounds.size.width, largest.max_width);
        assert!(largest.bounds.size.width > px(WIDTH as f32));
        cx.simulate_mouse_up(far_outside, MouseButton::Left, Modifiers::default());
        assert_eq!(commands.borrow().len(), 1);
    }

    #[gpui::test]
    fn a_click_on_the_handle_or_the_image_changes_nothing(cx: &mut TestAppContext) {
        let (editor, cx, line, commands) = editor_with_an_image(cx);
        let image = painted_image(&editor, cx);
        let corner = point(
            image.bounds.right() - px(4.0),
            image.bounds.bottom() - px(4.0),
        );
        cx.simulate_mouse_down(corner, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(corner, MouseButton::Left, Modifiers::default());

        // The middle of the image is not the handle: it places the caret.
        let middle = image.bounds.center();
        cx.simulate_mouse_down(middle, MouseButton::Left, Modifiers::default());
        assert!(!cx.update(|_, cx| editor.read(cx).is_resizing_image()));
        cx.simulate_mouse_move(
            point(middle.x - px(50.0), middle.y - px(30.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(middle, MouseButton::Left, Modifiers::default());

        assert!(commands.borrow().is_empty(), "{:?}", commands.borrow());
        assert_eq!(Some(&image_on_line(&editor, cx)), line.content.image());
    }
}
