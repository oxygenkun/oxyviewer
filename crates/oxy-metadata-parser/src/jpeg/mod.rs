//! JPEG segment parser (J1-J12).

use crate::core::{Error, Reader, Result};

/// JPEG marker bytes (the byte after `0xFF`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Marker {
    /// Start of Image
    Soi = 0xD8,
    /// End of Image
    Eoi = 0xD9,
    /// Start of Scan
    Sos = 0xDA,
    /// Define Quantization Table
    Dqt = 0xDB,
    /// Define Huffman Table
    Dht = 0xC4,
    /// Define Arithmetic Coding
    Dac = 0xCC,
    /// Define Restart Interval
    Dri = 0xDD,
    /// Comment
    Com = 0xFE,
    /// Start of Frame - Baseline DCT
    Sof0 = 0xC0,
    /// Start of Frame - Extended Sequential DCT
    Sof1 = 0xC1,
    /// Start of Frame - Progressive DCT
    Sof2 = 0xC2,
    /// Start of Frame - Lossless
    Sof3 = 0xC3,
    /// Start of Frame - Differential Sequential DCT
    Sof5 = 0xC5,
    /// Start of Frame - Differential Progressive DCT
    Sof6 = 0xC6,
    /// Start of Frame - Differential Lossless
    Sof7 = 0xC7,
    /// Start of Frame - Extended Sequential DCT (arithmetic)
    Sof9 = 0xC9,
    /// Start of Frame - Progressive DCT (arithmetic)
    Sof10 = 0xCA,
    /// Start of Frame - Lossless (arithmetic)
    Sof11 = 0xCB,
    /// Start of Frame - Differential Sequential DCT (arithmetic)
    Sof13 = 0xCD,
    /// Start of Frame - Differential Progressive DCT (arithmetic)
    Sof14 = 0xCE,
    /// Start of Frame - Differential Lossless (arithmetic)
    Sof15 = 0xCF,
    /// APP0 (JFIF)
    App0 = 0xE0,
    /// APP1 (EXIF / XMP)
    App1 = 0xE1,
    /// APP2 (ICC profile / FlashPix)
    App2 = 0xE2,
    /// APP3
    App3 = 0xE3,
    /// APP4
    App4 = 0xE4,
    /// APP5
    App5 = 0xE5,
    /// APP6
    App6 = 0xE6,
    /// APP7
    App7 = 0xE7,
    /// APP8
    App8 = 0xE8,
    /// APP9
    App9 = 0xE9,
    /// APP10
    App10 = 0xEA,
    /// APP11
    App11 = 0xEB,
    /// APP12
    App12 = 0xEC,
    /// APP13 (IPTC / Photoshop IRB)
    App13 = 0xED,
    /// APP14 (Adobe)
    App14 = 0xEE,
    /// APP15
    App15 = 0xEF,
}

impl Marker {
    /// Try to convert a raw byte to a known marker.
    pub fn from_byte(b: u8) -> Option<Marker> {
        match b {
            0xD8 => Some(Marker::Soi),
            0xD9 => Some(Marker::Eoi),
            0xDA => Some(Marker::Sos),
            0xDB => Some(Marker::Dqt),
            0xC4 => Some(Marker::Dht),
            0xCC => Some(Marker::Dac),
            0xDD => Some(Marker::Dri),
            0xFE => Some(Marker::Com),
            0xC0 => Some(Marker::Sof0),
            0xC1 => Some(Marker::Sof1),
            0xC2 => Some(Marker::Sof2),
            0xC3 => Some(Marker::Sof3),
            0xC5 => Some(Marker::Sof5),
            0xC6 => Some(Marker::Sof6),
            0xC7 => Some(Marker::Sof7),
            0xC9 => Some(Marker::Sof9),
            0xCA => Some(Marker::Sof10),
            0xCB => Some(Marker::Sof11),
            0xCD => Some(Marker::Sof13),
            0xCE => Some(Marker::Sof14),
            0xCF => Some(Marker::Sof15),
            0xE0 => Some(Marker::App0),
            0xE1 => Some(Marker::App1),
            0xE2 => Some(Marker::App2),
            0xE3 => Some(Marker::App3),
            0xE4 => Some(Marker::App4),
            0xE5 => Some(Marker::App5),
            0xE6 => Some(Marker::App6),
            0xE7 => Some(Marker::App7),
            0xE8 => Some(Marker::App8),
            0xE9 => Some(Marker::App9),
            0xEA => Some(Marker::App10),
            0xEB => Some(Marker::App11),
            0xEC => Some(Marker::App12),
            0xED => Some(Marker::App13),
            0xEE => Some(Marker::App14),
            0xEF => Some(Marker::App15),
            _ => None,
        }
    }

