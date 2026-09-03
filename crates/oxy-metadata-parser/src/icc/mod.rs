//! ICC profile parser (C1-C5).
//!
//! Parses ICC profile headers (128 bytes) and tag tables per ICC.1:2022.

use crate::core::{Error, Reader, Result};

/// ICC profile header (128 bytes) - C1.
#[derive(Debug, Clone)]
pub struct IccProfile<'a> {
    /// Profile size in bytes.
    pub size: u32,
    /// Preferred CMM type (4-char code).
    pub cmm_type: [u8; 4],
    /// Profile version (major.minor.bugfix).
    pub version: (u8, u8, u8),
    /// Device class - C5.
    pub device_class: ProfileClass,
    /// Color space of data - C5.
    pub color_space: ColorSpace,
    /// Profile Connection Space.
    pub pcs: ColorSpace,
    /// Creation date/time.
    pub date_time: IccDateTime,
    /// Primary platform signature.
    pub primary_platform: [u8; 4],
    /// Profile flags.
    pub profile_flags: u32,
    /// Device manufacturer signature.
    pub device_manufacturer: [u8; 4],
    /// Device model signature.
    pub device_model: [u8; 4],
    /// Device attributes (8-byte bitfield).
    pub device_attributes: u64,
    /// PCS illuminant (XYZ).
    pub pcs_illuminant: [f64; 3],
    /// Profile creator signature.
    pub profile_creator: [u8; 4],
    /// Profile ID (16-byte MD5).
    pub profile_id: [u8; 16],
    /// Rendering intent - C5.
    pub rendering_intent: RenderingIntent,
    /// Tag table entries - C2.
    pub tags: Vec<IccTag<'a>>,
    /// Profile description - C3.
    pub description: Option<String>,
    /// Copyright - C4.
    pub copyright: Option<String>,
}

/// ICC date/time (from header bytes 24-35).
#[derive(Debug, Clone, Copy)]
pub struct IccDateTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

/// ICC tag table entry - C2.
#[derive(Debug, Clone)]
pub struct IccTag<'a> {
    /// Tag signature (4-char code).
    pub signature: [u8; 4],
    /// Byte offset within the profile.
    pub offset: u32,
    /// Size in bytes.
    pub size: u32,
    /// Tag data (slice into the profile).
    pub data: &'a [u8],
}

impl<'a> IccTag<'a> {
    /// Tag signature as a string.
    pub fn signature_str(&self) -> &str {
        std::str::from_utf8(&self.signature).unwrap_or("????")
    }
}

/// ICC profile class (device class) - C5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileClass {
    Input,
    Display,
    Output,
    Link,
    Abstract,
    ColorSpace,
    NamedColor,
    Unknown([u8; 4]),
}

impl ProfileClass {
    fn from_bytes(b: &[u8; 4]) -> Self {
        match b {
            b"scnr" => ProfileClass::Input,
            b"mntr" => ProfileClass::Display,
            b"prtr" => ProfileClass::Output,
            b"link" => ProfileClass::Link,
            b"abst" => ProfileClass::Abstract,
            b"spac" => ProfileClass::ColorSpace,
            b"nmcl" => ProfileClass::NamedColor,
            _ => ProfileClass::Unknown(*b),
        }
    }

    /// Display name for the profile class.
    pub fn as_str(&self) -> &str {
        match self {
            ProfileClass::Input => "Input Device",
            ProfileClass::Display => "Display Device",
            ProfileClass::Output => "Output Device",
            ProfileClass::Link => "Device Link",
            ProfileClass::Abstract => "Abstract",
            ProfileClass::ColorSpace => "Color Space Conversion",
            ProfileClass::NamedColor => "Named Color",
            ProfileClass::Unknown(_) => "Unknown",
        }
    }
}

/// Color space signature - C5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Xyz,
    Lab,
    Luv,
    YCbCr,
    Yxy,
    Rgb,
    Gray,
    Hsv,
    Hls,
    Cmyk,
    Cmy,
    Unknown([u8; 4]),
}

