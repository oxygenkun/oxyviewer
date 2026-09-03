//! BMP parser (B1-B3).

use crate::core::{Error, Reader, Result};

/// Parsed BMP file metadata.
#[derive(Debug)]
pub struct BmpInfo {
    /// File size from BMP header.
    pub file_size: u32,
    /// Offset to pixel data.
    pub data_offset: u32,
    /// DIB header version.
    pub dib_version: DibVersion,
    /// Image width in pixels.
    pub width: i32,
    /// Image height in pixels (negative = top-down).
    pub height: i32,
    /// Color planes (always 1).
    pub planes: u16,
    /// Bits per pixel.
    pub bpp: u16,
    /// Compression method.
    pub compression: u32,
    /// Image data size.
    pub image_size: u32,
    /// Horizontal resolution (pixels per meter).
    pub x_ppm: i32,
    /// Vertical resolution (pixels per meter).
    pub y_ppm: i32,
    /// Number of colors in palette.
    pub colors_used: u32,
    /// Number of important colors.
    pub colors_important: u32,
    /// ICC profile data offset and size (B3, V5 only).
    pub icc_profile: Option<(u32, u32)>,
}

/// DIB header version (B2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DibVersion {
    /// BITMAPCOREHEADER (12 bytes) - OS/2 1.x
    Core,
    /// BITMAPINFOHEADER (40 bytes) - Windows 3.x
    Info,
    /// BITMAPV2INFOHEADER (52 bytes)
    V2,
    /// BITMAPV3INFOHEADER (56 bytes)
    V3,
    /// BITMAPV4HEADER (108 bytes)
    V4,
    /// BITMAPV5HEADER (124 bytes)
    V5,
}