    /// Returns true if this is a standalone marker with no payload (SOI, EOI, RST0-RST7, TEM).
    fn is_standalone(byte: u8) -> bool {
        // SOI (D8), EOI (D9), RST0-RST7 (D0-D7), TEM (01)
        matches!(byte, 0xD8 | 0xD9 | 0xD0..=0xD7 | 0x01)
    }

    /// Returns true if this is a SOF marker (any frame type).
    pub fn is_sof(&self) -> bool {
        matches!(
            self,
            Marker::Sof0
                | Marker::Sof1
                | Marker::Sof2
                | Marker::Sof3
                | Marker::Sof5
                | Marker::Sof6
                | Marker::Sof7
                | Marker::Sof9
                | Marker::Sof10
                | Marker::Sof11
                | Marker::Sof13
                | Marker::Sof14
                | Marker::Sof15
        )
    }

    /// Returns true if this is a progressive DCT frame marker.
    pub fn is_progressive(&self) -> bool {
        matches!(
            self,
            Marker::Sof2 | Marker::Sof6 | Marker::Sof10 | Marker::Sof14
        )
    }
}

impl std::fmt::Display for Marker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} (0x{:02X})", self, *self as u8)
    }
}

/// A parsed JPEG segment.
#[derive(Debug, Clone)]
pub struct Segment<'a> {
    /// The marker identifying this segment.
    pub marker: Marker,
    /// The raw marker byte (useful for unknown markers).
    pub marker_byte: u8,
    /// Byte offset of the `FF` byte in the file.
    pub offset: usize,
    /// Segment payload (excluding the marker and length field).
    /// Empty for standalone markers (SOI, EOI).
    /// For SOS, this contains the header only (not the entropy-coded data).
    pub data: &'a [u8],
    /// For SOS segments: offset and length of the entropy-coded data that follows.
    pub entropy_data: Option<(usize, usize)>,
}

/// Identifies what kind of APP1 payload this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App1Kind {
    /// EXIF data (starts with `Exif\0\0`)
    Exif,
    /// XMP data (starts with `http://ns.adobe.com/xap/1.0/\0`)
    Xmp,
    /// Extended XMP (starts with `http://ns.adobe.com/xmp/extension/\0`)
    ExtendedXmp,
    /// Unknown APP1 payload
    Unknown,
}

const EXIF_HEADER: &[u8] = b"Exif\0\0";
const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXT_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const JFIF_HEADER: &[u8] = b"JFIF\0";
const ICC_HEADER: &[u8] = b"ICC_PROFILE\0";
const PHOTOSHOP_HEADER: &[u8] = b"Photoshop 3.0\0";

impl<'a> Segment<'a> {
    /// For APP1 segments, detect whether this is EXIF, XMP, or Extended XMP.
    pub fn app1_kind(&self) -> Option<App1Kind> {
        if self.marker != Marker::App1 {
            return None;
        }
        Some(if self.data.starts_with(EXIF_HEADER) {
            App1Kind::Exif
        } else if self.data.starts_with(XMP_HEADER) {
            App1Kind::Xmp
        } else if self.data.starts_with(XMP_EXT_HEADER) {
            App1Kind::ExtendedXmp
        } else {
            App1Kind::Unknown
        })
    }

