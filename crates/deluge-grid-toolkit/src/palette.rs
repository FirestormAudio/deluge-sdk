//! Colour palette definitions for the Deluge grid interface.
//!
//! - Standard colours with Deluge-specific modifications
//! - Pastel colours for softer UI elements
//! - Kelly's twenty-two colours of maximum contrast
//! - WAD colour palette for accessible colour selection

use crate::color::ColorExt as _;
#[allow(unused_imports)] // needed on targets whose `core` lacks inherent f32 math
use crate::float_ext::F32Ext as _;
use deluge_bsp::rgb::Color;

/// Type alias for colour (matches C++ naming).
pub type Colour = Color;

// Standard palette, with some modifications for the deluge.
pub const BLACK: Colour = Color::monochrome(0);
pub const GREY: Colour = Color::monochrome(7);
pub const WHITE_FULL: Colour = Color::monochrome(255);
pub const WHITE: Colour = WHITE_FULL.dim(1);
pub const RED: Colour = Color::rgb(255, 0, 0);
pub const RED_DULL: Colour = Color::rgb(60, 15, 15);
pub const RED_ORANGE: Colour = Color::rgb(255, 64, 0);
pub const ORANGE: Colour = Color::rgb(255, 128, 0);
pub const YELLOW_ORANGE: Colour = Color::rgb(255, 160, 0);
pub const YELLOW: Colour = Color::rgb(255, 255, 0);
pub const LIME: Colour = Color::rgb(128, 255, 0);
pub const GREEN: Colour = Color::rgb(0, 255, 0);
pub const TURQUOISE: Colour = Color::rgb(0, 255, 128);
pub const CYAN_FULL: Colour = Color::rgb(0, 255, 255);
pub const CYAN: Colour = Color::rgb(0, 128, 128);
pub const DARKBLUE: Colour = Color::rgb(0, 128, 255);
pub const BLUE: Colour = Color::rgb(0, 0, 255);
pub const PURPLE: Colour = Color::rgb(128, 0, 255);
pub const MAGENTA_FULL: Colour = Color::rgb(255, 0, 255);
pub const MAGENTA: Colour = Color::rgb(128, 0, 128);
pub const MAGENTA_DULL: Colour = Color::rgb(60, 15, 60);
pub const PINK_FULL: Colour = Color::rgb(255, 128, 128);
pub const PINK: Colour = Color::rgb(255, 44, 50);
pub const AMBER: Colour = Color::rgb(255, 48, 0);

// These colours are used globally by the deluge.
pub const DISABLED: Colour = RED;
pub const GROUP_ENABLED: Colour = GREEN;
pub const ENABLED: Colour = Color::rgb(0, 255, 6);
pub const MUTED: Colour = YELLOW_ORANGE;
pub const MIDI_COMMAND: Colour = Color::rgb(255, 80, 120);
pub const MIDI_NO_COMMAND: Colour = Color::monochrome(60);
pub const SELECTED_DRUM: Colour = Color::rgb(30, 30, 10);

/// Cycle the hue of a colour by `delta` steps (each step ≈ 15°).
///
/// Achromatic colours (greys) are left unchanged.
pub fn cycle_hue(color: Color, delta: i8) -> Color {
    let (h, s, v) = color.to_hsv();
    if s < 0.01 {
        return color;
    }
    let step = 15.0;
    let new_hue = (h + delta as f32 * step).rem_euclid(360.0);
    Color::from_hsv(new_hue, s, v)
}

/// Section colours for session view (matches Deluge's `defaultClipSectionColours`).
pub mod section {
    use deluge_bsp::rgb::Color;

    /// All 24 default section colours in order.
    pub const DEFAULT_COLORS: [Color; 24] = [
        Color::rgb(0, 90, 165),
        Color::rgb(176, 0, 79),
        Color::rgb(176, 79, 0),
        Color::rgb(0, 199, 56),
        Color::rgb(255, 0, 0),
        Color::rgb(128, 255, 0),
        Color::rgb(0, 0, 255),
        Color::rgb(234, 21, 0),
        Color::rgb(51, 0, 204),
        Color::rgb(255, 255, 0),
        Color::rgb(0, 255, 0),
        Color::rgb(109, 0, 146),
        Color::rgb(51, 109, 145),
        Color::rgb(255, 128, 128),
        Color::rgb(221, 72, 13),
        Color::rgb(85, 182, 72),
        Color::rgb(41, 9, 10),
        Color::rgb(22, 42, 2),
        Color::rgb(1, 21, 21),
        Color::rgb(42, 22, 2),
        Color::rgb(22, 2, 42),
        Color::rgb(28, 30, 2),
        Color::rgb(1, 41, 1),
        Color::rgb(21, 1, 21),
    ];

