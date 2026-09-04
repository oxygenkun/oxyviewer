//! EXIF decoder (E1-E10).
//!
//! Navigates the TIFF IFD structure embedded in a JPEG APP1 segment,
//! following sub-IFD pointers to ExifIFD, GPS IFD, and Interop IFD.

use crate::core::Result;
use crate::tiff::{self, Ifd, IfdEntry, TiffHeader};

// Well-known EXIF tag IDs
pub const TAG_EXIF_IFD_POINTER: u16 = 0x8769;
pub const TAG_GPS_IFD_POINTER: u16 = 0x8825;
pub const TAG_INTEROP_IFD_POINTER: u16 = 0xA005;
pub const TAG_MAKER_NOTE: u16 = 0x927C;
pub const TAG_USER_COMMENT: u16 = 0x9286;
pub const TAG_EXIF_VERSION: u16 = 0x9000;
pub const TAG_FLASHPIX_VERSION: u16 = 0xA000;
pub const TAG_JPEG_INTERCHANGE_FORMAT: u16 = 0x0201;
pub const TAG_JPEG_INTERCHANGE_FORMAT_LENGTH: u16 = 0x0202;
pub const TAG_COMPRESSION: u16 = 0x0103;

/// Parsed EXIF data from a JPEG APP1 segment or standalone TIFF.
#[derive(Debug)]
pub struct ExifData<'a> {
    /// The TIFF header (byte order, BigTIFF flag).
    pub header: TiffHeader,
    /// IFD0 - primary image tags (Make, Model, Orientation, DateTime, etc.).
    pub ifd0: Ifd<'a>,
    /// IFD1 - thumbnail metadata (if present).
    pub ifd1: Option<Ifd<'a>>,
    /// Exif SubIFD - exposure, ISO, flash, lens, etc.
    pub exif_ifd: Option<Ifd<'a>>,
    /// GPS IFD - latitude, longitude, altitude, timestamp.
    pub gps_ifd: Option<Ifd<'a>>,
    /// Interoperability IFD.
    pub interop_ifd: Option<Ifd<'a>>,
    /// Raw MakerNote data (tag 0x927C from ExifIFD), if present.
    pub maker_note: Option<MakerNoteRef<'a>>,
    /// Embedded thumbnail JPEG data (from IFD1), if present.
    pub thumbnail: Option<&'a [u8]>,
    /// Reference to the full TIFF data (for offset-based access).
    #[allow(dead_code)]
    tiff_data: &'a [u8],
}

/// Reference to the raw MakerNote data for deferred parsing (E8).
#[derive(Debug)]
pub struct MakerNoteRef<'a> {
    /// Raw MakerNote bytes.
    pub data: &'a [u8],
    /// Byte offset of the MakerNote within the TIFF data.
    pub offset: usize,
    /// Detected MakerNote format.
    pub format: MakerNoteFormat,
}

/// Detected MakerNote format (E8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerNoteFormat {
    /// Standard IFD structure (e.g., Canon, Panasonic).
    StandardIfd,
    /// IFD with a known header prefix that must be skipped.
    HeaderIfd {
        /// Bytes to skip before the IFD starts.
        header_size: usize,
        /// Whether offsets are relative to the MakerNote start.
        relative_offsets: bool,
    },
    /// Nikon type 3: own TIFF header inside the MakerNote.
    NikonTiff {
        /// Offset to the embedded TIFF header within the MakerNote.
        tiff_offset: usize,
    },
    /// Unknown format - cannot parse further.
    Unknown,
}

/// Character encoding for UserComment (E9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserCommentEncoding {
    Ascii,
    Jis,
    Unicode,
    Undefined,
}

/// Parsed UserComment value (E9).
#[derive(Debug)]
pub struct UserComment<'a> {
    pub encoding: UserCommentEncoding,
    pub data: &'a [u8],
}

impl<'a> ExifData<'a> {
    /// Parse EXIF data from raw TIFF bytes (E1).
    ///
    /// The input should be the TIFF data after stripping the `Exif\0\0` header
    /// (i.e., what `Segment::exif_tiff_data()` returns).
    pub fn parse(tiff_data: &'a [u8]) -> Result<Self> {
        let header = tiff::parse_header(tiff_data)?;
        let be = header.big_endian;

        // E2: Parse IFD0
        let ifds = tiff::parse_ifd_chain(tiff_data, header.ifd0_offset, be, header.bigtiff)?;

        let ifd0 = ifds
            .into_iter()
            .next()
            .ok_or_else(|| crate::core::Error::Format("EXIF: no IFD0 found".into()))?;

        // E6: IFD1 (thumbnail)
        let ifd1 = if ifd0.next_ifd_offset != 0 {
            tiff::parse_ifd(tiff_data, ifd0.next_ifd_offset, be, header.bigtiff).ok()
        } else {
            None
        };

        // E3: ExifIFD (SubIFD pointer)
        let exif_ifd = ifd0
            .sub_ifd_offset(TAG_EXIF_IFD_POINTER, be)
            .and_then(|off| tiff::parse_ifd(tiff_data, off, be, header.bigtiff).ok());

        // E4: GPS IFD
        let gps_ifd = ifd0
            .sub_ifd_offset(TAG_GPS_IFD_POINTER, be)
            .and_then(|off| tiff::parse_ifd(tiff_data, off, be, header.bigtiff).ok());

        // E5: Interop IFD (pointer in ExifIFD)
        let interop_ifd = exif_ifd
            .as_ref()
            .and_then(|eifd| eifd.sub_ifd_offset(TAG_INTEROP_IFD_POINTER, be))
            .and_then(|off| tiff::parse_ifd(tiff_data, off, be, header.bigtiff).ok());

        // E8: MakerNote
        let maker_note = exif_ifd
            .as_ref()
            .and_then(|eifd| eifd.entry(TAG_MAKER_NOTE))
            .map(|entry| detect_maker_note(entry, tiff_data));

        // E7: Thumbnail extraction
        let thumbnail = ifd1
            .as_ref()
            .and_then(|ifd| extract_thumbnail(ifd, tiff_data, be));

        Ok(ExifData {
            header,
            ifd0,
            ifd1,
            exif_ifd,
            gps_ifd,
            interop_ifd,
            maker_note,
            thumbnail,
            tiff_data,
        })
    }

