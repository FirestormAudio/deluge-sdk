//! Icon loading from embedded BMP files

/// Parse a monochrome BMP file and extract icon data
/// 
/// This is a const function that can be used at compile time
pub const fn parse_bmp_icon(bmp_data: &[u8]) -> IconData {
    // BMP header parsing
    // File header is 14 bytes
    // Info header is 40 bytes (BITMAPINFOHEADER)
    // Color palette is 8 bytes (2 colors × 4 bytes)
    // Pixel data starts at offset 62 (14 + 40 + 8)
    
    // Extract width and height from info header
    let width = u32::from_le_bytes([
        bmp_data[18], bmp_data[19], bmp_data[20], bmp_data[21]
    ]) as u8;
    
    let height_i32 = i32::from_le_bytes([
        bmp_data[22], bmp_data[23], bmp_data[24], bmp_data[25]
    ]);
    let height = height_i32.abs() as u8;
    
    // For now, just return the raw BMP data
    // The actual pixel extraction will be done at runtime
    IconData {
        data: bmp_data,
        width,
        height,
        format: IconFormat::Bmp,
    }
}

/// Icon data format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconFormat {
    /// Column-major format (Deluge firmware format)
    ColumnMajor { bytes_per_column: u8 },
    /// Embedded BMP file
    Bmp,
}

/// Icon data structure
#[derive(Debug, Clone, Copy)]
pub struct IconData {
    pub data: &'static [u8],
    pub width: u8,
    pub height: u8,
    pub format: IconFormat,
}

impl IconData {
    /// Create a new 7px icon (1 byte per column, column-major format)
    pub const fn new(data: &'static [u8], width: u8) -> Self {
        Self {
            data,
            width,
            height: 7,
            format: IconFormat::ColumnMajor { bytes_per_column: 1 },
        }
    }

    /// Create a new 16px icon (2 bytes per column, column-major format)
    pub const fn new_tall(data: &'static [u8], width: u8) -> Self {
        Self {
            data,
            width,
            height: 16,
            format: IconFormat::ColumnMajor { bytes_per_column: 2 },
        }
    }
    
    /// Create an icon from embedded BMP data
    pub const fn from_bmp(bmp_data: &'static [u8]) -> Self {
        parse_bmp_icon(bmp_data)
    }
}

/// Macro to embed an icon from a BMP file
#[macro_export]
macro_rules! icon_bmp {
    ($path:expr) => {
        $crate::icons::IconData::from_bmp(include_bytes!($path))
    };
}

/// Helper to get pixel from BMP data
pub fn get_bmp_pixel(icon: &IconData, x: u8, y: u8) -> bool {
    if icon.format != IconFormat::Bmp {
        return false;
    }
    
    let bmp_data = icon.data;
    
    // Calculate row size (padded to 4-byte boundary)
    let row_size_bytes = (icon.width as usize + 7) / 8;
    let row_size_padded = ((row_size_bytes + 3) / 4) * 4;
    
    // Pixel data starts at offset 62 (14 + 40 + 8)
    let pixel_data_offset = 62;
    
    // BMP stores rows from bottom to top
    let row_from_bottom = (icon.height - 1 - y) as usize;
    let row_offset = pixel_data_offset + row_from_bottom * row_size_padded;
    
    let byte_idx = x as usize / 8;
    let bit_idx = 7 - (x as usize % 8); // MSB first
    
    if row_offset + byte_idx < bmp_data.len() {
        (bmp_data[row_offset + byte_idx] & (1 << bit_idx)) != 0
    } else {
        false
    }
}
