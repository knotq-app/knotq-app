pub const PALETTE: &[u32] = &[
    0xff453a, // SwiftUI red, dark
    0xff9f0a, // SwiftUI orange, dark
    0x30d158, // SwiftUI green, dark
    0x0a84ff, // SwiftUI blue, dark
    0xbf5af2, // SwiftUI purple, dark
    0xffd60a, // SwiftUI yellow, dark
    0xff375f, 0xff5e5ce6, 0x64d2ff, 0x5ac8fa, 0x34c759, 0x30b0c7,
    0xaf52de, 0xff2d55, 0xff9f0a, 0xac8e68, 0x8e8e93, 0x636366,
];

// Muted scheme colors that harmonize with the knotq.com light theme: the site's
// named --red / --green / --blue / --purple, with orange and amber to fill out
// the six-slot palette.
const PALETTE_LIGHT: &[u32] = &[
    0xb84433, 0xc47400, 0x28764f, 0x2563a6, 0x735aa6, 0xe0a800,
    0x9f3f2f, 0x4f46a5, 0x267d9f, 0x287c9b, 0x27735f, 0x326b49,
    0x7a3e9b, 0xb12f4b, 0xa66a00, 0x785d3c, 0x5f636b, 0x4f535b,
];

pub fn scheme_color(index: u8, is_dark: bool) -> u32 {
    if is_dark {
        PALETTE[(index as usize) % PALETTE.len()]
    } else {
        PALETTE_LIGHT[(index as usize) % PALETTE_LIGHT.len()]
    }
}