    /// Get the EXIF version string (E10). e.g., "0232" -> "2.32".
    pub fn exif_version(&self) -> Option<String> {
        self.exif_ifd
            .as_ref()
            .and_then(|ifd| ifd.entry(TAG_EXIF_VERSION))
            .and_then(|e| parse_version_tag(e))
    }

    /// Get the FlashPix version string (E10).
    pub fn flashpix_version(&self) -> Option<String> {
        self.exif_ifd
            .as_ref()
            .and_then(|ifd| ifd.entry(TAG_FLASHPIX_VERSION))
            .and_then(|e| parse_version_tag(e))
    }

    /// Parse UserComment tag (E9).
    pub fn user_comment(&self) -> Option<UserComment<'a>> {
        self.exif_ifd
            .as_ref()
            .and_then(|ifd| ifd.entry(TAG_USER_COMMENT))
            .and_then(|entry| parse_user_comment(entry))
    }

    /// Get an IFD0 entry's ASCII value by tag ID.
    pub fn ifd0_ascii(&self, tag: u16) -> Option<&'a str> {
        self.ifd0.entry(tag).and_then(super::IfdEntry::as_ascii)
    }

    /// Get an ExifIFD entry's ASCII value by tag ID.
    pub fn exif_ascii(&self, tag: u16) -> Option<&'a str> {
        self.exif_ifd
            .as_ref()?
            .entry(tag)
            .and_then(super::IfdEntry::as_ascii)
    }
}

/// E8: Detect MakerNote format from the raw data.
fn detect_maker_note<'a>(entry: &IfdEntry<'a>, tiff_data: &'a [u8]) -> MakerNoteRef<'a> {
    let data = entry.data;

    // Calculate the offset of the maker note within the TIFF data
    let offset = if entry.inline {
        0 // Can't determine easily for inline; rare for MakerNotes
    } else {
        // The data pointer is into tiff_data
        let data_ptr = data.as_ptr() as usize;
        let base_ptr = tiff_data.as_ptr() as usize;
        data_ptr.saturating_sub(base_ptr)
    };

    let format = detect_maker_note_format(data);

    MakerNoteRef {
        data,
        offset,
        format,
    }
}

fn detect_maker_note_format(data: &[u8]) -> MakerNoteFormat {
    if data.len() < 10 {
        return MakerNoteFormat::Unknown;
    }

    // Modern Sony cameras use a 12-byte `SONY DSC \0\0\0` header followed by
    // an IFD whose value offsets are relative to the parent TIFF payload.
    if data.starts_with(b"SONY DSC \0\0\0") {
        return MakerNoteFormat::HeaderIfd {
            header_size: 12,
            relative_offsets: false,
        };
    }

    // Nikon type 3: "Nikon\0" + version byte + padding + TIFF header
    if data.starts_with(b"Nikon\0") && data.len() > 18 {
        if data[6] == 0x02 {
            // Nikon type 3: embedded TIFF at offset 10
            return MakerNoteFormat::NikonTiff { tiff_offset: 10 };
        }
        if data[6] == 0x01 {
            // Nikon type 2 (old Coolpix): 8-byte header, IFD with absolute offsets
            return MakerNoteFormat::HeaderIfd {
                header_size: 8,
                relative_offsets: false,
            };
        }
    }

    // Fujifilm: "FUJIFILM" + offset
    if data.starts_with(b"FUJIFILM") {
        return MakerNoteFormat::HeaderIfd {
            header_size: 12,
            relative_offsets: true,
        };
    }

    // Olympus: "OLYMP\0" or "OLYMPUS\0"
    if data.starts_with(b"OLYMP\0") {
        return MakerNoteFormat::HeaderIfd {
            header_size: 8,
            relative_offsets: false,
        };
    }
    if data.starts_with(b"OLYMPUS\0") {
        // OLYMPUS\0 + byte order (II/MM) + version + IFD
        // Offsets are relative to byte 8 (the embedded TIFF-like header)
        return MakerNoteFormat::HeaderIfd {
            header_size: 8,
            relative_offsets: true,
        };
    }

    // Panasonic: "Panasonic\0"
    if data.starts_with(b"Panasonic\0") {
        return MakerNoteFormat::HeaderIfd {
            header_size: 12,
            relative_offsets: false,
        };
    }

    // Apple: "Apple iOS\0"
    if data.starts_with(b"Apple iOS\0") {
        return MakerNoteFormat::HeaderIfd {
            header_size: 14,
            relative_offsets: true,
        };
    }

    // Pentax: "AOC\0" + byte order (2 bytes) -> 6-byte header, absolute offsets
    // Data offsets for entries > 4 bytes point into the TIFF data, not relative to MN start.
    if data.starts_with(b"AOC\0") && data.len() > 10 {
        return MakerNoteFormat::HeaderIfd {
            header_size: 6,
            relative_offsets: false,
        };
    }

    // Casio Type 2: "QVC\0\0\0" -> 6-byte header, absolute offsets
    if data.starts_with(b"QVC\0\0\0") {
        return MakerNoteFormat::HeaderIfd {
            header_size: 6,
            relative_offsets: false,
        };
    }

    // Sanyo: "SANYO\0" + byte order (II/MM) -> 8-byte header, absolute offsets
    if data.starts_with(b"SANYO\0") && data.len() > 12 {
        return MakerNoteFormat::HeaderIfd {
            header_size: 8,
            relative_offsets: false,
        };
    }

    // Sigma: "SIGMA\0\0\0" or "FOVEON\0\0" -> 10-byte header, absolute offsets
    if data.starts_with(b"SIGMA\0\0\0") || data.starts_with(b"FOVEON\0\0") {
        return MakerNoteFormat::HeaderIfd {
            header_size: 10,
            relative_offsets: false,
        };
    }

    // Ricoh: "Ricoh" or "RICOH" -> 8-byte header, absolute offsets
    // Some Ricoh models use "Ricoh\0", others use "Ricoh\xcf\x00\x00"
    if (data.starts_with(b"Ricoh") || data.starts_with(b"RICOH")) && data.len() > 10 {
        return MakerNoteFormat::HeaderIfd {
            header_size: 8,
            relative_offsets: false,
        };
    }

    // GE: "GE\0\0\0\0\x01\0\0\0MM\0\x2a\0\0\0\x08" - 18-byte header
    // Contains embedded TIFF at offset 10, IFD at offset 18
    // FixBase required: value offsets may need auto-correction
    if data.starts_with(b"GE\0") && data.len() > 20 {
        return MakerNoteFormat::HeaderIfd {
            header_size: 18,
            relative_offsets: true,
        };
    }

    // JVC: "JVC " -> 4-byte header, absolute offsets
    if data.starts_with(b"JVC ") {
        return MakerNoteFormat::HeaderIfd {
            header_size: 4,
            relative_offsets: false,
        };
    }

    // Motorola: "MOT\0" + 4 pad bytes -> 8-byte header, offsets relative to MN start
    if data.starts_with(b"MOT\0") {
        return MakerNoteFormat::HeaderIfd {
            header_size: 8,
            relative_offsets: true,
        };
    }

    // Minolta: "MLT0" -> 4-byte header, standard IFD
    // (Minolta without header, Casio Type 1 fall through to StandardIfd)

    // Kodak binary maker notes - NOT IFD-based, must detect before the generic IFD check.
    // Type 2 (DC220/DC260/DC265/DC290): starts with 01 00 [00|01] 00 00 00 04 00 + ASCII,
    //   or has "Eastman Kodak" at offset 8
    if data.len() > 44
        && ((data[0] == 0x01
            && data[1] == 0x00
            && (data[2] == 0x00 || data[2] == 0x01)
            && data[3] == 0x00
            && data[4] == 0x00
            && data[5] == 0x00
            && data[6] == 0x04
            && data[7] == 0x00
            && data[8..12].iter().all(u8::is_ascii_alphabetic))
            || data.get(8..21) == Some(b"Eastman Kodak"))
    {
        return MakerNoteFormat::Unknown;
    }
    // Type 4 (DC200/DC215): bytes 41..44 == "JPG"
    if data.len() > 44 && data.get(41..44) == Some(b"JPG") {
        return MakerNoteFormat::Unknown;
    }
    // Type 3 (DC240/DC280/DC3400/DC5000): doesn't start with MM/II/AOC, byte 12 == 0x07
    if data.len() > 0x50
        && !data.starts_with(b"MM")
        && !data.starts_with(b"II")
        && !data.starts_with(b"AOC")
        && data.get(12) == Some(&0x07)
    {
        return MakerNoteFormat::Unknown;
    }
    // Type 9 (Z1015): starts with "IIII"
    if data.len() > 0x36 && data.starts_with(b"IIII") {
        return MakerNoteFormat::Unknown;
    }

    // Check if it looks like a raw IFD (starts with a reasonable entry count)
    let possible_count = u16::from_le_bytes([data[0], data[1]]);
    let possible_count_be = u16::from_be_bytes([data[0], data[1]]);
    if (1..=500).contains(&possible_count) || (1..=500).contains(&possible_count_be) {
        return MakerNoteFormat::StandardIfd;
    }

    MakerNoteFormat::Unknown
}

