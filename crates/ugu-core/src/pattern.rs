// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 NewfolderGames

//! Repeating 1-bit pixel patterns aligned to the document grid `(0, 0)`.
//!
//! Brushes and fills use a pattern as a binary mask: `'1'` pixels paint with
//! the primary colour and `'0'` pixels leave what is already on the layer
//! untouched.

/// A repeating 1-bit tile up to 8×8 pixels.
///
/// Row `y` stores pixels `0..width` from the most significant bit (`bit 7`)
/// down. Unused bits on the right of each used row and rows `>= height` must
/// be zero so structural equality and serialization stay canonical.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pattern {
    pub width: u8,
    pub height: u8,
    pub rows: [u8; 8],
}

impl Pattern {
    pub const fn new(width: u8, height: u8, rows: [u8; 8]) -> Self {
        Self {
            width,
            height,
            rows,
        }
    }

    pub fn is_valid(self) -> bool {
        if !(1..=8).contains(&self.width) || !(1..=8).contains(&self.height) {
            return false;
        }
        let unused_bits = if self.width == 8 {
            0
        } else {
            0xffu8 >> self.width
        };
        let height = usize::from(self.height);
        self.rows[..height]
            .iter()
            .all(|&row| row & unused_bits == 0)
            && self.rows[height..].iter().all(|&row| row == 0)
    }

    /// Whether document pixel `(x, y)` is set in the repeating tile.
    #[inline]
    pub fn contains(self, x: i32, y: i32) -> bool {
        let px = x.rem_euclid(i32::from(self.width)) as u8;
        let py = y.rem_euclid(i32::from(self.height)) as usize;
        (self.rows[py] >> (7 - px)) & 1 != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternPreset {
    pub id: &'static str,
    pub pattern: Pattern,
}

const fn tile(id: &'static str, width: u8, height: u8, rows: [u8; 8]) -> PatternPreset {
    PatternPreset {
        id,
        pattern: Pattern::new(width, height, rows),
    }
}

pub const PATTERNS: [PatternPreset; 16] = [
    tile(
        "dither-12",
        4,
        4,
        [0b1000_0000, 0, 0b0010_0000, 0, 0, 0, 0, 0],
    ),
    tile(
        "dither-25",
        2,
        2,
        [0b1000_0000, 0b0000_0000, 0, 0, 0, 0, 0, 0],
    ),
    tile(
        "checker-50",
        2,
        2,
        [0b1000_0000, 0b0100_0000, 0, 0, 0, 0, 0, 0],
    ),
    tile(
        "dither-75",
        2,
        2,
        [0b1100_0000, 0b1000_0000, 0, 0, 0, 0, 0, 0],
    ),
    tile(
        "dither-87",
        4,
        4,
        [
            0b0111_0000,
            0b1111_0000,
            0b1101_0000,
            0b1111_0000,
            0,
            0,
            0,
            0,
        ],
    ),
    tile(
        "horizontal-2",
        1,
        2,
        [0b1000_0000, 0b0000_0000, 0, 0, 0, 0, 0, 0],
    ),
    tile("vertical-2", 2, 1, [0b1000_0000, 0, 0, 0, 0, 0, 0, 0]),
    tile(
        "diagonal-left",
        4,
        4,
        [
            0b0001_0000,
            0b0010_0000,
            0b0100_0000,
            0b1000_0000,
            0,
            0,
            0,
            0,
        ],
    ),
    tile(
        "diagonal-right",
        4,
        4,
        [
            0b1000_0000,
            0b0100_0000,
            0b0010_0000,
            0b0001_0000,
            0,
            0,
            0,
            0,
        ],
    ),
    tile(
        "crosshatch-4",
        4,
        4,
        [
            0b1111_0000,
            0b1000_0000,
            0b1000_0000,
            0b1000_0000,
            0,
            0,
            0,
            0,
        ],
    ),
    tile(
        "diagonal-cross",
        4,
        4,
        [
            0b1000_0000,
            0b0101_0000,
            0b0010_0000,
            0b0101_0000,
            0,
            0,
            0,
            0,
        ],
    ),
    tile("polka-4", 4, 4, [0b1000_0000, 0, 0, 0, 0, 0, 0, 0]),
    tile(
        "halftone-4",
        8,
        8,
        [
            0b1100_0000,
            0b1100_0000,
            0b0000_0000,
            0b0000_0000,
            0b0000_1100,
            0b0000_1100,
            0b0000_0000,
            0b0000_0000,
        ],
    ),
    tile(
        "brick-8",
        8,
        8,
        [
            0b1111_1111,
            0b1000_0000,
            0b1000_0000,
            0b1000_0000,
            0b1111_1111,
            0b0000_1000,
            0b0000_1000,
            0b0000_1000,
        ],
    ),
    tile(
        "weave-4",
        4,
        4,
        [
            0b1101_0000,
            0b0001_0000,
            0b0111_0000,
            0b0100_0000,
            0,
            0,
            0,
            0,
        ],
    ),
    tile(
        "stipple-8",
        8,
        8,
        [
            0b1000_0000,
            0b0000_1000,
            0b0010_0000,
            0b0000_0001,
            0b0100_0000,
            0b0001_0000,
            0b0000_0010,
            0b0000_0100,
        ],
    ),
];

pub fn find(id: &str) -> Option<&'static PatternPreset> {
    PATTERNS.iter().find(|preset| preset.id == id)
}

pub fn name_of(pattern: Pattern) -> Option<&'static str> {
    PATTERNS
        .iter()
        .find(|preset| preset.pattern == pattern)
        .map(|preset| preset.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_valid_and_named_once() {
        let mut ids: Vec<_> = PATTERNS.iter().map(|preset| preset.id).collect();
        for preset in PATTERNS {
            assert!(preset.pattern.is_valid(), "{}", preset.id);
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PATTERNS.len());
    }

    #[test]
    fn pattern_wraps_across_negative_and_positive_coordinates() {
        let checker = find("checker-50").unwrap().pattern;
        for y in -4..6 {
            for x in -4..6 {
                assert_eq!(checker.contains(x, y), (x + y).rem_euclid(2) == 0);
            }
        }
    }

    #[test]
    fn pattern_validation_rejects_dirty_unused_bits_or_bad_size() {
        assert!(!Pattern::new(0, 2, [0; 8]).is_valid());
        assert!(!Pattern::new(9, 2, [0; 8]).is_valid());
        assert!(!Pattern::new(2, 2, [0b0010_0000, 0, 0, 0, 0, 0, 0, 0]).is_valid());
        assert!(!Pattern::new(2, 2, [0b1000_0000, 0, 0b1000_0000, 0, 0, 0, 0, 0]).is_valid());
    }
}