impl ColorSpace {
    fn from_bytes(b: &[u8; 4]) -> Self {
        match b {
            b"XYZ " => ColorSpace::Xyz,
            b"Lab " => ColorSpace::Lab,
            b"Luv " => ColorSpace::Luv,
            b"YCbr" => ColorSpace::YCbCr,
            b"Yxy " => ColorSpace::Yxy,
            b"RGB " => ColorSpace::Rgb,
            b"GRAY" => ColorSpace::Gray,
            b"HSV " => ColorSpace::Hsv,
            b"HLS " => ColorSpace::Hls,
            b"CMYK" => ColorSpace::Cmyk,
            b"CMY " => ColorSpace::Cmy,
            _ => ColorSpace::Unknown(*b),
        }
    }

    /// Display name for the color space.
    pub fn as_str(&self) -> &str {
        match self {
            ColorSpace::Xyz => "XYZ",
            ColorSpace::Lab => "Lab",
            ColorSpace::Luv => "Luv",
            ColorSpace::YCbCr => "YCbCr",
            ColorSpace::Yxy => "Yxy",
            ColorSpace::Rgb => "RGB",
            ColorSpace::Gray => "Gray",
            ColorSpace::Hsv => "HSV",
            ColorSpace::Hls => "HLS",
            ColorSpace::Cmyk => "CMYK",
            ColorSpace::Cmy => "CMY",
            ColorSpace::Unknown(_) => "Unknown",
        }
    }
}

/// Rendering intent - C5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderingIntent {
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
    Unknown(u32),
}

impl RenderingIntent {
    fn from_u32(v: u32) -> Self {
        match v {
            0 => RenderingIntent::Perceptual,
            1 => RenderingIntent::RelativeColorimetric,
            2 => RenderingIntent::Saturation,
            3 => RenderingIntent::AbsoluteColorimetric,
            _ => RenderingIntent::Unknown(v),
        }
    }

    /// Display name.
    pub fn as_str(&self) -> &str {
        match self {
            RenderingIntent::Perceptual => "Perceptual",
            RenderingIntent::RelativeColorimetric => "Relative Colorimetric",
            RenderingIntent::Saturation => "Saturation",
            RenderingIntent::AbsoluteColorimetric => "Absolute Colorimetric",
            RenderingIntent::Unknown(_) => "Unknown",
        }
    }
}

