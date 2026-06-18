use crate::GlyphDescriptor;

/// Bitmap data for font_5px font (5px height).
/// Font is stored in row-major format (rotated from original column-major OLED format).
/// Each glyph's rows are stored sequentially, with bits packed left-to-right.
/// Total size: 350 bytes
pub const FONT_5PX_BITMAP: &[u8] = &[
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x00, 0x01, 0x05, 0x05, 0x00, 0x00, 0x00, 0x0A,
    0x1F, 0x0A, 0x1F, 0x0A, 0x1E, 0x05, 0x0E, 0x14, 0x0F, 0x01, 0x04, 0x02, 0x01, 0x04, 0x06, 0x01,
    0x0A, 0x05, 0x0A, 0x01, 0x01, 0x00, 0x00, 0x00, 0x02, 0x01, 0x01, 0x01, 0x02, 0x01, 0x02, 0x02,
    0x02, 0x01, 0x00, 0x05, 0x02, 0x05, 0x00, 0x00, 0x02, 0x07, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x01, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x04, 0x04, 0x02, 0x01, 0x01,
    0x06, 0x05, 0x05, 0x05, 0x03, 0x02, 0x03, 0x02, 0x02, 0x07, 0x03, 0x04, 0x02, 0x01, 0x07, 0x03,
    0x04, 0x03, 0x04, 0x03, 0x04, 0x05, 0x05, 0x07, 0x04, 0x07, 0x01, 0x07, 0x04, 0x03, 0x06, 0x01,
    0x07, 0x05, 0x07, 0x07, 0x04, 0x02, 0x01, 0x01, 0x06, 0x05, 0x02, 0x05, 0x03, 0x07, 0x05, 0x07,
    0x04, 0x03, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x00, 0x02, 0x01, 0x04, 0x02, 0x01, 0x02,
    0x04, 0x00, 0x07, 0x00, 0x07, 0x00, 0x01, 0x02, 0x04, 0x02, 0x01, 0x01, 0x02, 0x01, 0x00, 0x01,
    0x07, 0x08, 0x0B, 0x0B, 0x0D, 0x02, 0x05, 0x07, 0x05, 0x05, 0x03, 0x05, 0x03, 0x05, 0x03, 0x06,
    0x01, 0x01, 0x01, 0x06, 0x03, 0x05, 0x05, 0x05, 0x03, 0x07, 0x01, 0x07, 0x01, 0x07, 0x07, 0x01,
    0x07, 0x01, 0x01, 0x06, 0x01, 0x05, 0x05, 0x06, 0x05, 0x05, 0x07, 0x05, 0x05, 0x07, 0x02, 0x02,
    0x02, 0x07, 0x06, 0x04, 0x04, 0x05, 0x07, 0x01, 0x05, 0x03, 0x05, 0x05, 0x01, 0x01, 0x01, 0x01,
    0x07, 0x05, 0x07, 0x07, 0x05, 0x05, 0x03, 0x05, 0x05, 0x05, 0x05, 0x02, 0x05, 0x05, 0x05, 0x02,
    0x07, 0x05, 0x07, 0x01, 0x01, 0x02, 0x05, 0x05, 0x05, 0x06, 0x03, 0x05, 0x03, 0x05, 0x05, 0x06,
    0x01, 0x02, 0x04, 0x03, 0x07, 0x02, 0x02, 0x02, 0x02, 0x05, 0x05, 0x05, 0x05, 0x07, 0x05, 0x05,
    0x05, 0x05, 0x02, 0x05, 0x05, 0x07, 0x07, 0x05, 0x05, 0x05, 0x02, 0x05, 0x05, 0x05, 0x05, 0x02,
    0x02, 0x02, 0x07, 0x04, 0x02, 0x01, 0x07, 0x03, 0x01, 0x01, 0x01, 0x03, 0x01, 0x01, 0x02, 0x04,
    0x04, 0x03, 0x02, 0x02, 0x02, 0x03, 0x02, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07,
    0x01, 0x02, 0x00, 0x00, 0x00, 0x06, 0x02, 0x01, 0x02, 0x06, 0x01, 0x01, 0x01, 0x01, 0x01, 0x03,
    0x02, 0x04, 0x02, 0x03, 0x00, 0x0B, 0x0D, 0x00, 0x00, 0x01, 0x01, 0x07, 0x05, 0x03,
];

/// Glyph descriptors for font_5px font.
/// Maps character indices to their width and bitmap location.
pub const FONT_5PX_DESCRIPTORS: &[GlyphDescriptor] = &[
    GlyphDescriptor {
        w_px: 2,
        glyph_index: 0,
    },
    GlyphDescriptor {
        w_px: 1,
        glyph_index: 5,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 10,
    },
    GlyphDescriptor {
        w_px: 5,
        glyph_index: 15,
    },
    GlyphDescriptor {
        w_px: 5,
        glyph_index: 20,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 25,
    },
    GlyphDescriptor {
        w_px: 4,
        glyph_index: 30,
    },
    GlyphDescriptor {
        w_px: 1,
        glyph_index: 35,
    },
    GlyphDescriptor {
        w_px: 2,
        glyph_index: 40,
    },
    GlyphDescriptor {
        w_px: 2,
        glyph_index: 45,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 50,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 55,
    },
    GlyphDescriptor {
        w_px: 1,
        glyph_index: 60,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 65,
    },
    GlyphDescriptor {
        w_px: 1,
        glyph_index: 70,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 75,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 80,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 85,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 90,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 95,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 100,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 105,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 110,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 115,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 120,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 125,
    },
    GlyphDescriptor {
        w_px: 1,
        glyph_index: 130,
    },
    GlyphDescriptor {
        w_px: 2,
        glyph_index: 135,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 140,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 145,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 150,
    },
    GlyphDescriptor {
        w_px: 2,
        glyph_index: 155,
    },
    GlyphDescriptor {
        w_px: 4,
        glyph_index: 160,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 165,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 170,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 175,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 180,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 185,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 190,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 195,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 200,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 205,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 210,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 215,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 220,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 225,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 230,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 235,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 240,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 245,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 250,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 255,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 260,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 265,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 270,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 275,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 280,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 285,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 290,
    },
    GlyphDescriptor {
        w_px: 2,
        glyph_index: 295,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 300,
    },
    GlyphDescriptor {
        w_px: 2,
        glyph_index: 305,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 310,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 315,
    },
    GlyphDescriptor {
        w_px: 2,
        glyph_index: 320,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 325,
    },
    GlyphDescriptor {
        w_px: 1,
        glyph_index: 330,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 335,
    },
    GlyphDescriptor {
        w_px: 4,
        glyph_index: 340,
    },
    GlyphDescriptor {
        w_px: 3,
        glyph_index: 345,
    },
];

/// Height of the font_5px font in pixels.
pub const FONT_5PX_HEIGHT: u8 = 5;

/// Baseline offset for the font_5px font.
/// This is the distance from the top of the glyph to the baseline.
pub const FONT_5PX_BASELINE: u8 = 0;
