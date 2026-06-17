use crate::{rgb, rgba, Theme};

pub fn theme_parchment() -> Theme {
    Theme {
        name: "Parchment",
        is_dark: false,
        bg_app: rgb(0xf4f1ea),
        bg_sidebar: rgb(0xe4ddd3),
        bg_toolbar: rgb(0xefe7dc),
        bg_upcoming: rgb(0xe7e5e2),
        bg_hint: rgb(0xe8dfd2),
        bg_cal_hdr: rgb(0xe9e0d4),
        bg_modal: rgb(0xf8f4ee),
        border_main: rgba(0xb8ab99ff),
        border_soft: rgba(0xcabfaeff),
        border_strong: rgba(0xae9f8cff),
        border_overlay: rgba(0x4d3f2d24),
        text_primary: rgb(0x2c2720),
        text_dim: rgba(0x4c4337a8),
        text_muted: rgba(0x5c534758),
        text_soft: rgba(0x4f473c9e),
        text_highlight: rgba(0x2e291fe6),
        highlight_bg: rgba(0x9b7fd4f2),
        highlight_text: rgb(0xffffff),
        text_placeholder: rgba(0x5c534740),
        text_today: rgb(0xe66d5d),
        caret_color: rgb(0x7aa0ff),
        row_alt: rgba(0x5a46350c),
        row_hover: rgba(0x5a463514),
        row_hover_strong: rgba(0x5a46351c),
        row_selected: rgba(0x5a463526),
        button_bg: rgba(0x5a463514),
        button_hover: rgba(0x5a463524),
        divider: rgba(0x5a463520),
        divider_soft: rgba(0x5a463516),
        divider_faint: rgba(0x5a463518),
        divider_tiny: rgba(0x5a46350d),
        overlay_scrim: rgba(0x705c4790),
        cal_grid: rgba(0x5a463520),
        cal_grid_soft: rgba(0x5a46350e),
        cal_past: rgba(0x8c8c8c28),
        event_bg: rgba(0xe6e8ecee),
        event_border: rgba(0x5a4635a0),
        checkbox_border_on: rgba(0x4f73d0b8),
        checkbox_border_off: rgba(0x7b715fb0),
        checkbox_fill_on: rgba(0x6f8fe090),
        checkbox_fill_off: rgba(0xffffff60),
        checkbox_mark: rgba(0x2f2a22d0),
        done_text: rgba(0x4c433780),
        drag_preview_bg: rgba(0xe4ddd3b8),
        toolbar_chip_bg: rgba(0xe9edf3f0),
        toolbar_chip_border: rgba(0xaab4c4aa),
        toolbar_chip_selected_text: rgb(0x111418),
        toolbar_chip_muted: rgba(0x65707cff),
        toolbar_chip_separator: rgba(0x77869a66),
        daily_title_active: rgb(0x111418),
        daily_title_muted: rgb(0x6f7985),
        link: rgb(0x0645ad),
        link_hover: rgb(0x0b56d0),
        cal_event_text: rgba(0x24272dcc),
        cal_weekend_tint: rgba(0x00000008),
        row_stripe: rgba(0x006fff10),
    }
}