/// C1: Parse ICC profile header and tag table.
pub fn parse_icc_profile<'a>(data: &'a [u8]) -> Result<IccProfile<'a>> {
    if data.len() < 128 {
        return Err(Error::Truncated {
            needed: 128,
            available: data.len(),
        });
    }

    let mut reader = Reader::new(data);

    // C1: Header (128 bytes, always big-endian)
    let size = reader.read_u32_be()?;

    let cmm_bytes = reader.read_bytes(4)?;
    let cmm_type: [u8; 4] = [cmm_bytes[0], cmm_bytes[1], cmm_bytes[2], cmm_bytes[3]];

    // Version: byte 8 = major, byte 9 = minor.bugfix (packed BCD)
    let ver_major = reader.read_u8()?;
    let ver_minor_bug = reader.read_u8()?;
    let version = (ver_major, ver_minor_bug >> 4, ver_minor_bug & 0x0F);
    reader.skip(2)?; // reserved version bytes

    // Device class (bytes 12-15)
    let class_bytes = reader.read_bytes(4)?;
    let class_arr: [u8; 4] = [
        class_bytes[0],
        class_bytes[1],
        class_bytes[2],
        class_bytes[3],
    ];
    let device_class = ProfileClass::from_bytes(&class_arr);

    // Color space (bytes 16-19)
    let cs_bytes = reader.read_bytes(4)?;
    let cs_arr: [u8; 4] = [cs_bytes[0], cs_bytes[1], cs_bytes[2], cs_bytes[3]];
    let color_space = ColorSpace::from_bytes(&cs_arr);

    // PCS (bytes 20-23)
    let pcs_bytes = reader.read_bytes(4)?;
    let pcs_arr: [u8; 4] = [pcs_bytes[0], pcs_bytes[1], pcs_bytes[2], pcs_bytes[3]];
    let pcs = ColorSpace::from_bytes(&pcs_arr);

    // Date/time (bytes 24-35)
    let date_time = IccDateTime {
        year: reader.read_u16_be()?,
        month: reader.read_u16_be()?,
        day: reader.read_u16_be()?,
        hour: reader.read_u16_be()?,
        minute: reader.read_u16_be()?,
        second: reader.read_u16_be()?,
    };

    // Signature 'acsp' at bytes 36-39
    let sig = reader.read_bytes(4)?;
    if sig != b"acsp" {
        return Err(Error::Format("ICC: missing 'acsp' signature".into()));
    }

    // Primary platform (bytes 40-43)
    let plat_bytes = reader.read_bytes(4)?;
    let primary_platform: [u8; 4] = [plat_bytes[0], plat_bytes[1], plat_bytes[2], plat_bytes[3]];

    // Profile flags (bytes 44-47)
    let profile_flags = reader.read_u32_be()?;

    // Device manufacturer (bytes 48-51)
    let mfr_bytes = reader.read_bytes(4)?;
    let device_manufacturer: [u8; 4] = [mfr_bytes[0], mfr_bytes[1], mfr_bytes[2], mfr_bytes[3]];

    // Device model (bytes 52-55)
    let mdl_bytes = reader.read_bytes(4)?;
    let device_model: [u8; 4] = [mdl_bytes[0], mdl_bytes[1], mdl_bytes[2], mdl_bytes[3]];

    // Device attributes (bytes 56-63)
    let device_attributes = reader.read_u64_be()?;

    // All remaining header fields read directly from data[] for clarity
    // (Reader position doesn't matter - we jump to byte 128 for tag table anyway)

    // Rendering intent (bytes 64-67) - already read above

    // PCS illuminant (bytes 68-79): 3 × s15Fixed16Number (XYZ)
    let pcs_illuminant = [
        read_s15fixed16(&data[68..72]),
        read_s15fixed16(&data[72..76]),
        read_s15fixed16(&data[76..80]),
    ];

    // Profile creator (bytes 80-83)
    let profile_creator: [u8; 4] = [data[80], data[81], data[82], data[83]];

    // Profile ID (bytes 84-99): 16-byte MD5 hash
    let mut profile_id = [0u8; 16];
    profile_id.copy_from_slice(&data[84..100]);

    // Bytes 100-127: reserved
    // Set reader to byte 128 for tag table
    reader = Reader::new(&data[128..]);

    // Rendering intent is at bytes 64-67
    let intent_val = u32::from_be_bytes([data[64], data[65], data[66], data[67]]);
    let rendering_intent = RenderingIntent::from_u32(intent_val);

    // C2: Tag table (starts at byte 128)
    let mut tags = Vec::new();
    if reader.remaining() >= 4 {
        let tag_count = reader.read_u32_be()? as usize;
        // Sanity check
        let tag_count = tag_count.min(1000);

        for _ in 0..tag_count {
            if reader.remaining() < 12 {
                break;
            }
            let sig_bytes = reader.read_bytes(4)?;
            let offset = reader.read_u32_be()?;
            let tag_size = reader.read_u32_be()?;

            let signature: [u8; 4] = [sig_bytes[0], sig_bytes[1], sig_bytes[2], sig_bytes[3]];
            let off = offset as usize;
            let end = (off + tag_size as usize).min(data.len());
            let tag_data = if off < data.len() {
                &data[off..end]
            } else {
                &[]
            };

            tags.push(IccTag {
                signature,
                offset,
                size: tag_size,
                data: tag_data,
            });
        }
    }

    // C3: Extract description
    let description = tags
        .iter()
        .find(|t| &t.signature == b"desc")
        .and_then(|t| parse_text_description(t.data));

    // C4: Extract copyright
    let copyright = tags
        .iter()
        .find(|t| &t.signature == b"cprt")
        .and_then(|t| parse_text_type(t.data));

    Ok(IccProfile {
        size,
        cmm_type,
        version,
        device_class,
        color_space,
        pcs,
        date_time,
        primary_platform,
        profile_flags,
        device_manufacturer,
        device_model,
        device_attributes,
        pcs_illuminant,
        profile_creator,
        profile_id,
        rendering_intent,
        tags,
        description,
        copyright,
    })
}

/// Read s15Fixed16Number from 4 bytes (big-endian).
fn read_s15fixed16(data: &[u8]) -> f64 {
    let raw = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    raw as f64 / 65536.0
}

