//! WebP RIFF container parser (W1-W6).

use crate::core::{Error, Reader, Result};

/// Parsed WebP file.
#[derive(Debug)]
pub struct WebP<'a> {
    /// File size from RIFF header.
    pub file_size: u32,
    /// VP8X flags (if extended format).
    pub flags: Option<Vp8xFlags>,
    /// Image dimensions.
    pub width: u32,
    pub height: u32,
    /// All RIFF chunks.
    pub chunks: Vec<RiffChunk<'a>>,
}

/// VP8X extended header flags (W2).
#[derive(Debug, Clone, Copy)]
pub struct Vp8xFlags {
    pub has_icc: bool,
    pub has_alpha: bool,
    pub has_exif: bool,
    pub has_xmp: bool,
    pub has_animation: bool,
}

/// A RIFF chunk.
#[derive(Debug, Clone)]
pub struct RiffChunk<'a> {
    pub fourcc: [u8; 4],
    pub offset: usize,
    pub data: &'a [u8],
}

impl<'a> RiffChunk<'a> {
    pub fn fourcc_str(&self) -> &str {
        std::str::from_utf8(&self.fourcc).unwrap_or("????")
    }
}

/// Parse a WebP file (W1-W6).
pub fn parse_webp<'a>(data: &'a [u8]) -> Result<WebP<'a>> {
    let mut reader = Reader::new(data);

    // W1: RIFF header
    let riff = reader.read_bytes(4)?;
    if riff != b"RIFF" {
        return Err(Error::Format("not a WebP: missing RIFF header".into()));
    }
    let file_size = reader.read_u32_le()?;
    let webp = reader.read_bytes(4)?;
    if webp != b"WEBP" {
        return Err(Error::Format("not a WebP: missing WEBP fourcc".into()));
    }

    let mut chunks = Vec::new();
    let mut flags = None;
    let mut width = 0u32;
    let mut height = 0u32;

    while reader.remaining() >= 8 {
        let chunk_offset = reader.position();
        let fourcc_bytes = reader.read_bytes(4)?;
        let fourcc: [u8; 4] = [
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
        ];
        let chunk_size = reader.read_u32_le()? as usize;

        let actual_size = chunk_size.min(reader.remaining());
        let chunk_data = reader.read_bytes(actual_size)?;

        // RIFF chunks are padded to even size
        if chunk_size % 2 == 1 && reader.remaining() > 0 {
            let _ = reader.skip(1);
        }

        // W2: VP8X extended header
        if &fourcc == b"VP8X" && chunk_data.len() >= 10 {
            let f = chunk_data[0];
            flags = Some(Vp8xFlags {
                has_icc: f & 0x20 != 0,
                has_alpha: f & 0x10 != 0,
                has_exif: f & 0x08 != 0,
                has_xmp: f & 0x04 != 0,
                has_animation: f & 0x02 != 0,
            });
            // W6: Canvas dimensions from VP8X
            width = u32::from_le_bytes([chunk_data[4], chunk_data[5], chunk_data[6], 0]) + 1;
            height = u32::from_le_bytes([chunk_data[7], chunk_data[8], chunk_data[9], 0]) + 1;
        }

        // W6: Dimensions from VP8 (lossy)
        if &fourcc == b"VP8 " && chunk_data.len() >= 10 && width == 0 {
            // VP8 bitstream starts with frame tag (3 bytes) + sync code 9D 01 2A + width(2) + height(2)
            if chunk_data.len() >= 10
                && chunk_data[3] == 0x9D
                && chunk_data[4] == 0x01
                && chunk_data[5] == 0x2A
            {
                width = u16::from_le_bytes([chunk_data[6], chunk_data[7]]) as u32 & 0x3FFF;
                height = u16::from_le_bytes([chunk_data[8], chunk_data[9]]) as u32 & 0x3FFF;
            }
        }

        // W6: Dimensions from VP8L (lossless)
        if &fourcc == b"VP8L" && chunk_data.len() >= 5 && width == 0 {
            if chunk_data[0] == 0x2F {
                let bits = u32::from_le_bytes([
                    chunk_data[1],
                    chunk_data[2],
                    chunk_data[3],
                    chunk_data[4],
                ]);
                width = (bits & 0x3FFF) + 1;
                height = ((bits >> 14) & 0x3FFF) + 1;
            }
        }

        chunks.push(RiffChunk {
            fourcc,
            offset: chunk_offset,
            data: chunk_data,
        });
    }

    Ok(WebP {
        file_size,
        flags,
        width,
        height,
        chunks,
    })
}

