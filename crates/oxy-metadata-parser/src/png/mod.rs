//! PNG chunk parser (P1-P10).

use crate::core::{Error, Reader, Result};

/// PNG file signature (8 bytes).
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// A parsed PNG chunk.
#[derive(Debug, Clone)]
pub struct Chunk<'a> {
    /// 4-byte chunk type (e.g., "IHDR", "tEXt", "eXIf").
    pub chunk_type: [u8; 4],
    /// Byte offset of this chunk in the file (start of length field).
    pub offset: usize,
    /// Chunk data (excluding length, type, and CRC).
    pub data: &'a [u8],
    /// CRC-32 value from the file.
    pub crc: u32,
}

impl<'a> Chunk<'a> {
    /// Chunk type as a string.
    pub fn type_str(&self) -> &str {
        std::str::from_utf8(&self.chunk_type).unwrap_or("????")
    }

    /// Whether this is a critical chunk (first byte of type is uppercase).
    pub fn is_critical(&self) -> bool {
        self.chunk_type[0].is_ascii_uppercase()
    }
}

/// Parsed IHDR data (P8).
#[derive(Debug, Clone, Copy)]
pub struct Ihdr {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: u8,
    pub compression: u8,
    pub filter: u8,
    pub interlace: u8,
}

/// Parsed pHYs data (P9).
#[derive(Debug, Clone, Copy)]
pub struct Phys {
    pub pixels_per_unit_x: u32,
    pub pixels_per_unit_y: u32,
    /// 0 = unknown, 1 = meter.
    pub unit: u8,
}

/// Parsed tIME data (P10).
#[derive(Debug, Clone, Copy)]
pub struct Time {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Parsed text chunk (tEXt, iTXt, or zTXt).
#[derive(Debug, Clone)]
pub struct TextChunk {
    pub key: String,
    pub value: String,
    pub language: Option<String>,
    pub translated_key: Option<String>,
}

/// Parse all chunks from a PNG file (P1, P2).
pub fn parse_chunks<'a>(data: &'a [u8]) -> Result<Vec<Chunk<'a>>> {
    // P1: Validate signature
    if data.len() < 8 {
        return Err(Error::Truncated {
            needed: 8,
            available: data.len(),
        });
    }
    if data[..8] != PNG_SIGNATURE {
        return Err(Error::Format("not a PNG: invalid signature".into()));
    }

    let mut reader = Reader::new(data);
    reader.seek(8);
    let mut chunks = Vec::new();

    // P2: Parse chunks
    loop {
        if reader.remaining() < 4 {
            break; // truncated
        }

        let chunk_offset = reader.position();
        let length = reader.read_u32_be()? as usize;

        if reader.remaining() < 4 {
            break;
        }
        let type_bytes = reader.read_bytes(4)?;
        let chunk_type: [u8; 4] = [type_bytes[0], type_bytes[1], type_bytes[2], type_bytes[3]];

        // Get chunk data
        let actual_len = length.min(reader.remaining());
        let chunk_data = if actual_len > 0 {
            reader.read_bytes(actual_len)?
        } else {
            &[]
        };

        // CRC (4 bytes)
        let crc = if reader.remaining() >= 4 {
            reader.read_u32_be()?
        } else {
            0
        };

        let is_iend = &chunk_type == b"IEND";

        chunks.push(Chunk {
            chunk_type,
            offset: chunk_offset,
            data: chunk_data,
            crc,
        });

        if is_iend {
            break;
        }
    }

    Ok(chunks)
}

/// Parse IHDR chunk data (P8).
pub fn parse_ihdr(chunk: &Chunk<'_>) -> Result<Ihdr> {
    if chunk.data.len() < 13 {
        return Err(Error::Truncated {
            needed: 13,
            available: chunk.data.len(),
        });
    }
    let mut r = Reader::new(chunk.data);
    Ok(Ihdr {
        width: r.read_u32_be()?,
        height: r.read_u32_be()?,
        bit_depth: r.read_u8()?,
        color_type: r.read_u8()?,
        compression: r.read_u8()?,
        filter: r.read_u8()?,
        interlace: r.read_u8()?,
    })
}