/// Parse XYZType tag data: returns (X, Y, Z) as f64.
pub fn parse_xyz_type(data: &[u8]) -> Option<[f64; 3]> {
    // XYZType: signature(4) + reserved(4) + one or more XYZNumber(12 each)
    if data.len() < 20 {
        return None;
    }
    Some([
        read_s15fixed16(&data[8..12]),
        read_s15fixed16(&data[12..16]),
        read_s15fixed16(&data[16..20]),
    ])
}

/// Parse curveType or parametricCurveType tag data.
/// Returns a descriptive string like ExifTool does.
pub fn describe_trc(data: &[u8]) -> Option<String> {
    if data.len() < 12 {
        return None;
    }
    let type_sig = &data[..4];
    match type_sig {
        b"curv" => {
            let count = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
            if count == 0 {
                Some("Linear".to_string())
            } else if count == 1 && data.len() >= 14 {
                let gamma = u16::from_be_bytes([data[12], data[13]]) as f64 / 256.0;
                Some(format!("Gamma {gamma:.1}"))
            } else {
                Some(format!(
                    "(Binary data {count} bytes, use -b option to extract)",
                    count = count * 2
                ))
            }
        }
        b"para" => {
            let func_type = u16::from_be_bytes([data[8], data[9]]);
            if data.len() >= 16 {
                let gamma = read_s15fixed16(&data[12..16]);
                Some(format!("Parametric type {func_type} gamma {gamma:.4}"))
            } else {
                Some(format!("Parametric type {func_type}"))
            }
        }
        _ => None,
    }
}

/// Parse s15Fixed16ArrayType (e.g., chromatic adaptation 'chad' - 3×3 matrix).
pub fn parse_s15fixed16_array(data: &[u8]) -> Option<Vec<f64>> {
    // s15Fixed16ArrayType: signature(4) + reserved(4) + values
    if data.len() < 12 {
        return None;
    }
    let num_values = (data.len() - 8) / 4;
    let mut values = Vec::with_capacity(num_values);
    for i in 0..num_values {
        let off = 8 + i * 4;
        if off + 4 <= data.len() {
            values.push(read_s15fixed16(&data[off..off + 4]));
        }
    }
    Some(values)
}