    /// For EXIF APP1 segments, returns the embedded TIFF data (after `Exif\0\0`).
    pub fn exif_tiff_data(&self) -> Option<&'a [u8]> {
        if self.marker == Marker::App1 && self.data.starts_with(EXIF_HEADER) {
            Some(&self.data[EXIF_HEADER.len()..])
        } else {
            None
        }
    }

    /// For XMP APP1 segments, returns the XMP XML (after the namespace header).
    pub fn xmp_data(&self) -> Option<&'a [u8]> {
        if self.marker == Marker::App1 && self.data.starts_with(XMP_HEADER) {
            Some(&self.data[XMP_HEADER.len()..])
        } else {
            None
        }
    }

    /// For Extended XMP APP1 segments, returns the raw payload after the header.
    ///
    /// The returned data contains: 32-byte GUID + 4-byte total_len + 4-byte offset + XMP chunk.
    pub fn extended_xmp_payload(&self) -> Option<&'a [u8]> {
        if self.marker == Marker::App1 && self.data.starts_with(XMP_EXT_HEADER) {
            Some(&self.data[XMP_EXT_HEADER.len()..])
        } else {
            None
        }
    }

    /// Returns true if this is an APP0/JFIF segment.
    pub fn is_jfif(&self) -> bool {
        self.marker == Marker::App0 && self.data.starts_with(JFIF_HEADER)
    }

    /// Returns true if this is an APP2/ICC_PROFILE segment.
    pub fn is_icc_profile(&self) -> bool {
        self.marker == Marker::App2 && self.data.starts_with(ICC_HEADER)
    }

    /// For APP2/ICC_PROFILE segments, returns (chunk_number, total_chunks, profile_data).
    pub fn icc_profile_chunk(&self) -> Option<(u8, u8, &'a [u8])> {
        if !self.is_icc_profile() || self.data.len() < ICC_HEADER.len() + 2 {
            return None;
        }
        let chunk_num = self.data[ICC_HEADER.len()];
        let total = self.data[ICC_HEADER.len() + 1];
        let data = &self.data[ICC_HEADER.len() + 2..];
        Some((chunk_num, total, data))
    }

    /// Returns true if this is an APP13/Photoshop segment (contains IPTC).
    pub fn is_photoshop(&self) -> bool {
        self.marker == Marker::App13 && self.data.starts_with(PHOTOSHOP_HEADER)
    }

    /// Returns Canon CIFF data if this APP0 segment contains a HEAPJPGM block.
    /// The returned slice starts at the "II" byte order mark before the CIFF header.
    pub fn ciff_data(&self) -> Option<&'a [u8]> {
        if self.marker != Marker::App0 {
            return None;
        }
        // Look for "II" + header_offset + "HEAPJPGM" pattern
        // The CIFF block starts with byte order (II or MM), then 4-byte header length,
        // then "HEAPJPGM" signature
        if self.data.len() >= 14 {
            let bom = &self.data[..2];
            if (bom == b"II" || bom == b"MM") && &self.data[6..14] == b"HEAPJPGM" {
                return Some(self.data);
            }
        }
        None
    }

    /// Returns Qualcomm Camera Attributes data if this is an APP7 segment with the
    /// `\x1aQualcomm Camera Attributes` header.
    pub fn qualcomm_data(&self) -> Option<&'a [u8]> {
        const HEADER: &[u8] = b"\x1aQualcomm Camera Attributes";
        if self.marker == Marker::App7
            && self.data.len() > HEADER.len()
            && self.data.starts_with(HEADER)
        {
            Some(&self.data[HEADER.len()..])
        } else {
            None
        }
    }
}

