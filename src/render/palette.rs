//! Note colors.

/// Default part colors (sRGB), chosen to read well on a near-black background.
const DEFAULT_SRGB: [u32; 8] = [
    0xFF7A2F, // ember orange
    0x38C6F4, // sky cyan
    0xE84393, // magenta
    0x9BE15D, // lime
    0x8E7CFF, // violet
    0xFFC93C, // gold
    0x2ED9B5, // teal
    0xFF5E62, // coral
];

/// Part colors converted to linear RGB once.
#[derive(Debug, Clone)]
pub struct Palette {
    colors: Vec<[f32; 3]>,
}

impl Palette {
    /// The built-in palette.
    pub fn default_palette() -> Self {
        Self { colors: DEFAULT_SRGB.iter().map(|&hex| hex_to_linear(hex)).collect() }
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
        let p = Palette::default_palette();
        assert_eq!(p.color(0), p.color(8));
    }
}