/// E7: Extract thumbnail JPEG data from IFD1.
fn extract_thumbnail<'a>(
    ifd1: &Ifd<'a>,
    tiff_data: &'a [u8],
    big_endian: bool,
) -> Option<&'a [u8]> {
    let offset = ifd1
        .entry(TAG_JPEG_INTERCHANGE_FORMAT)?
        .as_u32(big_endian)? as usize;
    let length = ifd1
        .entry(TAG_JPEG_INTERCHANGE_FORMAT_LENGTH)?
        .as_u32(big_endian)? as usize;

    if length == 0 || offset >= tiff_data.len() {
        return None;
    }

    let end = (offset + length).min(tiff_data.len());
    let thumb = &tiff_data[offset..end];

    // Validate that it actually starts with JPEG SOI
    if thumb.len() >= 2 && thumb[0] == 0xFF && thumb[1] == 0xD8 {
        Some(thumb)
    } else {
        None
    }
}

/// E9: Parse UserComment tag value.
fn parse_user_comment<'a>(entry: &IfdEntry<'a>) -> Option<UserComment<'a>> {
    if entry.data.len() < 8 {
        return None;
    }

    let charset_id = &entry.data[..8];
    let data = &entry.data[8..];

    let encoding = if charset_id == b"ASCII\0\0\0" {
        UserCommentEncoding::Ascii
    } else if charset_id == b"JIS\0\0\0\0\0" {
        UserCommentEncoding::Jis
    } else if charset_id == b"UNICODE\0" {
        UserCommentEncoding::Unicode
    } else {
        UserCommentEncoding::Undefined
    };

    Some(UserComment { encoding, data })
}

/// E10: Parse version tags (ExifVersion, FlashPixVersion).
///
/// These are stored as UNDEFINED type with ASCII digit bytes, e.g., `[0x30, 0x32, 0x33, 0x32]` = "0232".
/// We format as "2.32" for display.
fn parse_version_tag(entry: &IfdEntry<'_>) -> Option<String> {
    if entry.data.len() < 4 {
        return None;
    }

    // The bytes should be ASCII digits
    let s: String = entry.data[..4].iter().map(|&b| b as char).collect();

    if s.chars().all(|c| c.is_ascii_digit()) {
        Some(s)
    } else {
        None
    }
}

// -- Multi-page TIFF support --------------------------------------------

/// NewSubfileType (tag 0x00FE) bit flags.
pub const SUBFILE_FULL_RES: u32 = 0;
pub const SUBFILE_REDUCED_RES: u32 = 1;
pub const SUBFILE_PAGE: u32 = 2;
pub const SUBFILE_MASK: u32 = 4;