    /// Get section colour by index (wraps around for indices ≥ 24).
    pub const fn get_color(section: u8) -> Color {
        DEFAULT_COLORS[(section as usize) % DEFAULT_COLORS.len()]
    }
}

/// Pastel colours for softer UI elements.
pub mod pastel {
    use deluge_bsp::rgb::Color;

    pub const ORANGE: Color = Color::rgb(221, 72, 13);
    pub const YELLOW: Color = Color::rgb(170, 182, 0);
    pub const GREEN: Color = Color::rgb(85, 182, 72);
    pub const BLUE: Color = Color::rgb(51, 109, 145);
    pub const PINK: Color = Color::rgb(144, 72, 91);

    pub const ORANGE_TAIL: Color = Color::rgb(46, 16, 2);
    pub const PINK_TAIL: Color = Color::rgb(37, 15, 37);
}

/// Twenty-two Colours of Maximum Contrast by Kelly.
///
/// Reference: <http://www.iscc-archive.org/pdf/PC54_1724_001.pdf>
pub mod kelly {
    use deluge_bsp::rgb::Color;

    pub const VIVID_YELLOW: Color = Color::rgb(255, 179, 0);
    pub const STRONG_PURPLE: Color = Color::rgb(128, 62, 117);
    pub const VIVID_ORANGE: Color = Color::rgb(255, 104, 0);
    pub const VERY_LIGHT_BLUE: Color = Color::rgb(166, 189, 215);
    pub const VIVID_RED: Color = Color::rgb(193, 0, 32);
    pub const GRAYISH_YELLOW: Color = Color::rgb(206, 162, 98);
    pub const MEDIUM_GRAY: Color = Color::rgb(129, 112, 102);

    pub const VIVID_GREEN: Color = Color::rgb(0, 125, 52);
    pub const STRONG_PURPLISH_PINK: Color = Color::rgb(246, 118, 142);
    pub const STRONG_BLUE: Color = Color::rgb(0, 83, 138);
    pub const STRONG_YELLOWISH_PINK: Color = Color::rgb(255, 122, 92);
    pub const STRONG_VIOLET: Color = Color::rgb(83, 55, 122);
    pub const VIVID_ORANGE_YELLOW: Color = Color::rgb(255, 142, 0);
    pub const STRONG_PURPLISH_RED: Color = Color::rgb(179, 40, 81);
    pub const VIVID_GREENISH_YELLOW: Color = Color::rgb(244, 200, 0);
    pub const STRONG_REDDISH_BROWN: Color = Color::rgb(127, 24, 13);
    pub const VIVID_YELLOWISH_GREEN: Color = Color::rgb(147, 170, 0);
    pub const DEEP_YELLOWISH_BROWN: Color = Color::rgb(89, 51, 21);
    pub const VIVID_REDDISH_ORANGE: Color = Color::rgb(241, 58, 19);
    pub const DARK_OLIVE_GREEN: Color = Color::rgb(35, 44, 22);
}

/// WAD colour palette, designed for accessibility.
///
/// Reference: <https://alumni.media.mit.edu/~wad/color/palette.html>
pub mod wad {
    use deluge_bsp::rgb::Color;

    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const DARK_GRAY: Color = Color::rgb(87, 87, 87);
    pub const RED: Color = Color::rgb(173, 35, 35);
    pub const BLUE: Color = Color::rgb(42, 75, 215);
    pub const GREEN: Color = Color::rgb(29, 105, 20);
    pub const BROWN: Color = Color::rgb(129, 74, 25);
    pub const PURPLE: Color = Color::rgb(129, 38, 192);
    pub const LIGHT_GRAY: Color = Color::rgb(160, 160, 160);
    pub const LIGHT_GREEN: Color = Color::rgb(129, 197, 122);
    pub const LIGHT_BLUE: Color = Color::rgb(157, 175, 255);
    pub const CYAN: Color = Color::rgb(41, 208, 208);
    pub const ORANGE: Color = Color::rgb(255, 146, 51);
    pub const YELLOW: Color = Color::rgb(255, 238, 51);
    pub const TAN: Color = Color::rgb(233, 222, 187);
    pub const PINK: Color = Color::rgb(255, 205, 243);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_colors() {
        assert_eq!(BLACK, Color::BLACK);
        assert_eq!(WHITE_FULL, Color::WHITE);
        assert_eq!(RED, Color::RED);
        assert_eq!(GREEN, Color::GREEN);
        assert_eq!(BLUE, Color::BLUE);
    }

    #[test]
    fn test_white_dimmed() {
        assert_eq!(WHITE, WHITE_FULL.dim(1));
    }

    #[test]
    fn test_global_colors() {
        assert_eq!(DISABLED, RED);
        assert_eq!(GROUP_ENABLED, GREEN);
        assert_eq!(MUTED, YELLOW_ORANGE);
    }
}
