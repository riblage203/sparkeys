//! 88-key piano geometry shared by the keyboard and the note lanes.

/// Lowest key on the piano (A0).
pub const FIRST_KEY: u8 = 21;
/// Highest key on the piano (C8).
pub const LAST_KEY: u8 = 108;
/// Number of keys.
pub const KEY_COUNT: usize = (LAST_KEY - FIRST_KEY + 1) as usize;
/// Number of white keys on an 88-key piano.
pub const WHITE_KEY_COUNT: usize = 52;

/// Black key width relative to a white key.
const BLACK_WIDTH: f32 = 0.58;
/// Black key length relative to a white key.
pub const BLACK_LENGTH: f32 = 0.63;

/// Whether `pitch` is a black key.
pub fn is_black(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

/// Horizontal placement of one key, in white-key units (0..52).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyGeom {
    /// Left edge.
    pub x: f32,
    /// Width.
    pub width: f32,
    /// Whether this is a black key.
    pub black: bool,
}

/// Geometry of all 88 keys.
#[derive(Debug, Clone)]
pub struct KeyboardLayout {
    keys: [KeyGeom; KEY_COUNT],
}

impl KeyboardLayout {
    /// Builds the layout with realistic black-key offsets: on a real piano the
    /// black keys of the C#/D# and F#/G#/A# groups are pushed apart, not centered
    /// on the white key boundaries.
    pub fn new() -> Self {
        let mut keys = [KeyGeom { x: 0.0, width: 1.0, black: false }; KEY_COUNT];
        let mut white_index = 0.0f32;
        for (i, key) in keys.iter_mut().enumerate() {
            // i < 88, so the cast fits.
            let pitch = FIRST_KEY + i as u8;
            if is_black(pitch) {
                let offset = match pitch % 12 {
                    1 => -0.10,
                    3 => 0.10,
                    6 => -0.12,
                    10 => 0.12,
                    _ => 0.0,
                };
                *key = KeyGeom { x: white_index - BLACK_WIDTH / 2.0 + offset, width: BLACK_WIDTH, black: true };
            } else {
                *key = KeyGeom { x: white_index, width: 1.0, black: false };
                white_index += 1.0;
            }
        }
        Self { keys }
    }

    /// Geometry of `pitch`, or `None` outside A0..=C8.
    pub fn key(&self, pitch: u8) -> Option<KeyGeom> {
        let idx = pitch.checked_sub(FIRST_KEY)?;
        self.keys.get(usize::from(idx)).copied()
    }

    /// All keys from A0 upward, with their pitch.
    pub fn iter(&self) -> impl Iterator<Item = (u8, KeyGeom)> + '_ {
        // KEY_COUNT < 256, so the cast fits.
        self.keys.iter().enumerate().map(|(i, k)| (FIRST_KEY + i as u8, *k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_extent() {
        let layout = KeyboardLayout::new();
        assert_eq!(layout.iter().filter(|(_, k)| !k.black).count(), WHITE_KEY_COUNT);
        assert_eq!(layout.iter().filter(|(_, k)| k.black).count(), 36);
        let a0 = layout.key(21).unwrap();
        let c8 = layout.key(108).unwrap();
        assert_eq!(a0.x, 0.0);
        assert_eq!(c8.x + c8.width, WHITE_KEY_COUNT as f32);
        assert!(layout.key(20).is_none() && layout.key(109).is_none());
    }

    #[test]
    fn black_keys_do_not_overlap_each_other() {
        let layout = KeyboardLayout::new();
        let blacks: Vec<KeyGeom> = layout.iter().map(|(_, k)| k).filter(|k| k.black).collect();
        for pair in blacks.windows(2) {
            assert!(pair[0].x + pair[0].width < pair[1].x);
        }
    }
}