const TAG_NEW_SUBFILE_TYPE: u16 = 0x00FE;
#[allow(dead_code)] // TIFF tag, kept for completeness; SubIFD walking is TODO.
const TAG_SUB_IFDS: u16 = 0x014A;
const TAG_PAGE_NUMBER: u16 = 0x0129;
const TAG_PAGE_NAME: u16 = 0x011D;
const TAG_IMAGE_WIDTH: u16 = 0x0100;
const TAG_IMAGE_HEIGHT: u16 = 0x0101;

/// A single page/image in a multi-page TIFF document.
#[derive(Debug)]
pub struct TiffPage<'a> {
    /// The IFD for this page.
    pub ifd: Ifd<'a>,
    /// SubIFDs referenced by tag 0x014A (reduced-res, thumbnails, raw data).
    pub sub_ifds: Vec<Ifd<'a>>,
    /// Exif SubIFD (tag 0x8769), if present on this page.
    pub exif_ifd: Option<Ifd<'a>>,
    /// GPS IFD (tag 0x8825), if present on this page.
    pub gps_ifd: Option<Ifd<'a>>,
    /// Interop IFD (tag 0xA005 in ExifIFD), if present.
    pub interop_ifd: Option<Ifd<'a>>,
}

impl<'a> TiffPage<'a> {
    /// NewSubfileType flags (0 = full-resolution image).
    pub fn subfile_type(&self, big_endian: bool) -> u32 {
        self.ifd
            .entry(TAG_NEW_SUBFILE_TYPE)
            .and_then(|e| e.as_u32(big_endian))
            .unwrap_or(0)
    }

    /// True if this is a full-resolution image (not reduced-res, not a mask).
    pub fn is_full_resolution(&self, big_endian: bool) -> bool {
        self.subfile_type(big_endian) == SUBFILE_FULL_RES
    }

    /// True if this is a reduced-resolution version.
    pub fn is_reduced_resolution(&self, big_endian: bool) -> bool {
        self.subfile_type(big_endian) & SUBFILE_REDUCED_RES != 0
    }

    /// True if this is a page in a multi-page document.
    pub fn is_page(&self, big_endian: bool) -> bool {
        self.subfile_type(big_endian) & SUBFILE_PAGE != 0
    }

    /// True if this is a transparency mask.
    pub fn is_mask(&self, big_endian: bool) -> bool {
        self.subfile_type(big_endian) & SUBFILE_MASK != 0
    }

    /// Page number (tag 0x0129), if present. Returns (page_index, total_pages).
    pub fn page_number(&self, big_endian: bool) -> Option<(u16, u16)> {
        let entry = self.ifd.entry(TAG_PAGE_NUMBER)?;
        if entry.count >= 2 && entry.data.len() >= 4 {
            let page = if big_endian {
                u16::from_be_bytes([entry.data[0], entry.data[1]])
            } else {
                u16::from_le_bytes([entry.data[0], entry.data[1]])
            };
            let total = if big_endian {
                u16::from_be_bytes([entry.data[2], entry.data[3]])
            } else {
                u16::from_le_bytes([entry.data[2], entry.data[3]])
            };
            Some((page, total))
        } else {
            None
        }
    }

    /// Page name (tag 0x011D), if present.
    pub fn page_name(&self) -> Option<&'a str> {
        self.ifd.entry(TAG_PAGE_NAME)?.as_ascii()
    }

    /// Image dimensions (width, height) from tags 0x0100 and 0x0101.
    pub fn dimensions(&self, big_endian: bool) -> Option<(u32, u32)> {
        let w = self.ifd.entry(TAG_IMAGE_WIDTH)?;
        let h = self.ifd.entry(TAG_IMAGE_HEIGHT)?;
        let width = w
            .as_u32(big_endian)
            .or_else(|| w.as_u16(big_endian).map(|v| v as u32))?;
        let height = h
            .as_u32(big_endian)
            .or_else(|| h.as_u16(big_endian).map(|v| v as u32))?;
        Some((width, height))
    }
}

/// A parsed multi-page TIFF document.
#[derive(Debug)]
pub struct TiffDocument<'a> {
    /// The TIFF header (byte order, BigTIFF flag).
    pub header: TiffHeader,
    /// All pages in the document, in IFD chain order.
    pub pages: Vec<TiffPage<'a>>,
}

impl<'a> TiffDocument<'a> {
    /// Parse a multi-page TIFF document from raw TIFF bytes.
    pub fn parse(tiff_data: &'a [u8]) -> Result<Self> {
        let header = tiff::parse_header(tiff_data)?;
        let be = header.big_endian;
        let bt = header.bigtiff;

        let ifds = tiff::parse_ifd_chain(tiff_data, header.ifd0_offset, be, bt)?;

        let mut pages = Vec::with_capacity(ifds.len());
        for ifd in ifds {
            let page = Self::build_page(tiff_data, ifd, be, bt);
            pages.push(page);
        }

        Ok(TiffDocument { header, pages })
    }

    fn build_page(tiff_data: &'a [u8], ifd: Ifd<'a>, be: bool, bt: bool) -> TiffPage<'a> {
        // Parse SubIFDs (tag 0x014A)
        let sub_ifd_offsets = ifd.sub_ifd_offsets(be);
        let sub_ifds: Vec<Ifd<'a>> = sub_ifd_offsets
            .iter()
            .filter_map(|&off| tiff::parse_ifd(tiff_data, off, be, bt).ok())
            .collect();

        // Parse ExifIFD
        let exif_ifd = ifd
            .sub_ifd_offset(TAG_EXIF_IFD_POINTER, be)
            .and_then(|off| tiff::parse_ifd(tiff_data, off, be, bt).ok());

        // Parse GPS IFD
        let gps_ifd = ifd
            .sub_ifd_offset(TAG_GPS_IFD_POINTER, be)
            .and_then(|off| tiff::parse_ifd(tiff_data, off, be, bt).ok());

        // Parse Interop IFD
        let interop_ifd = exif_ifd
            .as_ref()
            .and_then(|eifd| eifd.sub_ifd_offset(TAG_INTEROP_IFD_POINTER, be))
            .and_then(|off| tiff::parse_ifd(tiff_data, off, be, bt).ok());