/// Parse all segments from a JPEG file.
///
/// Handles:
/// - SOI validation (J1), with optional leading garbage bytes (J12)
/// - Full segment enumeration (J2) with correct length handling (J3)
/// - Multiple APP1 segments (J4), APP0/JFIF (J5), APP2/ICC (J6), APP13/IPTC (J7), COM (J8)
/// - SOS entropy-coded data scanning (J9), progressive JPEGs with multiple SOS (J10)
/// - Truncated files - returns partial results (J11)
pub fn parse_segments<'a>(data: &'a [u8]) -> Result<Vec<Segment<'a>>> {
    let mut reader = Reader::new(data);
    let mut segments = Vec::new();

    // J12: Skip leading garbage bytes before SOI
    let soi_offset = find_soi(data)?;
    reader.seek(soi_offset);

    // J1: Validate SOI
    let b0 = reader.read_u8()?;
    let b1 = reader.read_u8()?;
    if b0 != 0xFF || b1 != 0xD8 {
        return Err(Error::Format("not a JPEG: missing SOI marker".into()));
    }

    segments.push(Segment {
        marker: Marker::Soi,
        marker_byte: 0xD8,
        offset: soi_offset,
        data: &[],
        entropy_data: None,
    });

    // J2: Scan segments
    loop {
        // Find next FF marker, skipping any padding FF bytes
        let marker_byte = match find_next_marker(&mut reader) {
            Ok(b) => b,
            Err(_) => break, // J11: truncated - return what we have
        };

        let marker_offset = reader.position() - 2; // position of the FF byte

        // EOI - we're done
        if marker_byte == 0xD9 {
            segments.push(Segment {
                marker: Marker::Eoi,
                marker_byte: 0xD9,
                offset: marker_offset,
                data: &[],
                entropy_data: None,
            });
            break;
        }

        // Standalone markers (RST, TEM) - no payload
        if Marker::is_standalone(marker_byte) {
            if let Some(m) = Marker::from_byte(marker_byte) {
                segments.push(Segment {
                    marker: m,
                    marker_byte,
                    offset: marker_offset,
                    data: &[],
                    entropy_data: None,
                });
            }
            continue;
        }

        // J3: Read segment length (includes the 2 length bytes themselves)
        let length = match reader.read_u16_be() {
            Ok(l) => l as usize,
            Err(_) => break, // J11: truncated
        };

        if length < 2 {
            return Err(Error::Format(format!(
                "invalid segment length {} at offset {marker_offset}",
                length
            )));
        }

        let payload_len = length - 2;

        // J11: If payload extends past EOF, take what we can
        let available = reader.remaining();
        let actual_len = payload_len.min(available);
        let payload = match reader.read_bytes(actual_len) {
            Ok(p) => p,
            Err(_) => break,
        };

        let marker = Marker::from_byte(marker_byte);

        // J9/J10: SOS - scan entropy-coded data
        let entropy_data = if marker_byte == 0xDA {
            let ecs_start = reader.position();
            let ecs_end = scan_entropy_coded_data(data, ecs_start);
            reader.seek(ecs_end);
            Some((ecs_start, ecs_end - ecs_start))
        } else {
            None
        };

        if let Some(m) = marker {
            segments.push(Segment {
                marker: m,
                marker_byte,
                offset: marker_offset,
                data: payload,
                entropy_data,
            });
        }
        // Unknown markers are silently skipped (position already advanced past payload)
    }

    Ok(segments)
}

/// Find SOI marker, allowing leading garbage (J12).
fn find_soi(data: &[u8]) -> Result<usize> {
    if data.len() < 2 {
        return Err(Error::Truncated {
            needed: 2,
            available: data.len(),
        });
    }

    // Fast path: SOI at offset 0
    if data[0] == 0xFF && data[1] == 0xD8 {
        return Ok(0);
    }

    // Scan for SOI (limited to first 64KB to avoid scanning entire non-JPEG files)
    let limit = data.len().min(65536);
    for i in 0..limit - 1 {
        if data[i] == 0xFF && data[i + 1] == 0xD8 {
            return Ok(i);
        }
    }

    Err(Error::Format("not a JPEG: SOI marker not found".into()))
}

/// Find the next marker byte, skipping fill bytes (0xFF padding).
fn find_next_marker(reader: &mut Reader<'_>) -> Result<u8> {
    // Find 0xFF
    loop {
        let b = reader.read_u8()?;
        if b == 0xFF {
            break;
        }
        // Unexpected byte outside entropy-coded data; skip it
    }

    // Skip fill bytes (consecutive 0xFF)
    loop {
        let b = reader.read_u8()?;
        if b != 0xFF && b != 0x00 {
            return Ok(b);
        }
        // 0xFF followed by 0x00 is a stuffed byte (shouldn't happen outside SOS)
        // 0xFF followed by 0xFF is a fill byte - keep looking
        if b == 0x00 {
            // Stuffed byte outside SOS - skip and look for next FF
            return find_next_marker(reader);
        }
    }
}