/// Parse pHYs chunk data (P9).
pub fn parse_phys(chunk: &Chunk<'_>) -> Result<Phys> {
    if chunk.data.len() < 9 {
        return Err(Error::Truncated {
            needed: 9,
            available: chunk.data.len(),
        });
    }
    let mut r = Reader::new(chunk.data);
    Ok(Phys {
        pixels_per_unit_x: r.read_u32_be()?,
        pixels_per_unit_y: r.read_u32_be()?,
        unit: r.read_u8()?,
    })
}

/// Parse tIME chunk data (P10).
pub fn parse_time(chunk: &Chunk<'_>) -> Result<Time> {
    if chunk.data.len() < 7 {
        return Err(Error::Truncated {
            needed: 7,
            available: chunk.data.len(),
        });
    }
    let mut r = Reader::new(chunk.data);
    Ok(Time {
        year: r.read_u16_be()?,
        month: r.read_u8()?,
        day: r.read_u8()?,
        hour: r.read_u8()?,
        minute: r.read_u8()?,
        second: r.read_u8()?,
    })
}

/// Extract tEXt chunk (P3): Latin-1 key + null + Latin-1 value.
pub fn parse_text(chunk: &Chunk<'_>) -> Option<TextChunk> {
    let null_pos = chunk.data.iter().position(|&b| b == 0)?;
    let key = latin1_to_string(&chunk.data[..null_pos]);
    let value = latin1_to_string(&chunk.data[null_pos + 1..]);
    Some(TextChunk {
        key,
        value,
        language: None,
        translated_key: None,
    })
}

/// Extract iTXt chunk (P4): UTF-8 key + null + compressed flag + method + lang + null + translated-key + null + value.
pub fn parse_itxt(chunk: &Chunk<'_>) -> Option<TextChunk> {
    let data = chunk.data;
    let null_pos = data.iter().position(|&b| b == 0)?;
    let key = String::from_utf8_lossy(&data[..null_pos]).into_owned();

    let rest = &data[null_pos + 1..];
    if rest.len() < 2 {
        return None;
    }

    let compressed = rest[0];
    let _method = rest[1];
    let rest = &rest[2..];

    // Language tag (null-terminated)
    let lang_null = rest.iter().position(|&b| b == 0)?;
    let language = String::from_utf8_lossy(&rest[..lang_null]).into_owned();
    let rest = &rest[lang_null + 1..];

    // Translated keyword (null-terminated)
    let tkey_null = rest.iter().position(|&b| b == 0)?;
    let translated_key = String::from_utf8_lossy(&rest[..tkey_null]).into_owned();
    let text_data = &rest[tkey_null + 1..];

    let value = if compressed == 1 {
        // P5-like: compressed iTXt uses deflate
        decompress_zlib(text_data).unwrap_or_default()
    } else {
        String::from_utf8_lossy(text_data).into_owned()
    };

    Some(TextChunk {
        key,
        value,
        language: if language.is_empty() {
            None
        } else {
            Some(language)
        },
        translated_key: if translated_key.is_empty() {
            None
        } else {
            Some(translated_key)
        },
    })
}

/// Extract zTXt chunk (P5): Latin-1 key + null + compression method + compressed data.
pub fn parse_ztxt(chunk: &Chunk<'_>) -> Option<TextChunk> {
    let null_pos = chunk.data.iter().position(|&b| b == 0)?;
    let key = latin1_to_string(&chunk.data[..null_pos]);

    if null_pos + 1 >= chunk.data.len() {
        return None;
    }

    let _method = chunk.data[null_pos + 1]; // should be 0 (deflate)
    let compressed = &chunk.data[null_pos + 2..];
    let value = decompress_zlib(compressed).unwrap_or_default();

    Some(TextChunk {
        key,
        value,
        language: None,
        translated_key: None,
    })
}

/// Extract eXIf chunk raw data (P6) - returns raw TIFF/EXIF bytes.
pub fn find_exif_chunk<'a>(chunks: &[Chunk<'a>]) -> Option<&'a [u8]> {
    chunks
        .iter()
        .find(|c| &c.chunk_type == b"eXIf")
        .map(|c| c.data)
}

