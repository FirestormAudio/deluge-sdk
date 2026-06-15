//! Seven-segment display emulation for OLED
//!
//! This module provides rendering of the Deluge's 7-segment LED display
//! as graphics on an OLED screen, matching the hardware appearance.

#[cfg(feature = "embedded-graphics")]
use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};

/// Seven-segment display patterns for digits 0-9
///
/// Bit layout:
/// ```text
///  -1-
/// |   |
/// 6   2
/// |   |
///  -7-
/// |   |
/// 5   3
/// |   |
///  -4-  .0
/// ```
pub const NUMBER_SEGMENTS: [u8; 10] = [
    0x7E, // 0
    0x30, // 1
    0x6D, // 2
    0x79, // 3
    0x33, // 4
    0x5B, // 5
    0x5F, // 6
    0x70, // 7
    0x7F, // 8
    0x7B, // 9
];

/// Seven-segment display patterns for letters A-Z (uppercase and lowercase)
///
/// Note: Not all letters display well on 7-segment displays.
/// Index 0 = 'A', index 25 = 'Z', then some symbols, then lowercase a-z
pub const LETTER_SEGMENTS: [u8; 58] = [
    0x77, // A
    0x1F, // B
    0x4E, // C
    0x3D, // D
    0x4F, // E
    0x47, // F
    0x5E, // G
    0x37, // H
    0x04, // I
    0x38, // J
    0x57, // K
    0x0E, // L
    0x55, // M
    0x15, // N
    0x1D, // O
    0x67, // P
    0x73, // Q
    0x05, // R
    0x5B, // S
    0x0F, // T
    0x3E, // U
    0x27, // V
    0x5C, // W
    0x49, // X
    0x3B, // Y
    0x6D, // Z
    0x00, // [
    0x00, // backslash
    0x00, // ]
    0x00, // ^
    0x00, // _
    0x00, // `
    // Lowercase
    0x77, // a
    0x1F, // b
    0x0D, // c
    0x3D, // d
    0x4F, // e
    0x47, // f
    0x5E, // g
    0x37, // h (same as H)
    0x04, // i
    0x38, // j
    0x57, // k
    0x0E, // l
    0x55, // m
    0x15, // n
    0x1D, // o
    0x67, // p
    0x73, // q
    0x05, // r
    0x5B, // s
    0x0F, // t
    0x3E, // u
    0x27, // v
    0x5C, // w
    0x49, // x
    0x3B, // y
    0x6D, // z
];

/// Get the 7-segment pattern for a character
pub fn get_segment_pattern(ch: char) -> u8 {
    match ch {
        '0'..='9' => NUMBER_SEGMENTS[(ch as u8 - b'0') as usize],
        'A'..='Z' => LETTER_SEGMENTS[(ch as u8 - b'A') as usize],
        'a'..='z' => LETTER_SEGMENTS[32 + (ch as u8 - b'a') as usize],
        _ => 0x00, // Blank for unsupported characters
    }
}

/// Render a single 7-segment digit at the specified position
///
/// This matches the Deluge firmware's rendering style with proper segment positioning.
#[cfg(feature = "embedded-graphics")]
pub fn render_digit<D>(target: &mut D, pattern: u8, x: i32, y: i32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    const DY: i32 = 17; // Vertical spacing between segment groups

    // Horizontal segments (top, middle, bottom)
    let horz = [6, 0, 3]; // Bit positions for horizontal segments
    for (seg_y, &bit_pos) in horz.iter().enumerate() {
        if (pattern & (1 << bit_pos)) != 0 {
            let ybase = y + 7 + DY * seg_y as i32;
            // Draw horizontal segment (3 rows)
            draw_horizontal_segment(target, x + 3, ybase, 15)?;
        }
    }

    // Vertical segments (left and right sides, top and bottom halves)
    let vert = [1, 2, 5, 4]; // Bit positions for vertical segments
    for seg_x in 0..2 {
        let xside = seg_x as i32 * 2 - 1;
        for seg_y in 0..2 {
            if (pattern & (1 << vert[2 * seg_x + seg_y])) != 0 {
                let xbase = x + 18 * seg_x as i32 + 1;
                let ybase = y + 10 + DY * seg_y as i32;
                let yside = seg_y as i32 * -2 + 1;
                draw_vertical_segment(target, xbase, ybase, xside, yside)?;
            }
        }
    }

    // Decimal point (bit 7)
    if (pattern & (1 << 7)) != 0 {
        draw_decimal_point(target, x + 21, y + 41)?;
    }

    Ok(())
}

#[cfg(feature = "embedded-graphics")]
fn draw_horizontal_segment<D>(target: &mut D, x: i32, y: i32, width: i32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

    // Top edge
    Rectangle::new(Point::new(x, y), Size::new(width as u32, 1))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target)?;

    // Middle (thicker)
    Rectangle::new(Point::new(x - 1, y + 1), Size::new((width + 2) as u32, 1))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target)?;

    // Bottom edge
    Rectangle::new(Point::new(x, y + 2), Size::new(width as u32, 1))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target)?;

    Ok(())
}

#[cfg(feature = "embedded-graphics")]
fn draw_vertical_segment<D>(
    target: &mut D,
    x: i32,
    y: i32,
    xside: i32,
    yside: i32,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

    // Three columns of varying heights to create angled ends
    Rectangle::new(Point::new(x + xside, y + yside), Size::new(1, 14))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target)?;

    Rectangle::new(Point::new(x, y), Size::new(1, 14))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target)?;

    Rectangle::new(Point::new(x - xside, y + 1), Size::new(1, 12))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target)?;

    Ok(())
}

#[cfg(feature = "embedded-graphics")]
fn draw_decimal_point<D>(target: &mut D, x: i32, y: i32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

    Rectangle::new(Point::new(x, y), Size::new(3, 3))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target)?;

    Ok(())
}

/// Render a 4-digit 7-segment display (matching Deluge's numeric display)
///
/// # Arguments
/// * `target` - The display to draw on
/// * `text` - String to display (e.g., "1234", "LOAD", "P.A.S.S.")
///   - Supports up to 4 digit positions
///   - Period (.) adds decimal point to previous digit without taking a position
/// * `x` - Starting X position (default: 1 for full 128px width display)
/// * `y` - Starting Y position (default: 0)
#[cfg(feature = "embedded-graphics")]
pub fn render_display<D>(target: &mut D, text: &str, x: i32, y: i32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let mut digits = [0u8; 4];
    let mut digit_idx = 0;

    for ch in text.chars() {
        if ch == '.' {
            // Add decimal point to previous digit if there is one
            if digit_idx > 0 {
                digits[digit_idx - 1] |= 0x80; // Set bit 7 (decimal point)
            }
        } else {
            // Regular character
            if digit_idx < 4 {
                digits[digit_idx] = get_segment_pattern(ch);
                digit_idx += 1;
            }
        }
    }

    for (i, &pattern) in digits.iter().enumerate() {
        let digit_x = x + 33 * i as i32;
        render_digit(target, pattern, digit_x, y)?;
    }
    Ok(())
}
