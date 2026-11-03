//! Note colors.

use crate::config::PaletteKind;

/// Default part colors (sRGB), chosen to read well on a near-black background.
/// Index 0 / 1 double as right / left hand colors.
const EMBER: [u32; 8] = [
    0xFF7A2F, // ember orange
    0x38C6F4, // sky cyan
    0xE84393, // magenta
    0x9BE15D, // lime
    0x8E7CFF, // violet
    0xFFC93C, // gold
    0x2ED9B5, // teal
    0xFF5E62, // coral
];
const AURORA: [u32; 8] = [0x4FE3C1, 0x7A5CFF, 0x3FA7FF, 0xFF5FA2, 0xB8F25C, 0xFFD166, 0x00B4D8, 0xC77DFF];
const NEON: [u32; 8] = [0xFF2E88, 0x00E5FF, 0xFFE600, 0x39FF14, 0xB026FF, 0xFF6B00, 0x00FFA3, 0xFF4D4D];
const ICE: [u32; 8] = [0xDCE8FF, 0x7FB2FF, 0xA9C9E8, 0x9FA8FF, 0xC8F1FF, 0x6FD3E8, 0xE6D8FF, 0x8EC5FF];

/// Part colors converted to linear RGB once.
#[derive(Debug, Clone)]
pub struct Palette {
    colors: Vec<[f32; 3]>,
}

impl Palette {
    /// One of the built-in schemes.
    pub fn new(kind: PaletteKind) -> Self {
        let hexes = match kind {
            PaletteKind::Ember => &EMBER,
            PaletteKind::Aurora => &AURORA,
            PaletteKind::Neon => &NEON,
            PaletteKind::Ice => &ICE,
        };
        Self { colors: hexes.iter().map(|&hex| hex_to_linear(hex)).collect() }
    }

    /// Linear color of part `index`, cycling through the palette.
    pub fn color(&self, index: u16) -> [f32; 3] {
        // The palette is never empty: it is built from a non-empty constant.
        self.colors[usize::from(index) % self.colors.len()]
    }
}

fn hex_to_linear(hex: u32) -> [f32; 3] {
    let channel = |shift: u32| srgb_to_linear(((hex >> shift) & 0xFF) as f32 / 255.0);
    [channel(16), channel(8), channel(0)]
}

/// Converts one sRGB-encoded channel to linear light.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_endpoints_and_cycling() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb_to_linear(0.5) - 0.214).abs() < 1e-3);
        for kind in PaletteKind::ALL {
            let p = Palette::new(kind);
            assert_eq!(p.color(0), p.color(8));
        }
    }
}