impl<'a> IccProfile<'a> {
    /// Find a tag by its 4-byte signature.
    pub fn find_tag(&self, sig: &[u8; 4]) -> Option<&IccTag<'a>> {
        self.tags.iter().find(|t| &t.signature == sig)
    }

    /// Get XYZ value for a given tag signature.
    pub fn xyz_tag(&self, sig: &[u8; 4]) -> Option<[f64; 3]> {
        self.find_tag(sig).and_then(|t| parse_xyz_type(t.data))
    }

    /// Get TRC description for a given tag signature.
    pub fn trc_tag(&self, sig: &[u8; 4]) -> Option<String> {
        self.find_tag(sig).and_then(|t| describe_trc(t.data))
    }

    /// Get chromatic adaptation matrix (3×3).
    pub fn chromatic_adaptation(&self) -> Option<Vec<f64>> {
        self.find_tag(b"chad")
            .and_then(|t| parse_s15fixed16_array(t.data))
    }

    /// Platform name from 4-byte signature.
    pub fn platform_name(&self) -> &str {
        match &self.primary_platform {
            b"APPL" => "Apple Computer Inc.",
            b"MSFT" => "Microsoft Corporation",
            b"SGI " => "Silicon Graphics Inc.",
            b"SUNW" => "Sun Microsystems Inc.",
            b"\0\0\0\0" => "",
            _ => "Unknown",
        }
    }

    /// CMM type name from 4-byte signature.
    pub fn cmm_name(&self) -> &str {
        match &self.cmm_type {
            b"APPL" => "Apple Computer Inc.",
            b"appl" => "Apple Computer Inc.",
            b"ADBE" => "Adobe Systems Inc.",
            b"MSFT" => "Microsoft Corporation",
            b"KCMS" => "Kodak Color Management System",
            b"LITR" => "Little CMS",
            b"lcms" => "Little CMS",
            b"\0\0\0\0" => "",
            _ => {
                // Return the 4-char code as-is
                ""
            }
        }
    }

    /// Creator name from 4-byte signature.
    pub fn creator_name(&self) -> &str {
        match &self.profile_creator {
            b"APPL" => "Apple Computer Inc.",
            b"appl" => "Apple Computer Inc.",
            b"ADBE" => "Adobe Systems Inc.",
            b"MSFT" => "Microsoft Corporation",
            b"\0\0\0\0" => "",
            _ => "",
        }
    }

    /// Device manufacturer name.
    pub fn manufacturer_name(&self) -> &str {
        match &self.device_manufacturer {
            b"APPL" => "Apple Computer Inc.",
            b"appl" => "Apple Computer Inc.",
            b"\0\0\0\0" => "",
            _ => "",
        }
    }

    /// Format profile flags as descriptive string.
    pub fn flags_str(&self) -> String {
        let embedded = if self.profile_flags & 1 != 0 {
            "Embedded"
        } else {
            "Not Embedded"
        };
        let independent = if self.profile_flags & 2 != 0 {
            "Dependent"
        } else {
            "Independent"
        };
        format!("{embedded}, {independent}")
    }

    /// Format device attributes as descriptive string.
    pub fn attributes_str(&self) -> String {
        let reflective = if self.device_attributes & 1 != 0 {
            "Transparency"
        } else {
            "Reflective"
        };
        let glossy = if self.device_attributes & 2 != 0 {
            "Matte"
        } else {
            "Glossy"
        };
        let positive = if self.device_attributes & 4 != 0 {
            "Negative"
        } else {
            "Positive"
        };
        let color = if self.device_attributes & 8 != 0 {
            "Black & White"
        } else {
            "Color"
        };
        format!("{reflective}, {glossy}, {positive}, {color}")
    }

    /// Format profile ID as hex string.
    pub fn profile_id_hex(&self) -> String {
        self.profile_id.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Format date/time as ExifTool-compatible string.
    pub fn date_time_str(&self) -> String {
        let dt = &self.date_time;
        format!(
            "{:04}:{:02}:{:02} {:02}:{:02}:{:02}",
            dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
        )
    }

    /// Format PCS illuminant as string.
    pub fn illuminant_str(&self) -> String {
        format!(
            "{:.5} {:.5} {:.5}",
            self.pcs_illuminant[0], self.pcs_illuminant[1], self.pcs_illuminant[2]
        )
    }
}

/// C3: Parse 'desc' tag - textDescriptionType or multiLocalizedUnicodeType.
fn parse_text_description(data: &[u8]) -> Option<String> {
    if data.len() < 12 {
        return None;
    }

    let type_sig = &data[..4];

    match type_sig {
        // textDescriptionType (ICC v2)
        b"desc" => {
            // Skip signature(4) + reserved(4) = 8, then ASCII count(4) + ASCII string
            if data.len() < 12 {
                return None;
            }
            let count = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
            if count == 0 || data.len() < 12 + count {
                return None;
            }
            let s = &data[12..12 + count];
            let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
            std::str::from_utf8(&s[..end]).ok().map(|s| s.to_string())
        }
        // multiLocalizedUnicodeType (ICC v4) - 'mluc'
        b"mluc" => parse_mluc(data),
        _ => None,
    }
}

/// C4: Parse text type tags ('cprt', etc.).
fn parse_text_type(data: &[u8]) -> Option<String> {
    if data.len() < 8 {
        return None;
    }

    let type_sig = &data[..4];

    match type_sig {
        // textType (ICC v2)
        b"text" => {
            // Skip signature(4) + reserved(4) = 8
            let s = &data[8..];
            let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
            std::str::from_utf8(&s[..end]).ok().map(|s| s.to_string())
        }
        b"desc" => parse_text_description(data),
        b"mluc" => parse_mluc(data),
        _ => {
            // Try as raw text
            let s = &data[8..];
            let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
            std::str::from_utf8(&s[..end]).ok().map(|s| s.to_string())
        }
    }
}

