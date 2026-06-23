use super::super::*;

use super::{insert_images_at_text_col, persist_image_files};

impl SchemeEditor {
    pub(in crate::scheme_editor) fn insert_image_from_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }

        self.focus(window, cx);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Insert image".into()),
        });
        cx.spawn(
            async move |editor: gpui::WeakEntity<SchemeEditor>, cx: &mut gpui::AsyncApp| {
                let paths = match paths.await {
                    Ok(Ok(Some(paths))) => paths,
                    _ => return,
                };
                let (media, rejections) = persist_image_files(&paths);
                if media.is_empty() && rejections.is_empty() {
                    return;
                }
                let _ = editor.update(cx, |editor, cx| {
                    editor.notify_media_rejections(&rejections, cx);
                    if !media.is_empty() {
                        let row = editor.current_row_index();
                        editor.append_media_to_row(row, media, None, cx);
                    }
                });
            },
        )
        .detach();
    }

    fn notify_media_rejections(
        &self,
        rejections: &[(Option<String>, MediaError)],
        cx: &mut Context<Self>,
    ) {
        if rejections.is_empty() {
            return;
        }
        cx.emit(EditorEvent::Notice {
            title: "Image not added".to_string(),
            message: media_rejection_message(rejections),
        });
    }

    pub(in crate::scheme_editor) fn paste_image(
        &mut self,
        image: &Image,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let media = match persist_clipboard_image(image) {
            Ok(media) => media,
            Err(error) => {
                self.notify_media_rejections(&[(None, error)], cx);
                return false;
            }
        };
        let row = self.current_row_index();
        self.append_media_to_row(row, vec![media], window, cx)
    }

    pub(in crate::scheme_editor) fn drop_image_paths(
        &mut self,
        paths: &ExternalPaths,
        position: Point<Pixels>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let (media, rejections) = persist_image_files(paths.paths());
        self.notify_media_rejections(&rejections, cx);
        if media.is_empty() {
            return false;
        }
        let row = self
            .location_for_window_position(position)
            .row
            .min(self.rows.len().saturating_sub(1));
        self.selection = TextSelection::collapsed(TextLocation {
            row,
            col: self.line_len(row),
        });
        self.append_media_to_row(row, media, window, cx)
    }

    pub(in crate::scheme_editor) fn append_media_to_row(
        &mut self,
        row: usize,
        media: Vec<ImageInline>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || media.is_empty() || self.rows.is_empty() {
            return false;
        }
        let row = row.min(self.rows.len().saturating_sub(1));
        let insert_col = if self.selection.head.row == row {
            self.selection.head.col.min(self.line_len(row))
        } else {
            self.line_len(row)
        };
        let is_cell = self
            .rows
            .get(row)
            .map(|r| r.path.is_cell())
            .unwrap_or(false);
        if !is_cell {
            // Inserting an image into a normal line splits it: leading/trailing
            // text stay as their own lines and the image lands on a line of its own.
            let blocks = media.into_iter().map(Inline::Image).collect::<Vec<_>>();
            return self.insert_block_lines_at_doc_row(row, insert_col, blocks, window, cx);
        }

        // Table cell line: insert in place (cells are sub-documents).
        let old_top = reconstruct_top_level(&self.rows);
        let Some(editor_row) = self.rows.get_mut(row) else {
            return false;
        };
        insert_images_at_text_col(&mut editor_row.item, insert_col, media);

        let new_top = reconstruct_top_level(&self.rows);
        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        self.selection = TextSelection::collapsed(TextLocation {
            row,
            col: insert_col.min(self.line_len(row)),
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        self.emit_top_level_diff(&old_top, &new_top, cx);
        cx.notify();
        true
    }

    /// Insert `blocks` (image/table inlines) at `col` on a *document* line,
    /// splitting the line's text around them so each block becomes its own line.
    pub(in crate::scheme_editor) fn insert_block_lines_at_doc_row(
        &mut self,
        row: usize,
        col: usize,
        blocks: Vec<Inline>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || blocks.is_empty() || self.rows.is_empty() {
            return false;
        }
        let row = row.min(self.rows.len() - 1);
        let old_top = reconstruct_top_level(&self.rows);
        let Some(pos) = top_level_index_for_flat_row(&self.rows, row) else {
            return false;
        };
        let mut new_top = old_top.clone();
        let Some(orig) = new_top.get(pos) else {
            return false;
        };
        let replacement = split_line_with_blocks(orig, col, blocks);
        let inserted = replacement.len();
        new_top.splice(pos..=pos, replacement);

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        // Caret just after the inserted block(s): the start of the trailing text
        // line when one exists, otherwise the end of the last block line.
        let cursor_top = pos + inserted.saturating_sub(1);
        let target_row = flat_row_for_top_level_index(&self.rows, cursor_top);
        let col = if self
            .rows
            .get(target_row)
            .is_some_and(|r| item_has_block_object(&r.item))
        {
            self.line_len(target_row)
        } else {
            0
        };
        self.selection = TextSelection::collapsed(TextLocation {
            row: target_row,
            col,
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        self.emit_top_level_diff(&old_top, &new_top, cx);
        cx.notify();
        true
    }
}
