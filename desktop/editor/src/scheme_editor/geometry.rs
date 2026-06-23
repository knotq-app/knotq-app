use gpui::{Bounds, Pixels, Point};

mod hit_test;
mod positioning;
mod selection_paint;

pub(super) fn bounds_contains(bounds: Bounds<Pixels>, point: Point<Pixels>) -> bool {
    point.x >= bounds.left()
        && point.x <= bounds.right()
        && point.y >= bounds.top()
        && point.y <= bounds.bottom()
}