/// W3: Extract EXIF data from WebP.
pub fn find_exif<'a>(webp: &WebP<'a>) -> Option<&'a [u8]> {
    webp.chunks.iter().find(|c| &c.fourcc == b"EXIF").map(|c| {
        // Some WebP files have "Exif\0\0" prefix before the TIFF data
        if c.data.starts_with(b"Exif\0\0") {
            &c.data[6..]
        } else {
            c.data
        }
    })
}

/// W4: Extract XMP data from WebP.
pub fn find_xmp<'a>(webp: &WebP<'a>) -> Option<&'a [u8]> {
    webp.chunks
        .iter()
        .find(|c| &c.fourcc == b"XMP ")
        .map(|c| c.data)
}

/// W5: Extract ICC profile data from WebP.
pub fn find_iccp<'a>(webp: &WebP<'a>) -> Option<&'a [u8]> {
    webp.chunks
        .iter()
        .find(|c| &c.fourcc == b"ICCP")
        .map(|c| c.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_webp(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        for &(fourcc, data) in chunks {
            body.extend_from_slice(fourcc);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 == 1 {
                body.push(0); // padding
            }
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn vp8x_data(flags: u8, w: u32, h: u32) -> Vec<u8> {
        let mut d = vec![flags, 0, 0, 0];
        let w1 = w - 1;
        let h1 = h - 1;
        d.push((w1 & 0xFF) as u8);
        d.push(((w1 >> 8) & 0xFF) as u8);
        d.push(((w1 >> 16) & 0xFF) as u8);
        d.push((h1 & 0xFF) as u8);
        d.push(((h1 >> 8) & 0xFF) as u8);
        d.push(((h1 >> 16) & 0xFF) as u8);
        d
    }

    #[test]
    fn w1_riff_header() {
        let data = build_webp(&[(b"VP8X", &vp8x_data(0, 100, 100))]);
        let webp = parse_webp(&data).unwrap();
        assert!(!webp.chunks.is_empty());
    }

    #[test]
    fn w1_not_webp() {
        assert!(parse_webp(b"RIFF\x04\0\0\0AVI ").is_err());
        assert!(parse_webp(b"not webp").is_err());
    }

    #[test]
    fn w2_vp8x_flags() {
        let flags = 0x20 | 0x08; // ICC + EXIF
        let data = build_webp(&[(b"VP8X", &vp8x_data(flags, 640, 480))]);
        let webp = parse_webp(&data).unwrap();
        let f = webp.flags.unwrap();
        assert!(f.has_icc);
        assert!(f.has_exif);
        assert!(!f.has_xmp);
        assert!(!f.has_alpha);
        assert!(!f.has_animation);
    }

    #[test]
    fn w3_exif() {
        let exif = b"II*\0\x08\0\0\0";
        let data = build_webp(&[(b"VP8X", &vp8x_data(0x08, 100, 100)), (b"EXIF", exif)]);
        let webp = parse_webp(&data).unwrap();
        let exif_data = find_exif(&webp).unwrap();
        assert_eq!(&exif_data[..4], b"II*\0");
    }

    #[test]
    fn w4_xmp() {
        let xmp = b"<x:xmpmeta/>";
        let data = build_webp(&[(b"VP8X", &vp8x_data(0x04, 100, 100)), (b"XMP ", xmp)]);
        let webp = parse_webp(&data).unwrap();
        assert!(find_xmp(&webp).is_some());
    }

    #[test]
    fn w5_iccp() {
        let icc = b"fake-icc-profile";
        let data = build_webp(&[(b"VP8X", &vp8x_data(0x20, 100, 100)), (b"ICCP", icc)]);
        let webp = parse_webp(&data).unwrap();
        assert_eq!(find_iccp(&webp).unwrap(), b"fake-icc-profile");
    }

    #[test]
    fn w6_dimensions_vp8x() {
        let data = build_webp(&[(b"VP8X", &vp8x_data(0, 1920, 1080))]);
        let webp = parse_webp(&data).unwrap();
        assert_eq!(webp.width, 1920);
        assert_eq!(webp.height, 1080);
    }
}