        TiffPage {
            ifd,
            sub_ifds,
            exif_ifd,
            gps_ifd,
            interop_ifd,
        }
    }

    /// Number of pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Get a page by index.
    pub fn page(&self, index: usize) -> Option<&TiffPage<'a>> {
        self.pages.get(index)
    }

    /// Get only full-resolution pages (excludes thumbnails and masks).
    pub fn full_res_pages(&self) -> Vec<&TiffPage<'a>> {
        self.pages
            .iter()
            .filter(|p| p.is_full_resolution(self.header.big_endian))
            .collect()
    }
}

/// UserComment helper: try to decode as a UTF-8 string.
impl<'a> UserComment<'a> {
    pub fn as_string(&self) -> Option<String> {
        match self.encoding {
            UserCommentEncoding::Ascii | UserCommentEncoding::Undefined => {
                // Strip trailing nulls/spaces
                let end = self
                    .data
                    .iter()
                    .rposition(|&b| b != 0 && b != b' ')
                    .map_or(0, |p| p + 1);
                if end == 0 {
                    return None;
                }
                String::from_utf8(self.data[..end].to_vec()).ok()
            }
            UserCommentEncoding::Unicode => {
                // UTF-16 (usually LE)
                if self.data.len() < 2 {
                    return None;
                }
                let words: Vec<u16> = self
                    .data
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&w| w != 0)
                    .collect();
                String::from_utf16(&words).ok()
            }
            UserCommentEncoding::Jis => {
                // JIS X 0208 - would need a decoder; return None for now
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiff::DataType;

    type TestEntry = (u16, u16, u32, Vec<u8>);

    /// Build minimal TIFF data with IFD0 containing given entries,
    /// plus optional SubIFD entries at ExifIFD pointer.
    fn build_exif_tiff(
        ifd0_entries: &[TestEntry],
        exif_entries: Option<&[TestEntry]>,
        gps_entries: Option<&[TestEntry]>,
    ) -> Vec<u8> {
        let mut data = Vec::new();

        // Header: II, magic 42, IFD0 offset = 8
        data.extend_from_slice(b"II");
        data.extend_from_slice(&42u16.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());

        // We'll need to know total entries in IFD0 (original + pointer tags)
        let extra_pointers = exif_entries.is_some() as usize + gps_entries.is_some() as usize;
        let total_ifd0_entries = ifd0_entries.len() + extra_pointers;

        // IFD0
        data.extend_from_slice(&(total_ifd0_entries as u16).to_le_bytes());

        let ifd0_entries_end = 8 + 2 + total_ifd0_entries * 12 + 4;
        let mut external_offset = ifd0_entries_end;
        let mut external_data = Vec::new();

        for &(tag, dtype, count, ref value) in ifd0_entries {
            data.extend_from_slice(&tag.to_le_bytes());
            data.extend_from_slice(&dtype.to_le_bytes());
            data.extend_from_slice(&count.to_le_bytes());

            let type_size = DataType::from_u16(dtype).map_or(1, |t| t.size());
            let total = count as usize * type_size;

            if total <= 4 {
                let mut padded = [0u8; 4];
                let copy_len = value.len().min(4);
                padded[..copy_len].copy_from_slice(&value[..copy_len]);
                data.extend_from_slice(&padded);
            } else {
                data.extend_from_slice(&(external_offset as u32).to_le_bytes());
                // Pad external data to maintain alignment
                external_data.extend_from_slice(value);
                external_offset += value.len();
            }
        }

        // Calculate where sub-IFDs will go
        let sub_ifd_offset = external_offset;

        // ExifIFD pointer
        if exif_entries.is_some() {
            data.extend_from_slice(&TAG_EXIF_IFD_POINTER.to_le_bytes());
            data.extend_from_slice(&4u16.to_le_bytes()); // LONG
            data.extend_from_slice(&1u32.to_le_bytes());
            // We'll fill this offset after calculating sub-IFD positions
            let exif_ifd_pos = sub_ifd_offset;
            data.extend_from_slice(&(exif_ifd_pos as u32).to_le_bytes());
        }

        // GPS pointer
        if let Some(gps) = gps_entries {
            let gps_ifd_pos = if let Some(exif) = exif_entries {
                sub_ifd_offset + 2 + exif.len() * 12 + 4 // after ExifIFD
            } else {
                sub_ifd_offset
            };
            data.extend_from_slice(&TAG_GPS_IFD_POINTER.to_le_bytes());
            data.extend_from_slice(&4u16.to_le_bytes()); // LONG
            data.extend_from_slice(&1u32.to_le_bytes());
            data.extend_from_slice(&(gps_ifd_pos as u32).to_le_bytes());
            let _ = gps;
        }

        // Next IFD offset = 0
        data.extend_from_slice(&0u32.to_le_bytes());

        // External data
        data.extend_from_slice(&external_data);

        // ExifIFD
        if let Some(entries) = exif_entries {
            let exif_entries_end = data.len() + 2 + entries.len() * 12 + 4;
            let mut exif_ext_offset = exif_entries_end;
            let mut exif_ext_data = Vec::new();

            data.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for &(tag, dtype, count, ref value) in entries {
                data.extend_from_slice(&tag.to_le_bytes());
                data.extend_from_slice(&dtype.to_le_bytes());
                data.extend_from_slice(&count.to_le_bytes());

                let type_size = DataType::from_u16(dtype).map_or(1, |t| t.size());
                let total = count as usize * type_size;

                if total <= 4 {
                    let mut padded = [0u8; 4];
                    let copy_len = value.len().min(4);
                    padded[..copy_len].copy_from_slice(&value[..copy_len]);
                    data.extend_from_slice(&padded);
                } else {
                    data.extend_from_slice(&(exif_ext_offset as u32).to_le_bytes());
                    exif_ext_data.extend_from_slice(value);
                    exif_ext_offset += value.len();
                }
            }
            data.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
            data.extend_from_slice(&exif_ext_data);
        }

        // GPS IFD
        if let Some(entries) = gps_entries {
            let gps_entries_end = data.len() + 2 + entries.len() * 12 + 4;
            let mut gps_ext_offset = gps_entries_end;
            let mut gps_ext_data = Vec::new();

            data.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for &(tag, dtype, count, ref value) in entries {
                data.extend_from_slice(&tag.to_le_bytes());
                data.extend_from_slice(&dtype.to_le_bytes());
                data.extend_from_slice(&count.to_le_bytes());

                let type_size = DataType::from_u16(dtype).map_or(1, |t| t.size());
                let total = count as usize * type_size;

                if total <= 4 {
                    let mut padded = [0u8; 4];
                    let copy_len = value.len().min(4);
                    padded[..copy_len].copy_from_slice(&value[..copy_len]);
                    data.extend_from_slice(&padded);
                } else {
                    data.extend_from_slice(&(gps_ext_offset as u32).to_le_bytes());
                    gps_ext_data.extend_from_slice(value);
                    gps_ext_offset += value.len();
                }
            }
            data.extend_from_slice(&0u32.to_le_bytes());
            data.extend_from_slice(&gps_ext_data);
        }

        data
    }

    #[test]
    fn e1_parse_exif_header() {
        let tiff_data = build_exif_tiff(
            &[(271, 2, 5, b"Test\0".to_vec())], // Make
            None,
            None,
        );
        let exif = ExifData::parse(&tiff_data).unwrap();
        assert!(!exif.header.big_endian);
        assert!(!exif.header.bigtiff);
    }

    #[test]
    fn e2_ifd0_tags() {
        let tiff_data = build_exif_tiff(
            &[
                (271, 2, 6, b"Canon\0".to_vec()),    // Make
                (272, 2, 8, b"EOS 5D\0\0".to_vec()), // Model (padded)
            ],
            None,
            None,
        );
        let exif = ExifData::parse(&tiff_data).unwrap();
        assert_eq!(exif.ifd0_ascii(271), Some("Canon"));
        assert_eq!(exif.ifd0_ascii(272), Some("EOS 5D"));
    }

    #[test]
    fn e3_exif_subifd() {
        let tiff_data = build_exif_tiff(
            &[(271, 2, 5, b"Test\0".to_vec())],
            Some(&[
                (0x9000, 7, 4, b"0232".to_vec()), // ExifVersion
            ]),
            None,
        );
        let exif = ExifData::parse(&tiff_data).unwrap();
        assert!(exif.exif_ifd.is_some());
        assert_eq!(exif.exif_version(), Some("0232".into()));
    }

    #[test]
    fn e4_gps_ifd() {
        let tiff_data = build_exif_tiff(
            &[(271, 2, 5, b"Test\0".to_vec())],
            None,
            Some(&[
                (0, 1, 4, vec![2, 3, 0, 0]), // GPSVersionID = 2.3.0.0
            ]),
        );
        let exif = ExifData::parse(&tiff_data).unwrap();
        assert!(exif.gps_ifd.is_some());
        let gps = exif.gps_ifd.as_ref().unwrap();
        assert!(gps.entry(0).is_some()); // GPSVersionID
    }

    #[test]
    fn e8_maker_note_detection() {
        // Test Nikon detection
        assert_eq!(
            detect_maker_note_format(b"Nikon\0\x02\x10\0\0MM\0\x2A\0\0\0\x08\0\x01extra"),
            MakerNoteFormat::NikonTiff { tiff_offset: 10 }
        );

        // Fujifilm
        assert!(matches!(
            detect_maker_note_format(b"FUJIFILM\x0C\0\0\0data"),
            MakerNoteFormat::HeaderIfd {
                header_size: 12,
                relative_offsets: true
            }
        ));

        // Standard IFD (starts with count)
        let mut standard = vec![0x05, 0x00]; // count = 5 (LE)
        standard.extend_from_slice(&[0; 70]); // entries
        assert_eq!(
            detect_maker_note_format(&standard),
            MakerNoteFormat::StandardIfd
        );
    }

    #[test]
    fn e9_user_comment_ascii() {
        let comment_data = b"ASCII\0\0\0Hello World".to_vec();
        let entry = IfdEntry {
            tag: TAG_USER_COMMENT,
            data_type: DataType::Undefined,
            raw_type: 7,
            count: comment_data.len() as u64,
            data: &comment_data,
            inline: false,
        };
        let uc = parse_user_comment(&entry).unwrap();
        assert_eq!(uc.encoding, UserCommentEncoding::Ascii);
        assert_eq!(uc.as_string(), Some("Hello World".into()));
    }

    #[test]
    fn e9_user_comment_unicode() {
        let mut data = b"UNICODE\0".to_vec();
        // "Hi" in UTF-16LE
        data.extend_from_slice(&[0x48, 0x00, 0x69, 0x00, 0x00, 0x00]);
        let entry = IfdEntry {
            tag: TAG_USER_COMMENT,
            data_type: DataType::Undefined,
            raw_type: 7,
            count: data.len() as u64,
            data: &data,
            inline: false,
        };
        let uc = parse_user_comment(&entry).unwrap();
        assert_eq!(uc.encoding, UserCommentEncoding::Unicode);
        assert_eq!(uc.as_string(), Some("Hi".into()));
    }

    #[test]
    fn e10_version_tags() {
        let entry = IfdEntry {
            tag: TAG_EXIF_VERSION,
            data_type: DataType::Undefined,
            raw_type: 7,
            count: 4,
            data: b"0232",
            inline: true,
        };
        assert_eq!(parse_version_tag(&entry), Some("0232".into()));

        let entry2 = IfdEntry {
            tag: TAG_FLASHPIX_VERSION,
            data_type: DataType::Undefined,
            raw_type: 7,
            count: 4,
            data: b"0100",
            inline: true,
        };
        assert_eq!(parse_version_tag(&entry2), Some("0100".into()));
    }

    // -- Multi-page TIFF tests ------------------------------------------

    /// Build a multi-page TIFF (LE) with the given list of pages.
    /// Each page is a list of (tag, type, count, value_bytes) entries.
    fn build_multipage_tiff(pages: &[Vec<TestEntry>]) -> Vec<u8> {
        let mut data = Vec::new();

        // Header
        data.extend_from_slice(b"II");
        data.extend_from_slice(&42u16.to_le_bytes());
        // IFD0 offset placeholder - will be 8
        data.extend_from_slice(&8u32.to_le_bytes());

        // Pre-calculate all IFD positions
        let mut ifd_offsets = Vec::new();
        let mut current_offset = 8usize;
        for page_entries in pages {
            ifd_offsets.push(current_offset);
            // count(2) + entries(N*12) + next_ifd(4)
            let ifd_size = 2 + page_entries.len() * 12 + 4;
            // External data for entries > 4 bytes
            let ext_size: usize = page_entries
                .iter()
                .map(|(_, dtype, count, value)| {
                    let type_size = DataType::from_u16(*dtype).map_or(1, |t| t.size());
                    let total = *count as usize * type_size;
                    if total > 4 { value.len() } else { 0 }
                })
                .sum();
            current_offset += ifd_size + ext_size;
        }

        // Write each IFD
        for (page_idx, page_entries) in pages.iter().enumerate() {
            let ifd_start = ifd_offsets[page_idx];
            // Pad if needed
            while data.len() < ifd_start {
                data.push(0);
            }

            data.extend_from_slice(&(page_entries.len() as u16).to_le_bytes());

            let entries_end = ifd_start + 2 + page_entries.len() * 12 + 4;
            let mut ext_offset = entries_end;
            let mut ext_data = Vec::new();

            for &(tag, dtype, count, ref value) in page_entries {
                data.extend_from_slice(&tag.to_le_bytes());
                data.extend_from_slice(&dtype.to_le_bytes());
                data.extend_from_slice(&count.to_le_bytes());

                let type_size = DataType::from_u16(dtype).map_or(1, |t| t.size());
                let total = count as usize * type_size;

                if total <= 4 {
                    let mut padded = [0u8; 4];
                    let copy_len = value.len().min(4);
                    padded[..copy_len].copy_from_slice(&value[..copy_len]);
                    data.extend_from_slice(&padded);
                } else {
                    data.extend_from_slice(&(ext_offset as u32).to_le_bytes());
                    ext_data.extend_from_slice(value);
                    ext_offset += value.len();
                }
            }

            // Next IFD offset
            let next_offset = if page_idx + 1 < pages.len() {
                ifd_offsets[page_idx + 1] as u32
            } else {
                0
            };
            data.extend_from_slice(&next_offset.to_le_bytes());

            // External data
            data.extend_from_slice(&ext_data);
        }

        data
    }

    #[test]
    fn multipage_basic_3_pages() {
        let pages = vec![
            vec![
                (0x0100, 3, 1, 640u16.to_le_bytes().to_vec()), // ImageWidth
                (0x0101, 3, 1, 480u16.to_le_bytes().to_vec()), // ImageHeight
            ],
            vec![
                (0x0100, 3, 1, 320u16.to_le_bytes().to_vec()),
                (0x0101, 3, 1, 240u16.to_le_bytes().to_vec()),
            ],
            vec![
                (0x0100, 3, 1, 160u16.to_le_bytes().to_vec()),
                (0x0101, 3, 1, 120u16.to_le_bytes().to_vec()),
            ],
        ];
        let tiff_data = build_multipage_tiff(&pages);
        let doc = TiffDocument::parse(&tiff_data).unwrap();

        assert_eq!(doc.page_count(), 3);
        assert_eq!(doc.pages[0].dimensions(false), Some((640, 480)));
        assert_eq!(doc.pages[1].dimensions(false), Some((320, 240)));
        assert_eq!(doc.pages[2].dimensions(false), Some((160, 120)));
    }

    #[test]
    fn multipage_subfile_types() {
        let pages = vec![
            vec![
                (0x00FE, 4, 1, 0u32.to_le_bytes().to_vec()), // Full-res
                (0x0100, 3, 1, 4000u16.to_le_bytes().to_vec()),
                (0x0101, 3, 1, 3000u16.to_le_bytes().to_vec()),
            ],
            vec![
                (0x00FE, 4, 1, 1u32.to_le_bytes().to_vec()), // Reduced-res
                (0x0100, 3, 1, 200u16.to_le_bytes().to_vec()),
                (0x0101, 3, 1, 150u16.to_le_bytes().to_vec()),
            ],
            vec![
                (0x00FE, 4, 1, 2u32.to_le_bytes().to_vec()), // Page
                (0x0100, 3, 1, 800u16.to_le_bytes().to_vec()),
                (0x0101, 3, 1, 600u16.to_le_bytes().to_vec()),
            ],
            vec![
                (0x00FE, 4, 1, 4u32.to_le_bytes().to_vec()), // Mask
                (0x0100, 3, 1, 4000u16.to_le_bytes().to_vec()),
                (0x0101, 3, 1, 3000u16.to_le_bytes().to_vec()),
            ],
        ];
        let tiff_data = build_multipage_tiff(&pages);
        let doc = TiffDocument::parse(&tiff_data).unwrap();

        assert_eq!(doc.page_count(), 4);
        assert!(doc.pages[0].is_full_resolution(false));
        assert!(doc.pages[1].is_reduced_resolution(false));
        assert!(doc.pages[2].is_page(false));
        assert!(doc.pages[3].is_mask(false));

        let full = doc.full_res_pages();
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].dimensions(false), Some((4000, 3000)));
    }

