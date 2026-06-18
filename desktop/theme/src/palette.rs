pub const PALETTE: &[u32] = &[
    0xff453a, // SwiftUI red, dark
    0xff9f0a, // SwiftUI orange, dark
    0x30d158, // SwiftUI green, dark
    0x0a84ff, // SwiftUI blue, dark
    0xbf5af2, // SwiftUI purple, dark
    0xffd60a, // SwiftUI yellow, dark
];

// Muted scheme colors that harmonize with the knotq.com light theme: the site's
// named --red / --green / --blue / --purple, with orange and amber to fill out
// the six-slot palette.
const PALETTE_LIGHT: &[u32] = &[0xb84433, 0xc47400, 0x28764f, 0x2563a6, 0x735aa6, 0xe0a800];

pub fn scheme_color(index: u8, is_dark: bool) -> u32 {
    if is_dark {
        PALETTE[(index as usize) % PALETTE.len()]
    } else {
        PALETTE_LIGHT[(index as usize) % PALETTE_LIGHT.len()]
    }
}
