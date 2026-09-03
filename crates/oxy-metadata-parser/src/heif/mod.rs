//! HEIF/HEIC ISOBMFF parser (H1-H6).

use crate::core::{Error, Reader, Result};

/// A parsed ISOBMFF box.
#[derive(Debug, Clone)]
pub struct Box<'a> {
    /// Box type (4-character code).
    pub box_type: [u8; 4],
    /// Byte offset of this box in the file.
    pub offset: u64,
    /// Total box size (including header).
    pub size: u64,
    /// Box payload data (excluding header).
    pub data: &'a [u8],
}

impl<'a> Box<'a> {
    pub fn type_str(&self) -> &str {
        std::str::from_utf8(&self.box_type).unwrap_or("????")
    }
}

/// File type information from ftyp box (H2).
#[derive(Debug, Clone)]
pub struct FileTypeBox {
    pub major_brand: [u8; 4],
    pub minor_version: u32,
    pub compatible_brands: Vec<[u8; 4]>,
}

/// Parsed HEIF metadata.
#[derive(Debug)]
pub struct HeifInfo<'a> {
    pub ftyp: FileTypeBox,
    pub boxes: Vec<Box<'a>>,
    /// Image width from ispe property (H6).
    pub width: Option<u32>,
    /// Image height from ispe property (H6).
    pub height: Option<u32>,
    /// Raw EXIF data (H4).
    pub exif_data: Option<&'a [u8]>,
    /// Raw XMP data (H5).
    pub xmp_data: Option<&'a [u8]>,
    /// ICC profile data from colr box.
    pub icc_data: Option<&'a [u8]>,
    /// Rotation angle in degrees from irot box (0, 90, 180, 270).
    pub rotation: Option<u16>,
    /// Bit depth per channel from pixi box.
    pub pixel_depths: Option<Vec<u8>>,
    /// HEVC/AV1 codec configuration.
    pub codec_config: Option<CodecConfig>,
    /// Auxiliary image type URN (e.g., HDR gain map).
    pub aux_type: Option<String>,
    /// Handler type from hdlr box.
    pub handler_type: Option<[u8; 4]>,
    /// Primary item ID from pitm box.
    pub primary_item_id: Option<u32>,
    /// Media data box offset in file.
    pub mdat_offset: Option<u64>,
    /// Media data box size.
    pub mdat_size: Option<u64>,
}

/// HEVC or AV1 codec configuration extracted from hvcC/av1C boxes.
#[derive(Debug, Clone)]
pub struct CodecConfig {
    pub codec: [u8; 4],
    pub profile_idc: u8,
    pub level_idc: u8,
    pub chroma_format: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    // HEVC-specific fields (only set for hvcC)
    pub config_version: u8,
    pub general_profile_space: u8,
    pub general_tier_flag: bool,
    pub general_profile_compat_flags: u32,
    pub constraint_indicator_flags: [u8; 6],
    pub min_spatial_segmentation_idc: u16,
    pub parallelism_type: u8,
    pub num_temporal_layers: u8,
    pub temporal_id_nested: bool,
    pub constant_frame_rate: u8,
    pub avg_frame_rate: u16,
}

/// H1: Parse ISOBMFF box structure.
pub fn parse_boxes<'a>(data: &'a [u8]) -> Result<Vec<Box<'a>>> {
    let mut boxes = Vec::new();
    let mut offset = 0u64;

    while (offset as usize) + 8 <= data.len() {
        let off = offset as usize;
        let mut reader = Reader::new(&data[off..]);

        let size32 = reader.read_u32_be()? as u64;
        let type_bytes = reader.read_bytes(4)?;
        let box_type: [u8; 4] = [type_bytes[0], type_bytes[1], type_bytes[2], type_bytes[3]];

        let (header_size, box_size) = if size32 == 1 {
            // Extended size (64-bit)
            if reader.remaining() < 8 {
                break;
            }
            let size64 = reader.read_u64_be()?;
            (16u64, size64)
        } else if size32 == 0 {
            // Box extends to end of file
            (8u64, (data.len() - off) as u64)
        } else {
            (8u64, size32)
        };

        let payload_size = box_size.saturating_sub(header_size) as usize;
        let payload_start = off + header_size as usize;
        let payload_end = (payload_start + payload_size).min(data.len());

        if payload_start > data.len() {
            break;
        }

        let payload = &data[payload_start..payload_end];

        boxes.push(Box {
            box_type,
            offset,
            size: box_size,
            data: payload,
        });

        offset += box_size;
        if box_size == 0 {
            break;
        }
    }

    Ok(boxes)
}