pub fn theme_rose_pine_dawn() -> Theme {
    Theme {
        name: "Rosé Piné Dawn",
        is_dark: false,
        bg_app: rgb(0xfaf4ed),
        bg_sidebar: rgb(0xf2e9e1),
        bg_toolbar: rgb(0xf4ede8),
        bg_upcoming: rgb(0xf2e9e1),
        bg_hint: rgb(0xf4ede8),
        bg_cal_hdr: rgb(0xede7df),
        bg_modal: rgb(0xfffaf3),
        border_main: rgba(0x9893a578),
        border_soft: rgba(0x9893a548),
        border_strong: rgba(0x79759360),
        border_overlay: rgba(0x57527820),
        text_primary: rgb(0x575279),
        text_dim: rgba(0x9893a5a0),
        text_muted: rgba(0x9893a560),
        text_soft: rgba(0x79759390),
        text_highlight: rgba(0x575279e0),
        highlight_bg: rgba(0x9b7fd4f2),
        highlight_text: rgb(0xffffff),
        text_placeholder: rgba(0x9893a540),
        text_today: rgb(0xb4637a),
        caret_color: rgb(0x907aa9),
        row_alt: rgba(0x57527808),
        row_hover: rgba(0x57527812),
        row_hover_strong: rgba(0x57527818),
        row_selected: rgba(0x57527822),
        button_bg: rgba(0x57527812),
        button_hover: rgba(0x57527820),
        divider: rgba(0x57527820),
        divider_soft: rgba(0x57527816),
        divider_faint: rgba(0x57527818),
        divider_tiny: rgba(0x5752780d),
        overlay_scrim: rgba(0x57527870),
        cal_grid: rgba(0x57527818),
        cal_grid_soft: rgba(0x5752780c),
        cal_past: rgba(0x9893a520),
        event_bg: rgba(0xf2e9e1ee),
        event_border: rgba(0x9893a578),
        checkbox_border_on: rgba(0x907aa9b0),
        checkbox_border_off: rgba(0x9893a5a0),
        checkbox_fill_on: rgba(0x907aa990),
        checkbox_fill_off: rgba(0xffffff60),
        checkbox_mark: rgba(0x575279d0),
        done_text: rgba(0x57527970),
        drag_preview_bg: rgba(0xf2e9e1b8),
        toolbar_chip_bg: rgba(0xf4ede8f0),
        toolbar_chip_border: rgba(0xb4adcaaa),
        toolbar_chip_selected_text: rgb(0x191724),
        toolbar_chip_muted: rgba(0x79759380),
        toolbar_chip_separator: rgba(0x7975936e),
        daily_title_active: rgb(0x575279),
        daily_title_muted: rgb(0x9893a5),
        link: rgb(0x907aa9),
        link_hover: rgb(0xb4a1cb),
        cal_event_text: rgba(0x24272dcc),
        cal_weekend_tint: rgba(0x00000008),
        row_stripe: rgba(0x907aa910),
    }
}

pub fn theme_catppuccin_latte() -> Theme {
    Theme {
        name: "Catppuccin Latte",
        is_dark: false,
        bg_app: rgb(0xeff1f5),
        bg_sidebar: rgb(0xe6e9ef),
        bg_toolbar: rgb(0xdce0e8),
        bg_upcoming: rgb(0xe6e9ef),
        bg_hint: rgb(0xdce0e8),
        bg_cal_hdr: rgb(0xdce0e8),
        bg_modal: rgb(0xeff1f5),
        border_main: rgba(0xbcc0ccff),
        border_soft: rgba(0xccd0daff),
        border_strong: rgba(0xacb0beff),
        border_overlay: rgba(0x4c4f6920),
        text_primary: rgb(0x4c4f69),
        text_dim: rgba(0x6c6f85a0),
        text_muted: rgba(0x8c8fa158),
        text_soft: rgba(0x5c5f779e),
        text_highlight: rgba(0x4c4f69e6),
        highlight_bg: rgba(0x9b7fd4f2),
        highlight_text: rgb(0xffffff),
        text_placeholder: rgba(0x8c8fa140),
        text_today: rgb(0xd20f39),
        caret_color: rgb(0x7287fd),
        row_alt: rgba(0x4c4f690c),
        row_hover: rgba(0x4c4f6914),
        row_hover_strong: rgba(0x4c4f691c),
        row_selected: rgba(0x4c4f6926),
        button_bg: rgba(0x4c4f6914),
        button_hover: rgba(0x4c4f6924),
        divider: rgba(0x4c4f6920),
        divider_soft: rgba(0x4c4f6916),
        divider_faint: rgba(0x4c4f6918),
        divider_tiny: rgba(0x4c4f690d),
        overlay_scrim: rgba(0x4c4f6990),
        cal_grid: rgba(0x4c4f6920),
        cal_grid_soft: rgba(0x4c4f690e),
        cal_past: rgba(0x8c8c8c24),
        event_bg: rgba(0xe6e9efee),
        event_border: rgba(0xacb0bea0),
        checkbox_border_on: rgba(0x7287fdb8),
        checkbox_border_off: rgba(0x8c8fa1b0),
        checkbox_fill_on: rgba(0x7287fd90),
        checkbox_fill_off: rgba(0xffffff60),
        checkbox_mark: rgba(0x4c4f69d0),
        done_text: rgba(0x4c4f6980),
        drag_preview_bg: rgba(0xe6e9efb8),
        toolbar_chip_bg: rgba(0xe9edf3f0),
        toolbar_chip_border: rgba(0xaab4c4aa),
        toolbar_chip_selected_text: rgb(0x111418),
        toolbar_chip_muted: rgba(0x65707cff),
        toolbar_chip_separator: rgba(0x77869a66),
        daily_title_active: rgb(0x4c4f69),
        daily_title_muted: rgb(0x6c6f85),
        link: rgb(0x1e66f5),
        link_hover: rgb(0x3573f7),
        cal_event_text: rgba(0x24272dcc),
        cal_weekend_tint: rgba(0x00000008),
        row_stripe: rgba(0x1e66f51a),
    }
}