/// Extract iCCP chunk data (P7) - returns (profile_name, compressed_profile_data).
pub fn find_iccp_chunk<'a>(chunks: &[Chunk<'a>]) -> Option<(String, &'a [u8])> {
    let chunk = chunks.iter().find(|c| &c.chunk_type == b"iCCP")?;
    let null_pos = chunk.data.iter().position(|&b| b == 0)?;
    let name = latin1_to_string(&chunk.data[..null_pos]);
    // Skip compression method byte (should be 0 = deflate)
    let compressed = &chunk.data[null_pos + 2..];
    Some((name, compressed))
}

/// Find IHDR chunk and parse it.
pub fn find_ihdr(chunks: &[Chunk<'_>]) -> Option<Ihdr> {
    chunks
        .iter()
        .find(|c| &c.chunk_type == b"IHDR")
        .and_then(|c| parse_ihdr(c).ok())
}

/// Collect all text chunks (tEXt, iTXt, zTXt).
pub fn collect_text_chunks(chunks: &[Chunk<'_>]) -> Vec<TextChunk> {
    let mut texts = Vec::new();
    for chunk in chunks {
        match &chunk.chunk_type {
            b"tEXt" => {
                if let Some(t) = parse_text(chunk) {
                    texts.push(t);
                }
            }
            b"iTXt" => {
                if let Some(t) = parse_itxt(chunk) {
                    texts.push(t);
                }
            }
            b"zTXt" => {
                if let Some(t) = parse_ztxt(chunk) {
                    texts.push(t);
                }
            }
            _ => {}
        }
    }
    texts
}

/// Find XMP in PNG: iTXt chunk with key "XML:com.adobe.xmp" (P2 for X2).
pub fn find_xmp_data<'a>(chunks: &[Chunk<'a>]) -> Option<&'a [u8]> {
    for chunk in chunks {
        if &chunk.chunk_type == b"iTXt" {
            let key = b"XML:com.adobe.xmp";
            if chunk.data.starts_with(key) && chunk.data.get(key.len()) == Some(&0) {
                // Skip: key + null + compressed(1) + method(1) + lang + null + translated-key + null
                let rest = &chunk.data[key.len() + 1..];
                if rest.len() < 2 {
                    continue;
                }
                let rest = &rest[2..]; // skip compressed flag + method
                // Skip language tag
                if let Some(p) = rest.iter().position(|&b| b == 0) {
                    let rest = &rest[p + 1..];
                    // Skip translated keyword
                    if let Some(p) = rest.iter().position(|&b| b == 0) {
                        return Some(&rest[p + 1..]);
                    }
                }
            }
        }
    }
    None
}