/// H2: Parse ftyp box.
pub fn parse_ftyp(data: &[u8]) -> Result<FileTypeBox> {
    if data.len() < 8 {
        return Err(Error::Truncated {
            needed: 8,
            available: data.len(),
        });
    }
    let major_brand: [u8; 4] = [data[0], data[1], data[2], data[3]];
    let minor_version = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

    let mut compatible_brands = Vec::new();
    let mut i = 8;
    while i + 4 <= data.len() {
        compatible_brands.push([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        i += 4;
    }

    Ok(FileTypeBox {
        major_brand,
        minor_version,
        compatible_brands,
    })
}

/// Parse a HEIF/HEIC file (H1-H6).
pub fn parse_heif<'a>(data: &'a [u8]) -> Result<HeifInfo<'a>> {
    let boxes = parse_boxes(data)?;

    // H2: Find and parse ftyp
    let ftyp_box = boxes
        .iter()
        .find(|b| &b.box_type == b"ftyp")
        .ok_or_else(|| Error::Format("HEIF: no ftyp box".into()))?;
    let ftyp = parse_ftyp(ftyp_box.data)?;

    let mut width = None;
    let mut height = None;
    let mut exif_data = None;
    let mut xmp_data = None;
    let mut icc_data = None;
    let mut rotation = None;
    let mut pixel_depths = None;
    let mut codec_config = None;
    let mut aux_type = None;
    let mut handler_type = None;
    let mut primary_item_id = None;
    let mut mdat_offset = None;
    let mut mdat_size = None;

    // Find mdat box (media data)
    if let Some(mdat_box) = boxes.iter().find(|b| &b.box_type == b"mdat") {
        mdat_offset = Some(mdat_box.offset);
        mdat_size = Some(mdat_box.size);
    }

    // H3: Find meta box and parse its contents
    if let Some(meta_box) = boxes.iter().find(|b| &b.box_type == b"meta") {
        let meta_data = if meta_box.data.len() >= 4 {
            &meta_box.data[4..] // skip version + flags (fullbox)
        } else {
            meta_box.data
        };

        let meta_children = parse_boxes(meta_data)?;

        // Look for iloc, iinf, iprp, hdlr
        for child in &meta_children {
            match &child.box_type {
                b"hdlr" => {
                    // Handler type at offset 4 (after version+flags)
                    if child.data.len() >= 12 {
                        handler_type =
                            Some([child.data[8], child.data[9], child.data[10], child.data[11]]);
                    }
                }
                b"pitm" => {
                    // Primary item ID: version(1) + flags(3) + item_id(2 or 4)
                    if child.data.len() >= 6 {
                        let version = child.data[0];
                        if version == 0 && child.data.len() >= 6 {
                            primary_item_id =
                                Some(u16::from_be_bytes([child.data[4], child.data[5]]) as u32);
                        } else if child.data.len() >= 8 {
                            primary_item_id = Some(u32::from_be_bytes([
                                child.data[4],
                                child.data[5],
                                child.data[6],
                                child.data[7],
                            ]));
                        }
                    }
                }
                b"iprp" => {
                    // Parse ipco (item property container) inside iprp
                    let iprp_children = parse_boxes(child.data)?;
                    for ipco in iprp_children.iter().filter(|b| &b.box_type == b"ipco") {
                        let props = parse_boxes(ipco.data)?;
                        for prop in &props {
                            match &prop.box_type {
                                // H6: Find ispe (image spatial extents)
                                // Take the largest ispe (primary image, not aux)
                                b"ispe" if prop.data.len() >= 12 => {
                                    let d = prop.data;
                                    // Skip version(1) + flags(3)
                                    let w = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
                                    let h = u32::from_be_bytes([d[8], d[9], d[10], d[11]]);
                                    let cur_pixels =
                                        width.unwrap_or(0) as u64 * height.unwrap_or(0) as u64;
                                    let new_pixels = w as u64 * h as u64;
                                    if new_pixels > cur_pixels {
                                        width = Some(w);
                                        height = Some(h);
                                    }
                                }
                                // ICC profile from colr box
                                b"colr" if prop.data.len() >= 4 => {
                                    let colour_type = &prop.data[..4];
                                    if colour_type == b"prof" || colour_type == b"rICC" {
                                        icc_data = Some(&prop.data[4..]);
                                    }
                                }
                                // irot: image rotation
                                b"irot" if !prop.data.is_empty() => {
                                    // Single byte: rotation angle (1=90, 2=180, 3=270)
                                    let angle = match prop.data[0] & 0x03 {
                                        1 => 270, // HEIF spec: angle 1 = 270° CCW = 90° CW
                                        2 => 180,
                                        3 => 90,
                                        _ => 0,
                                    };
                                    rotation = Some(angle);
                                }
                                // pixi: pixel information (bit depths per channel)
                                b"pixi" if prop.data.len() >= 5 => {
                                    // version(1) + flags(3) + num_channels(1) + depths
                                    let num_channels = prop.data[4] as usize;
                                    if prop.data.len() >= 5 + num_channels {
                                        pixel_depths =
                                            Some(prop.data[5..5 + num_channels].to_vec());
                                    }
                                }
                                // hvcC: HEVC decoder configuration
                                b"hvcC" if prop.data.len() >= 23 && codec_config.is_none() => {
                                    let d = prop.data;
                                    // HEVCDecoderConfigurationRecord layout:
                                    // [0]  configurationVersion
                                    // [1]  general_profile_space(2) | general_tier_flag(1) | general_profile_idc(5)
                                    // [2-5] general_profile_compatibility_flags
                                    // [6-11] general_constraint_indicator_flags
                                    // [12] general_level_idc
                                    // [13-14] min_spatial_segmentation_idc (reserved upper 4 bits)
                                    // [15] parallelismType (reserved upper 6 bits)
                                    // [16] chromaFormatIdc (reserved upper 6 bits)
                                    // [17] bitDepthLumaMinus8 (reserved upper 5 bits)
                                    // [18] bitDepthChromaMinus8 (reserved upper 5 bits)
                                    // [19-20] avgFrameRate
                                    // [21] constantFrameRate(2) | numTemporalLayers(3) | temporalIdNested(1) | lengthSizeMinusOne(2)
                                    let config_version = d[0];
                                    let general_profile_space = (d[1] >> 6) & 0x03;
                                    let general_tier_flag = (d[1] >> 5) & 0x01 != 0;
                                    let profile_idc = d[1] & 0x1F;
                                    let general_profile_compat_flags =
                                        u32::from_be_bytes([d[2], d[3], d[4], d[5]]);
                                    let mut constraint_indicator_flags = [0u8; 6];
                                    constraint_indicator_flags.copy_from_slice(&d[6..12]);
                                    let level_idc = d[12];
                                    let min_spatial_segmentation_idc =
                                        u16::from_be_bytes([d[13] & 0x0F, d[14]]);
                                    let parallelism_type = d[15] & 0x03;
                                    let chroma_format = d[16] & 0x03;
                                    let bit_depth_luma = (d[17] & 0x07) + 8;
                                    let bit_depth_chroma = (d[18] & 0x07) + 8;
                                    let avg_frame_rate = u16::from_be_bytes([d[19], d[20]]);
                                    let constant_frame_rate = (d[21] >> 6) & 0x03;
                                    let num_temporal_layers = (d[21] >> 3) & 0x07;
                                    let temporal_id_nested = (d[21] >> 2) & 0x01 != 0;
                                    codec_config = Some(CodecConfig {
                                        codec: *b"hvcC",
                                        profile_idc,
                                        level_idc,
                                        chroma_format,
                                        bit_depth_luma,
                                        bit_depth_chroma,
                                        config_version,
                                        general_profile_space,
                                        general_tier_flag,
                                        general_profile_compat_flags,
                                        constraint_indicator_flags,
                                        min_spatial_segmentation_idc,
                                        parallelism_type,
                                        num_temporal_layers,
                                        temporal_id_nested,
                                        constant_frame_rate,
                                        avg_frame_rate,
                                    });
                                }
                                // av1C: AV1 codec configuration
                                b"av1C" if prop.data.len() >= 4 => {
                                    let d = prop.data;
                                    let profile_idc = (d[1] >> 5) & 0x07;
                                    let level_idc = d[1] & 0x1F;
                                    let chroma = (d[2] >> 4) & 0x03;
                                    let depth = if d[2] & 0x40 != 0 {
                                        if d[2] & 0x20 != 0 { 12 } else { 10 }
                                    } else {
                                        8
                                    };
                                    codec_config = Some(CodecConfig {
                                        codec: *b"av1C",
                                        profile_idc,
                                        level_idc,
                                        chroma_format: chroma,
                                        bit_depth_luma: depth,
                                        bit_depth_chroma: depth,
                                        config_version: d[0] & 0x7F,
                                        general_profile_space: 0,
                                        general_tier_flag: false,
                                        general_profile_compat_flags: 0,
                                        constraint_indicator_flags: [0; 6],
                                        min_spatial_segmentation_idc: 0,
                                        parallelism_type: 0,
                                        num_temporal_layers: 0,
                                        temporal_id_nested: false,
                                        constant_frame_rate: 0,
                                        avg_frame_rate: 0,
                                    });
                                }
                                // auxC: auxiliary type information
                                b"auxC" if prop.data.len() > 4 => {
                                    // version(1) + flags(3) + null-terminated URN string
                                    if let Some(end) = prop.data[4..].iter().position(|&b| b == 0) {
                                        if let Ok(s) = std::str::from_utf8(&prop.data[4..4 + end]) {
                                            aux_type = Some(s.to_string());
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                // H4, H5: Look for EXIF/XMP in iloc-referenced items
                // Simplified: scan for Exif header or XML content in item data
                _ => {}
            }
        }

        // H4: Find EXIF - look for iloc entries pointing to EXIF items
        // Simplified approach: scan for "Exif" in the file near meta references
        // (Full iloc parsing would be more robust but much more complex)
        exif_data = find_exif_in_heif(data);

        // H5: XMP - look for XMP data
        xmp_data = find_xmp_in_heif(data);
    }

    Ok(HeifInfo {
        ftyp,
        boxes,
        width,
        height,
        exif_data,
        xmp_data,
        icc_data,
        rotation,
        pixel_depths,
        codec_config,
        aux_type,
        handler_type,
        primary_item_id,
        mdat_offset,
        mdat_size,
    })
}

/// EXIF finder for HEIF - scans for Exif item data (H4).
///
/// HEIF Exif items use the format:
///   4 bytes: tiff_header_offset (big-endian) - offset from start of payload to TIFF header
///   N bytes: prefix data (usually "Exif\0\0" when offset=6, or empty when offset=0)
///   TIFF header + IFDs
fn find_exif_in_heif<'a>(data: &'a [u8]) -> Option<&'a [u8]> {
    // Strategy 1: Look for "Exif\0\0" followed by TIFF header
    let exif_marker = b"Exif\x00\x00";
    for i in 0..data.len().saturating_sub(14) {
        if &data[i..i + 6] == exif_marker {
            let tiff_start = i + 6;
            if tiff_start + 8 <= data.len() {
                let is_tiff = (data[tiff_start] == b'I'
                    && data[tiff_start + 1] == b'I'
                    && data[tiff_start + 2] == 0x2A
                    && data[tiff_start + 3] == 0x00)
                    || (data[tiff_start] == b'M'
                        && data[tiff_start + 1] == b'M'
                        && data[tiff_start + 2] == 0x00
                        && data[tiff_start + 3] == 0x2A);
                if is_tiff {
                    return Some(&data[tiff_start..]);
                }
            }
        }
    }

    // Strategy 2: Look for TIFF header with a small 4-byte offset prefix
    for i in 4..data.len().saturating_sub(8) {
        let is_tiff =
            (data[i] == b'I' && data[i + 1] == b'I' && data[i + 2] == 0x2A && data[i + 3] == 0x00)
                || (data[i] == b'M'
                    && data[i + 1] == b'M'
                    && data[i + 2] == 0x00
                    && data[i + 3] == 0x2A);
        if is_tiff {
            let prefix = u32::from_be_bytes([data[i - 4], data[i - 3], data[i - 2], data[i - 1]]);
            if prefix == 0 {
                return Some(&data[i..]);
            }
        }
    }

    None
}

/// Simplified XMP finder for HEIF - scans for XMP data (H5).
fn find_xmp_in_heif<'a>(data: &'a [u8]) -> Option<&'a [u8]> {
    // Look for "<?xpacket" or "<x:xmpmeta"
    let markers: &[&[u8]] = &[b"<?xpacket", b"<x:xmpmeta"];
    for marker in markers {
        if let Some(pos) = data.windows(marker.len()).position(|w| w == *marker) {
            // Try <?xpacket end?> first
            let end_marker = b"<?xpacket end";
            if let Some(end_pos) = data[pos..]
                .windows(end_marker.len())
                .position(|w| w == end_marker)
                .and_then(|p| {
                    data[pos + p..]
                        .iter()
                        .position(|&b| b == b'>')
                        .map(|q| pos + p + q + 1)
                })
            {
                return Some(&data[pos..end_pos]);
            }
            // Fall back to </x:xmpmeta>
            let meta_end = b"</x:xmpmeta>";
            if let Some(p) = data[pos..]
                .windows(meta_end.len())
                .position(|w| w == meta_end)
            {
                return Some(&data[pos..pos + p + meta_end.len()]);
            }
            // Last resort: cap at null byte or 1MB
            let max_len = (data.len() - pos).min(1024 * 1024);
            let end = data[pos..pos + max_len]
                .iter()
                .position(|&b| b == 0)
                .map(|p| pos + p)
                .unwrap_or(pos + max_len);
            return Some(&data[pos..end]);
        }
    }
    None
}

/// Check if a brand indicates HEIF/HEIC.
pub fn is_heif_brand(brand: &[u8; 4]) -> bool {
    matches!(
        brand,
        b"heic"
            | b"heix"
            | b"hevc"
            | b"hevx"
            | b"heim"
            | b"heis"
            | b"hevm"
            | b"hevs"
            | b"mif1"
            | b"msf1"
            | b"avif"
            | b"avis"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_box(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (payload.len() + 8) as u32;
        let mut data = size.to_be_bytes().to_vec();
        data.extend_from_slice(box_type);
        data.extend_from_slice(payload);
        data
    }

    fn build_heif_minimal() -> Vec<u8> {
        let mut ftyp_payload = b"heic".to_vec(); // major brand
        ftyp_payload.extend_from_slice(&0u32.to_be_bytes()); // minor version
        ftyp_payload.extend_from_slice(b"heic"); // compatible brand

        let mut data = build_box(b"ftyp", &ftyp_payload);

        // meta box (fullbox with 4 bytes version+flags)
        let meta_payload = vec![0, 0, 0, 0]; // version + flags
        // Empty meta for now
        let meta = build_box(b"meta", &meta_payload);
        data.extend_from_slice(&meta);

        data
    }

    #[test]
    fn h1_box_structure() {
        let data = build_box(b"test", b"hello");
        let boxes = parse_boxes(&data).unwrap();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].type_str(), "test");
        assert_eq!(boxes[0].data, b"hello");
        assert_eq!(boxes[0].size, 13);
    }

    #[test]
    fn h1_extended_size() {
        let mut data = vec![0, 0, 0, 1]; // size = 1 (extended)
        data.extend_from_slice(b"test");
        data.extend_from_slice(&24u64.to_be_bytes()); // real size = 24
        data.extend_from_slice(b"12345678"); // 8 bytes payload

        let boxes = parse_boxes(&data).unwrap();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].data.len(), 8);
    }

    #[test]
    fn h2_ftyp() {
        let mut payload = b"heic".to_vec();
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(b"heicmif1");

        let ftyp = parse_ftyp(&payload).unwrap();
        assert_eq!(&ftyp.major_brand, b"heic");
        assert_eq!(ftyp.compatible_brands.len(), 2);
        assert_eq!(&ftyp.compatible_brands[0], b"heic");
        assert_eq!(&ftyp.compatible_brands[1], b"mif1");
    }

    #[test]
    fn h2_brand_detection() {
        assert!(is_heif_brand(b"heic"));
        assert!(is_heif_brand(b"avif"));
        assert!(is_heif_brand(b"mif1"));
        assert!(!is_heif_brand(b"mp41"));
        assert!(!is_heif_brand(b"isom"));
    }

    #[test]
    fn h3_parse_heif() {
        let data = build_heif_minimal();
        let info = parse_heif(&data).unwrap();
        assert_eq!(&info.ftyp.major_brand, b"heic");
    }

    #[test]
    fn multiple_boxes() {
        let mut data = build_box(b"ftyp", b"heic\0\0\0\0");
        data.extend_from_slice(&build_box(b"mdat", b"imagedata"));
        let boxes = parse_boxes(&data).unwrap();
        assert_eq!(boxes.len(), 2);
    }
}
