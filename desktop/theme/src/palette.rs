pub const PALETTE: &[u32] = &[
    0xff453a, // SwiftUI red, dark
    0xff9f0a, // SwiftUI orange, dark
    0x30d158, // SwiftUI green, dark
    0x0a84ff, // SwiftUI blue, dark
    0xbf5af2, // SwiftUI purple, dark
    0xffd60a, // SwiftUI yellow, dark
];

const PALETTE_LIGHT: &[u32] = &[0xff3b30, 0xff9500, 0x34c759, 0x007aff, 0xaf52de, 0xffcc00];

pub fn scheme_color(index: u8, is_dark: bool) -> u32 {
    if is_dark {
        PALETTE[(index as usize) % PALETTE.len()]
    } else {
        PALETTE_LIGHT[(index as usize) % PALETTE_LIGHT.len()]
    }
}