pub fn theme_light() -> Theme {
    let mut theme = theme_parchment();
    theme.name = "Light";
    // Warm, muted backgrounds — no pure whites
    theme.bg_app = rgb(0xe8e2d8);
    theme.bg_sidebar = rgb(0xe0d8cc);
    theme.bg_toolbar = rgb(0xe3dcd2);
    theme.bg_upcoming = rgb(0xe5ded5);
    theme.bg_hint = rgb(0xe0d8cc);
    theme.bg_cal_hdr = rgb(0xe0d8cc);
    theme.bg_modal = rgb(0xece6dd);
    theme.border_main = rgba(0xc4b5a4ff);
    theme.border_soft = rgba(0xd6cabcff);
    theme.border_strong = rgba(0xa89580ff);
    theme.border_overlay = rgba(0x3d2a1830);
    // Softened text for less harsh contrast
    theme.text_primary = rgb(0x2c2420);
    theme.text_dim = rgba(0x302520e0);
    theme.text_muted = rgba(0x5a4a3cc0);
    theme.text_soft = rgba(0x382c22d8);
    theme.text_highlight = rgba(0x1a1410f4);
    theme.text_placeholder = rgba(0x5a4a3c90);
    theme.text_today = rgb(0xd04e1a);
    theme.caret_color = rgb(0x1a1410);
    theme.row_alt = rgba(0x5a46350c);
    theme.row_hover = rgba(0x5a463518);
    theme.row_hover_strong = rgba(0x5a463524);
    theme.row_selected = rgba(0xe66f1f1a);
    theme.button_bg = rgba(0x5a463518);
    theme.button_hover = rgba(0xe66f1f20);
    theme.divider = rgba(0x5a463524);
    theme.divider_soft = rgba(0x5a463518);
    theme.divider_faint = rgba(0x5a463514);
    theme.divider_tiny = rgba(0x5a46350d);
    theme.overlay_scrim = rgba(0x3b24169a);
    theme.cal_grid = rgba(0x5a463524);
    theme.cal_grid_soft = rgba(0x5a463510);
    theme.cal_past = rgba(0x8c8c8c40);
    // Event blocks: cooler/grayer tint to stand out from warm bg
    theme.event_bg = rgba(0xd3d2ceee);
    theme.event_border = rgba(0xa89580c0);
    theme.checkbox_border_on = rgba(0xd04e1ac8);
    theme.checkbox_border_off = rgba(0x6b5d50c0);
    theme.checkbox_fill_on = rgba(0xe66f1f90);
    theme.checkbox_fill_off = rgba(0xe8e2d890);
    theme.checkbox_mark = rgba(0x1a1410e0);
    theme.done_text = rgba(0x5a4a3ca8);
    theme.drag_preview_bg = rgba(0xe0d8ccc8);
    theme.toolbar_chip_bg = rgba(0xebeff5f0);
    theme.toolbar_chip_border = rgba(0xb0b8c8aa);
    theme.toolbar_chip_selected_text = rgb(0x111418);
    theme.toolbar_chip_muted = rgba(0x4a4050ff);
    theme.toolbar_chip_separator = rgba(0x7a808a66);
    theme.daily_title_active = rgb(0x1a1410);
    theme.daily_title_muted = rgb(0x4a3e34);
    theme.link = rgb(0xc04510);
    theme.link_hover = rgb(0xe66f1f);
    // Fully opaque dark text on calendar events
    theme.cal_event_text = rgba(0x1a1410f0);
    theme.cal_weekend_tint = rgba(0xe66f1f06);
    theme.row_stripe = rgba(0xe66f1f12);
    theme
}