    #[test]
    fn multipage_page_number() {
        let pages = vec![
            vec![
                (0x0129, 3, 2, {
                    // PageNumber: page 0 of 3
                    let mut v = Vec::new();
                    v.extend_from_slice(&0u16.to_le_bytes());
                    v.extend_from_slice(&3u16.to_le_bytes());
                    v
                }),
                (0x0100, 3, 1, 100u16.to_le_bytes().to_vec()),
                (0x0101, 3, 1, 100u16.to_le_bytes().to_vec()),
            ],
            vec![
                (0x0129, 3, 2, {
                    let mut v = Vec::new();
                    v.extend_from_slice(&1u16.to_le_bytes());
                    v.extend_from_slice(&3u16.to_le_bytes());
                    v
                }),
                (0x0100, 3, 1, 100u16.to_le_bytes().to_vec()),
                (0x0101, 3, 1, 100u16.to_le_bytes().to_vec()),
            ],
        ];
        let tiff_data = build_multipage_tiff(&pages);
        let doc = TiffDocument::parse(&tiff_data).unwrap();

        assert_eq!(doc.pages[0].page_number(false), Some((0, 3)));
        assert_eq!(doc.pages[1].page_number(false), Some((1, 3)));
    }

    #[test]
    fn multipage_page_name() {
        let pages = vec![vec![
            (0x011D, 2, 7, b"Page 1\0".to_vec()), // PageName
            (0x0100, 3, 1, 100u16.to_le_bytes().to_vec()),
            (0x0101, 3, 1, 100u16.to_le_bytes().to_vec()),
        ]];
        let tiff_data = build_multipage_tiff(&pages);
        let doc = TiffDocument::parse(&tiff_data).unwrap();

        assert_eq!(doc.pages[0].page_name(), Some("Page 1"));
    }

