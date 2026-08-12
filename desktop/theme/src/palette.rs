pub const PALETTE: &[u32] = &[
    0xff453a, // SwiftUI red, dark
    0xff9f0a, // SwiftUI orange, dark
    0x30d158, // SwiftUI green, dark
    0x0a84ff, // SwiftUI blue, dark
    0xbf5af2, // SwiftUI purple, dark
    0xffd60a, // SwiftUI yellow, dark
    0xff375f, // pink
    0x64d2ff, // cyan
    0x66d4cf, // mint
    0x5e5ce6, // indigo
    0xac8e68, // brown
    0x8e8e93, // graphite
    0xff6b6b, // coral
    0xa8e063, // lime
    0x4cc9f0, // sky
    0x8b5cf6, // violet
    0xe85aad, // magenta
    0xd4a017, // gold
];

// Muted scheme colors that harmonize with the knotq.com light theme: the site's
// named --red / --green / --blue / --purple, with orange and amber to fill out
// the original six slots plus additional choices. The first six entries must
// remain stable because their indices are already persisted and synced.
const PALETTE_LIGHT: &[u32] = &[
    0xb84433, 0xc47400, 0x28764f, 0x2563a6, 0x735aa6, 0xe0a800, 0xc13563, 0x0e7490, 0x147d73,
    0x4f46a5, 0x7a5b3a, 0x666a73, 0xb84f4f, 0x5d7d2a, 0x287ea3, 0x6d4ab5, 0xa83d78, 0x9a6a00,
];

/// Display order for the scheme color picker. Indices are the persisted wire
/// values; arranging them here does not change existing schemes' colors.
pub const SCHEME_COLOR_ORDER: &[u8] =
    &[0, 12, 6, 16, 4, 15, 9, 3, 14, 7, 8, 2, 13, 5, 17, 1, 10, 11];

pub fn scheme_color(index: u8, is_dark: bool) -> u32 {
    if is_dark {
        PALETTE[(index as usize) % PALETTE.len()]
    } else {
        PALETTE_LIGHT[(index as usize) % PALETTE_LIGHT.len()]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn light_and_dark_palettes_have_one_distinct_color_per_wire_index() {
        assert_eq!(PALETTE.len(), PALETTE_LIGHT.len());
        assert_eq!(PALETTE.len(), SCHEME_COLOR_ORDER.len());
        assert_eq!(
            PALETTE.iter().copied().collect::<HashSet<_>>().len(),
            PALETTE.len()
        );
        assert_eq!(
            PALETTE_LIGHT.iter().copied().collect::<HashSet<_>>().len(),
            PALETTE_LIGHT.len()
        );
        assert_eq!(
            SCHEME_COLOR_ORDER
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .len(),
            PALETTE.len()
        );
        assert!(SCHEME_COLOR_ORDER
            .iter()
            .all(|index| usize::from(*index) < PALETTE.len()));
    }

    #[test]
    fn original_six_wire_indices_keep_their_existing_colors() {
        assert_eq!(
            &PALETTE[..6],
            &[0xff453a, 0xff9f0a, 0x30d158, 0x0a84ff, 0xbf5af2, 0xffd60a]
        );
        assert_eq!(
            &PALETTE_LIGHT[..6],
            &[0xb84433, 0xc47400, 0x28764f, 0x2563a6, 0x735aa6, 0xe0a800]
        );
    }

    #[test]
    fn every_u8_wire_value_maps_to_a_color_without_panicking() {
        for index in u8::MIN..=u8::MAX {
            let _ = scheme_color(index, false);
            let _ = scheme_color(index, true);
        }
    }
}