/// Parse a BMP file (B1-B3).
pub fn parse_bmp(data: &[u8]) -> Result<BmpInfo> {
    let mut reader = Reader::new(data);

    // B1: File header - "BM" + file size + reserved + data offset
    let magic = reader.read_bytes(2)?;
    if magic != b"BM" {
        return Err(Error::Format("not a BMP: missing BM signature".into()));
    }

    let file_size = reader.read_u32_le()?;
    reader.skip(4)?; // reserved
    let data_offset = reader.read_u32_le()?;

    // B2: DIB header
    let dib_size = reader.read_u32_le()?;
    let dib_version = match dib_size {
        12 => DibVersion::Core,
        40 => DibVersion::Info,
        52 => DibVersion::V2,
        56 => DibVersion::V3,
        108 => DibVersion::V4,
        124 => DibVersion::V5,
        _ => {
            return Err(Error::Unsupported(format!(
                "unknown DIB header size: {dib_size}"
            )));
        }
    };

    let (width, height, planes, bpp) = if dib_version == DibVersion::Core {
        // BITMAPCOREHEADER: width(2) + height(2) + planes(2) + bpp(2)
        let w = reader.read_u16_le()? as i32;
        let h = reader.read_u16_le()? as i32;
        let p = reader.read_u16_le()?;
        let b = reader.read_u16_le()?;
        (w, h, p, b)
    } else {
        // BITMAPINFOHEADER and later: width(4) + height(4) + planes(2) + bpp(2)
        let w = reader.read_i32_le()?;
        let h = reader.read_i32_le()?;
        let p = reader.read_u16_le()?;
        let b = reader.read_u16_le()?;
        (w, h, p, b)
    };

    let (compression, image_size, x_ppm, y_ppm, colors_used, colors_important) =
        if dib_version == DibVersion::Core {
            (0, 0, 0, 0, 0, 0)
        } else {
            let comp = reader.read_u32_le()?;
            let isz = reader.read_u32_le()?;
            let xp = reader.read_i32_le()?;
            let yp = reader.read_i32_le()?;
            let cu = reader.read_u32_le()?;
            let ci = reader.read_u32_le()?;
            (comp, isz, xp, yp, cu, ci)
        };

    // B3: ICC profile (V5 header only)
    let icc_profile = if dib_version == DibVersion::V5 && dib_size >= 124 {
        // ICC profile data is at offset 112 in the DIB header (from start of DIB)
        // Seek to offset 14 (file header) + 4 (dib_size) + ...
        // In V5, fields after colors_important:
        //   red_mask(4) + green_mask(4) + blue_mask(4) + alpha_mask(4) +
        //   cs_type(4) + endpoints(36) + gamma_red(4) + gamma_green(4) + gamma_blue(4) +
        //   intent(4) + profile_data(4) + profile_size(4) + reserved(4)
        // That's 40 bytes already read + 84 more = 124 total
        // profile_data at DIB offset 112, profile_size at 116
        let remaining_to_skip = 124 - 40 - 8; // already read 40+4(size) = 44 of DIB; we read 8 more (comp..ci)
        if reader.remaining() >= remaining_to_skip as usize {
            let _saved_pos = reader.position();
            // Jump to profile_data offset: DIB starts at file offset 14
            // profile_data is at DIB offset 112 = file offset 14+4+112 = 130
            // profile_size is at DIB offset 116 = file offset 134
            let profile_data_offset_pos = 14 + 112;
            let profile_size_pos = 14 + 116;
            if data.len() > profile_size_pos + 4 {
                let prof_offset = u32::from_le_bytes([
                    data[profile_data_offset_pos],
                    data[profile_data_offset_pos + 1],
                    data[profile_data_offset_pos + 2],
                    data[profile_data_offset_pos + 3],
                ]);
                let prof_size = u32::from_le_bytes([
                    data[profile_size_pos],
                    data[profile_size_pos + 1],
                    data[profile_size_pos + 2],
                    data[profile_size_pos + 3],
                ]);
                if prof_size > 0 {
                    Some((prof_offset, prof_size))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    Ok(BmpInfo {
        file_size,
        data_offset,
        dib_version,
        width,
        height,
        planes,
        bpp,
        compression,
        image_size,
        x_ppm,
        y_ppm,
        colors_used,
        colors_important,
        icc_profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal BMP with BITMAPINFOHEADER.
    fn build_bmp(width: i32, height: i32, bpp: u16) -> Vec<u8> {
        let mut data = Vec::new();

        // File header (14 bytes)
        data.extend_from_slice(b"BM");
        data.extend_from_slice(&0u32.to_le_bytes()); // file size (will be wrong, ok for tests)
        data.extend_from_slice(&0u32.to_le_bytes()); // reserved
        data.extend_from_slice(&54u32.to_le_bytes()); // data offset

        // BITMAPINFOHEADER (40 bytes)
        data.extend_from_slice(&40u32.to_le_bytes()); // header size
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes()); // planes
        data.extend_from_slice(&bpp.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes()); // compression (BI_RGB)
        data.extend_from_slice(&0u32.to_le_bytes()); // image size
        data.extend_from_slice(&2835i32.to_le_bytes()); // x ppm (72 DPI)
        data.extend_from_slice(&2835i32.to_le_bytes()); // y ppm
        data.extend_from_slice(&0u32.to_le_bytes()); // colors used
        data.extend_from_slice(&0u32.to_le_bytes()); // colors important

        data
    }

    #[test]
    fn b1_file_header() {
        let data = build_bmp(100, 100, 24);
        let info = parse_bmp(&data).unwrap();
        assert_eq!(info.data_offset, 54);
    }

    #[test]
    fn b1_not_bmp() {
        assert!(parse_bmp(b"NOT BMP").is_err());
    }

    #[test]
    fn b2_info_header() {
        let data = build_bmp(640, 480, 24);
        let info = parse_bmp(&data).unwrap();
        assert_eq!(info.dib_version, DibVersion::Info);
        assert_eq!(info.width, 640);
        assert_eq!(info.height, 480);
        assert_eq!(info.bpp, 24);
        assert_eq!(info.planes, 1);
        assert_eq!(info.compression, 0);
    }

    #[test]
    fn b2_top_down() {
        let data = build_bmp(100, -100, 32);
        let info = parse_bmp(&data).unwrap();
        assert_eq!(info.height, -100); // negative = top-down
    }

    #[test]
    fn b2_core_header() {
        let mut data = Vec::new();
        data.extend_from_slice(b"BM");
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&26u32.to_le_bytes()); // data offset

        // BITMAPCOREHEADER (12 bytes)
        data.extend_from_slice(&12u32.to_le_bytes());
        data.extend_from_slice(&320u16.to_le_bytes());
        data.extend_from_slice(&240u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&24u16.to_le_bytes());

        let info = parse_bmp(&data).unwrap();
        assert_eq!(info.dib_version, DibVersion::Core);
        assert_eq!(info.width, 320);
        assert_eq!(info.height, 240);
    }

    #[test]
    fn b2_resolution() {
        let data = build_bmp(100, 100, 24);
        let info = parse_bmp(&data).unwrap();
        assert_eq!(info.x_ppm, 2835); // ~72 DPI
        assert_eq!(info.y_ppm, 2835);
    }
}
