use gpui::{px, Pixels};

const VIEWPORT_MARGIN: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopoverAnchor {
    pub x: Pixels,
    pub y: Pixels,
}

pub fn clamped_popover_left(desired_left: Pixels, width: Pixels, viewport_width: Pixels) -> Pixels {
    let margin = px(VIEWPORT_MARGIN);
    let max_left = (viewport_width - width - margin).max(margin);
    desired_left.clamp(margin, max_left)
}

pub fn popover_top_biased_below(
    desired_below_top: Pixels,
    height: Pixels,
    viewport_height: Pixels,
) -> Pixels {
    let margin = px(VIEWPORT_MARGIN);
    let max_top = (viewport_height - height - margin).max(margin);
    desired_below_top.clamp(margin, max_top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamped_left_keeps_popover_inside_viewport() {
        assert_eq!(
            clamped_popover_left(px(480.0), px(100.0), px(500.0)),
            px(392.0)
        );
    }

    #[test]
    fn biased_below_clamps_when_popover_would_overflow() {
        assert_eq!(
            popover_top_biased_below(px(340.0), px(120.0), px(420.0)),
            px(292.0)
        );
    }
}