/// Scan past entropy-coded data after SOS (J9).
///
/// Entropy-coded data ends at the next `FF xx` where `xx` is not `00` (byte stuffing)
/// and not a restart marker `D0`-`D7`.
fn scan_entropy_coded_data(data: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }

        // Found 0xFF - check next byte
        if i + 1 >= data.len() {
            return data.len(); // truncated
        }

        let next = data[i + 1];
        match next {
            // Byte stuffing: FF 00 - part of entropy data
            0x00 => {
                i += 2;
            }
            // Restart markers: FF D0-D7 - part of entropy data
            0xD0..=0xD7 => {
                i += 2;
            }
            // Fill byte
            0xFF => {
                i += 1;
            }
            // Any other marker - end of entropy-coded data
            _ => {
                return i;
            }
        }
    }
    data.len()
}

/// Reassemble extended XMP from all ExtendedXmp APP1 segments.
///
/// Returns the full extended XMP XML string, or `None` if no extended XMP segments exist.
#[cfg(feature = "xmp")] // reassembly is delegated to the xmp parser
pub fn reassemble_extended_xmp(segments: &[Segment<'_>]) -> Option<String> {
    let payloads: Vec<&[u8]> = segments
        .iter()
        .filter_map(|s| {
            if s.app1_kind() == Some(App1Kind::ExtendedXmp) {
                Some(s.data.as_ref())
            } else {
                None
            }
        })
        .collect();

    if payloads.is_empty() {
        return None;
    }

    crate::xmp::reassemble_extended_xmp_from_segments(&payloads)
}

/// Reassemble a multi-segment ICC profile from APP2 segments (J6).
pub fn reassemble_icc_profile(segments: &[Segment<'_>]) -> Option<Vec<u8>> {
    let mut chunks: Vec<(u8, &[u8])> = Vec::new();
    let mut total = 0u8;

    for seg in segments {
        if let Some((num, tot, data)) = seg.icc_profile_chunk() {
            total = tot;
            chunks.push((num, data));
        }
    }

    if chunks.is_empty() || total == 0 {
        return None;
    }

    chunks.sort_by_key(|(num, _)| *num);

    // Verify we have all chunks
    if chunks.len() != total as usize {
        return None;
    }

    let mut profile = Vec::new();
    for (_, data) in &chunks {
        profile.extend_from_slice(data);
    }
    Some(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid JPEG with given segments.
    fn build_jpeg(segments: &[(u8, &[u8])]) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8]; // SOI
        for &(marker, payload) in segments {
            data.push(0xFF);
            data.push(marker);
            let len = (payload.len() + 2) as u16;
            data.extend_from_slice(&len.to_be_bytes());
            data.extend_from_slice(payload);
        }
        data.push(0xFF);
        data.push(0xD9); // EOI
        data
    }

    #[test]
    fn j1_soi_validation() {
        let data = build_jpeg(&[]);
        let segs = parse_segments(&data).unwrap();
        assert_eq!(segs[0].marker, Marker::Soi);
        assert_eq!(segs.last().unwrap().marker, Marker::Eoi);
    }

    #[test]
    fn j1_not_jpeg() {
        let data = b"not a jpeg";
        assert!(parse_segments(data).is_err());
    }

    #[test]
    fn j2_enumerate_segments() {
        let data = build_jpeg(&[
            (0xE0, b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00"),
            (0xE1, b"Exif\0\0II*\0\x08\0\0\0"),
            (0xFE, b"hello"),
        ]);
        let segs = parse_segments(&data).unwrap();
        // SOI + APP0 + APP1 + COM + EOI = 5
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[1].marker, Marker::App0);
        assert_eq!(segs[2].marker, Marker::App1);
        assert_eq!(segs[3].marker, Marker::Com);
    }

    #[test]
    fn j3_segment_length() {
        // Payload = 5 bytes, length field = 7 (5 + 2)
        let data = build_jpeg(&[(0xFE, b"hello")]);
        let segs = parse_segments(&data).unwrap();
        let com = segs.iter().find(|s| s.marker == Marker::Com).unwrap();
        assert_eq!(com.data, b"hello");
    }

    #[test]
    fn j4_multiple_app1() {
        let data = build_jpeg(&[
            (0xE1, b"Exif\0\0II*\0\x08\0\0\0"),
            (0xE1, b"http://ns.adobe.com/xap/1.0/\0<xmp/>"),
        ]);
        let segs = parse_segments(&data).unwrap();
        let app1s: Vec<_> = segs.iter().filter(|s| s.marker == Marker::App1).collect();
        assert_eq!(app1s.len(), 2);
        assert_eq!(app1s[0].app1_kind(), Some(App1Kind::Exif));
        assert_eq!(app1s[1].app1_kind(), Some(App1Kind::Xmp));
    }

    #[test]
    fn j5_jfif_detection() {
        let data = build_jpeg(&[(0xE0, b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00")]);
        let segs = parse_segments(&data).unwrap();
        let app0 = segs.iter().find(|s| s.marker == Marker::App0).unwrap();
        assert!(app0.is_jfif());
    }

    #[test]
    fn j6_icc_profile() {
        // Simulate 2-chunk ICC profile
        let mut chunk1 = b"ICC_PROFILE\0".to_vec();
        chunk1.push(1); // chunk 1
        chunk1.push(2); // of 2
        chunk1.extend_from_slice(b"AAAA");

        let mut chunk2 = b"ICC_PROFILE\0".to_vec();
        chunk2.push(2); // chunk 2
        chunk2.push(2); // of 2
        chunk2.extend_from_slice(b"BBBB");

        let data = build_jpeg(&[(0xE2, &chunk1), (0xE2, &chunk2)]);
        let segs = parse_segments(&data).unwrap();
        let profile = reassemble_icc_profile(&segs).unwrap();
        assert_eq!(profile, b"AAAABBBB");
    }

    #[test]
    fn j7_app13_photoshop() {
        let mut payload = b"Photoshop 3.0\0".to_vec();
        payload.extend_from_slice(b"8BIM\x04\x04\x00\x00");
        let data = build_jpeg(&[(0xED, &payload)]);
        let segs = parse_segments(&data).unwrap();
        let app13 = segs.iter().find(|s| s.marker == Marker::App13).unwrap();
        assert!(app13.is_photoshop());
    }

    #[test]
    fn j8_comment() {
        let data = build_jpeg(&[(0xFE, b"test comment")]);
        let segs = parse_segments(&data).unwrap();
        let com = segs.iter().find(|s| s.marker == Marker::Com).unwrap();
        assert_eq!(com.data, b"test comment");
    }

    #[test]
    fn j9_sos_entropy_scanning() {
        // Build a JPEG with SOS followed by entropy-coded data containing:
        // - Regular bytes
        // - FF 00 (stuffed byte)
        // - FF D0 (restart marker)
        // Then a real marker FF DB (DQT)
        let mut data = vec![0xFF, 0xD8]; // SOI

        // Minimal SOF0 (needed before SOS in real JPEG, but parser doesn't enforce order)
        // SOS header: length=2+1+2+3=8, components=1
        data.extend_from_slice(&[0xFF, 0xDA]); // SOS marker
        data.extend_from_slice(&[0x00, 0x08]); // length = 8
        data.extend_from_slice(&[0x01]); // num components = 1
        data.extend_from_slice(&[0x01, 0x00]); // component 1, dc/ac table 0/0
        data.extend_from_slice(&[0x00, 0x3F, 0x00]); // Ss=0, Se=63, Ah/Al=0

        // Entropy-coded data
        data.extend_from_slice(&[0xAA, 0xBB]); // regular bytes
        data.extend_from_slice(&[0xFF, 0x00]); // stuffed byte (should not end scan)
        data.extend_from_slice(&[0xCC]);
        data.extend_from_slice(&[0xFF, 0xD0]); // RST0 (should not end scan)
        data.extend_from_slice(&[0xDD]);

        // Real marker to end entropy data
        data.extend_from_slice(&[0xFF, 0xD9]); // EOI

        let segs = parse_segments(&data).unwrap();
        let sos = segs.iter().find(|s| s.marker == Marker::Sos).unwrap();
        assert!(sos.entropy_data.is_some());
        let (ecs_off, ecs_len) = sos.entropy_data.unwrap();
        let ecs = &data[ecs_off..ecs_off + ecs_len];
        // Should include everything up to the FF D9
        assert_eq!(ecs, &[0xAA, 0xBB, 0xFF, 0x00, 0xCC, 0xFF, 0xD0, 0xDD]);
    }

    #[test]
    fn j10_progressive_multiple_sos() {
        // Progressive JPEG has multiple SOS segments
        let mut data = vec![0xFF, 0xD8]; // SOI

        // Two SOS segments with minimal entropy data each
        for _ in 0..2 {
            data.extend_from_slice(&[0xFF, 0xDA]); // SOS
            data.extend_from_slice(&[0x00, 0x08]); // length
            data.extend_from_slice(&[0x01, 0x01, 0x00]); // 1 component
            data.extend_from_slice(&[0x00, 0x3F, 0x00]); // Ss, Se, Ah/Al
            data.extend_from_slice(&[0xAA, 0xBB]); // entropy data
        }

        data.extend_from_slice(&[0xFF, 0xD9]); // EOI

        let segs = parse_segments(&data).unwrap();
        let sos_count = segs.iter().filter(|s| s.marker == Marker::Sos).count();
        assert_eq!(sos_count, 2);
    }

    #[test]
    fn j11_truncated_returns_partial() {
        // JPEG truncated after APP0 header - no EOI
        let mut data = vec![0xFF, 0xD8]; // SOI
        data.extend_from_slice(&[0xFF, 0xE0]); // APP0
        data.extend_from_slice(&[0x00, 0x05]); // length = 5 (3 bytes payload)
        data.extend_from_slice(&[0x41, 0x42, 0x43]); // payload "ABC"
        // No EOI - file is truncated

        let segs = parse_segments(&data).unwrap();
        assert!(segs.len() >= 2); // SOI + APP0 at minimum
        assert_eq!(segs[0].marker, Marker::Soi);
        assert_eq!(segs[1].marker, Marker::App0);
        // No EOI expected
        assert!(segs.last().unwrap().marker != Marker::Eoi);
    }

    #[test]
    fn j11_truncated_mid_segment() {
        // Segment claims 100 bytes but file ends after 10
        let mut data = vec![0xFF, 0xD8]; // SOI
        data.extend_from_slice(&[0xFF, 0xFE]); // COM
        data.extend_from_slice(&[0x00, 0x64]); // length = 100 (98 bytes payload)
        data.extend_from_slice(&[0x41; 10]); // only 10 bytes available

        let segs = parse_segments(&data).unwrap();
        assert!(segs.len() >= 2);
        let com = segs.iter().find(|s| s.marker == Marker::Com).unwrap();
        assert_eq!(com.data.len(), 10); // got what was available
    }

    #[test]
    fn j12_leading_garbage() {
        let mut data = vec![0x00, 0x00, 0x00]; // 3 bytes of garbage
        data.push(0xFF);
        data.push(0xD8); // SOI
        data.push(0xFF);
        data.push(0xD9); // EOI

        let segs = parse_segments(&data).unwrap();
        assert_eq!(segs[0].marker, Marker::Soi);
        assert_eq!(segs[0].offset, 3); // SOI found at offset 3
    }

    #[test]
    fn exif_tiff_data_extraction() {
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(b"II*\0\x08\0\0\0");
        let data = build_jpeg(&[(0xE1, &payload)]);
        let segs = parse_segments(&data).unwrap();
        let app1 = segs.iter().find(|s| s.marker == Marker::App1).unwrap();
        let tiff = app1.exif_tiff_data().unwrap();
        assert_eq!(&tiff[..4], b"II*\0");
    }

    #[test]
    fn marker_display() {
        assert_eq!(Marker::Soi.to_string(), "Soi (0xD8)");
        assert_eq!(Marker::App1.to_string(), "App1 (0xE1)");
    }

    #[test]
    fn marker_sof_checks() {
        assert!(Marker::Sof0.is_sof());
        assert!(Marker::Sof2.is_sof());
        assert!(Marker::Sof2.is_progressive());
        assert!(!Marker::Sof0.is_progressive());
        assert!(!Marker::App1.is_sof());
    }
}