    #[test]
    fn multipage_sub_ifds() {
        // Build a TIFF where page 0 has a SubIFDs tag pointing to 2 sub-IFDs
        // We need to manually construct this since SubIFDs are separate IFDs
        let mut data = Vec::new();

        // Header
        data.extend_from_slice(b"II");
        data.extend_from_slice(&42u16.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());

        // IFD0 at offset 8: 3 entries (ImageWidth, ImageHeight, SubIFDs)
        let entry_count: u16 = 3;
        data.extend_from_slice(&entry_count.to_le_bytes()); // offset 8

        // Entry 1: ImageWidth = 4000
        data.extend_from_slice(&0x0100u16.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&4000u32.to_le_bytes());

        // Entry 2: ImageHeight = 3000
        data.extend_from_slice(&0x0101u16.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&3000u32.to_le_bytes());

        // Entry 3: SubIFDs (0x014A) = LONG array, count=2, offsets stored externally
        let ifd0_end = 8 + 2 + 3 * 12 + 4; // = 50
        let sub_ifd_data_offset = ifd0_end; // SubIFD offset array at 50
        data.extend_from_slice(&0x014Au16.to_le_bytes());
        data.extend_from_slice(&4u16.to_le_bytes()); // LONG
        data.extend_from_slice(&2u32.to_le_bytes()); // count=2
        data.extend_from_slice(&(sub_ifd_data_offset as u32).to_le_bytes());

        // Next IFD = 0
        data.extend_from_slice(&0u32.to_le_bytes()); // offset 46

        // SubIFD offset array at offset 50: [sub_ifd_0_offset, sub_ifd_1_offset]
        let sub_ifd_0_offset = sub_ifd_data_offset + 8; // after the 8-byte offset array
        let sub_ifd_1_offset = sub_ifd_0_offset + 2 + 2 * 12 + 4; // after sub_ifd_0
        data.extend_from_slice(&(sub_ifd_0_offset as u32).to_le_bytes());
        data.extend_from_slice(&(sub_ifd_1_offset as u32).to_le_bytes());

        // Sub-IFD 0: reduced-res preview (200x150)
        data.extend_from_slice(&2u16.to_le_bytes()); // 2 entries
        // ImageWidth = 200
        data.extend_from_slice(&0x0100u16.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&200u32.to_le_bytes());
        // ImageHeight = 150
        data.extend_from_slice(&0x0101u16.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&150u32.to_le_bytes());
        // No next IFD
        data.extend_from_slice(&0u32.to_le_bytes());

        // Sub-IFD 1: thumbnail (80x60)
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&0x0100u16.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&80u32.to_le_bytes());
        data.extend_from_slice(&0x0101u16.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&60u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());