/// Find EXIF data in PNG text chunks ("Raw profile type exif").
///
/// Some tools (ImageMagick, GIMP) embed EXIF as hex-encoded data in tEXt/zTXt
/// chunks with key "Raw profile type exif". Falls back to this if no eXIf chunk.
pub fn find_raw_profile_exif(chunks: &[Chunk<'_>]) -> Option<Vec<u8>> {
    let texts = collect_text_chunks(chunks);
    for text in &texts {
        if text.key == "Raw profile type exif" {
            return decode_raw_profile(&text.value);
        }
    }
    None
}

/// Find IPTC data in PNG text chunks ("Raw profile type iptc").
///
/// May be bare IPTC (starting with 0x1C) or wrapped in Photoshop IRB (8BIM).
pub fn find_raw_profile_iptc(chunks: &[Chunk<'_>]) -> Option<Vec<u8>> {
    let texts = collect_text_chunks(chunks);
    for text in &texts {
        if text.key == "Raw profile type iptc" {
            let raw = decode_raw_profile(&text.value)?;
            // Check if this is bare IPTC or wrapped in IRB
            if raw.starts_with(&[0x1C]) {
                return Some(raw);
            }
            // Try extracting IPTC from Photoshop IRB (8BIM resources)
            if raw.starts_with(b"8BIM") {
                return extract_iptc_from_irb(&raw);
            }
            return Some(raw);
        }
    }
    None
}

/// Decode a "Raw profile type" text value (ImageMagick/GIMP format).
///
/// Format: `"\n<type>\n<length>\n<hex data across lines>\n"`
fn decode_raw_profile(text: &str) -> Option<Vec<u8>> {
    let mut lines = text.lines();
    lines.next()?; // skip empty first line (or type line)
    // Next non-empty line is the type (e.g., "iptc" or "exif")
    let type_line = lines.next()?.trim();
    if type_line.is_empty() {
        return None;
    }
    // Next line is the byte count
    let _count_line = lines.next()?.trim();
    // Remaining lines are hex data
    let mut hex = String::new();
    for line in lines {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            hex.push_str(trimmed);
        }
    }
    // Decode hex string to bytes
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let hex_bytes = hex.as_bytes();
    let mut i = 0;
    while i + 1 < hex_bytes.len() {
        let hi = hex_nibble(hex_bytes[i])?;
        let lo = hex_nibble(hex_bytes[i + 1])?;
        bytes.push((hi << 4) | lo);
        i += 2;
    }
    Some(bytes)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Extract IPTC data from Photoshop IRB (8BIM resource blocks).
fn extract_iptc_from_irb(data: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    while pos + 12 <= data.len() {
        if &data[pos..pos + 4] != b"8BIM" {
            break;
        }
        pos += 4;
        if pos + 2 > data.len() {
            break;
        }
        let resource_id = u16::from_be_bytes([data[pos], data[pos + 1]]);
        pos += 2;
        if pos >= data.len() {
            break;
        }
        let name_len = data[pos] as usize;
        pos += 1;
        let padded_name_len = if (name_len + 1) % 2 == 0 {
            name_len
        } else {
            name_len + 1
        };
        pos += padded_name_len;
        if pos + 4 > data.len() {
            break;
        }
        let data_size =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;
        if pos + data_size > data.len() {
            break;
        }
        if resource_id == 0x0404 {
            return Some(data[pos..pos + data_size].to_vec());
        }
        pos += data_size;
        if data_size % 2 == 1 {
            pos += 1;
        }
    }
    None
}

fn latin1_to_string(data: &[u8]) -> String {
    data.iter().map(|&b| b as char).collect()
}

fn decompress_zlib(data: &[u8]) -> Option<String> {
    let decompressed = miniz_oxide::inflate::decompress_to_vec_zlib(data).ok()?;
    String::from_utf8(decompressed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal PNG with given chunks. Each chunk is (type_4cc, data).
    fn build_png(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut data = PNG_SIGNATURE.to_vec();
        for &(chunk_type, chunk_data) in chunks {
            let len = chunk_data.len() as u32;
            data.extend_from_slice(&len.to_be_bytes());
            data.extend_from_slice(chunk_type);
            data.extend_from_slice(chunk_data);
            data.extend_from_slice(&0u32.to_be_bytes()); // fake CRC
        }
        data
    }

    fn ihdr_data(w: u32, h: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&w.to_be_bytes());
        d.extend_from_slice(&h.to_be_bytes());
        d.push(8); // bit depth
        d.push(2); // color type (RGB)
        d.push(0); // compression
        d.push(0); // filter
        d.push(0); // interlace
        d
    }

    #[test]
    fn p1_signature_validation() {
        let data = build_png(&[(b"IHDR", &ihdr_data(1, 1)), (b"IEND", &[])]);
        let chunks = parse_chunks(&data).unwrap();
        assert!(!chunks.is_empty());

        // Invalid signature
        assert!(parse_chunks(b"not a png file").is_err());
    }

    #[test]
    fn p2_chunk_structure() {
        let ihdr = ihdr_data(640, 480);
        let data = build_png(&[
            (b"IHDR", &ihdr),
            (b"tEXt", b"Comment\0Hello"),
            (b"IEND", &[]),
        ]);
        let chunks = parse_chunks(&data).unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].type_str(), "IHDR");
        assert_eq!(chunks[1].type_str(), "tEXt");
        assert_eq!(chunks[2].type_str(), "IEND");
    }

    #[test]
    fn p3_text_chunk() {
        let data = build_png(&[(b"tEXt", b"Author\0John Doe"), (b"IEND", &[])]);
        let chunks = parse_chunks(&data).unwrap();
        let text = parse_text(&chunks[0]).unwrap();
        assert_eq!(text.key, "Author");
        assert_eq!(text.value, "John Doe");
    }

    #[test]
    fn p4_itxt_chunk() {
        // iTXt: key + null + compressed(0) + method(0) + lang + null + translated-key + null + value
        let mut itxt = b"Description".to_vec();
        itxt.push(0); // null
        itxt.push(0); // not compressed
        itxt.push(0); // method
        itxt.extend_from_slice(b"en"); // language
        itxt.push(0);
        itxt.extend_from_slice(b"Desc"); // translated key
        itxt.push(0);
        itxt.extend_from_slice(b"A test image");

        let data = build_png(&[(b"iTXt", &itxt), (b"IEND", &[])]);
        let chunks = parse_chunks(&data).unwrap();
        let text = parse_itxt(&chunks[0]).unwrap();
        assert_eq!(text.key, "Description");
        assert_eq!(text.value, "A test image");
        assert_eq!(text.language, Some("en".into()));
        assert_eq!(text.translated_key, Some("Desc".into()));
    }

    #[test]
    fn p6_exif_chunk() {
        let exif = b"MM\0\x2A\0\0\0\x08"; // minimal TIFF header
        let data = build_png(&[(b"eXIf", exif), (b"IEND", &[])]);
        let chunks = parse_chunks(&data).unwrap();
        let exif_data = find_exif_chunk(&chunks).unwrap();
        assert_eq!(&exif_data[..4], b"MM\0\x2A");
    }

    #[test]
    fn p7_iccp_chunk() {
        let mut iccp = b"sRGB".to_vec();
        iccp.push(0); // null terminator
        iccp.push(0); // compression method = deflate
        iccp.extend_from_slice(b"compressed-data");

        let data = build_png(&[(b"iCCP", &iccp), (b"IEND", &[])]);
        let chunks = parse_chunks(&data).unwrap();
        let (name, compressed) = find_iccp_chunk(&chunks).unwrap();
        assert_eq!(name, "sRGB");
        assert_eq!(compressed, b"compressed-data");
    }

    #[test]
    fn p8_ihdr() {
        let ihdr = ihdr_data(1920, 1080);
        let data = build_png(&[(b"IHDR", &ihdr), (b"IEND", &[])]);
        let chunks = parse_chunks(&data).unwrap();
        let info = find_ihdr(&chunks).unwrap();
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert_eq!(info.bit_depth, 8);
        assert_eq!(info.color_type, 2);
    }

    #[test]
    fn p9_phys() {
        let mut phys = Vec::new();
        phys.extend_from_slice(&3780u32.to_be_bytes()); // 96 DPI
        phys.extend_from_slice(&3780u32.to_be_bytes());
        phys.push(1); // meter

        let data = build_png(&[(b"pHYs", &phys), (b"IEND", &[])]);
        let chunks = parse_chunks(&data).unwrap();
        let p = parse_phys(&chunks[0]).unwrap();
        assert_eq!(p.pixels_per_unit_x, 3780);
        assert_eq!(p.unit, 1);
    }

    #[test]
    fn p10_time() {
        let time_data = [
            0x07, 0xE6, // year 2022
            0x06, // month 6
            0x0F, // day 15
            0x0A, // hour 10
            0x1E, // minute 30
            0x00, // second 0
        ];
        let data = build_png(&[(b"tIME", &time_data), (b"IEND", &[])]);
        let chunks = parse_chunks(&data).unwrap();
        let t = parse_time(&chunks[0]).unwrap();
        assert_eq!(t.year, 2022);
        assert_eq!(t.month, 6);
        assert_eq!(t.day, 15);
        assert_eq!(t.hour, 10);
    }

    #[test]
    fn collect_texts() {
        let data = build_png(&[
            (b"tEXt", b"Author\0Alice"),
            (b"tEXt", b"Comment\0Nice"),
            (b"IEND", &[]),
        ]);
        let chunks = parse_chunks(&data).unwrap();
        let texts = collect_text_chunks(&chunks);
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0].key, "Author");
        assert_eq!(texts[1].key, "Comment");
    }

    #[test]
    fn critical_check() {
        let data = build_png(&[
            (b"IHDR", &ihdr_data(1, 1)),
            (b"tEXt", b"k\0v"),
            (b"IEND", &[]),
        ]);
        let chunks = parse_chunks(&data).unwrap();
        assert!(chunks[0].is_critical()); // IHDR
        assert!(!chunks[1].is_critical()); // tEXt
    }
}
