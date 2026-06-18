//! Const font definitions for Deluge variable-width fonts.
//!
//! This module provides const instances of [`VariFont`] for each font
//! available in the deluge-fonts crate, following the pattern
//! used by embedded-graphics' MonoFont constants.

use super::VariFont;
use deluge_fonts::Font as DelugeFont;

/// 5px variable-width font (U+0020 to U+005A).
///
/// Original 5px font from the Deluge firmware. This is the smallest font available.
pub const FONT_5PX: VariFont = VariFont {
    font: DelugeFont::Font5px,
    character_spacing: 1,
    baseline: 4,
};

/// Apple II 7px variable-width font (U+0020 to U+005A).
///
/// Classic Apple II style font from the Deluge firmware.
pub const FONT_APPLE: VariFont = VariFont {
    font: DelugeFont::FontApple,
    character_spacing: 1,
    baseline: 5,
};

/// Metric Bold 9px variable-width font (U+0020 to U+007E).
///
/// Professional Metric Bold font at 9px height. This is the default font
/// used throughout the Deluge UI toolkit.
///
/// **Note**: The Metric font is proprietary and licensed to Synthstrom Audible Limited
/// from Klim Type Foundry. It is not free to use in other projects.
pub const FONT_METRIC_BOLD_9PX: VariFont = VariFont {
    font: DelugeFont::MetricBold9px,
    character_spacing: 1,
    baseline: 7,
};

/// Metric Bold 13px variable-width font (U+0020 to U+007E).
///
/// Professional Metric Bold font at 13px height. This is a medium-sized font
/// suitable for headers and prominent UI elements.
///
/// **Note**: The Metric font is proprietary and licensed to Synthstrom Audible Limited
/// from Klim Type Foundry. It is not free to use in other projects.
pub const FONT_METRIC_BOLD_13PX: VariFont = VariFont {
    font: DelugeFont::MetricBold13px,
    character_spacing: 1,
    baseline: 10,
};

/// Metric Bold 20px variable-width font (U+0020 to U+007F).
///
/// Professional Metric Bold font at 20px height. This is the largest font available,
/// suitable for titles and very prominent UI elements.
///
/// **Note**: The Metric font is proprietary and licensed to Synthstrom Audible Limited
/// from Klim Type Foundry. It is not free to use in other projects.
pub const FONT_METRIC_BOLD_20PX: VariFont = VariFont {
    font: DelugeFont::MetricBold20px,
    character_spacing: 1,
    baseline: 16,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_heights() {
        assert_eq!(FONT_5PX.height(), 5);
        assert_eq!(FONT_APPLE.height(), 7);
        assert_eq!(FONT_METRIC_BOLD_9PX.height(), 9);
        assert_eq!(FONT_METRIC_BOLD_13PX.height(), 13);
        assert_eq!(FONT_METRIC_BOLD_20PX.height(), 20);
    }

    #[test]
    fn test_font_baselines() {
        assert_eq!(FONT_5PX.baseline, 4);
        assert_eq!(FONT_APPLE.baseline, 5);
        assert_eq!(FONT_METRIC_BOLD_9PX.baseline, 7);
        assert_eq!(FONT_METRIC_BOLD_13PX.baseline, 10);
        assert_eq!(FONT_METRIC_BOLD_20PX.baseline, 16);
    }

    #[test]
    fn test_default_spacing() {
        // All fonts should have 1px default spacing
        assert_eq!(FONT_5PX.character_spacing, 1);
        assert_eq!(FONT_APPLE.character_spacing, 1);
        assert_eq!(FONT_METRIC_BOLD_9PX.character_spacing, 1);
        assert_eq!(FONT_METRIC_BOLD_13PX.character_spacing, 1);
        assert_eq!(FONT_METRIC_BOLD_20PX.character_spacing, 1);
    }

    #[test]
    fn test_text_width() {
        // Test that text_width works correctly
        let width = FONT_METRIC_BOLD_9PX.text_width("A");
        assert!(width > 0);

        // Test with multiple characters
        let width2 = FONT_METRIC_BOLD_9PX.text_width("AB");
        assert!(width2 > width);
    }
}