        let doc = TiffDocument::parse(&data).unwrap();

        assert_eq!(doc.page_count(), 1);
        assert_eq!(doc.pages[0].dimensions(false), Some((4000, 3000)));
        assert_eq!(doc.pages[0].sub_ifds.len(), 2);

        // Sub-IFD 0: 200x150
        let sub0 = &doc.pages[0].sub_ifds[0];
        let w = sub0.entry(0x0100).unwrap().as_u16(false).unwrap();
        let h = sub0.entry(0x0101).unwrap().as_u16(false).unwrap();
        assert_eq!((w, h), (200, 150));

        // Sub-IFD 1: 80x60
        let sub1 = &doc.pages[0].sub_ifds[1];
        let w = sub1.entry(0x0100).unwrap().as_u16(false).unwrap();
        let h = sub1.entry(0x0101).unwrap().as_u16(false).unwrap();
        assert_eq!((w, h), (80, 60));
    }

    #[test]
    fn multipage_exif_data_unchanged() {
        // Verify ExifData still works as before
        let tiff_data = build_exif_tiff(
            &[(271, 2, 6, b"Canon\0".to_vec())],
            Some(&[(0x9000, 7, 4, b"0232".to_vec())]),
            Some(&[(0, 1, 4, vec![2, 3, 0, 0])]),
        );
        let exif = ExifData::parse(&tiff_data).unwrap();
        assert_eq!(exif.ifd0_ascii(271), Some("Canon"));
        assert!(exif.exif_ifd.is_some());
        assert!(exif.gps_ifd.is_some());

        // TiffDocument also works on the same data
        let doc = TiffDocument::parse(&tiff_data).unwrap();
        assert_eq!(doc.page_count(), 1);
        assert!(doc.pages[0].exif_ifd.is_some());
        assert!(doc.pages[0].gps_ifd.is_some());
    }

    #[test]
    fn multipage_single_page_no_subfile_type() {
        // Standard single-image TIFF without NewSubfileType
        let pages = vec![vec![
            (0x0100, 3, 1, 1920u16.to_le_bytes().to_vec()),
            (0x0101, 3, 1, 1080u16.to_le_bytes().to_vec()),
        ]];
        let tiff_data = build_multipage_tiff(&pages);
        let doc = TiffDocument::parse(&tiff_data).unwrap();

        assert_eq!(doc.page_count(), 1);
        // Without NewSubfileType, defaults to full-res
        assert!(doc.pages[0].is_full_resolution(false));
        assert_eq!(doc.pages[0].subfile_type(false), 0);
    }
}
