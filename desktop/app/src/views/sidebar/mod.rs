use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Entity, IntoElement, MouseButton, MouseDownEvent, Pixels, Point,
    Render, SharedString, Window,
};
use gpui_component::{Icon, IconName, Sizable};
use knotq_commands::Command;
use knotq_model::{FolderId, NodeRef, SchemeId};

use crate::app::{
    KnotQApp, NewNodeKind, SidebarContextMenu, SidebarContextTarget, View,
    daily_queue_marker_color, DAILY_QUEUE_TITLE,
};
use crate::theme_gpui::{scheme_square_color, token_hsla, token_rgba, Theme, FONT_UI};
use knotq_ui::single_line_editor::SingleLineEditor;
use knotq_ui::{clamped_popover_left, popover_top_biased_below};

pub(super) const ZED_FOLDER_ICON: &str = "icons/zed-folder.svg";
pub(super) const ZED_FOLDER_OPEN_ICON: &str = "icons/zed-folder-open.svg";
pub(super) const DELETE_ICON: &str = "icons/delete.svg";
pub(super) const NAV_ROW_INDENT_BASE: f32 = 4.0;
pub(super) const NAV_ICON_SLOT: f32 = 12.0;
pub(super) const NAV_ICON_GAP: f32 = 7.0;
pub(super) const NAV_ROW_HEIGHT: f32 = 26.0;
pub(super) const NAV_DROP_ZONE_HEIGHT: f32 = 3.0;
pub(super) const SCHEME_SQUARE_SIZE: f32 = 9.0;
pub(super) const FOLDER_ICON_SIZE: f32 = 10.5;
pub(super) const SIDEBAR_TEXT_SIZE: f32 = 13.0;
pub(super) const SIDEBAR_LINE_HEIGHT: f32 = 17.0;
pub(super) const FOOTER_TEXT_SIZE: f32 = 11.5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavigatorNodeKind {
    Folder,
    Scheme,
}

#[derive(Clone, Debug)]
pub(super) struct NavigatorDragInfo {
    node: NodeRef,
    kind: NavigatorNodeKind,
    source_parent: FolderId,
    source_position: usize,
    root: FolderId,
    label: String,
    color_index: Option<u8>,
    theme: Theme,
}

mod components;
mod context_menu;
mod drag;
mod render;
mod rows;
mod trash;
mod tree;

use self::components::*;
use self::drag::*;