/// Parse multiLocalizedUnicodeType ('mluc').
fn parse_mluc(data: &[u8]) -> Option<String> {
    if data.len() < 16 {
        return None;
    }
    // mluc: signature(4) + reserved(4) + numberOfRecords(4) + recordSize(4)
    let num_records = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
    let record_size = u32::from_be_bytes([data[12], data[13], data[14], data[15]]) as usize;

    if num_records == 0 || record_size < 12 {
        return None;
    }

    // First record at offset 16: language(2) + country(2) + stringLength(4) + stringOffset(4)
    let rec_off = 16;
    if data.len() < rec_off + 12 {
        return None;
    }
    let str_len = u32::from_be_bytes([
        data[rec_off + 4],
        data[rec_off + 5],
        data[rec_off + 6],
        data[rec_off + 7],
    ]) as usize;
    let str_off = u32::from_be_bytes([
        data[rec_off + 8],
        data[rec_off + 9],
        data[rec_off + 10],
        data[rec_off + 11],
    ]) as usize;

    if str_off + str_len > data.len() || str_len < 2 {
        return None;
    }

    // UTF-16BE string
    let utf16_data = &data[str_off..str_off + str_len];
    let words: Vec<u16> = utf16_data
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .take_while(|&w| w != 0)
        .collect();

    String::from_utf16(&words).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_icc_header(class: &[u8; 4], color_space: &[u8; 4], pcs: &[u8; 4]) -> Vec<u8> {
        let mut data = vec![0u8; 128];

        // Size
        let size = 128u32 + 4; // header + tag count (0)
        data[..4].copy_from_slice(&size.to_be_bytes());

        // CMM type
        data[4..8].copy_from_slice(b"APPL");

        // Version 4.3.0
        data[8] = 4;
        data[9] = 0x30;

        // Device class
        data[12..16].copy_from_slice(class);

        // Color space
        data[16..20].copy_from_slice(color_space);

        // PCS
        data[20..24].copy_from_slice(pcs);

        // Date: 2024-01-15 12:00:00
        data[24..26].copy_from_slice(&2024u16.to_be_bytes());
        data[26..28].copy_from_slice(&1u16.to_be_bytes());
        data[28..30].copy_from_slice(&15u16.to_be_bytes());
        data[30..32].copy_from_slice(&12u16.to_be_bytes());
        data[32..34].copy_from_slice(&0u16.to_be_bytes());
        data[34..36].copy_from_slice(&0u16.to_be_bytes());

        // 'acsp' signature
        data[36..40].copy_from_slice(b"acsp");

        // Rendering intent = 1 (relative colorimetric) at offset 64
        data[64..68].copy_from_slice(&1u32.to_be_bytes());

        // Tag count = 0
        data.extend_from_slice(&0u32.to_be_bytes());

        data
    }

    #[test]
    fn c1_header_parsing() {
        let data = build_icc_header(b"mntr", b"RGB ", b"XYZ ");
        let profile = parse_icc_profile(&data).unwrap();
        assert_eq!(profile.version, (4, 3, 0));
        assert_eq!(profile.device_class, ProfileClass::Display);
        assert_eq!(profile.color_space, ColorSpace::Rgb);
        assert_eq!(profile.pcs, ColorSpace::Xyz);
        assert_eq!(profile.date_time.year, 2024);
        assert_eq!(profile.date_time.month, 1);
    }

    #[test]
    fn c1_too_short() {
        assert!(parse_icc_profile(&[0; 100]).is_err());
    }

    #[test]
    fn c1_bad_signature() {
        let mut data = vec![0u8; 132];
        data[..4].copy_from_slice(&132u32.to_be_bytes());
        data[36..40].copy_from_slice(b"XXXX"); // wrong sig
        assert!(parse_icc_profile(&data).is_err());
    }

    #[test]
    fn c2_tag_table() {
        let mut data = build_icc_header(b"mntr", b"RGB ", b"XYZ ");
        // Replace tag count with 1
        let tc_off = 128;
        data[tc_off..tc_off + 4].copy_from_slice(&1u32.to_be_bytes());

        // Tag entry: 'desc', data starts after tag table
        let tag_data_offset = (data.len() + 12) as u32; // after this 12-byte entry
        data.extend_from_slice(b"desc");
        data.extend_from_slice(&tag_data_offset.to_be_bytes());
        let tag_data_size = 4 + 4 + 4 + 5; // type(4)+reserved(4)+count(4)+string(5)
        data.extend_from_slice(&(tag_data_size as u32).to_be_bytes());

        // Tag data: textDescriptionType
        data.extend_from_slice(b"desc"); // type sig
        data.extend_from_slice(&0u32.to_be_bytes()); // reserved
        data.extend_from_slice(&5u32.to_be_bytes()); // count
        data.extend_from_slice(b"sRGB\0");

        // Update size
        let total = data.len() as u32;
        data[..4].copy_from_slice(&total.to_be_bytes());

        let profile = parse_icc_profile(&data).unwrap();
        assert_eq!(profile.tags.len(), 1);
        assert_eq!(profile.tags[0].signature_str(), "desc");
    }

    #[test]
    fn c3_description() {
        let mut data = build_icc_header(b"mntr", b"RGB ", b"XYZ ");
        let tc_off = 128;
        data[tc_off..tc_off + 4].copy_from_slice(&1u32.to_be_bytes());

        // desc tag entry - data starts after this 12-byte entry
        let tag_data_offset = (data.len() + 12) as u32;
        data.extend_from_slice(b"desc");
        data.extend_from_slice(&tag_data_offset.to_be_bytes());
        let tag_data_size = 4 + 4 + 4 + 13; // type+reserved+count+string
        data.extend_from_slice(&(tag_data_size as u32).to_be_bytes());

        // textDescriptionType data
        data.extend_from_slice(b"desc");
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&13u32.to_be_bytes());
        data.extend_from_slice(b"sRGB Profile\0");

        let total = data.len() as u32;
        data[..4].copy_from_slice(&total.to_be_bytes());

        let profile = parse_icc_profile(&data).unwrap();
        assert_eq!(profile.description, Some("sRGB Profile".into()));
    }

    #[test]
    fn c4_copyright() {
        let mut data = build_icc_header(b"mntr", b"RGB ", b"XYZ ");
        let tc_off = 128;
        data[tc_off..tc_off + 4].copy_from_slice(&1u32.to_be_bytes());

        // cprt tag entry - data starts after this 12-byte entry
        let tag_data_offset = (data.len() + 12) as u32;
        data.extend_from_slice(b"cprt");
        data.extend_from_slice(&tag_data_offset.to_be_bytes());
        let tag_data_size = 4 + 4 + 18; // type+reserved+string
        data.extend_from_slice(&(tag_data_size as u32).to_be_bytes());

        // textType data
        data.extend_from_slice(b"text");
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(b"CC0 Public Domain\0");

        let total = data.len() as u32;
        data[..4].copy_from_slice(&total.to_be_bytes());

        let profile = parse_icc_profile(&data).unwrap();
        assert_eq!(profile.copyright, Some("CC0 Public Domain".into()));
    }

    #[test]
    fn c5_device_classes() {
        assert_eq!(ProfileClass::from_bytes(b"scnr"), ProfileClass::Input);
        assert_eq!(ProfileClass::from_bytes(b"mntr"), ProfileClass::Display);
        assert_eq!(ProfileClass::from_bytes(b"prtr"), ProfileClass::Output);
        assert_eq!(ProfileClass::from_bytes(b"link"), ProfileClass::Link);
    }

    #[test]
    fn c5_color_spaces() {
        assert_eq!(ColorSpace::from_bytes(b"RGB "), ColorSpace::Rgb);
        assert_eq!(ColorSpace::from_bytes(b"CMYK"), ColorSpace::Cmyk);
        assert_eq!(ColorSpace::from_bytes(b"GRAY"), ColorSpace::Gray);
        assert_eq!(ColorSpace::from_bytes(b"Lab "), ColorSpace::Lab);
    }

    #[test]
    fn c5_rendering_intents() {
        assert_eq!(RenderingIntent::from_u32(0), RenderingIntent::Perceptual);
        assert_eq!(
            RenderingIntent::from_u32(1),
            RenderingIntent::RelativeColorimetric
        );
        assert_eq!(RenderingIntent::from_u32(2), RenderingIntent::Saturation);
        assert_eq!(
            RenderingIntent::from_u32(3),
            RenderingIntent::AbsoluteColorimetric
        );
    }

    #[test]
    fn c5_display_names() {
        assert_eq!(ProfileClass::Display.as_str(), "Display Device");
        assert_eq!(ColorSpace::Rgb.as_str(), "RGB");
        assert_eq!(RenderingIntent::Perceptual.as_str(), "Perceptual");
    }
}
