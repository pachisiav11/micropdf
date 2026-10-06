//! Reading modes: recolour rendered pages so white paper becomes the chosen paper colour and
//! black ink the chosen ink colour. Applied to tiles on the render workers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReadingMode {
    #[default]
    Normal,
    /// Recto's dark sheet: paper #1d2027, ink #e3e7ef.
    Dark,
    Sepia,
    Invert,
}

impl ReadingMode {
    pub fn index(self) -> i32 {
        match self {
            ReadingMode::Normal => 0,
            ReadingMode::Dark => 1,
            ReadingMode::Sepia => 2,
            ReadingMode::Invert => 3,
        }
    }

    /// Paper colour behind and around the rendered tiles.
    pub fn paper(self) -> [u8; 3] {
        match self {
            ReadingMode::Normal => [255, 255, 255],
            ReadingMode::Dark => [0x1d, 0x20, 0x27],
            ReadingMode::Sepia => [0xf4, 0xec, 0xd8],
            ReadingMode::Invert => [0, 0, 0],
        }
    }

    fn ink(self) -> [u8; 3] {
        match self {
            ReadingMode::Normal => [0, 0, 0],
            ReadingMode::Dark => [0xe3, 0xe7, 0xef],
            ReadingMode::Sepia => [0x5b, 0x46, 0x36],
            ReadingMode::Invert => [255, 255, 255],
        }
    }

    /// Recolours packed RGB pixels in place.
    pub fn apply(self, rgb: &mut [u8]) {
        if self == ReadingMode::Normal {
            return;
        }
        // Per channel: 255 (paper) -> paper colour, 0 (ink) -> ink colour, linear in between.
        let (paper, ink) = (self.paper(), self.ink());
        let tables: [[u8; 256]; 3] = std::array::from_fn(|c| {
            std::array::from_fn(|v| {
                let t = v as f32 / 255.0;
                (ink[c] as f32 + (paper[c] as f32 - ink[c] as f32) * t).round() as u8
            })
        });
        for px in rgb.as_chunks_mut::<3>().0 {
            for c in 0..3 {
                px[c] = tables[c][px[c] as usize];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_paper_and_ink() {
        let mut px = [255, 255, 255, 0, 0, 0];
        ReadingMode::Dark.apply(&mut px);
        assert_eq!(px, [0x1d, 0x20, 0x27, 0xe3, 0xe7, 0xef]);

        let mut px = [255, 255, 255, 0, 0, 0];
        ReadingMode::Invert.apply(&mut px);
        assert_eq!(px, [0, 0, 0, 255, 255, 255]);

        let mut px = [10, 20, 30];
        ReadingMode::Normal.apply(&mut px);
        assert_eq!(px, [10, 20, 30]);
    }
}
