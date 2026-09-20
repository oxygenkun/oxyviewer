//! Maker note parsers (MN1-MN10).
//!
//! Per-vendor IFD parsing with tag name tables for Canon, Nikon, Sony,
//! Fujifilm, Panasonic, Olympus, Samsung, Apple, Pentax, Casio, Minolta,
//! Kodak, Sanyo, Ricoh, and Sigma maker notes.
//!
//! Canon sub-array decoding: tags 0x0001 (CameraSettings), 0x0002 (FocalLength),
//! 0x0004 (ShotInfo) are int16s/int16u arrays where each index maps to a named field.

use crate::tiff::exif::{MakerNoteFormat, MakerNoteRef};
use crate::tiff::{self, Ifd, IfdEntry};

/// A decoded maker note tag (name + formatted value).
#[derive(Debug, Clone)]
pub struct DecodedTag {
    pub name: String,
    pub value: String,
}

/// Parsed maker note data.
#[derive(Debug)]
pub struct MakerNote<'a> {
    /// Camera manufacturer.
    pub vendor: Vendor,
    /// Parsed IFD entries (if the format is IFD-based).
    pub ifd: Option<Ifd<'a>>,
    /// Big-endian flag used for parsing.
    pub big_endian: bool,
    /// For Nikon Type 3: offset of the embedded TIFF header within `mn_data`.
    /// Sub-IFD offsets (e.g. PreviewIFD) are relative to this position.
    pub nikon_tiff_offset: usize,
}

/// Camera vendor identification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Canon,
    Nikon,
    Sony,
    Fujifilm,
    Panasonic,
    Olympus,
    Samsung,
    Apple,
    Pentax,
    Casio,
    Minolta,
    Kodak,
    Sanyo,
    Ricoh,
    Sigma,
    Motorola,
    Jvc,
    Reconyx,
    Flir,
    Ge,
    Unknown,
}

/// MN1: Parse a maker note given its reference and the parent TIFF context.
///
/// `tiff_data` is the full TIFF data (for absolute offset resolution).
/// `parent_be` is the byte order of the parent TIFF.
pub fn parse_maker_note<'a>(
    mnr: &MakerNoteRef<'a>,
    tiff_data: &'a [u8],
    parent_be: bool,
) -> Option<MakerNote<'a>> {
    let vendor = detect_vendor(mnr.data);

    match mnr.format {
        MakerNoteFormat::StandardIfd => {
            // MN2 (Canon), MN4 (Sony), MN7 (Olympus - headerless), MN9 (Apple - headerless)
            let mut be = match vendor {
                Vendor::Fujifilm => false, // Always LE
                _ => parent_be,
            };
            // Check for Canon TIFF footer (relocated MakerNotes fix)
            // The footer also tells us the actual byte order of the MakerNote data,
            // which may differ from the parent TIFF (e.g. Canon LE data in a BE TIFF).
            let (base_adjust, footer_be) = detect_canon_fix_base(mnr);
            if let Some(fbe) = footer_be {
                be = fbe;
            }
            let ifd = if base_adjust != 0 {
                tiff::parse_ifd_with_base(tiff_data, mnr.offset as u64, be, base_adjust).ok()
            } else {
                parse_ifd_at(tiff_data, mnr.offset as u64, be)
            };
            Some(MakerNote {
                vendor,
                ifd,
                big_endian: be,
                nikon_tiff_offset: 0,
            })
        }
        MakerNoteFormat::HeaderIfd {
            header_size,
            relative_offsets,
        } => {
            let be = match vendor {
                Vendor::Fujifilm => false, // MN5: Always LE regardless of file endianness
                _ => parent_be,
            };
            if relative_offsets {
                // Check for Olympus "OLYMPUS\0" embedded TIFF-like header
                if vendor == Vendor::Olympus && mnr.data.len() > header_size + 4 {
                    // Read byte order from embedded header at offset 8
                    let mn_be = mnr.data[header_size] == b'M' && mnr.data[header_size + 1] == b'M';
                    // IFD at offset header_size + 4 (after byte order + version)
                    // Offsets in IFD entries are relative to MN data start (byte 0)
                    let ifd = parse_ifd_at(mnr.data, (header_size + 4) as u64, mn_be);
                    return Some(MakerNote {
                        vendor,
                        ifd,
                        big_endian: mn_be,
                        nikon_tiff_offset: 0,
                    });
                }
                // GE: embedded TIFF at offset 10, IFD at offset 18
                // Value offsets need base_adj = -2 (FixBase auto-detection)
                if vendor == Vendor::Ge && mnr.data.len() > header_size + 4 {
                    let ge_be = mnr.data.len() > 12 && mnr.data[10] == b'M' && mnr.data[11] == b'M';
                    let ifd = parse_ifd_base_tolerant(mnr.data, header_size as u64, ge_be, -2);
                    return Some(MakerNote {
                        vendor,
                        ifd,
                        big_endian: ge_be,
                        nikon_tiff_offset: 0,
                    });
                }
                // Offsets are relative to maker note start - parse within MN data
                let ifd = parse_ifd_at(mnr.data, header_size as u64, be);
                Some(MakerNote {
                    vendor,
                    ifd,
                    big_endian: be,
                    nikon_tiff_offset: 0,
                })
            } else {
                // Offsets are absolute (relative to TIFF start)
                // Skip header, then parse IFD from the MN data but resolve offsets in tiff_data
                let ifd_offset = (mnr.offset + header_size) as u64;
                let ifd = parse_ifd_at(tiff_data, ifd_offset, be);
                Some(MakerNote {
                    vendor,
                    ifd,
                    big_endian: be,
                    nikon_tiff_offset: 0,
                })
            }
        }
        MakerNoteFormat::NikonTiff { tiff_offset } => {
            // MN3: Nikon Type 3 - embedded TIFF header
            if mnr.data.len() <= tiff_offset + 8 {
                return Some(MakerNote {
                    vendor,
                    ifd: None,
                    big_endian: parent_be,
                    nikon_tiff_offset: tiff_offset,
                });
            }
            // Use parent TIFF data starting at MN+tiff_offset when available,
            // so internal offsets that extend slightly past the MN tag boundary
            // can still be resolved (e.g. NikonCaptureVersion near MN end).
            let tiff_slice = if mnr.offset + tiff_offset < tiff_data.len() {
                &tiff_data[mnr.offset + tiff_offset..]
            } else {
                &mnr.data[tiff_offset..]
            };
            let header = tiff::parse_header(tiff_slice).ok()?;
            let ifd = tiff::parse_ifd_tolerant(
                tiff_slice,
                header.ifd0_offset,
                header.big_endian,
                header.bigtiff,
            );
            Some(MakerNote {
                vendor,
                ifd,
                big_endian: header.big_endian,
                nikon_tiff_offset: tiff_offset,
            })
        }
        MakerNoteFormat::Unknown => Some(MakerNote {
            vendor,
            ifd: None,
            big_endian: parent_be,
            nikon_tiff_offset: 0,
        }),
    }
}

/// Parse an IFD at the given offset, tolerating individual bad entries.
fn parse_ifd_at<'a>(data: &'a [u8], offset: u64, big_endian: bool) -> Option<Ifd<'a>> {
    tiff::parse_ifd_tolerant(data, offset, big_endian, false)
}

/// Parse an IFD with a base offset adjustment, tolerating individual bad entries.
/// Entries whose adjusted offset is invalid are silently skipped.
fn parse_ifd_base_tolerant<'a>(
    data: &'a [u8],
    offset: u64,
    be: bool,
    base_adj: i64,
) -> Option<Ifd<'a>> {
    let off = offset as usize;
    if off + 2 > data.len() {
        return None;
    }
    let entry_count = if be {
        u16::from_be_bytes([data[off], data[off + 1]])
    } else {
        u16::from_le_bytes([data[off], data[off + 1]])
    } as usize;
    if entry_count > 500 {
        return None;
    }
    let entries_start = off + 2;
    if entries_start + entry_count * 12 > data.len() {
        return None;
    }

    let mut entries = Vec::with_capacity(entry_count);
    for i in 0..entry_count {
        let eoff = entries_start + i * 12;
        if eoff + 12 > data.len() {
            break;
        }

        let tag = if be {
            u16::from_be_bytes([data[eoff], data[eoff + 1]])
        } else {
            u16::from_le_bytes([data[eoff], data[eoff + 1]])
        };
        let raw_type = if be {
            u16::from_be_bytes([data[eoff + 2], data[eoff + 3]])
        } else {
            u16::from_le_bytes([data[eoff + 2], data[eoff + 3]])
        };
        let data_type = tiff::DataType::from_u16(raw_type).unwrap_or(tiff::DataType::Undefined);
        let count = if be {
            u32::from_be_bytes([
                data[eoff + 4],
                data[eoff + 5],
                data[eoff + 6],
                data[eoff + 7],
            ])
        } else {
            u32::from_le_bytes([
                data[eoff + 4],
                data[eoff + 5],
                data[eoff + 6],
                data[eoff + 7],
            ])
        } as u64;
        let val_size = count.saturating_mul(data_type.size() as u64) as usize;

        let (entry_data, inline) = if val_size <= 4 {
            let end = (eoff + 8 + val_size).min(eoff + 12);
            (&data[eoff + 8..end], true)
        } else {
            let raw_off = if be {
                u32::from_be_bytes([
                    data[eoff + 8],
                    data[eoff + 9],
                    data[eoff + 10],
                    data[eoff + 11],
                ])
            } else {
                u32::from_le_bytes([
                    data[eoff + 8],
                    data[eoff + 9],
                    data[eoff + 10],
                    data[eoff + 11],
                ])
            } as i64;
            let adj_off = raw_off + base_adj;
            if adj_off < 0 || adj_off as usize >= data.len() {
                continue; // Skip this entry - bad offset after adjustment
            }
            let start = adj_off as usize;
            let end = start.saturating_add(val_size).min(data.len());
            (&data[start..end], false)
        };

        entries.push(tiff::IfdEntry {
            tag,
            data_type,
            raw_type,
            count,
            data: entry_data,
            inline,
        });
    }

    let ifd_end = entries_start + entry_count * 12;
    let next_ifd_offset = if ifd_end + 4 <= data.len() {
        if be {
            u32::from_be_bytes(data[ifd_end..ifd_end + 4].try_into().unwrap()) as u64
        } else {
            u32::from_le_bytes(data[ifd_end..ifd_end + 4].try_into().unwrap()) as u64
        }
    } else {
        0
    };

    Some(tiff::Ifd {
        offset,
        entries,
        next_ifd_offset,
    })
}

/// Detect Canon TIFF footer and compute base offset adjustment for relocated MakerNotes.
///
/// Canon MakerNotes have an 8-byte TIFF-like footer at the end of the data:
/// byte order marker (2 bytes) + TIFF magic 0x002A (2 bytes) + original offset (4 bytes).
/// When image editing software moves the MakerNote, the internal value offsets become stale.
/// This function detects the footer and returns (adjustment, footer_byte_order).
/// The footer byte order is the actual byte order of the MakerNote data, which may
/// differ from the parent TIFF (e.g. Canon always uses LE internally).
fn detect_canon_fix_base(mnr: &MakerNoteRef<'_>) -> (i64, Option<bool>) {
    let data = mnr.data;
    if data.len() < 8 {
        return (0, None);
    }

    // Check last 8 bytes for TIFF footer
    let footer = &data[data.len() - 8..];

    // Detect byte order marker
    let footer_be = if footer[0] == b'M' && footer[1] == b'M' {
        true
    } else if footer[0] == b'I' && footer[1] == b'I' {
        false
    } else {
        return (0, None);
    };

    // Check TIFF magic number (0x002A)
    let magic = if footer_be {
        u16::from_be_bytes([footer[2], footer[3]])
    } else {
        u16::from_le_bytes([footer[2], footer[3]])
    };
    if magic != 42 {
        return (0, None);
    }

    // Read original MakerNote offset from footer
    let original_offset = if footer_be {
        u32::from_be_bytes([footer[4], footer[5], footer[6], footer[7]])
    } else {
        u32::from_le_bytes([footer[4], footer[5], footer[6], footer[7]])
    } as i64;

    let current_offset = mnr.offset as i64;

    // If MakerNote has moved, compute the adjustment
    let fix = current_offset - original_offset;
    if fix == 0 {
        return (0, Some(footer_be));
    }

    // Validate: try the first IFD entry's external offset with the fix applied
    // to make sure it points within the TIFF data
    // Use footer_be for reading MN data since it declares the actual byte order
    let mn_be = footer_be;
    if data.len() > 14 {
        let entry_count = if mn_be {
            u16::from_be_bytes([data[0], data[1]])
        } else {
            u16::from_le_bytes([data[0], data[1]])
        } as usize;

        if entry_count > 0 && data.len() >= 14 {
            // First entry starts at offset 2, value/offset is at +8
            let raw_type = if mn_be {
                u16::from_be_bytes([data[4], data[5]])
            } else {
                u16::from_le_bytes([data[4], data[5]])
            };
            let dt = tiff::DataType::from_u16(raw_type).unwrap_or(tiff::DataType::Undefined);
            let count = if mn_be {
                u32::from_be_bytes([data[6], data[7], data[8], data[9]])
            } else {
                u32::from_le_bytes([data[6], data[7], data[8], data[9]])
            } as u64;
            let value_size = count.saturating_mul(dt.size() as u64);
            if value_size > 4 {
                // External offset - validate it makes sense with the fix
                let stored_offset = if mn_be {
                    u32::from_be_bytes([data[10], data[11], data[12], data[13]])
                } else {
                    u32::from_le_bytes([data[10], data[11], data[12], data[13]])
                } as i64;
                let adjusted = stored_offset + fix;
                if adjusted < 0 {
                    return (0, Some(footer_be)); // Fix would produce negative offset - invalid
                }
            }
        }
    }

    (fix, Some(footer_be))
}

/// One maker note dispatch rule.
///
/// Mirrors exiftool's `MakerNotes.pm` dispatch: rules are evaluated in order
/// and the first match wins, and every predicate of a rule must hold (AND).
/// Rules exiftool writes as a disjunction - for example `Make=~/^SONY/ or
/// (Make=~/^HASSELBLAD/ and Model=~/^(HV|Stellar|Lusso|Lunar)/)` - are split
/// into separate rules sharing one vendor, which is equivalent under
/// first-match evaluation.
///
/// This decides the *vendor* only. The payload layout (header size, whether
/// value offsets are absolute) is decided separately by
/// `tiff::exif::detect_maker_note_format`, so a newly added magic prefix has
/// to be taught there too, otherwise the maker note is parsed with the wrong
/// offsets.
#[derive(Clone, Copy)]
struct VendorRule {
    vendor: Vendor,
    /// Payload must start with one of these (empty = no constraint).
    magic: &'static [&'static [u8]],
    /// Payload must not start with any of these.
    magic_not: &'static [&'static [u8]],
    /// EXIF `Make` (0x010F) must start with one of these, ASCII case-insensitive.
    make: &'static [&'static str],
    /// EXIF `Model` (0x0110) must start with one of these.
    model: &'static [&'static str],
    /// EXIF `Model` must not start with any of these.
    model_not: &'static [&'static str],
}

impl VendorRule {
    const EMPTY: VendorRule = VendorRule {
        vendor: Vendor::Unknown,
        magic: &[],
        magic_not: &[],
        make: &[],
        model: &[],
        model_not: &[],
    };
}

/// Maker note dispatch table, in exiftool's evaluation order.
static VENDOR_RULES: &[VendorRule] = &[
    // MakerNoteApple
    VendorRule {
        vendor: Vendor::Apple,
        magic: &[b"Apple iOS\0"],
        ..VendorRule::EMPTY
    },
    // MakerNoteNikon3 / MakerNoteNikon2
    VendorRule {
        vendor: Vendor::Nikon,
        magic: &[b"Nikon\0\x02"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Nikon,
        magic: &[b"Nikon\0\x01"],
        ..VendorRule::EMPTY
    },
    // MakerNoteCanon
    VendorRule {
        vendor: Vendor::Canon,
        make: &["canon"],
        ..VendorRule::EMPTY
    },
    // MakerNoteCasio / MakerNoteCasio2
    VendorRule {
        vendor: Vendor::Casio,
        make: &["casio"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Casio,
        magic: &[b"QVC\0\0\0"],
        ..VendorRule::EMPTY
    },
    // MakerNoteFujifilm. exiftool also accepts `GENERALE` (General Electric
    // rebadges); that prefix has no layout rule yet, so it only resolves the
    // vendor.
    VendorRule {
        vendor: Vendor::Fujifilm,
        magic: &[b"FUJIFILM", b"GENERALE"],
        ..VendorRule::EMPTY
    },
    // MakerNoteGE. exiftool is stricter: `^GE(\0\0|NIC\0)`.
    VendorRule {
        vendor: Vendor::Ge,
        magic: &[b"GE\0", b"GENIC\0"],
        ..VendorRule::EMPTY
    },
    // MakerNoteJVC / MakerNoteJVC2 (Victor)
    VendorRule {
        vendor: Vendor::Jvc,
        magic: &[b"JVC "],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Jvc,
        magic: &[b"VER:"],
        make: &["jvc", "victor"],
        ..VendorRule::EMPTY
    },
    // MakerNoteKodak*: KDK payloads under an `EASTMAN KODAK` Make. The plain
    // Make rule keeps `AOC\0` payloads for Pentax, like exiftool does.
    VendorRule {
        vendor: Vendor::Kodak,
        magic: &[b"KDK"],
        make: &["eastman kodak"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Kodak,
        magic_not: &[b"AOC\0"],
        make: &["kodak"],
        ..VendorRule::EMPTY
    },
    // MakerNoteMinolta
    VendorRule {
        vendor: Vendor::Minolta,
        magic: &[b"MLT0"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Minolta,
        make: &["konica minolta", "minolta"],
        ..VendorRule::EMPTY
    },
    // MakerNoteOlympus / MakerNoteOlympus2 / OM System
    VendorRule {
        vendor: Vendor::Olympus,
        magic: &[b"OLYMP\0", b"EPSON\0"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Olympus,
        magic: &[b"OLYMPUS\0"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Olympus,
        magic: &[b"OM SYSTEM\0"],
        ..VendorRule::EMPTY
    },
    // MakerNotePanasonic, plus Leica rebadges that share the Panasonic tags
    VendorRule {
        vendor: Vendor::Panasonic,
        magic: &[b"Panasonic\0"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Panasonic,
        magic: &[b"MKE"],
        make: &["panasonic"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Panasonic,
        make: &["leica"],
        ..VendorRule::EMPTY
    },
    // MakerNotePentax. The Optio 330RS/430RS carry an `AOC\0` payload but a
    // Casio-style body, which exiftool excludes here.
    VendorRule {
        vendor: Vendor::Pentax,
        magic: &[b"AOC\0"],
        model_not: &["pentax optio 330rs", "pentax optio 430rs"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Pentax,
        magic: &[b"PENTAX \0"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Pentax,
        make: &["asahi", "pentax"],
        ..VendorRule::EMPTY
    },
    // MakerNoteSamsung
    VendorRule {
        vendor: Vendor::Samsung,
        magic: &[b"STMN"],
        ..VendorRule::EMPTY
    },
    // MakerNoteSanyo. exiftool splits on `Model` (C4/J*/S*) to pick between
    // two Sanyo tables; we only have one, so the split is not represented.
    VendorRule {
        vendor: Vendor::Sanyo,
        magic: &[b"SANYO\0"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Sanyo,
        make: &["sanyo"],
        ..VendorRule::EMPTY
    },
    // MakerNoteSony: 12-byte `SONY DSC \0\0\0` and friends. Only `SONY DSC `
    // has a layout rule, the others still resolve the vendor.
    VendorRule {
        vendor: Vendor::Sony,
        magic: &[
            b"SONY DSC ",
            b"SONY CAM ",
            b"SONY MOBILE",
            b"\0\0SONY PIC\0",
            b"VHAB     \0",
        ],
        ..VendorRule::EMPTY
    },
    // MakerNoteSony5 / MakerNoteSonySRF: headerless SR2 and ARW payloads.
    VendorRule {
        vendor: Vendor::Sony,
        make: &["sony"],
        ..VendorRule::EMPTY
    },
    // Sony-built Hasselblad rebadges (HV / Stellar / Lusso / Lunar).
    VendorRule {
        vendor: Vendor::Sony,
        make: &["hasselblad"],
        model: &["hv", "stellar", "lusso", "lunar"],
        ..VendorRule::EMPTY
    },
    // MakerNoteSigma
    VendorRule {
        vendor: Vendor::Sigma,
        magic: &[b"SIGMA\0\0\0", b"FOVEON\0\0"],
        ..VendorRule::EMPTY
    },
    // MakerNoteMotorola
    VendorRule {
        vendor: Vendor::Motorola,
        magic: &[b"MOT\0"],
        ..VendorRule::EMPTY
    },
    // MakerNoteReconyx, plus the HyperFire `01 F1` magic
    VendorRule {
        vendor: Vendor::Reconyx,
        magic: &[
            b"RECONYXUF\0",
            b"RECONYXH2\0",
            b"RECONYXMF\0",
            b"RECONYXHF4K\0",
        ],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Reconyx,
        magic: &[b"\x01\xf1"],
        ..VendorRule::EMPTY
    },
    // MakerNoteRicoh. exiftool routes `RICOH\0(II|MM)` payloads to the Pentax
    // table (GR III); we keep them under Ricoh.
    VendorRule {
        vendor: Vendor::Ricoh,
        magic: &[b"Ricoh", b"RICOH"],
        ..VendorRule::EMPTY
    },
    VendorRule {
        vendor: Vendor::Ricoh,
        make: &["ricoh", "pentax ricoh"],
        ..VendorRule::EMPTY
    },
    // MakerNoteFLIR
    VendorRule {
        vendor: Vendor::Flir,
        make: &["flir systems", "teledyne flir"],
        ..VendorRule::EMPTY
    },
];

fn starts_with_ignore_case(value: &str, lower_prefix: &str) -> bool {
    value.len() >= lower_prefix.len()
        && value.as_bytes()[..lower_prefix.len()].eq_ignore_ascii_case(lower_prefix.as_bytes())
}

/// A `Make`/`Model` predicate holds when it is empty, or when the tag is
/// present and starts with one of the prefixes.
fn tag_starts_with(value: Option<&str>, prefixes: &[&str]) -> bool {
    match value {
        Some(value) => {
            prefixes.is_empty() || prefixes.iter().any(|p| starts_with_ignore_case(value, p))
        }
        None => prefixes.is_empty(),
    }
}

fn rule_matches(rule: &VendorRule, data: &[u8], make: Option<&str>, model: Option<&str>) -> bool {
    if !rule.magic.is_empty() && !rule.magic.iter().any(|magic| data.starts_with(magic)) {
        return false;
    }
    if rule.magic_not.iter().any(|magic| data.starts_with(magic)) {
        return false;
    }
    if !tag_starts_with(make, rule.make) {
        return false;
    }
    if !tag_starts_with(model, rule.model) {
        return false;
    }
    if rule
        .model_not
        .iter()
        .any(|prefix| model.is_some_and(|model| starts_with_ignore_case(model, prefix)))
    {
        return false;
    }
    true
}

/// Detect vendor from raw maker note data.
///
/// Payload-only form of [`resolve_vendor`]: rules that need `Make` or `Model`
/// cannot match, so only the magic prefixes are considered.
pub fn detect_vendor(data: &[u8]) -> Vendor {
    resolve_vendor(data, None, None)
}

/// Detect vendor from maker note data plus the EXIF `Make` (0x010F) and
/// `Model` (0x0110) strings.
///
/// Rules are evaluated in order and the first match wins. Anything the table
/// misses falls through to [`vendor_from_make`], which is broader (substring
/// match) and keeps vendors with no maker note header working.
pub fn resolve_vendor(data: &[u8], make: Option<&str>, model: Option<&str>) -> Vendor {
    for rule in VENDOR_RULES {
        if rule_matches(rule, data, make, model) {
            return rule.vendor;
        }
    }
    vendor_from_make(make.unwrap_or(""))
}

/// Identify vendor from the EXIF Make string.
pub fn vendor_from_make(make: &str) -> Vendor {
    let lower = make.to_ascii_lowercase();
    if lower.contains("canon") {
        return Vendor::Canon;
    }
    if lower.contains("nikon") {
        return Vendor::Nikon;
    }
    if lower.contains("sony") {
        return Vendor::Sony;
    }
    if lower.contains("fuji") {
        return Vendor::Fujifilm;
    }
    if lower.contains("panasonic") || lower.contains("leica") {
        return Vendor::Panasonic;
    }
    if lower.contains("olympus") || lower.contains("om digital") {
        return Vendor::Olympus;
    }
    if lower.contains("samsung") {
        return Vendor::Samsung;
    }
    if lower.contains("apple") {
        return Vendor::Apple;
    }
    if lower.contains("pentax") || lower.contains("asahi") {
        return Vendor::Pentax;
    }
    if lower.contains("casio") {
        return Vendor::Casio;
    }
    if lower.contains("minolta") || lower.contains("konica") {
        return Vendor::Minolta;
    }
    if lower.contains("kodak") || lower.contains("eastman") {
        return Vendor::Kodak;
    }
    if lower.contains("sanyo") {
        return Vendor::Sanyo;
    }
    if lower.contains("ricoh") {
        return Vendor::Ricoh;
    }
    if lower.contains("sigma") || lower.contains("foveon") {
        return Vendor::Sigma;
    }
    if lower.contains("motorola") {
        return Vendor::Motorola;
    }
    if lower.contains("jvc") || lower.contains("victor") {
        return Vendor::Jvc;
    }
    if lower.contains("reconyx") {
        return Vendor::Reconyx;
    }
    if lower.contains("flir") {
        return Vendor::Flir;
    }
    if lower.contains("general imaging") || lower.contains("ge ") {
        return Vendor::Ge;
    }
    Vendor::Unknown
}

/// Decode all maker note tags into name-value pairs.
///
/// For Canon, this includes sub-array decoding of CameraSettings (0x0001),
/// FocalLength (0x0002), and ShotInfo (0x0004) arrays.
/// For other vendors, this decodes top-level IFD entries.
pub fn decode_maker_tags(mn: &MakerNote<'_>) -> Vec<DecodedTag> {
    decode_maker_tags_with_data(mn, &[], 0, 0)
}

/// Decode MakerNote tags with access to the raw MakerNote data for sub-IFD resolution.
/// `tiff_base` is the offset of the TIFF header from the start of the file,
/// used to adjust offset-type tags (PreviewImageStart) to file-relative values.
/// `mn_file_offset` is the absolute file position of the MakerNote data.
pub fn decode_maker_tags_with_data<'a>(
    mn: &MakerNote<'a>,
    mn_data: &'a [u8],
    tiff_base: usize,
    mn_file_offset: usize,
) -> Vec<DecodedTag> {
    decode_maker_tags_impl(mn, mn_data, tiff_base, mn_file_offset, &[])
}

/// Decode MakerNote tags with access to full TIFF data for absolute offset resolution.
pub fn decode_maker_tags_with_tiff<'a>(
    mn: &MakerNote<'a>,
    mn_data: &'a [u8],
    tiff_base: usize,
    mn_file_offset: usize,
    tiff_data: &'a [u8],
) -> Vec<DecodedTag> {
    decode_maker_tags_impl(mn, mn_data, tiff_base, mn_file_offset, tiff_data)
}

fn decode_maker_tags_impl<'a>(
    mn: &MakerNote<'a>,
    mn_data: &'a [u8],
    tiff_base: usize,
    mn_file_offset: usize,
    tiff_data: &'a [u8],
) -> Vec<DecodedTag> {
    let mut tags = Vec::new();

    // Kodak binary maker notes - not IFD-based, decode from raw bytes
    if mn.vendor == Vendor::Kodak && mn.ifd.is_none() && !mn_data.is_empty() {
        decode_kodak_binary(mn_data, &mut tags);
        return tags;
    }

    // Reconyx HyperFire binary maker notes
    if mn.vendor == Vendor::Reconyx && mn.ifd.is_none() && mn_data.len() > 0x56 {
        decode_reconyx_hyperfire(mn_data, &mut tags);
        return tags;
    }

    // JVC text-format maker notes: "VER:0100QTY:FINE"
    if mn.vendor == Vendor::Jvc && mn.ifd.is_none() && !mn_data.is_empty() {
        if let Ok(text) = std::str::from_utf8(mn_data) {
            if text.starts_with("VER:") {
                decode_jvc_text(text, &mut tags);
                return tags;
            }
        }
    }

    // Ricoh text-format maker notes (e.g. RDC5300): "Rv0207;Rg76;Bg60;Gg42;..."
    if mn.vendor == Vendor::Ricoh && mn.ifd.is_none() && !mn_data.is_empty() {
        if let Ok(text) = std::str::from_utf8(mn_data) {
            if text.starts_with("Rv") || text.starts_with("Rev") {
                decode_ricoh_text(text, &mut tags);
                return tags;
            }
        }
    }

    let Some(ifd) = mn.ifd.as_ref() else {
        return tags;
    };
    let be = mn.big_endian;

    // Detect Nikon Type 2 (old Coolpix) for different tag table
    let nikon_type2 = mn.vendor == Vendor::Nikon
        && mn_data.len() > 8
        && mn_data.starts_with(b"Nikon\0")
        && mn_data.get(6) == Some(&0x01);

    // Pre-scan Canon model from CanonImageType (tag 0x0006) for CameraInfo dispatch
    let canon_model = if mn.vendor == Vendor::Canon {
        ifd.entries
            .iter()
            .find(|e| e.tag == 0x0006)
            .and_then(|e| std::str::from_utf8(e.data).ok())
            .map(|s| s.trim_end_matches('\0').to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    // Pre-scan Olympus model from Equipment sub-IFD (0x2010) CameraType2 (0x0100)
    let olympus_model = if mn.vendor == Vendor::Olympus {
        extract_olympus_model(ifd, mn_data, tiff_data, be)
    } else {
        String::new()
    };

    for entry in &ifd.entries {
        let name = if nikon_type2 {
            lookup_tag(entry.tag, &NIKON_TYPE2_TAGS)
        } else {
            maker_tag_name(entry.tag, mn.vendor)
        };

        match mn.vendor {
            Vendor::Canon => {
                // Sub-array tags: decode individual fields
                match entry.tag {
                    0x0001 => decode_canon_camera_settings(entry.data, be, &mut tags),
                    0x0002 => decode_canon_focal_length(entry.data, be, &mut tags),
                    0x0004 => decode_canon_shot_info(entry.data, be, &mut tags),
                    0x000D => decode_canon_camera_info(entry, be, &canon_model, &mut tags),
                    0x0012 => decode_canon_af_info(entry.data, be, &mut tags),
                    0x0024 => decode_canon_face_detect1(entry.data, be, &mut tags),
                    0x0026 => decode_canon_af_info2(entry.data, be, &mut tags),
                    0x003C => decode_canon_af_info2(entry.data, be, &mut tags), // AFInfo3 uses same layout
                    0x002f => decode_canon_face_detect3(entry.data, be, &mut tags),
                    0x0035 => decode_canon_time_info(entry.data, be, &mut tags),
                    0x0093 => decode_canon_file_info(entry.data, be, &mut tags),
                    0x00E0 => decode_canon_sensor_info(entry.data, be, &mut tags),
                    0x009A => decode_canon_aspect_info(entry.data, be, &mut tags),
                    0x00A0 => decode_canon_processing_info(entry.data, be, &mut tags),
                    0x00A9 => decode_canon_color_balance(entry.data, be, &mut tags),
                    0x00AA => {
                        // MeasuredColor: FORMAT=int16u, FIRST_ENTRY=1
                        // Index 1 = MeasuredRGGB (4 x int16u) -> bytes 2..10
                        if entry.data.len() >= 10 {
                            let get_u16 = |off: usize| -> u16 {
                                if be {
                                    u16::from_be_bytes([entry.data[off], entry.data[off + 1]])
                                } else {
                                    u16::from_le_bytes([entry.data[off], entry.data[off + 1]])
                                }
                            };
                            let r = get_u16(2);
                            let g1 = get_u16(4);
                            let g2 = get_u16(6);
                            let b = get_u16(8);
                            tags.push(DecodedTag {
                                name: "MeasuredRGGB".to_string(),
                                value: format!("{r} {g1} {g2} {b}"),
                            });
                        }
                    }
                    0x001D => {
                        // MyColors: FORMAT=int16u, index 2 = MyColorMode
                        if entry.data.len() >= 6 {
                            let v = if be {
                                u16::from_be_bytes([entry.data[4], entry.data[5]])
                            } else {
                                u16::from_le_bytes([entry.data[4], entry.data[5]])
                            };
                            tags.push(DecodedTag {
                                name: "MyColorMode".to_string(),
                                value: match v {
                                    0 => "Off".into(),
                                    1 => "Positive Film".into(),
                                    2 => "Light Skin Tone".into(),
                                    3 => "Dark Skin Tone".into(),
                                    4 => "Vivid Blue".into(),
                                    5 => "Vivid Green".into(),
                                    6 => "Vivid Red".into(),
                                    7 => "Color Accent".into(),
                                    8 => "Color Swap".into(),
                                    9 => "Custom".into(),
                                    12 => "Vivid".into(),
                                    13 => "Neutral".into(),
                                    14 => "Sepia".into(),
                                    15 => "B&W".into(),
                                    v => format!("{v}"),
                                },
                            });
                        }
                    }
                    0x0023 => {
                        // Categories: 2 x int32u, first is always 8, second is bitmask
                        if entry.data.len() >= 8 {
                            let v = if be {
                                u32::from_be_bytes([
                                    entry.data[4],
                                    entry.data[5],
                                    entry.data[6],
                                    entry.data[7],
                                ])
                            } else {
                                u32::from_le_bytes([
                                    entry.data[4],
                                    entry.data[5],
                                    entry.data[6],
                                    entry.data[7],
                                ])
                            };
                            let mut cats = Vec::new();
                            if v & 0x01 != 0 {
                                cats.push("People");
                            }
                            if v & 0x02 != 0 {
                                cats.push("Scenery");
                            }
                            if v & 0x04 != 0 {
                                cats.push("Events");
                            }
                            if v & 0x08 != 0 {
                                cats.push("User 1");
                            }
                            if v & 0x10 != 0 {
                                cats.push("User 2");
                            }
                            if v & 0x20 != 0 {
                                cats.push("User 3");
                            }
                            if v & 0x40 != 0 {
                                cats.push("To Do");
                            }
                            let display = if cats.is_empty() {
                                "(none)".into()
                            } else {
                                cats.join(", ")
                            };
                            tags.push(DecodedTag {
                                name: "Categories".to_string(),
                                value: display,
                            });
                        }
                    }
                    0x0027 => {
                        // ContrastInfo: FORMAT=int16u, index 4 = IntelligentContrast
                        if entry.data.len() >= 10 {
                            let v = if be {
                                u16::from_be_bytes([entry.data[8], entry.data[9]])
                            } else {
                                u16::from_le_bytes([entry.data[8], entry.data[9]])
                            };
                            tags.push(DecodedTag {
                                name: "IntelligentContrast".to_string(),
                                value: match v {
                                    0x00 => "Off".into(),
                                    0x08 => "On".into(),
                                    0xffff => "n/a".into(),
                                    v => {
                                        if v & 0x08 != 0 {
                                            format!("On (0x{v:02x})")
                                        } else {
                                            format!("Off (0x{v:02x})")
                                        }
                                    }
                                },
                            });
                        }
                    }
                    0x0098 => decode_canon_crop_info(entry.data, be, &mut tags),
                    0x0099 => {
                        decode_canon_custom_functions2(entry.data, be, &canon_model, &mut tags);
                    }
                    0x4001 => decode_canon_color_data(entry.data, be, &mut tags),
                    0x4013 => decode_canon_af_micro_adj(entry.data, be, &mut tags),
                    0x4015 => decode_canon_vignetting_corr(entry.data, be, &mut tags),
                    0x4016 => decode_canon_vignetting_corr2(entry.data, be, &mut tags),
                    0x4018 => decode_canon_lighting_opt(entry.data, be, &mut tags),
                    0x4020 => decode_canon_ambience(entry.data, be, &mut tags),
                    0x4025 => decode_canon_hdr_info(entry.data, be, &mut tags),
                    0x0007 => {
                        // CanonFirmwareVersion: "Firmware Version X.Y.Z" -> also emit FirmwareVersion "X.Y.Z"
                        let val = format_canon_value(entry, name, be);
                        if let Some(ver) = val.strip_prefix("Firmware Version ") {
                            tags.push(DecodedTag {
                                name: "FirmwareVersion".to_string(),
                                value: ver.to_string(),
                            });
                        }
                        tags.push(DecodedTag {
                            name: name.to_string(),
                            value: val,
                        });
                    }
                    0x0028 => {
                        // ImageUniqueID: binary data -> hex string
                        let all_zero = entry.data.iter().all(|&b| b == 0);
                        if !all_zero && !entry.data.is_empty() {
                            let hex: String =
                                entry.data.iter().map(|b| format!("{b:02x}")).collect();
                            tags.push(DecodedTag {
                                name: "ImageUniqueID".to_string(),
                                value: hex,
                            });
                        }
                    }
                    _ if name != "Unknown" => {
                        let val = format_canon_value(entry, name, be);
                        tags.push(DecodedTag {
                            name: name.to_string(),
                            value: val,
                        });
                    }
                    _ => {}
                }
            }
            Vendor::Nikon if !nikon_type2 => {
                // Skip - processed separately below (needs pre-scan for encryption keys)
                continue;
            }
            Vendor::Nikon => {
                // Nikon Type 2 (old Coolpix): simple tag values, no encryption
                if name != "Unknown" {
                    let val = format_ifd_value(entry, be);
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
            Vendor::Fujifilm => {
                if name != "Unknown" {
                    let val = format_fuji_value(entry, name, be);
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
            Vendor::Panasonic => {
                if entry.tag == 0x004e {
                    decode_panasonic_faces(entry.data, be, &mut tags);
                } else if name != "Unknown" {
                    let val = format_panasonic_value(entry, name, be);
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
            Vendor::Apple => match entry.tag {
                0x0003 => decode_apple_runtime(entry.data, &mut tags),
                _ if name != "Unknown" => {
                    let val = format_apple_value(entry, name, be);
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
                _ => {}
            },
            Vendor::Olympus => {
                match entry.tag {
                    0x0208 => {
                        // TextInfo: space-separated key=value pairs (APP12-style)
                        decode_olympus_text_info(entry.data, &mut tags);
                    }
                    0x2010 | 0x2020 | 0x2030 | 0x2031 | 0x2040 | 0x2050 => {
                        // Old-style "OLYMP\0" uses absolute offsets (TIFF-relative),
                        // so sub-IFDs must be resolved against tiff_data.
                        // New-style "OLYMPUS\0" uses MN-relative offsets.
                        let oly_new_style = mn_data.starts_with(b"OLYMPUS\0");
                        let sub_ifd_data = if oly_new_style || tiff_data.is_empty() {
                            mn_data
                        } else {
                            tiff_data
                        };
                        decode_olympus_sub_ifd(entry, sub_ifd_data, be, &olympus_model, &mut tags);
                    }
                    _ if name != "Unknown" => {
                        let val = format_olympus_value(entry, name, be);
                        tags.push(DecodedTag {
                            name: name.to_string(),
                            value: val,
                        });
                    }
                    _ => {}
                }
            }
            Vendor::Pentax => {
                // Skip - processed separately below (needs Date/Time for ShutterCount decryption)
                continue;
            }
            Vendor::Casio => {
                let is_type2 = ifd.entries.iter().any(|e| e.tag >= 0x2000);
                // Override tag names for Type 2 dual-use IDs
                let out_name = if is_type2 {
                    match entry.tag {
                        0x0002 => "PreviewImageSize",
                        0x0003 => "PreviewImageLength",
                        0x0004 => "PreviewImageStart",
                        _ => name,
                    }
                } else {
                    name
                };
                if out_name != "Unknown" {
                    let val = format_casio_value(entry, out_name, be, entry.tag >= 0x2000);
                    tags.push(DecodedTag {
                        name: out_name.to_string(),
                        value: val,
                    });
                }
            }
            Vendor::Sanyo => {
                if name != "Unknown" {
                    let val = format_sanyo_value(entry, name, be);
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
            Vendor::Minolta => {
                match entry.tag {
                    0x0001 | 0x0003 => {
                        // MinoltaCameraSettings: binary int32u array, always big-endian
                        decode_minolta_camera_settings(entry.data, &mut tags);
                    }
                    _ if name != "Unknown" => {
                        let val = format_minolta_value(entry, name, be);
                        tags.push(DecodedTag {
                            name: name.to_string(),
                            value: val,
                        });
                    }
                    _ => {}
                }
            }
            Vendor::Sigma => {
                if name != "Unknown" {
                    let val = format_sigma_value(entry, name, be);
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
            Vendor::Kodak => {
                if name != "Unknown" {
                    let val = format_ifd_value(entry, be);
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
            Vendor::Ricoh => {
                match entry.tag {
                    0x0005 => {
                        // InternalSerialNumber: undef[16], hex-encoded
                        let hex = entry
                            .data
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>();
                        tags.push(DecodedTag {
                            name: "InternalSerialNumber".into(),
                            value: hex,
                        });
                    }
                    0x1001
                        if entry.data_type == crate::tiff::DataType::Undefined
                            && entry.data.len() >= 4 =>
                    {
                        // ImageInfo binary data: RicohImageWidth(int16u@0), RicohImageHeight(int16u@2)
                        // Ricoh uses big-endian for this data
                        let w = u16::from_be_bytes([entry.data[0], entry.data[1]]);
                        let h = u16::from_be_bytes([entry.data[2], entry.data[3]]);
                        tags.push(DecodedTag {
                            name: "RicohImageWidth".into(),
                            value: format!("{w}"),
                        });
                        tags.push(DecodedTag {
                            name: "RicohImageHeight".into(),
                            value: format!("{h}"),
                        });
                        // RicohDate: 7 bytes at offset 6, each byte's hex is used as date digits
                        if entry.data.len() >= 13 {
                            let d = &entry.data[6..13];
                            let val = format!(
                                "{:02x}{:02x}:{:02x}:{:02x} {:02x}:{:02x}:{:02x}",
                                d[0], d[1], d[2], d[3], d[4], d[5], d[6]
                            );
                            tags.push(DecodedTag {
                                name: "RicohDate".into(),
                                value: val,
                            });
                        }
                    }
                    0x2001 => {
                        // Ricoh sub-directory: starts with "[Ricoh Camera Info]" (20 bytes)
                        // followed by IFD with ManufactureDate1/2, big-endian.
                        // Some models have sub-IFD offsets relative to block start,
                        // others use TIFF-absolute offsets (need tiff_data for resolution).
                        decode_ricoh_subdir(entry, tiff_data, &mut tags);
                    }
                    _ if name != "Unknown" => {
                        let val = format_ricoh_value(entry, name, be);
                        // Tag 0x1003: if data type is not int16u, it's "Sharpness" not "FocusMode"
                        let out_name = if entry.tag == 0x1003
                            && entry.data_type != crate::tiff::DataType::Short
                        {
                            "Sharpness"
                        } else {
                            name
                        };
                        tags.push(DecodedTag {
                            name: out_name.to_string(),
                            value: val,
                        });
                    }
                    _ => {}
                }
            }
            Vendor::Sony => {
                if name != "Unknown" {
                    let val = if entry.tag == 0x2037 && entry.data.len() >= 6 {
                        let read = |offset: usize| {
                            if be {
                                u16::from_be_bytes([entry.data[offset], entry.data[offset + 1]])
                            } else {
                                u16::from_le_bytes([entry.data[offset], entry.data[offset + 1]])
                            }
                        };
                        let width = read(0);
                        let height = read(2);
                        let valid = read(4);
                        if valid == 0 {
                            "n/a".to_owned()
                        } else {
                            format!("{width}x{height}")
                        }
                    } else {
                        format_ifd_value(entry, be)
                    };
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
            Vendor::Jvc => {
                if name != "Unknown" {
                    let val = match entry.tag {
                        0x0002 => {
                            // CPUVersions: remove trailing nulls/spaces, split at remaining
                            let s = String::from_utf8_lossy(entry.data);
                            let s = s.trim_end_matches(['\0', ' ']);
                            s.split('\0').map(str::trim).collect::<Vec<_>>().join(", ")
                        }
                        0x0003 => {
                            // Quality: PrintConv
                            let v = if be {
                                entry.data.first().map_or(0, |&b| b as u16)
                            } else {
                                entry.data.first().map_or(0, |&b| b as u16)
                            };
                            let v = if entry.data.len() >= 2 {
                                if be {
                                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                                } else {
                                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                                }
                            } else {
                                v
                            };
                            match v {
                                0 => "Low".into(),
                                1 => "Normal".into(),
                                2 => "Fine".into(),
                                _ => format!("{v}"),
                            }
                        }
                        _ => format_ifd_value(entry, be),
                    };
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
            Vendor::Ge => {
                if name != "Unknown" {
                    let val = match entry.tag {
                        0x0202 => match entry_u16(entry, be) {
                            Some(0) => "Off".into(),
                            Some(1) => "On".into(),
                            _ => format_ifd_value(entry, be),
                        },
                        0x0207 | 0x0300 => {
                            // GEModel, GEMake: string (may be stored as undef)
                            let s = std::str::from_utf8(entry.data).unwrap_or("");
                            s.trim_end_matches('\0').to_string()
                        }
                        _ => format_ifd_value(entry, be),
                    };
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
            _ => {
                if name != "Unknown" {
                    let val = format_ifd_value(entry, be);
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
            }
        }
    }

    // Nikon: two-pass processing (pre-scan keys, then decode with decryption)
    if mn.vendor == Vendor::Nikon && !nikon_type2 {
        // Pass 1: find serial number and shutter count for decryption
        let mut serial: u32 = 0;
        let mut shutter_count: u32 = 0;
        for entry in &ifd.entries {
            match entry.tag {
                0x001D => {
                    // SerialNumber - extract numeric value
                    if let Some(s) = entry_string(entry) {
                        if let Ok(v) = s.parse::<u32>() {
                            serial = v;
                        }
                    }
                }
                0x00A7 => {
                    // ShutterCount - int32u
                    if let Some(v) = entry_u32(entry, be) {
                        shutter_count = v;
                    }
                }
                _ => {}
            }
        }

        // Pass 2: decode all tags
        for entry in &ifd.entries {
            let name = maker_tag_name(entry.tag, mn.vendor);
            match entry.tag {
                0x0011 => decode_nikon_preview_ifd(
                    entry,
                    mn_data,
                    mn.nikon_tiff_offset,
                    mn_file_offset,
                    be,
                    &mut tags,
                ),
                0x001F => decode_nikon_vr_info(entry.data, &mut tags),
                0x0021 => decode_nikon_face_detect(entry.data, be, &mut tags),
                0x0023 | 0x00BD => decode_nikon_picture_control(entry.data, &mut tags),
                0x0025 => decode_nikon_iso_info(entry.data, &mut tags),
                0x002B => decode_nikon_distort_info(entry.data, &mut tags),
                0x0024 => decode_nikon_world_time(entry.data, &mut tags),
                0x0088 => decode_nikon_af_info(entry.data, &mut tags),
                0x0098 => {
                    // LensData - decrypt if needed
                    if entry.data.len() >= 4 && entry.data[0..4].iter().any(|&b| b.is_ascii_digit())
                    {
                        let ver = std::str::from_utf8(&entry.data[..4]).unwrap_or("");
                        if ver.starts_with("02") || ver.starts_with("04") || ver.starts_with("08") {
                            let mut decrypted = entry.data.to_vec();
                            nikon_decrypt(&mut decrypted[4..], serial, shutter_count);
                            decode_nikon_lens_data(&decrypted, &mut tags);
                        } else {
                            decode_nikon_lens_data(entry.data, &mut tags);
                        }
                    } else {
                        decode_nikon_lens_data(entry.data, &mut tags);
                    }
                }
                0x0091 => {
                    // ShotInfo - decrypt if needed
                    if entry.data.len() >= 4 && entry.data[0..4].iter().any(|&b| b.is_ascii_digit())
                    {
                        let ver = std::str::from_utf8(&entry.data[..4]).unwrap_or("");
                        if ver.starts_with("02") {
                            let mut decrypted = entry.data.to_vec();
                            nikon_decrypt(&mut decrypted[4..], serial, shutter_count);
                            decode_nikon_shot_info(&decrypted, &mut tags);
                        } else {
                            decode_nikon_shot_info(entry.data, &mut tags);
                        }
                    } else {
                        decode_nikon_shot_info(entry.data, &mut tags);
                    }
                }
                0x0097 => {
                    // ColorBalance - version-dependent decryption + WB extraction
                    decode_nikon_color_balance(entry.data, serial, shutter_count, be, &mut tags);
                }
                0x00A8 => decode_nikon_flash_info(entry.data, &mut tags),
                0x00B0 => decode_nikon_multi_exposure(entry.data, &mut tags),
                0x00B7 => decode_nikon_af_info2(entry.data, &mut tags),
                0x00B8 => decode_nikon_file_info(entry.data, &mut tags),
                0x00B9 => decode_nikon_af_tune(entry.data, &mut tags),
                _ if name != "Unknown" => {
                    let val = format_nikon_value(entry, name, be);
                    tags.push(DecodedTag {
                        name: name.to_string(),
                        value: val,
                    });
                }
                _ => {}
            }
        }
    }

    // Pentax: two-pass processing (pre-scan Date/Time for ShutterCount decryption)
    if mn.vendor == Vendor::Pentax {
        // Pass 1: find Date and Time for ShutterCount decryption
        let mut pentax_date: Option<[u8; 4]> = None;
        let mut pentax_time: Option<[u8; 3]> = None;
        for entry in &ifd.entries {
            match entry.tag {
                0x0006 => {
                    // Date: 4 bytes (year_hi, year_lo, month, day) - always big-endian year
                    if entry.data.len() >= 4 {
                        pentax_date =
                            Some([entry.data[0], entry.data[1], entry.data[2], entry.data[3]]);
                    }
                }
                0x0007
                    // Time: 3 bytes (hour, minute, second)
                    if entry.data.len() >= 3 => {
                        pentax_time = Some([entry.data[0], entry.data[1], entry.data[2]]);
                    }
                _ => {}
            }
        }

        // Pass 2: decode all tags
        for entry in &ifd.entries {
            let name = maker_tag_name(entry.tag, mn.vendor);
            if name == "Unknown" {
                continue;
            }

            if name == "ShutterCount" {
                // Decrypt: val ^ date_u32 ^ (0xFFFFFFFF - time_u32)
                if let (Some(date), Some(time)) = (pentax_date, pentax_time) {
                    if let Some(raw) = entry_u32(entry, be) {
                        let date_u32 = u32::from_be_bytes(date);
                        let time_u32 = u32::from_be_bytes([time[0], time[1], time[2], 0]);
                        let decrypted = raw ^ date_u32 ^ (0xFFFFFFFF - time_u32);
                        tags.push(DecodedTag {
                            name: "ShutterCount".to_string(),
                            value: format!("{decrypted}"),
                        });
                        continue;
                    }
                }
            }

            match entry.tag {
                0x0205 => {
                    decode_pentax_camera_settings(entry.data, &mut tags);
                    continue;
                }
                0x0206 => {
                    decode_pentax_ae_info(entry.data, &mut tags);
                    continue;
                }
                0x0208 => {
                    decode_pentax_flash_info(entry.data, &mut tags);
                    continue;
                }
                0x005c => {
                    decode_pentax_sr_info(entry.data, &mut tags);
                    continue;
                }
                0x0207 => {
                    decode_pentax_lens_info(entry.data, &mut tags);
                    continue;
                }
                0x0215 => {
                    decode_pentax_camera_info(entry.data, &mut tags);
                    continue;
                }
                0x0216 => {
                    decode_pentax_battery_info(entry.data, &mut tags);
                    continue;
                }
                0x021f => {
                    decode_pentax_af_info(entry.data, &mut tags);
                    continue;
                }
                0x0222 => {
                    decode_pentax_color_info(entry.data, &mut tags);
                    continue;
                }
                _ => {}
            }

            let val = format_pentax_value(entry, name, be);
            tags.push(DecodedTag {
                name: name.to_string(),
                value: val,
            });
        }
    }

    // Adjust PreviewImageStart from TIFF-relative to file-relative offset.
    // Skip values already marked as file-relative (prefixed with '!').
    if tiff_base > 0 {
        for tag in &mut tags {
            if tag.name == "PreviewImageStart" {
                if tag.value.starts_with('!') {
                    // Already file-relative - remove the marker
                    tag.value = tag.value[1..].to_string();
                } else if let Ok(v) = tag.value.parse::<u64>() {
                    if v > 0 {
                        tag.value = format!("{}", v.wrapping_add(tiff_base as u64));
                    }
                }
            }
        }
    }

    tags
}

/// Format an IFD entry value as a display string.
fn format_ifd_value(entry: &IfdEntry<'_>, big_endian: bool) -> String {
    use crate::tiff::value::TagValue;
    if let Some(val) = TagValue::from_entry(entry, big_endian) {
        val.display()
    } else {
        format!("({} bytes)", entry.data.len())
    }
}

/// Extract a string value from an IFD entry, title-cased.
fn entry_string_titlecase(entry: &IfdEntry<'_>) -> Option<String> {
    let s = std::str::from_utf8(entry.data).ok()?;
    let s = s.trim_end_matches('\0').trim();
    if s.is_empty() {
        return None;
    }
    // Title-case each word, but preserve abbreviations
    let words: Vec<String> = s
        .split(' ')
        .map(|word| {
            if word.is_empty() {
                return String::new();
            }
            // Preserve abbreviations: short uppercase words with non-alpha chars (AF-S, CS)
            // or words ≤2 chars that are all uppercase (CS, VR, etc.)
            if word.contains('-') && word.chars().all(|c| c.is_ascii_uppercase() || c == '-') {
                return word.to_string();
            }
            if word.len() <= 2 && word.chars().all(|c| c.is_ascii_uppercase()) {
                return word.to_string();
            }
            let mut chars = word.chars();
            let first = chars.next().unwrap().to_uppercase().to_string();
            let rest: String = chars
                .map(|c| c.to_lowercase().next().unwrap_or(c))
                .collect();
            format!("{first}{rest}")
        })
        .collect();
    Some(words.join(" "))
}

/// Title-case a string (e.g. "STANDARD" -> "Standard").
fn titlecase_str(s: &str) -> String {
    s.split(' ')
        .map(|word| {
            if word.is_empty() {
                return String::new();
            }
            if word.contains('-') && word.chars().all(|c| c.is_ascii_uppercase() || c == '-') {
                return word.to_string();
            }
            if word.len() <= 2 && word.chars().all(|c| c.is_ascii_uppercase()) {
                return word.to_string();
            }
            let mut chars = word.chars();
            let first = chars.next().unwrap().to_uppercase().to_string();
            let rest: String = chars
                .map(|c| c.to_lowercase().next().unwrap_or(c))
                .collect();
            format!("{first}{rest}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract raw string from entry.
fn entry_string(entry: &IfdEntry<'_>) -> Option<String> {
    let s = std::str::from_utf8(entry.data).ok()?;
    let s = s.trim_end_matches('\0').trim();
    if s.is_empty() {
        return None;
    }
    Some(s.to_string())
}

/// Get first u16 from entry.
fn entry_u16(entry: &IfdEntry<'_>, be: bool) -> Option<u16> {
    if entry.data.len() < 2 {
        return None;
    }
    Some(if be {
        u16::from_be_bytes([entry.data[0], entry.data[1]])
    } else {
        u16::from_le_bytes([entry.data[0], entry.data[1]])
    })
}

/// Read all u16 values from a byte slice.
fn read_u16_array(data: &[u8], be: bool) -> Vec<u16> {
    data.chunks_exact(2)
        .map(|c| {
            if be {
                u16::from_be_bytes([c[0], c[1]])
            } else {
                u16::from_le_bytes([c[0], c[1]])
            }
        })
        .collect()
}

fn read_i16_array(data: &[u8], be: bool) -> Vec<i16> {
    data.chunks_exact(2)
        .map(|c| {
            if be {
                i16::from_be_bytes([c[0], c[1]])
            } else {
                i16::from_le_bytes([c[0], c[1]])
            }
        })
        .collect()
}

/// Get first u32 from entry.
fn entry_u32(entry: &IfdEntry<'_>, be: bool) -> Option<u32> {
    if entry.data.len() < 4 {
        return None;
    }
    Some(if be {
        u32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
    } else {
        u32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
    })
}

/// Format a 4-byte version field as "X.Y.Z.W" or "XYZW".
fn format_version_bytes(data: &[u8]) -> String {
    if data.len() >= 4 {
        // Check if bytes are ASCII digits
        if data[..4].iter().all(u8::is_ascii_digit) {
            return std::str::from_utf8(&data[..4]).unwrap_or("").to_string();
        }
        format!("{}.{}.{}.{}", data[0], data[1], data[2], data[3])
    } else {
        format!("{data:?}")
    }
}

/// Convert a Unix timestamp (seconds since epoch) to "YYYY:MM:DD HH:MM:SS".
fn unix_timestamp_to_string(ts: i64) -> String {
    // Simple conversion without external crate
    const SECS_PER_DAY: i64 = 86400;

    let mut days = ts / SECS_PER_DAY;
    let time_of_day = ts % SECS_PER_DAY;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Start from 1970-01-01
    let mut year = 1970i32;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days: [i64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0u32;
    for (i, &md) in month_days.iter().enumerate() {
        if days < md {
            month = i as u32 + 1;
            break;
        }
        days -= md;
    }
    if month == 0 {
        month = 12;
    }
    let day = days + 1;

    format!("{year:04}:{month:02}:{day:02} {hours:02}:{minutes:02}:{seconds:02}")
}

fn is_leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// -- Vendor-specific value formatters ----------------------------------

fn format_canon_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    match name {
        "CanonModelID" => {
            if let Some(v) = entry_u32(entry, be) {
                canon_model_name(v).map_or_else(|| format!("{v}"), std::string::ToString::to_string)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FileNumber" => {
            if let Some(v) = entry_u32(entry, be) {
                let dir = v / 10000;
                let file = v % 10000;
                format!("{dir}-{file:04}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "SerialNumberFormat" => {
            if let Some(v) = entry_u32(entry, be) {
                match v {
                    0x90000000 => "Format 1".into(),
                    0xA0000000 => "Format 2".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "DateStampMode" => {
            if let Some(v) = entry_u16(entry, be) {
                match v {
                    0 => "Off".into(),
                    1 => "Date".into(),
                    2 => "Date & Time".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ColorTone" => {
            if let Some(v) = entry_u16(entry, be) {
                if v == 0 || v == 0x7FFF {
                    "Normal".into()
                } else {
                    format!("{}", v as i16)
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FirmwareRevision" => {
            // int32u: 0xAVVVRR00 -> "V.VV rev R.RR", A=release type
            if let Some(val) = entry_u32(entry, be) {
                let rev = format!("{val:08x}");
                let bytes: Vec<u8> = rev.bytes().collect();
                if bytes.len() == 8 {
                    let rel_ch = bytes[0] as char;
                    let prefix = match rel_ch {
                        'a' => "Alpha ",
                        'b' => "Beta ",
                        '0' => "",
                        _ => "",
                    };
                    let v1 = bytes[1] as char;
                    let v2 = std::str::from_utf8(&bytes[2..4]).unwrap_or("00");
                    // Skip optional '0' after v2
                    let rest = std::str::from_utf8(&bytes[4..]).unwrap_or("0000");
                    let rest = rest.strip_prefix('0').unwrap_or(rest);
                    if rest.len() >= 2 {
                        let r2 = &rest[rest.len() - 2..];
                        let r1 = &rest[..rest.len() - 2];
                        let r1 = if r1.is_empty() { "0" } else { r1 };
                        format!("{prefix}{v1}.{v2} rev {r1}.{r2}")
                    } else {
                        format!("{val}")
                    }
                } else {
                    format!("{val}")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PictureStyleUserDef" | "PictureStylePC" => {
            // int16u[3], each value looked up in pictureStyles
            let vals = read_u16_array(entry.data, be);
            let names: Vec<String> = vals.iter().map(|&v| canon_picture_style(v)).collect();
            names.join("; ")
        }
        "CustomPictureStyleFileName" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').to_string()
        }
        _ => format_ifd_value(entry, be),
    }
}

fn format_nikon_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    match name {
        // String tags: ExifTool applies FormatString (title-case with patches)
        "ColorMode" | "WhiteBalance" | "Sharpness" | "FocusMode" | "FlashSetting"
        | "ImageAdjustment" | "AuxiliaryLens" | "ColorHue" | "SceneMode" | "ISOSelection"
        | "LightSource" | "ImageOptimization" | "NoiseReduction" | "VariProgram"
        | "ToningEffect" | "SceneAssist" | "AFResponse" | "SerialNumber" => {
            nikon_format_string(entry).unwrap_or_else(|| format_ifd_value(entry, be))
        }
        // Quality: title-case with "Raw"->"RAW" patch (matches ExifTool FormatString)
        // Numeric lookup for old cameras (E950)
        "Quality" => {
            if let Some(s) = nikon_format_string(entry) {
                s
            } else if let Some(v) = entry_u16(entry, be) {
                match v {
                    1 => "VGA Basic".into(),
                    2 => "VGA Normal".into(),
                    3 => "VGA Fine".into(),
                    4 => "SXGA Basic".into(),
                    5 => "SXGA Normal".into(),
                    6 => "SXGA Fine".into(),
                    10 => "2 MP Basic".into(),
                    11 => "2 MP Normal".into(),
                    12 => "2 MP Fine".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        // String tags that need special case handling
        "FlashType" => {
            // "Built-in,TTL" - keep parts after comma uppercase
            if let Some(s) = entry_string(entry) {
                if let Some((prefix, suffix)) = s.split_once(',') {
                    let prefix = {
                        let mut c = prefix.chars();
                        let first = c
                            .next()
                            .map(|ch| ch.to_uppercase().to_string())
                            .unwrap_or_default();
                        let rest: String =
                            c.map(|ch| ch.to_lowercase().next().unwrap_or(ch)).collect();
                        format!("{first}{rest}")
                    };
                    format!("{prefix},{}", suffix.to_uppercase())
                } else {
                    entry_string_titlecase(entry).unwrap_or_else(|| format_ifd_value(entry, be))
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ToneComp" => {
            // Special: preserve case after period (e.g. "Mid.H" not "Mid.h")
            if let Some(s) = entry_string(entry) {
                if let Some(dot_pos) = s.find('.') {
                    let (before, after) = s.split_at(dot_pos);
                    let before = {
                        let mut c = before.chars();
                        let first = c
                            .next()
                            .map(|ch| ch.to_uppercase().to_string())
                            .unwrap_or_default();
                        let rest: String =
                            c.map(|ch| ch.to_lowercase().next().unwrap_or(ch)).collect();
                        format!("{first}{rest}")
                    };
                    let after_dot = &after[1..]; // skip the dot
                    format!("{before}.{}", after_dot.to_uppercase())
                } else {
                    entry_string_titlecase(entry).unwrap_or_else(|| format_ifd_value(entry, be))
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "MakerNoteVersion" => {
            if entry.data.len() >= 4 {
                let bytes = &entry.data[..4];
                // ASCII format: "0210" -> "2.10"
                if bytes.iter().all(u8::is_ascii_digit) {
                    if let Ok(s) = std::str::from_utf8(bytes) {
                        let s = s.trim_start_matches('0');
                        if s.len() >= 2 {
                            return format!("{}.{}", &s[..s.len() - 2], &s[s.len() - 2..]);
                        } else if !s.is_empty() {
                            return format!("{s}.00");
                        }
                    }
                }
                // Binary format: [0, 2, 0, 0] -> "2.00"
                if bytes.iter().all(|b| *b <= 9) {
                    let major = bytes.iter().position(|b| *b > 0);
                    if let Some(i) = major {
                        let ver = format!(
                            "{}.{}{}",
                            bytes[i],
                            bytes.get(i + 1).unwrap_or(&0),
                            bytes.get(i + 2).unwrap_or(&0)
                        );
                        return ver;
                    }
                }
                format_version_bytes(entry.data)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ManualFocusDistance" => {
            // 0/0 means "undef"
            if entry.data.len() >= 8 {
                let num = if be {
                    u32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                } else {
                    u32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                };
                let den = if be {
                    u32::from_be_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                } else {
                    u32::from_le_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                };
                if den == 0 {
                    return "undef".to_string();
                }
                format!("{}", num as f64 / den as f64)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "NikonCaptureVersion" => {
            // Stored as Undefined but is an ASCII string - read directly
            // Use from_utf8_lossy to preserve all non-null content (don't trim trailing spaces)
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            let s = s.trim_end_matches('\0');
            if s.is_empty() {
                format_ifd_value(entry, be)
            } else {
                s.to_string()
            }
        }
        "DataDump" | "ContrastCurve" => {
            format!(
                "(Binary data {} bytes, use -b option to extract)",
                entry.data.len()
            )
        }
        "NEFCompression" => {
            if let Some(v) = entry_u16(entry, be) {
                match v {
                    1 => "Lossy (type 1)".into(),
                    2 => "Uncompressed".into(),
                    3 => "Lossless".into(),
                    4 => "Lossy (type 2)".into(),
                    5 => "Striped Packed 12-bit".into(),
                    6 => "Uncompressed (reduced to 12 bit)".into(),
                    7 => "Unpacked 12-bit".into(),
                    8 => "Small".into(),
                    9 => "Packed 12-bit".into(),
                    10 => "Packed 14-bit".into(),
                    13 => "High Efficiency".into(),
                    14 => "High Efficiency*".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FlashMode" => {
            let v = entry_u16(entry, be).or_else(|| entry.data.first().map(|&b| b as u16));
            match v {
                Some(0) => "Did Not Fire".into(),
                Some(1) => "Fired, Manual".into(),
                Some(3) => "Not Ready".into(),
                Some(7) => "Fired, External".into(),
                Some(8) => "Fired, Commander Mode".into(),
                Some(9) => "Fired, TTL Mode".into(),
                Some(v) => format!("{v}"),
                None => format_ifd_value(entry, be),
            }
        }
        "ShootingMode" => {
            if let Some(v) = entry_u16(entry, be) {
                match v {
                    0 => "Single-Frame".into(),
                    1 => "Continuous".into(),
                    2 => "Delay".into(),
                    4 => "Remote".into(),
                    5 => "Delayed Remote".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "HighISONoiseReduction" => {
            if let Some(v) = entry_u16(entry, be) {
                match v {
                    0 => "Off".into(),
                    1 => "Minimal".into(),
                    2 => "Low".into(),
                    3 => "Medium Low".into(),
                    4 => "Normal".into(),
                    5 => "Medium High".into(),
                    6 => "High".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "VignetteControl" => {
            if let Some(v) = entry_u16(entry, be) {
                match v {
                    0 => "Off".into(),
                    1 => "Low".into(),
                    3 => "Normal".into(),
                    5 => "High".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "Saturation" => {
            // Nikon saturation can be string or int
            if let Some(s) = entry_string_titlecase(entry) {
                s
            } else {
                format_ifd_value(entry, be)
            }
        }
        // Nikon EV-encoded values: byte[0]/byte[2] with byte[1] as scale indicator
        // Format: [value, scale, steps_per_ev, 0]
        "LensFStops" => {
            if entry.data.len() >= 4 && entry.data[2] != 0 {
                let val = entry.data[0] as f64 / entry.data[2] as f64;
                format!("{val:.2}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ProgramShift" | "ExternalFlashExposureComp" | "ExposureTuning" => {
            nikon_ev_format(entry.data, false)
        }
        "FlashExposureComp" => {
            if entry.data.len() >= 4 && entry.data[2] != 0 {
                let val = entry.data[0] as i8;
                if val == 0 {
                    "0".into()
                } else {
                    let steps = entry.data[2] as i32;
                    let num = val as i32;
                    let gcd = gcd_i32(num.unsigned_abs(), steps as u32);
                    let n = num / gcd as i32;
                    let d = steps / gcd as i32;
                    if d == 1 {
                        format!("{n}")
                    } else {
                        format!("{n}/{d}")
                    }
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FlashExposureBracketValue" => {
            if entry.data.len() >= 4 && entry.data[2] != 0 {
                let val = entry.data[0] as i8 as f64 / entry.data[2] as f64;
                format!("{val:.1}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ExposureDifference" => {
            if entry.data.len() >= 4 && entry.data[2] != 0 {
                let val = entry.data[0] as i8 as f64 / entry.data[2] as f64;
                if val == 0.0 {
                    "0".into()
                } else {
                    format!("{val:.1}")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        // ISOSetting / ISO: two u16 values, second is the actual ISO
        "ISOSetting" | "ISO" => {
            if entry.data.len() >= 4 {
                let iso = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                format!("{iso}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        // LensType: bit flags (D is suppressed when G or E is set)
        "LensType" => {
            if let Some(v) = entry.data.first() {
                let v = *v;
                let mut flags = Vec::new();
                if v & 0x01 != 0 {
                    flags.push("MF");
                }
                // D flag: only show if G and E are not set
                if v & 0x02 != 0 && v & 0x04 == 0 && v & 0x80 == 0 {
                    flags.push("D");
                }
                if v & 0x04 != 0 {
                    flags.push("G");
                }
                if v & 0x08 != 0 {
                    flags.push("VR");
                }
                if v & 0x10 != 0 {
                    flags.push("1");
                }
                if v & 0x20 != 0 {
                    flags.push("FT-1");
                }
                if v & 0x80 != 0 {
                    flags.push("E");
                }
                if v == 0 {
                    "AF".into()
                } else if flags.is_empty() {
                    format!("{v}")
                } else {
                    // E goes to front, FT-1 goes to end (ExifTool ordering)
                    let mut ordered = Vec::new();
                    if flags.contains(&"E") {
                        ordered.push("E");
                    }
                    if flags.contains(&"1") {
                        ordered.push("1");
                    }
                    for &f in &flags {
                        if f != "E" && f != "1" && f != "FT-1" {
                            ordered.push(f);
                        }
                    }
                    if flags.contains(&"FT-1") {
                        ordered.push("FT-1");
                    }
                    ordered.join(" ")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        // Lens: 4 rationals -> "Xmm f/Y" or "X-Ymm f/A-B"
        "Lens" => {
            if entry.data.len() >= 32 {
                let r = |off: usize| -> f64 {
                    let num = if be {
                        u32::from_be_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    } else {
                        u32::from_le_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    };
                    let den = if be {
                        u32::from_be_bytes([
                            entry.data[off + 4],
                            entry.data[off + 5],
                            entry.data[off + 6],
                            entry.data[off + 7],
                        ])
                    } else {
                        u32::from_le_bytes([
                            entry.data[off + 4],
                            entry.data[off + 5],
                            entry.data[off + 6],
                            entry.data[off + 7],
                        ])
                    };
                    if den == 0 {
                        0.0
                    } else {
                        num as f64 / den as f64
                    }
                };
                let min_fl = r(0);
                let max_fl = r(8);
                let min_ap = r(16);
                let max_ap = r(24);
                let fmt_f = |f: f64| -> String {
                    if f == f.floor() {
                        format!("{}", f as u32)
                    } else {
                        format!("{f:.1}")
                    }
                };
                if (min_fl - max_fl).abs() < 0.1 {
                    format!("{}mm f/{}", fmt_f(min_fl), fmt_f(min_ap))
                } else if (min_ap - max_ap).abs() < 0.01 {
                    format!("{}-{}mm f/{}", fmt_f(min_fl), fmt_f(max_fl), fmt_f(min_ap))
                } else {
                    format!(
                        "{}-{}mm f/{}-{}",
                        fmt_f(min_fl),
                        fmt_f(max_fl),
                        fmt_f(min_ap),
                        fmt_f(max_ap)
                    )
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        // WB_RBLevels: rationals to decimals
        "WB_RBLevels" => {
            format_urational_decimals(entry.data, be).unwrap_or_else(|| format_ifd_value(entry, be))
        }
        "CropHiSpeed" => {
            // Array of u16 values: mode, srcW, srcH, dstW, dstH, x, y
            if entry.data.len() >= 14 {
                let g = |idx: usize| -> u16 {
                    let off = idx * 2;
                    if be {
                        u16::from_be_bytes([entry.data[off], entry.data[off + 1]])
                    } else {
                        u16::from_le_bytes([entry.data[off], entry.data[off + 1]])
                    }
                };
                let mode = g(0);
                let mode_name = match mode {
                    0 => "Off",
                    1 => "1.3x Crop",
                    6 => "DX Crop",
                    9 => "FX Uncropped",
                    11 => "DX Uncropped",
                    _ => "Unknown",
                };
                format!(
                    "{} ({}x{} cropped to {}x{} at pixel {},{})",
                    mode_name,
                    g(1),
                    g(2),
                    g(3),
                    g(4),
                    g(5),
                    g(6)
                )
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ImageAuthentication" => match entry.data.first() {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ImageStabilization" => {
            // String tag, keep case after hyphen (e.g. "VR-On" not "VR-ON")
            if let Some(s) = entry_string(entry) {
                if let Some((prefix, suffix)) = s.split_once('-') {
                    let suffix_tc = {
                        let mut c = suffix.chars();
                        let first = c
                            .next()
                            .map(|ch| ch.to_uppercase().to_string())
                            .unwrap_or_default();
                        let rest: String =
                            c.map(|ch| ch.to_lowercase().next().unwrap_or(ch)).collect();
                        format!("{first}{rest}")
                    };
                    format!("{prefix}-{suffix_tc}")
                } else {
                    entry_string_titlecase(entry).unwrap_or_else(|| format_ifd_value(entry, be))
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "DateStampMode" => {
            if let Some(v) = entry_u16(entry, be) {
                match v {
                    0 => "Off".into(),
                    1 => "Date & Time".into(),
                    2 => "Date".into(),
                    3 => "Date Counter".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "RetouchHistory" => {
            // Array of u16 values; all zeros means "None"
            let count = entry.data.len() / 2;
            if count > 0 {
                let mut all_zero = true;
                for i in 0..count {
                    let off = i * 2;
                    let v = if be {
                        u16::from_be_bytes([entry.data[off], entry.data[off + 1]])
                    } else {
                        u16::from_le_bytes([entry.data[off], entry.data[off + 1]])
                    };
                    if v != 0 {
                        all_zero = false;
                        break;
                    }
                }
                if all_zero {
                    "None".into()
                } else {
                    // Individual values map to retouch types
                    let mut parts = Vec::with_capacity(count);
                    for i in 0..count {
                        let off = i * 2;
                        let v = if be {
                            u16::from_be_bytes([entry.data[off], entry.data[off + 1]])
                        } else {
                            u16::from_le_bytes([entry.data[off], entry.data[off + 1]])
                        };
                        if v == 0 {
                            continue;
                        }
                        let name = match v {
                            3 => "B & W",
                            4 => "Sepia",
                            5 => "Trim",
                            6 => "Small Picture",
                            7 => "D-Lighting",
                            8 => "Red Eye",
                            9 => "Cyanotype",
                            10 => "Sky Light",
                            11 => "Warm Tone",
                            12 => "Color Custom",
                            13 => "Image Overlay",
                            14 => "Red Intensifier",
                            15 => "Green Intensifier",
                            16 => "Blue Intensifier",
                            17 => "Cross Screen",
                            18 => "Quick Retouch",
                            19 => "NEF Processing",
                            23 => "Distortion Control",
                            25 => "Fisheye",
                            26 => "Straighten",
                            29 => "Perspective Control",
                            30 => "Color Outline",
                            31 => "Soft Filter",
                            33 => "Miniature Effect",
                            34 => "Selective Color",
                            35 => "Painting",
                            _ => "",
                        };
                        if name.is_empty() {
                            parts.push(format!("{v}"));
                        } else {
                            parts.push(name.to_string());
                        }
                    }
                    if parts.is_empty() {
                        "None".into()
                    } else {
                        parts.join(", ")
                    }
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "SensorPixelSize" => {
            // Two rationals: width and height in micrometers
            if entry.data.len() >= 16 {
                let r = |off: usize| -> f64 {
                    let num = if be {
                        u32::from_be_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    } else {
                        u32::from_le_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    };
                    let den = if be {
                        u32::from_be_bytes([
                            entry.data[off + 4],
                            entry.data[off + 5],
                            entry.data[off + 6],
                            entry.data[off + 7],
                        ])
                    } else {
                        u32::from_le_bytes([
                            entry.data[off + 4],
                            entry.data[off + 5],
                            entry.data[off + 6],
                            entry.data[off + 7],
                        ])
                    };
                    if den == 0 {
                        0.0
                    } else {
                        num as f64 / den as f64
                    }
                };
                let w = r(0);
                let h = r(8);
                // Format with enough precision, trim trailing zeros
                let fmt = |v: f64| -> String {
                    let s = format!("{v:.6}");
                    let s = s.trim_end_matches('0');
                    let s = s.trim_end_matches('.');
                    s.to_string()
                };
                format!("{} x {} um", fmt(w), fmt(h))
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PowerUpTime" => {
            // Binary-coded decimal date: YY YY MM DD HH MM SS 00
            if entry.data.len() >= 7 {
                let y = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let mo = entry.data[2];
                let d = entry.data[3];
                let h = entry.data[4];
                let mi = entry.data[5];
                let s = entry.data[6];
                format!("{y:04}:{mo:02}:{d:02} {h:02}:{mi:02}:{s:02}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ActiveD-Lighting" => {
            if let Some(v) = entry_u16(entry, be) {
                match v {
                    0 => "Off".into(),
                    1 => "Low".into(),
                    3 => "Normal".into(),
                    5 => "High".into(),
                    7 => "Extra High".into(),
                    8 => "Extra High 1".into(),
                    9 => "Extra High 2".into(),
                    10 => "Extra High 3".into(),
                    11 => "Extra High 4".into(),
                    65535 => "Auto".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ColorSpace" => {
            if let Some(v) = entry_u16(entry, be) {
                match v {
                    1 => "sRGB".into(),
                    2 => "Adobe RGB".into(),
                    _ => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "NEFBitDepth" => {
            // int16u[4] (or sometimes [2]), PrintConv maps specific patterns
            let count = entry.data.len() / 2;
            if count >= 2 {
                let vals: Vec<u16> = (0..count)
                    .map(|i| {
                        let off = i * 2;
                        if be {
                            u16::from_be_bytes([entry.data[off], entry.data[off + 1]])
                        } else {
                            u16::from_le_bytes([entry.data[off], entry.data[off + 1]])
                        }
                    })
                    .collect();
                let v = |i: usize| -> u16 { if i < vals.len() { vals[i] } else { 0 } };
                match (v(0), v(1), v(2), v(3)) {
                    (0, 0, 0, 0) => "n/a (JPEG)".into(),
                    (12, 0, 0, 0) => "12".into(),
                    (14, 0, 0, 0) => "14".into(),
                    (8, 8, 8, 0) => "8 x 3".into(),
                    (16, 16, 16, 0) => "16 x 3".into(),
                    _ => vals
                        .iter()
                        .map(|v| format!("{v}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        _ => format_ifd_value(entry, be),
    }
}

/// Decode Fujifilm InternalSerialNumber: hex body -> ASCII, extract date.
fn fuji_format_internal_serial(s: &str) -> String {
    // Pattern: prefix + hex_body + yymmdd + 12-char suffix
    // Find the split between prefix (with spaces) and hex portion
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Find last space to split prefix from hex+date portion
    if let Some(space_pos) = trimmed.rfind(' ') {
        let prefix = &trimmed[..=space_pos];
        let rest = &trimmed[space_pos + 1..];

        // rest should be: hex_body_number (even # of hex digits) + yymmdd (6 digits) + suffix (12 chars)
        // Total suffix = 6 (date) + 12 (hex) = 18 chars
        if rest.len() >= 18 && rest.chars().all(|c| c.is_ascii_hexdigit()) {
            let suffix_start = rest.len() - 18;
            let hex_body = &rest[..suffix_start];
            let yy: u32 = rest[suffix_start..suffix_start + 2].parse().unwrap_or(0);
            let mm = &rest[suffix_start + 2..suffix_start + 4];
            let dd = &rest[suffix_start + 4..suffix_start + 6];
            let hex_suffix = &rest[suffix_start + 6..];

            // Validate month/day
            let mm_val: u32 = mm.parse().unwrap_or(0);
            let dd_val: u32 = dd.parse().unwrap_or(0);
            if (1..=12).contains(&mm_val) && (1..=31).contains(&dd_val) {
                // Convert hex body to ASCII
                let body_ascii = hex_to_ascii(hex_body);
                let yr = if yy < 70 { 2000 + yy } else { 1900 + yy };
                return format!("{prefix}{body_ascii} {yr}:{mm}:{dd} {hex_suffix}");
            }
        }
    }
    s.to_string()
}

/// Convert hex string to ASCII bytes.
fn hex_to_ascii(hex: &str) -> String {
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks(2)
        .filter_map(|pair| {
            if pair.len() == 2 {
                u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()
            } else {
                None
            }
        })
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn format_fuji_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    // Most Fuji tags are u16 enums
    let v = entry_u16(entry, be);
    match name {
        "Version" => {
            if entry.data.len() >= 4 {
                format_version_bytes(entry.data)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "Quality" => entry_string(entry).unwrap_or_else(|| format_ifd_value(entry, be)),
        "Sharpness" => match v {
            Some(1) | Some(2) => format!("{} (soft)", v.unwrap() as i16 - 3),
            Some(3) => "0 (normal)".into(),
            Some(4) | Some(5) => format!("{} (hard)", v.unwrap() as i16 - 3),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "WhiteBalance" => match v {
            Some(0) => "Auto".into(),
            Some(256) => "Daylight".into(),
            Some(512) => "Cloudy".into(),
            Some(768) => "Daylight Fluorescent".into(),
            Some(769) => "Day White Fluorescent".into(),
            Some(770) => "White Fluorescent".into(),
            Some(1024) => "Incandescent".into(),
            Some(3840) => "Custom".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "Saturation" => match v {
            Some(0) => "0 (normal)".into(),
            Some(256) => "+1 (medium high)".into(),
            Some(512) => "+2 (high)".into(),
            Some(768) => "0 (normal, 768)".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "Contrast" | "ColorTemperature" => format_ifd_value(entry, be),
        "FujiFlashMode" => match v {
            Some(0) => "Auto".into(),
            Some(1) => "On".into(),
            Some(2) => "Off".into(),
            Some(3) => "Red-eye reduction".into(),
            Some(4) => "Slow Sync".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "Macro" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FocusMode" => match v {
            Some(0) => "Auto".into(),
            Some(1) => "Manual".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "AFMode" => match v {
            Some(0) => "No".into(),
            Some(1) => "Single Point".into(),
            Some(256) => "Zone".into(),
            Some(512) => "Wide/Tracking".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "SlowSync" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "PictureMode" => match v {
            Some(0) => "Auto".into(),
            Some(1) => "Portrait".into(),
            Some(2) => "Landscape".into(),
            Some(3) => "Macro".into(),
            Some(4) => "Sports".into(),
            Some(5) => "Night Scene".into(),
            Some(6) => "Program AE".into(),
            Some(7) => "Natural Light".into(),
            Some(8) => "Anti-blur".into(),
            Some(256) => "Aperture-priority AE".into(),
            Some(512) => "Shutter speed priority AE".into(),
            Some(768) => "Manual".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "AutoBracketing" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "SequenceNumber" => format_ifd_value(entry, be),
        "ColorMode" => match v {
            Some(0) => "Standard".into(),
            Some(16) => "Chrome".into(),
            Some(48) => "B&W".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "BlurWarning" => match v {
            Some(0) => "None".into(),
            Some(1) => "Blur Warning".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FocusWarning" => match v {
            Some(0) => "Good".into(),
            Some(1) => "Out of focus".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ExposureWarning" => match v {
            Some(0) => "Good".into(),
            Some(1) => "Bad exposure".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "DynamicRange" => match v {
            Some(1) => "Standard".into(),
            Some(3) => "Wide".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FilmMode" => match v {
            Some(0) => "F0/Standard (Provia)".into(),
            Some(256) => "F1/Studio Portrait".into(),
            Some(272) => "F1a/Studio Portrait Enhanced Saturation".into(),
            Some(288) => "F1b/Studio Portrait Smooth Skin Tone".into(),
            Some(304) => "F1c/Studio Portrait Increased Sharpness".into(),
            Some(512) => "F2/Fujichrome (Velvia)".into(),
            Some(768) => "F3/Studio Portrait Ex".into(),
            Some(1024) => "F4/Velvia".into(),
            Some(1280) => "Pro Neg. Std".into(),
            Some(1281) => "Pro Neg. Hi".into(),
            Some(1536) => "Classic Chrome".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "DynamicRangeSetting" => match v {
            Some(0) => "Auto".into(),
            Some(1) => "Manual".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "InternalSerialNumber" => {
            // Raw string: "FPX20582698 592D313134360702198C0020100A84"
            // ExifTool decodes: prefix, hex->ASCII body, YYYY:MM:DD, hex suffix
            let s = std::str::from_utf8(entry.data)
                .unwrap_or("")
                .trim_end_matches('\0');
            fuji_format_internal_serial(s)
        }
        "WhiteBalanceFineTune" => {
            // Two signed 32-bit values: Red adjustment, Blue adjustment
            if entry.data.len() >= 8 {
                let r = if be {
                    i32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                } else {
                    i32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                };
                let b = if be {
                    i32::from_be_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                } else {
                    i32::from_le_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                };
                let sign_r = if r >= 0 { "+" } else { "" };
                let sign_b = if b >= 0 { "+" } else { "" };
                format!("Red {sign_r}{r}, Blue {sign_b}{b}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        _ => format_ifd_value(entry, be),
    }
}

fn decode_panasonic_faces(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // FaceDetInfo: count followed by up to five (center X/Y, width/height)
    // int16u records. Ignore incomplete records and undocumented trailing data.
    if data.len() < 2 {
        return;
    }
    let words = read_u16_array(&data[..data.len().min(42)], be);
    let count = usize::from(words[0]).min(5).min((words.len() - 1) / 4);
    tags.push(DecodedTag {
        name: "NumFacePositions".into(),
        value: count.to_string(),
    });
    for index in 0..count {
        let offset = 1 + index * 4;
        let values = words[offset..offset + 4]
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        tags.push(DecodedTag {
            name: format!("Face{}Position", index + 1),
            value: values,
        });
    }
}

fn format_panasonic_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    let v = entry_u16(entry, be);
    match name {
        "AFPointPosition" => {
            if entry.data_type != crate::tiff::DataType::Rational || entry.data.len() != 16 {
                return "n/a".into();
            }
            let read = |bytes: &[u8]| {
                let bytes = bytes.try_into().unwrap();
                if be {
                    u32::from_be_bytes(bytes)
                } else {
                    u32::from_le_bytes(bytes)
                }
            };
            let mut values = Vec::new();
            for pair in entry.data.chunks_exact(8) {
                let numerator = read(&pair[..4]);
                let denominator = read(&pair[4..]);
                if denominator == 0 || numerator > denominator {
                    return "n/a".into();
                }
                values.push((f64::from(numerator) / f64::from(denominator)).to_string());
            }
            values.join(" ")
        }
        "FirmwareVersion" => {
            if entry.data.len() >= 4 {
                format!(
                    "{}.{}.{}.{}",
                    entry.data[0], entry.data[1], entry.data[2], entry.data[3]
                )
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PanasonicExifVersion" => {
            if entry.data.len() >= 4 {
                format_version_bytes(entry.data)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "InternalSerialNumber" => {
            // Match ExifTool: try pattern from raw data start (no null stripping)
            // Pattern: ^[A-Z][0-9A-Z]{2}\d{8,}
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            let s = s.trim_end_matches('\0');
            if !s.is_empty()
                && s.len() >= 13
                && s.as_bytes()[0].is_ascii_uppercase()
                && s.as_bytes()[1..3]
                    .iter()
                    .all(|&b| b.is_ascii_alphanumeric())
                && s[3..].bytes().all(|b| b.is_ascii_digit())
            {
                // Format: "(XNN) 20YY:MM:DD no. RRRR"
                let prefix = &s[..3];
                let yy = &s[3..5];
                let mm = &s[5..7];
                let dd = &s[7..9];
                let num = &s[9..];
                format!("({prefix}) 20{yy}:{mm}:{dd} no. {num}")
            } else {
                // Return trimmed string (strip null bytes from both ends)
                let start = entry
                    .data
                    .iter()
                    .position(|&b| b != 0)
                    .unwrap_or(entry.data.len());
                let s2 = std::str::from_utf8(&entry.data[start..]).unwrap_or("");
                let s2 = s2.trim_end_matches('\0');
                if s2.is_empty() {
                    format_ifd_value(entry, be)
                } else {
                    s2.to_string()
                }
            }
        }
        "ImageQuality" => match v {
            Some(2) => "High".into(),
            Some(3) => "Standard".into(),
            Some(6) => "Very High".into(),
            Some(7) => "RAW".into(),
            Some(9) => "Motion Picture".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "WhiteBalance" => match v {
            Some(1) => "Auto".into(),
            Some(2) => "Daylight".into(),
            Some(3) => "Cloudy".into(),
            Some(4) => "Incandescent".into(),
            Some(5) => "Manual".into(),
            Some(8) => "Flash".into(),
            Some(10) => "Black & White".into(),
            Some(11) => "Manual 2".into(),
            Some(12) => "Shade".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FocusMode" => match v {
            Some(1) => "Auto".into(),
            Some(2) => "Manual".into(),
            Some(4) => "Auto, Focus button".into(),
            Some(5) => "Auto, Continuous".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "AFAreaMode" => {
            // Multi-byte field
            if entry.data.len() >= 2 {
                let mode = entry.data[0];
                match mode {
                    0 => "9-area".into(),
                    1 => "3-area (high speed)".into(),
                    2 => "1-area".into(),
                    3 => "1-area (high speed)".into(),
                    4 => "Auto or Face detect".into(),
                    16 => "1-area".into(),
                    _ => format!("{mode}-area"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ImageStabilization" => match v {
            Some(2) => "On, Optical".into(),
            Some(3) => "Off".into(),
            Some(4) => "On, Mode 2".into(),
            Some(5) => "Panning".into(),
            Some(6) => "On, Mode 3".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "MacroMode" => match v {
            Some(1) => "On".into(),
            Some(2) => "Off".into(),
            Some(257) => "Tele-Macro".into(),
            Some(513) => "Macro Zoom".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ShootingMode" => match v {
            Some(1) => "Normal".into(),
            Some(2) => "Portrait".into(),
            Some(3) => "Scenery".into(),
            Some(4) => "Sports".into(),
            Some(5) => "Night Portrait".into(),
            Some(6) => "Program".into(),
            Some(7) => "Aperture Priority".into(),
            Some(8) => "Shutter Priority".into(),
            Some(9) => "Macro".into(),
            Some(10) => "Spot".into(),
            Some(11) => "Manual".into(),
            Some(13) => "Panning".into(),
            Some(14) => "Simple".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "Audio" => match v {
            Some(1) => "Yes".into(),
            Some(2) => "No".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ColorEffect" => match v {
            Some(1) => "Off".into(),
            Some(2) => "Warm".into(),
            Some(3) => "Cool".into(),
            Some(4) => "Black & White".into(),
            Some(5) => "Sepia".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "BurstMode" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(2) => "Infinity".into(),
            Some(4) => "Unlimited".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ContrastMode" => match v {
            Some(0) => "Normal".into(),
            Some(1) => "Low".into(),
            Some(2) => "High".into(),
            Some(6) => "Medium Low".into(),
            Some(7) => "Medium High".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "NoiseReduction" => match v {
            Some(0) => "Standard".into(),
            Some(1) => "Low (-1)".into(),
            Some(2) => "High (+1)".into(),
            Some(3) => "Lowest (-2)".into(),
            Some(4) => "Highest (+2)".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "SelfTimer" => match v {
            Some(1) => "Off".into(),
            Some(2) => "10 s".into(),
            Some(3) => "2 s".into(),
            Some(4) => "10 s / 3 shots".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "Rotation" => match v {
            Some(1) => "Horizontal (normal)".into(),
            Some(3) => "Rotate 180".into(),
            Some(6) => "Rotate 90 CW".into(),
            Some(8) => "Rotate 270 CW".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ColorMode" => match v {
            Some(0) => "Normal".into(),
            Some(1) => "Natural".into(),
            Some(2) => "Vivid".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "OpticalZoomMode" => match v {
            Some(1) => "Standard".into(),
            Some(2) => "Extended".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ConversionLens" => match v {
            Some(1) => "Off".into(),
            Some(2) => "Wide".into(),
            Some(3) => "Telephoto".into(),
            Some(4) => "Macro".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "TimeSincePowerOn" => {
            // Value is in 1/100s seconds
            if let Some(v) = entry_u32(entry, be) {
                let total_cs = v;
                let hours = total_cs / 360000;
                let mins = (total_cs % 360000) / 6000;
                let secs = (total_cs % 6000) / 100;
                let cs = total_cs % 100;
                format!("{hours:02}:{mins:02}:{secs:02}.{cs:02}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "AFAssistLamp" => match v {
            Some(1) => "Fired".into(),
            Some(2) => "Enabled but Not Used".into(),
            Some(3) => "Disabled but Required".into(),
            Some(4) => "Disabled and Not Required".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FlashFired" => match v {
            Some(1) => "No".into(),
            Some(2) => "Yes".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "LensType" => {
            if let Some(s) = entry_string(entry) {
                let s = s.trim_end_matches('\0');
                if s.is_empty() {
                    format_ifd_value(entry, be)
                } else {
                    s.to_string()
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "IntelligentExposure" => match v {
            Some(0) => "Off".into(),
            Some(1) => "Low".into(),
            Some(2) => "Standard".into(),
            Some(3) => "High".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "IntelligentResolution" => match v {
            Some(0) => "Off".into(),
            Some(1) => "Low".into(),
            Some(2) => "Standard".into(),
            Some(3) => "High".into(),
            Some(4) => "Extended".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "IntelligentDRange" => match v {
            Some(0) => "Off".into(),
            Some(1) => "Low".into(),
            Some(2) => "Standard".into(),
            Some(3) => "High".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ClearRetouch" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "PhotoStyle" => match v {
            Some(0) => "Standard".into(),
            Some(1) => "Vivid".into(),
            Some(2) => "Natural".into(),
            Some(3) => "Smooth".into(),
            Some(4) => "Portrait".into(),
            Some(5) => "Scenery".into(),
            Some(6) => "Impressive Art".into(),
            Some(7) => "Cross Process".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ShadingCompensation" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ShutterType" => match v {
            Some(0) => "Mechanical".into(),
            Some(1) => "Electronic".into(),
            Some(2) => "Hybrid".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "HDR" => match v {
            Some(0) => "Off".into(),
            Some(100) => "1 EV".into(),
            Some(200) => "2 EV".into(),
            Some(300) => "3 EV".into(),
            Some(32868) => "1 EV (Auto)".into(),
            Some(32968) => "2 EV (Auto)".into(),
            Some(33068) => "3 EV (Auto)".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "TouchAE" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "SceneMode" => match v {
            Some(1) => "Normal".into(),
            Some(2) => "Portrait".into(),
            Some(3) => "Scenery".into(),
            Some(4) => "Sports".into(),
            Some(5) => "Night Portrait".into(),
            Some(6) => "Program".into(),
            Some(7) => "Aperture Priority".into(),
            Some(8) => "Shutter Priority".into(),
            Some(9) => "Macro".into(),
            Some(11) => "Manual".into(),
            Some(13) => "Panning".into(),
            Some(14) => "Simple".into(),
            Some(15) => "Color Effects".into(),
            Some(18) => "Fireworks".into(),
            Some(20) => "Scenery (HDR)".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "MakerNoteVersion" => {
            if entry.data.len() >= 4 {
                format_version_bytes(entry.data)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "VideoFrameRate" => match v {
            Some(0) => "n/a".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        _ => format_ifd_value(entry, be),
    }
}

/// Format a sequence of SRATIONAL values as space-separated decimals.
fn format_rational_decimals(data: &[u8], be: bool) -> Option<String> {
    if data.len() < 8 || data.len() % 8 != 0 {
        return None;
    }
    let count = data.len() / 8;
    let mut parts = Vec::with_capacity(count);
    for i in 0..count {
        let off = i * 8;
        let num = if be {
            i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        };
        let den = if be {
            i32::from_be_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
        } else {
            i32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
        };
        if den == 0 {
            parts.push("0".to_string());
        } else {
            let val = num as f64 / den as f64;
            // Match ExifTool precision (~10 significant digits)
            let s = format_sig_digits(val, 10);
            parts.push(s);
        }
    }
    Some(parts.join(" "))
}

fn format_apple_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    // Apple tags use SLONG (4-byte int) - use u32 not u16
    let v = entry_u32(entry, be);
    match name {
        "MakerNoteVersion" => format_ifd_value(entry, be),
        "AEStable" | "AFStable" => match v {
            Some(0) => "No".into(),
            Some(1) => "Yes".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "HDRImageType" => match v {
            Some(3) => "HDR Image".into(),
            Some(4) => "Original Image".into(),
            Some(v) => format!("Unknown ({v})"),
            None => format_ifd_value(entry, be),
        },
        "ImageCaptureType" => match v {
            Some(1) => "ProRAW".into(),
            Some(2) => "Portrait".into(),
            Some(10) => "Photo".into(),
            Some(11) => "Manual Focus".into(),
            Some(12) => "Scene".into(),
            Some(v) => format!("Unknown ({v})"),
            None => format_ifd_value(entry, be),
        },
        "CameraType" => match v {
            Some(0) => "Back Wide".into(),
            Some(1) => "Back Normal".into(),
            Some(2) => "Back Telephoto".into(),
            Some(3) => "Front Normal".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "AccelerationVector" | "HDRHeadroom" | "SignalToNoiseRatio" | "HDRGain" => {
            format_rational_decimals(entry.data, be).unwrap_or_else(|| format_ifd_value(entry, be))
        }
        // bplist tags: decode Apple binary plist
        "SemanticStylePreset" | "SemanticStyleRenderingVer" | "SemanticStyle" => {
            decode_bplist(entry.data).unwrap_or_else(|| format_ifd_value(entry, be))
        }
        "AFPerformance" => {
            // int32s[2]: display as "val[0] val[1]>>28 val[1]&0xfffffff"
            if entry.data.len() >= 8 {
                let v0 = if be {
                    i32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                } else {
                    i32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                };
                let v1 = if be {
                    i32::from_be_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                } else {
                    i32::from_le_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                };
                format!("{} {} {}", v0, (v1 as u32) >> 28, v1 & 0x0FFFFFFF)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FocusDistanceRange" => {
            // Two RATIONAL values: near and far distance in meters
            if entry.data.len() >= 16 {
                let rat = |off: usize| -> f64 {
                    let num = if be {
                        u32::from_be_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    } else {
                        u32::from_le_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    };
                    let den = if be {
                        u32::from_be_bytes([
                            entry.data[off + 4],
                            entry.data[off + 5],
                            entry.data[off + 6],
                            entry.data[off + 7],
                        ])
                    } else {
                        u32::from_le_bytes([
                            entry.data[off + 4],
                            entry.data[off + 5],
                            entry.data[off + 6],
                            entry.data[off + 7],
                        ])
                    };
                    if den == 0 {
                        0.0
                    } else {
                        num as f64 / den as f64
                    }
                };
                let near = rat(0);
                let far = rat(8);
                // ExifTool shows "near - far m" with the smaller first
                let (lo, hi) = if near < far { (near, far) } else { (far, near) };
                format!("{lo:.2} - {hi:.2} m")
            } else {
                format_ifd_value(entry, be)
            }
        }
        _ => format_ifd_value(entry, be),
    }
}

fn format_olympus_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    let v = entry_u16(entry, be);
    match name {
        "Macro" | "BWMode" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(2) => "Super Macro".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FocusMode" => match v {
            Some(0) => "Auto".into(),
            Some(1) => "Manual".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "Quality" => {
            // Can be u16 or u32; use u32 to handle both
            let qv = entry_u32(entry, be).or_else(|| entry_u16(entry, be).map(|v| v as u32));
            match qv {
                Some(1) => "SQ (Low)".into(),
                Some(2) => "HQ (Normal)".into(),
                Some(3) => "SHQ (Fine)".into(),
                Some(4) => "RAW".into(),
                Some(5) => "Medium-Fine".into(),
                Some(6) => "Small-Fine".into(),
                Some(0x0101) => "SQ (Low)".into(),
                Some(0x0201) => "SQ (Normal)".into(),
                Some(0x0301) => "SQ (Fine)".into(),
                Some(0x0102) => "HQ (Low)".into(),
                Some(0x0202) => "HQ (Normal)".into(),
                Some(0x0302) => "HQ (Fine)".into(),
                Some(0x0103) => "SHQ (Low)".into(),
                Some(0x0203) => "SHQ (Normal)".into(),
                Some(0x0303) => "SHQ (Fine)".into(),
                Some(0x0504) => "SQ (Low)".into(),
                Some(v) => format!("{v}"),
                None => format_ifd_value(entry, be),
            }
        }
        "FocalPlaneDiagonal" => {
            if entry.data.len() >= 8 {
                let (num, den) = if be {
                    (
                        u32::from_be_bytes([
                            entry.data[0],
                            entry.data[1],
                            entry.data[2],
                            entry.data[3],
                        ]),
                        u32::from_be_bytes([
                            entry.data[4],
                            entry.data[5],
                            entry.data[6],
                            entry.data[7],
                        ]),
                    )
                } else {
                    (
                        u32::from_le_bytes([
                            entry.data[0],
                            entry.data[1],
                            entry.data[2],
                            entry.data[3],
                        ]),
                        u32::from_le_bytes([
                            entry.data[4],
                            entry.data[5],
                            entry.data[6],
                            entry.data[7],
                        ]),
                    )
                };
                if den > 0 {
                    let mm = num as f64 / den as f64;
                    // Trim trailing zeros after decimal point
                    let s = format!("{mm:.10}");
                    let s = s.trim_end_matches('0');
                    let s = s.strip_suffix('.').unwrap_or(s);
                    format!("{s} mm")
                } else {
                    format_ifd_value(entry, be)
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "CameraID" => {
            if let Some(s) = entry_string(entry) {
                s
            } else {
                let s = String::from_utf8_lossy(entry.data);
                let s = s.trim_end_matches('\0').trim();
                if s.is_empty() {
                    format_ifd_value(entry, be)
                } else {
                    s.to_string()
                }
            }
        }
        "SpecialMode" => {
            if entry.data.len() >= 12 {
                let get_u32 = |off: usize| -> u32 {
                    if be {
                        u32::from_be_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    } else {
                        u32::from_le_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    }
                };
                let mode_val = get_u32(0);
                let mode = match mode_val {
                    0 => "Normal".to_string(),
                    1 => "Unknown".to_string(),
                    2 => "Fast".to_string(),
                    3 => "Panorama".to_string(),
                    v => format!("Unknown ({v})"),
                };
                let seq = get_u32(4);
                let dir_val = get_u32(8);
                let dir = match dir_val {
                    0 => "(none)".to_string(),
                    1 => "Left to Right".to_string(),
                    2 => "Right to Left".to_string(),
                    3 => "Bottom to Top".to_string(),
                    4 => "Top to Bottom".to_string(),
                    v => format!("Unknown ({v})"),
                };
                format!("{mode}, Sequence: {seq}, Panorama: {dir}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FocusRange" => match v {
            Some(0) => "Normal".into(),
            Some(1) => "Macro".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "Sharpness" => match v {
            Some(0) => "Normal".into(),
            Some(1) => "Hard".into(),
            Some(2) => "Soft".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "Contrast" => match v {
            Some(0) => "High".into(),
            Some(1) => "Normal".into(),
            Some(2) => "Low".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ExternalFlashBounce" => match v {
            Some(0) => "No".into(),
            Some(1) => "Yes".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "WBMode" => {
            // Count == 2: format as "v0 v1" and look up
            if entry.data.len() >= 4 {
                let v0 = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let v1 = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                let key = format!("{v0} {v1}");
                match key.as_str() {
                    "1 0" => "Auto".into(),
                    "1 2" => "Auto (2)".into(),
                    "1 4" => "Auto (4)".into(),
                    "2 2" => "3000 Kelvin".into(),
                    "2 3" => "3700 Kelvin".into(),
                    "2 4" => "4000 Kelvin".into(),
                    "2 5" => "4500 Kelvin".into(),
                    "2 6" => "5500 Kelvin".into(),
                    "2 7" => "6500 Kelvin".into(),
                    "2 8" => "7500 Kelvin".into(),
                    "3 0" => "One-touch".into(),
                    _ => {
                        // Try just first value
                        if v0 == 1 { "Auto".into() } else { key }
                    }
                }
            } else {
                match v {
                    Some(1) => "Auto".into(),
                    _ => format_ifd_value(entry, be),
                }
            }
        }
        "RedBalance" | "BlueBalance" => {
            // ValueConv: first int16u value / 256
            if entry.data.len() >= 4 {
                let v0 = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let bal = v0 as f64 / 256.0;
                format_sig_digits(bal, 10)
            } else {
                format_ifd_value(entry, be)
            }
        }
        // ManualFocusDistance, FlashExposureComp, ExternalFlashGValue, LightValueCenter/Periphery: rational
        "ManualFocusDistance" => {
            if let Some(raw) = olympus_read_rational(entry, be) {
                let s = format_sig_digits(raw, 10);
                format!("{s} mm")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FlashExposureComp"
        | "ExternalFlashGValue"
        | "LightValueCenter"
        | "LightValuePeriphery" => {
            if let Some(raw) = olympus_read_rational(entry, be) {
                format_sig_digits(raw, 10)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "SerialNumber" => {
            // String type, but may be stored as undef
            if let Some(s) = entry_string(entry) {
                s
            } else {
                let s = String::from_utf8_lossy(entry.data);
                let s = s.trim_end_matches('\0').trim();
                if s.is_empty() {
                    format_ifd_value(entry, be)
                } else {
                    s.to_string()
                }
            }
        }
        "ColorMatrix" => {
            // int16s[N] - signed 16-bit values
            let count = entry.data.len() / 2;
            let mut vals = Vec::with_capacity(count);
            for i in 0..count {
                let off = i * 2;
                if off + 2 <= entry.data.len() {
                    let v = if be {
                        i16::from_be_bytes([entry.data[off], entry.data[off + 1]])
                    } else {
                        i16::from_le_bytes([entry.data[off], entry.data[off + 1]])
                    };
                    vals.push(format!("{v}"));
                }
            }
            vals.join(" ")
        }
        "SceneMode" => match v {
            Some(0) => "Normal".into(),
            Some(1) => "Standard".into(),
            Some(2) => "Auto".into(),
            Some(3) => "Intelligent Auto".into(),
            Some(4) => "Portrait".into(),
            Some(5) => "Landscape+Portrait".into(),
            Some(6) => "Landscape".into(),
            Some(7) => "Night Scene".into(),
            Some(8) => "Night+Portrait".into(),
            Some(9) => "Sport".into(),
            Some(10) => "Self Portrait".into(),
            Some(11) => "Indoor".into(),
            Some(12) => "Beach & Snow".into(),
            Some(13) => "Beach".into(),
            Some(14) => "Snow".into(),
            Some(15) => "Self Portrait+Self Timer".into(),
            Some(16) => "Sunset".into(),
            Some(17) => "Cuisine".into(),
            Some(18) => "Documents".into(),
            Some(19) => "Candle".into(),
            Some(20) => "Fireworks".into(),
            Some(21) => "Available Light".into(),
            Some(22) => "Vivid".into(),
            Some(23) => "Underwater Wide1".into(),
            Some(24) => "Underwater Macro".into(),
            Some(25) => "Museum".into(),
            Some(26) => "Behind Glass".into(),
            Some(27) => "Auction".into(),
            Some(28) => "Shoot & Select1".into(),
            Some(29) => "Shoot & Select2".into(),
            Some(30) => "Underwater Wide2".into(),
            Some(31) => "Digital Image Stabilization".into(),
            Some(32) => "Face Portrait".into(),
            Some(33) => "Pet".into(),
            Some(34) => "Smile Shot".into(),
            Some(35) => "Quick Shutter".into(),
            Some(43) => "Hand-held Starlight".into(),
            Some(100) => "Panorama".into(),
            Some(101) => "Magic Filter".into(),
            Some(103) => "HDR".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        // Rational tags with APEX conversions
        "DigitalZoom"
        | "ShutterSpeedValue"
        | "ISOValue"
        | "ApertureValue"
        | "BrightnessValue"
        | "ExposureCompensation" => {
            if let Some(raw) = olympus_read_rational(entry, be) {
                match name {
                    "ShutterSpeedValue" => {
                        // APEX: exposure time = 2^(-val), then PrintExposureTime
                        let t = if raw.abs() < 100.0 {
                            2.0_f64.powf(-raw)
                        } else {
                            0.0
                        };
                        if t > 0.0 && t < 0.25001 {
                            let recip = (0.5 + 1.0 / t) as u32;
                            format!("1/{recip}")
                        } else {
                            let s = format!("{t:.1}");
                            s.trim_end_matches(".0").to_string()
                        }
                    }
                    "ISOValue" => {
                        // APEX: ISO = 100 * 2^(val - 5)
                        let iso = 100.0 * 2.0_f64.powf(raw - 5.0);
                        // Round to 2 decimal places
                        let rounded = (iso * 100.0 + 0.5).floor() / 100.0;
                        format_sig_digits(rounded, 10)
                    }
                    "ApertureValue" => {
                        // APEX: f-number = 2^(val/2)
                        let fnum = 2.0_f64.powf(raw / 2.0);
                        format!("{fnum:.1}")
                    }
                    _ => {
                        // DigitalZoom, BrightnessValue, ExposureCompensation: show decimal
                        format_sig_digits(raw, 10)
                    }
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        // FlashMode: show "Unknown (N)" for unknown values
        "FlashMode" => match v {
            Some(0) => "Unknown (0)".into(),
            Some(1) => "On".into(),
            Some(2) => "Off".into(),
            Some(3) => "Red-eye Reduction".into(),
            Some(v) => format!("Unknown ({v})"),
            None => format_ifd_value(entry, be),
        },
        // FlashDevice: show "Unknown (N N)" for unknown values
        "FlashDevice" => {
            if entry.data.len() >= 4 {
                let v0 = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let v1 = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                format!("Unknown ({v0} {v1})")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "CameraType" => {
            let raw = std::str::from_utf8(entry.data)
                .unwrap_or("")
                .trim_end_matches('\0')
                .trim();
            if raw.is_empty() {
                // Try lossy conversion for non-UTF8 data
                let lossy = String::from_utf8_lossy(entry.data);
                let lossy = lossy.trim_end_matches('\0').trim();
                if lossy.is_empty() {
                    return format_ifd_value(entry, be);
                }
                format!("Unknown ({lossy})")
            } else {
                olympus_camera_type(raw).unwrap_or_else(|| {
                    if raw.bytes().all(|b| b.is_ascii_alphanumeric()) {
                        raw.to_string()
                    } else {
                        format!("Unknown ({raw})")
                    }
                })
            }
        }
        _ => format_ifd_value(entry, be),
    }
}

/// Look up Olympus lens type from hex key -> lens name.
/// Key format: "{make:x} {model:02x} {submodel:02x}" (e.g. "0 05 00").
/// Based on publicly documented Olympus/Panasonic/Sigma/Tamron lens IDs.
fn olympus_lens_type(key: &str) -> Option<String> {
    static TABLE: &[(&str, &str)] = &[
        ("0 00 00", "None"),
        ("0 01 00", "Olympus Zuiko Digital ED 50mm F2.0 Macro"),
        ("0 01 01", "Olympus Zuiko Digital 40-150mm F3.5-4.5"),
        ("0 01 10", "Olympus M.Zuiko Digital ED 14-42mm F3.5-5.6"),
        ("0 02 00", "Olympus Zuiko Digital ED 150mm F2.0"),
        ("0 02 10", "Olympus M.Zuiko Digital 17mm F2.8 Pancake"),
        ("0 03 00", "Olympus Zuiko Digital ED 300mm F2.8"),
        (
            "0 03 10",
            "Olympus M.Zuiko Digital ED 14-150mm F4.0-5.6 [II]",
        ),
        ("0 04 10", "Olympus M.Zuiko Digital ED 9-18mm F4.0-5.6"),
        ("0 05 00", "Olympus Zuiko Digital 14-54mm F2.8-3.5"),
        ("0 05 01", "Olympus Zuiko Digital Pro ED 90-250mm F2.8"),
        ("0 05 10", "Olympus M.Zuiko Digital ED 14-42mm F3.5-5.6 L"),
        ("0 06 00", "Olympus Zuiko Digital ED 50-200mm F2.8-3.5"),
        ("0 06 01", "Olympus Zuiko Digital ED 8mm F3.5 Fisheye"),
        ("0 06 10", "Olympus M.Zuiko Digital ED 40-150mm F4.0-5.6"),
        ("0 07 00", "Olympus Zuiko Digital 11-22mm F2.8-3.5"),
        ("0 07 01", "Olympus Zuiko Digital 18-180mm F3.5-6.3"),
        ("0 07 10", "Olympus M.Zuiko Digital ED 12mm F2.0"),
        ("0 08 01", "Olympus Zuiko Digital 70-300mm F4.0-5.6"),
        ("0 08 10", "Olympus M.Zuiko Digital ED 75-300mm F4.8-6.7"),
        ("0 09 10", "Olympus M.Zuiko Digital 14-42mm F3.5-5.6 II"),
        ("0 10 01", "Kenko Tokina Reflex 300mm F6.3 MF Macro"),
        ("0 10 10", "Olympus M.Zuiko Digital ED 12-50mm F3.5-6.3 EZ"),
        ("0 11 10", "Olympus M.Zuiko Digital 45mm F1.8"),
        ("0 12 10", "Olympus M.Zuiko Digital ED 60mm F2.8 Macro"),
        ("0 13 10", "Olympus M.Zuiko Digital 14-42mm F3.5-5.6 II R"),
        ("0 14 10", "Olympus M.Zuiko Digital ED 40-150mm F4.0-5.6 R"),
        ("0 15 00", "Olympus Zuiko Digital ED 7-14mm F4.0"),
        ("0 15 10", "Olympus M.Zuiko Digital ED 75mm F1.8"),
        ("0 16 10", "Olympus M.Zuiko Digital 17mm F1.8"),
        ("0 17 00", "Olympus Zuiko Digital Pro ED 35-100mm F2.0"),
        ("0 18 00", "Olympus Zuiko Digital 14-45mm F3.5-5.6"),
        ("0 18 10", "Olympus M.Zuiko Digital ED 75-300mm F4.8-6.7 II"),
        ("0 19 10", "Olympus M.Zuiko Digital ED 12-40mm F2.8 Pro"),
        ("0 20 00", "Olympus Zuiko Digital 35mm F3.5 Macro"),
        ("0 20 10", "Olympus M.Zuiko Digital ED 40-150mm F2.8 Pro"),
        ("0 21 10", "Olympus M.Zuiko Digital ED 14-42mm F3.5-5.6 EZ"),
        ("0 22 00", "Olympus Zuiko Digital 17.5-45mm F3.5-5.6"),
        ("0 22 10", "Olympus M.Zuiko Digital 25mm F1.8"),
        ("0 23 00", "Olympus Zuiko Digital ED 14-42mm F3.5-5.6"),
        ("0 23 10", "Olympus M.Zuiko Digital ED 7-14mm F2.8 Pro"),
        ("0 24 00", "Olympus Zuiko Digital ED 40-150mm F4.0-5.6"),
        ("0 24 10", "Olympus M.Zuiko Digital ED 300mm F4.0 IS Pro"),
        ("0 25 10", "Olympus M.Zuiko Digital ED 8mm F1.8 Fisheye Pro"),
        ("0 26 10", "Olympus M.Zuiko Digital ED 12-100mm F4.0 IS Pro"),
        ("0 27 10", "Olympus M.Zuiko Digital ED 30mm F3.5 Macro"),
        ("0 28 10", "Olympus M.Zuiko Digital ED 25mm F1.2 Pro"),
        ("0 29 10", "Olympus M.Zuiko Digital ED 17mm F1.2 Pro"),
        ("0 30 00", "Olympus Zuiko Digital ED 50-200mm F2.8-3.5 SWD"),
        ("0 30 10", "Olympus M.Zuiko Digital ED 45mm F1.2 Pro"),
        ("0 31 00", "Olympus Zuiko Digital ED 12-60mm F2.8-4.0 SWD"),
        ("0 32 00", "Olympus Zuiko Digital ED 14-35mm F2.0 SWD"),
        ("0 32 10", "Olympus M.Zuiko Digital ED 12-200mm F3.5-6.3"),
        ("0 33 00", "Olympus Zuiko Digital 25mm F2.8"),
        (
            "0 33 10",
            "Olympus M.Zuiko Digital 150-400mm F4.5 TC1.25x IS Pro",
        ),
        ("0 34 00", "Olympus Zuiko Digital ED 9-18mm F4.0-5.6"),
        ("0 34 10", "Olympus M.Zuiko Digital ED 12-45mm F4.0 Pro"),
        ("0 35 00", "Olympus Zuiko Digital 14-54mm F2.8-3.5 II"),
        ("0 35 10", "Olympus M.Zuiko 100-400mm F5.0-6.3"),
        ("0 36 10", "Olympus M.Zuiko Digital ED 8-25mm F4 Pro"),
        ("0 37 10", "Olympus M.Zuiko Digital ED 40-150mm F4.0 Pro"),
        ("0 38 10", "Olympus M.Zuiko Digital ED 20mm F1.4 Pro"),
        (
            "0 39 10",
            "Olympus M.Zuiko Digital ED 90mm F3.5 Macro IS Pro",
        ),
        ("0 40 10", "Olympus M.Zuiko Digital ED 150-600mm F5.0-6.3"),
        (
            "0 41 10",
            "OM System M.Zuiko Digital ED 50-200mm F2.8 IS Pro",
        ),
        ("1 01 00", "Sigma 18-50mm F3.5-5.6 DC"),
        ("1 01 10", "Sigma 30mm F2.8 EX DN"),
        ("1 02 00", "Sigma 55-200mm F4.0-5.6 DC"),
        ("1 02 10", "Sigma 19mm F2.8 EX DN"),
        ("1 03 00", "Sigma 18-125mm F3.5-5.6 DC"),
        ("1 03 10", "Sigma 30mm F2.8 DN | A"),
        ("1 04 00", "Sigma 18-125mm F3.5-5.6 DC"),
        ("1 04 10", "Sigma 19mm F2.8 DN | A"),
        ("1 05 00", "Sigma 30mm F1.4 EX DC HSM"),
        ("1 05 10", "Sigma 60mm F2.8 DN | A"),
        ("1 06 00", "Sigma APO 50-500mm F4.0-6.3 EX DG HSM"),
        ("1 06 10", "Sigma 30mm F1.4 DC DN | C"),
        ("1 07 00", "Sigma Macro 105mm F2.8 EX DG"),
        ("1 07 10", "Sigma 16mm F1.4 DC DN | C (017)"),
        ("1 08 00", "Sigma APO Macro 150mm F2.8 EX DG HSM"),
        ("1 09 00", "Sigma 18-50mm F2.8 EX DC Macro"),
        ("1 10 00", "Sigma 24mm F1.8 EX DG Aspherical Macro"),
        ("1 11 00", "Sigma APO 135-400mm F4.5-5.6 DG"),
        ("1 12 00", "Sigma APO 300-800mm F5.6 EX DG HSM"),
        ("1 13 00", "Sigma 30mm F1.4 EX DC HSM"),
        ("1 14 00", "Sigma APO 50-500mm F4.0-6.3 EX DG HSM"),
        ("1 15 00", "Sigma 10-20mm F4.0-5.6 EX DC HSM"),
        ("1 16 00", "Sigma APO 70-200mm F2.8 II EX DG Macro HSM"),
        ("1 17 00", "Sigma 50mm F1.4 EX DG HSM"),
        ("2 01 00", "Leica D Vario Elmarit 14-50mm F2.8-3.5 Asph."),
        ("2 01 10", "Lumix G Vario 14-45mm F3.5-5.6 Asph. Mega OIS"),
        ("2 02 00", "Leica D Summilux 25mm F1.4 Asph."),
        ("2 02 10", "Lumix G Vario 45-200mm F4.0-5.6 Mega OIS"),
        (
            "2 03 00",
            "Leica D Vario Elmar 14-50mm F3.8-5.6 Asph. Mega OIS",
        ),
        ("2 03 01", "Leica D Vario Elmar 14-50mm F3.8-5.6 Asph."),
        (
            "2 03 10",
            "Lumix G Vario HD 14-140mm F4.0-5.8 Asph. Mega OIS",
        ),
        ("2 04 00", "Leica D Vario Elmar 14-150mm F3.5-5.6"),
        ("2 04 10", "Lumix G Vario 7-14mm F4.0 Asph."),
        ("2 05 10", "Lumix G 20mm F1.7 Asph."),
        ("2 06 10", "Leica DG Macro-Elmarit 45mm F2.8 Asph. Mega OIS"),
        ("2 07 10", "Lumix G Vario 14-42mm F3.5-5.6 Asph. Mega OIS"),
        ("2 08 10", "Lumix G Fisheye 8mm F3.5"),
        ("2 09 10", "Lumix G Vario 100-300mm F4.0-5.6 Mega OIS"),
        ("2 10 10", "Lumix G 14mm F2.5 Asph."),
        ("2 11 10", "Lumix G 12.5mm F12 3D"),
        ("2 12 10", "Leica DG Summilux 25mm F1.4 Asph."),
        (
            "2 13 10",
            "Lumix G X Vario PZ 45-175mm F4.0-5.6 Asph. Power OIS",
        ),
        (
            "2 14 10",
            "Lumix G X Vario PZ 14-42mm F3.5-5.6 Asph. Power OIS",
        ),
        ("2 15 10", "Lumix G X Vario 12-35mm F2.8 Asph. Power OIS"),
        ("2 16 10", "Lumix G Vario 45-150mm F4.0-5.6 Asph. Mega OIS"),
        ("2 17 10", "Lumix G X Vario 35-100mm F2.8 Power OIS"),
        (
            "2 18 10",
            "Lumix G Vario 14-42mm F3.5-5.6 II Asph. Mega OIS",
        ),
        ("2 19 10", "Lumix G Vario 14-140mm F3.5-5.6 Asph. Power OIS"),
        ("2 20 10", "Lumix G Vario 12-32mm F3.5-5.6 Asph. Mega OIS"),
        ("2 21 10", "Leica DG Nocticron 42.5mm F1.2 Asph. Power OIS"),
        ("2 22 10", "Leica DG Summilux 15mm F1.7 Asph."),
        ("2 23 10", "Lumix G Vario 35-100mm F4.0-5.6 Asph. Mega OIS"),
        ("2 24 10", "Lumix G Macro 30mm F2.8 Asph. Mega OIS"),
        ("2 25 10", "Lumix G 42.5mm F1.7 Asph. Power OIS"),
        ("2 26 10", "Lumix G 25mm F1.7 Asph."),
        (
            "2 27 10",
            "Leica DG Vario-Elmar 100-400mm F4.0-6.3 Asph. Power OIS",
        ),
        ("2 28 10", "Lumix G Vario 12-60mm F3.5-5.6 Asph. Power OIS"),
        ("2 29 10", "Leica DG Summilux 12mm F1.4 Asph."),
        (
            "2 30 10",
            "Leica DG Vario-Elmarit 12-60mm F2.8-4 Asph. Power OIS",
        ),
        ("2 31 10", "Lumix G Vario 45-200mm F4.0-5.6 II"),
        ("2 32 10", "Lumix G Vario 100-300mm F4.0-5.6 II"),
        ("2 33 10", "Lumix G X Vario 12-35mm F2.8 II Asph. Power OIS"),
        ("2 34 10", "Lumix G Vario 35-100mm F2.8 II"),
        ("2 35 10", "Leica DG Vario-Elmarit 8-18mm F2.8-4 Asph."),
        ("2 36 10", "Leica DG Elmarit 200mm F2.8 Power OIS"),
        (
            "2 37 10",
            "Leica DG Vario-Elmarit 50-200mm F2.8-4 Asph. Power OIS",
        ),
        ("2 38 10", "Leica DG Vario-Summilux 10-25mm F1.7 Asph."),
        ("2 39 10", "Leica DG Summilux 25mm F1.4 II Asph."),
        ("2 40 10", "Leica DG Vario-Summilux 25-50mm F1.7 Asph."),
        ("2 41 10", "Leica DG Summilux 9mm F1.7 Asph."),
        ("3 01 00", "Leica D Vario Elmarit 14-50mm F2.8-3.5 Asph."),
        ("3 02 00", "Leica D Summilux 25mm F1.4 Asph."),
        ("5 01 10", "Tamron 14-150mm F3.5-5.8 Di III"),
        ("18 01 10", "Venus Optics Laowa 50mm F2.8 2x Macro"),
        ("f7 03 10", "LAOWA C&D-Dreamer MFT 7.5mm F2.0"),
        ("f7 0a 10", "LAOWA C&D-Dreamer MFT 6.0mm F2.0"),
    ];
    TABLE
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
}

/// Look up Olympus camera type product code -> model name.
/// Based on publicly documented Olympus product IDs.
fn olympus_camera_type(code: &str) -> Option<String> {
    // Binary search on sorted array for O(log n) lookup
    static TABLE: &[(&str, &str)] = &[
        ("D4028", "X-2,C-50Z"),
        ("D4029", "E-20,E-20N,E-20P"),
        ("D4034", "C720UZ"),
        ("D4040", "E-1"),
        ("D4041", "E-300"),
        ("D4083", "C2Z,D520Z,C220Z"),
        ("D4106", "u20D,S400D,u400D"),
        ("D4120", "X-1"),
        ("D4122", "u10D,S300D,u300D"),
        ("D4125", "AZ-1"),
        ("D4141", "C150,D390"),
        ("D4193", "C-5000Z"),
        ("D4194", "X-3,C-60Z"),
        ("D4199", "u30D,S410D,u410D"),
        ("D4205", "X450,D535Z,C370Z"),
        ("D4210", "C160,D395"),
        ("D4211", "C725UZ"),
        ("D4213", "FerrariMODEL2003"),
        ("D4216", "u15D"),
        ("D4217", "u25D"),
        ("D4220", "u-miniD,Stylus V"),
        ("D4221", "u40D,S500,uD500"),
        ("D4231", "FerrariMODEL2004"),
        ("D4240", "X500,D590Z,C470Z"),
        ("D4244", "uD800,S800"),
        ("D4256", "u720SW,S720SW"),
        ("D4261", "X600,D630,FE5500"),
        ("D4262", "uD600,S600"),
        ("D4301", "u810/S810"),
        ("D4302", "u710,S710"),
        ("D4303", "u700,S700"),
        ("D4304", "FE100,X710"),
        ("D4305", "FE110,X705"),
        ("D4310", "FE-130,X-720"),
        ("D4311", "FE-140,X-725"),
        ("D4312", "FE150,X730"),
        ("D4313", "FE160,X735"),
        ("D4314", "u740,S740"),
        ("D4315", "u750,S750"),
        ("D4316", "u730/S730"),
        ("D4317", "FE115,X715"),
        ("D4321", "SP550UZ"),
        ("D4322", "SP510UZ"),
        ("D4324", "FE170,X760"),
        ("D4326", "FE200"),
        ("D4327", "FE190/X750"),
        ("D4328", "u760,S760"),
        ("D4330", "FE180/X745"),
        ("D4331", "u1000/S1000"),
        ("D4332", "u770SW,S770SW"),
        ("D4333", "FE240/X795"),
        ("D4334", "FE210,X775"),
        ("D4336", "FE230/X790"),
        ("D4337", "FE220,X785"),
        ("D4338", "u725SW,S725SW"),
        ("D4339", "FE250/X800"),
        ("D4341", "u780,S780"),
        ("D4343", "u790SW,S790SW"),
        ("D4344", "u1020,S1020"),
        ("D4346", "FE15,X10"),
        ("D4348", "FE280,X820,C520"),
        ("D4349", "FE300,X830"),
        ("D4350", "u820,S820"),
        ("D4351", "u1200,S1200"),
        ("D4352", "FE270,X815,C510"),
        ("D4353", "u795SW,S795SW"),
        ("D4354", "u1030SW,S1030SW"),
        ("D4355", "SP560UZ"),
        ("D4356", "u1010,S1010"),
        ("D4357", "u830,S830"),
        ("D4359", "u840,S840"),
        ("D4360", "FE350WIDE,X865"),
        ("D4361", "u850SW,S850SW"),
        ("D4362", "FE340,X855,C560"),
        ("D4363", "FE320,X835,C540"),
        ("D4364", "SP570UZ"),
        ("D4366", "FE330,X845,C550"),
        ("D4368", "FE310,X840,C530"),
        ("D4370", "u1050SW,S1050SW"),
        ("D4371", "u1060,S1060"),
        ("D4372", "FE370,X880,C575"),
        ("D4374", "SP565UZ"),
        ("D4377", "u1040,S1040"),
        ("D4378", "FE360,X875,C570"),
        ("D4379", "FE20,X15,C25"),
        ("D4380", "uT6000,ST6000"),
        ("D4381", "uT8000,ST8000"),
        ("D4382", "u9000,S9000"),
        ("D4384", "SP590UZ"),
        ("D4385", "FE3010,X895"),
        ("D4386", "FE3000,X890"),
        ("D4387", "FE35,X30"),
        ("D4388", "u550WP,S550WP"),
        ("D4390", "FE5000,X905"),
        ("D4391", "u5000"),
        ("D4392", "u7000,S7000"),
        ("D4396", "FE5010,X915"),
        ("D4397", "FE25,X20"),
        ("D4398", "FE45,X40"),
        ("D4401", "XZ-1"),
        ("D4402", "uT6010,ST6010"),
        ("D4406", "u7010,S7010 / u7020,S7020"),
        ("D4407", "FE4010,X930"),
        ("D4408", "X560WP"),
        ("D4409", "FE26,X21"),
        ("D4410", "FE4000,X920,X925"),
        ("D4411", "FE46,X41,X42"),
        ("D4412", "FE5020,X935"),
        ("D4413", "uTough-3000"),
        ("D4414", "StylusTough-6020"),
        ("D4415", "StylusTough-8010"),
        ("D4417", "u5010,S5010"),
        ("D4418", "u7040,S7040"),
        ("D4419", "u9010,S9010"),
        ("D4423", "FE4040"),
        ("D4424", "FE47,X43"),
        ("D4426", "FE4030,X950"),
        ("D4428", "FE5030,X965,X960"),
        ("D4430", "u7030,S7030"),
        ("D4432", "SP600UZ"),
        ("D4434", "SP800UZ"),
        ("D4439", "FE4020,X940"),
        ("D4442", "FE5035"),
        ("D4448", "FE4050,X970"),
        ("D4450", "FE5050,X985"),
        ("D4454", "u-7050"),
        ("D4464", "T10,X27"),
        ("D4470", "FE5040,X980"),
        ("D4472", "TG-310"),
        ("D4474", "TG-610"),
        ("D4476", "TG-810"),
        ("D4478", "VG145,VG140,D715"),
        ("D4479", "VG130,D710"),
        ("D4480", "VG120,D705"),
        ("D4482", "VR310,D720"),
        ("D4484", "VR320,D725"),
        ("D4486", "VR330,D730"),
        ("D4488", "VG110,D700"),
        ("D4490", "SP-610UZ"),
        ("D4492", "SZ-10"),
        ("D4494", "SZ-20"),
        ("D4496", "SZ-30MR"),
        ("D4498", "SP-810UZ"),
        ("D4500", "SZ-11"),
        ("D4504", "TG-615"),
        ("D4508", "TG-620"),
        ("D4510", "TG-820"),
        ("D4512", "TG-1"),
        ("D4516", "SH-21"),
        ("D4519", "SZ-14"),
        ("D4520", "SZ-31MR"),
        ("D4521", "SH-25MR"),
        ("D4523", "SP-720UZ"),
        ("D4529", "VG170"),
        ("D4530", "VH210"),
        ("D4531", "XZ-2"),
        ("D4535", "SP-620UZ"),
        ("D4536", "TG-320"),
        ("D4537", "VR340,D750"),
        ("D4538", "VG160,X990,D745"),
        ("D4541", "SZ-12"),
        ("D4545", "VH410"),
        ("D4546", "XZ-10"),
        ("D4547", "TG-2"),
        ("D4548", "TG-830"),
        ("D4549", "TG-630"),
        ("D4550", "SH-50"),
        ("D4553", "SZ-16,DZ-105"),
        ("D4562", "SP-820UZ"),
        ("D4566", "SZ-15"),
        ("D4572", "STYLUS1"),
        ("D4574", "TG-3"),
        ("D4575", "TG-850"),
        ("D4579", "SP-100EE"),
        ("D4580", "SH-60"),
        ("D4581", "SH-1"),
        ("D4582", "TG-835"),
        ("D4585", "SH-2 / SH-3"),
        ("D4586", "TG-4"),
        ("D4587", "TG-860"),
        ("D4590", "TG-TRACKER"),
        ("D4591", "TG-870"),
        ("D4593", "TG-5"),
        ("D4603", "TG-6"),
        ("D4605", "TG-7"),
        ("D4809", "C2500L"),
        ("D4842", "E-10"),
        ("D4856", "C-1"),
        ("D4857", "C-1Z,D-150Z"),
        ("DCHC", "D500L"),
        ("DCHT", "D600L / D620L"),
        ("K0055", "AIR-A01"),
        ("S0003", "E-330"),
        ("S0004", "E-500"),
        ("S0009", "E-400"),
        ("S0010", "E-510"),
        ("S0011", "E-3"),
        ("S0013", "E-410"),
        ("S0016", "E-420"),
        ("S0017", "E-30"),
        ("S0018", "E-520"),
        ("S0019", "E-P1"),
        ("S0023", "E-620"),
        ("S0026", "E-P2"),
        ("S0027", "E-PL1"),
        ("S0029", "E-450"),
        ("S0030", "E-600"),
        ("S0032", "E-P3"),
        ("S0033", "E-5"),
        ("S0034", "E-PL2"),
        ("S0036", "E-M5"),
        ("S0038", "E-PL3"),
        ("S0039", "E-PM1"),
        ("S0040", "E-PL1s"),
        ("S0042", "E-PL5"),
        ("S0043", "E-PM2"),
        ("S0044", "E-P5"),
        ("S0045", "E-PL6"),
        ("S0046", "E-PL7"),
        ("S0047", "E-M1"),
        ("S0051", "E-M10"),
        ("S0052", "E-M5MarkII"),
        ("S0059", "E-M10MarkII"),
        ("S0061", "PEN-F"),
        ("S0065", "E-PL8"),
        ("S0067", "E-M1MarkII"),
        ("S0068", "E-M10MarkIII"),
        ("S0076", "E-PL9"),
        ("S0080", "E-M1X"),
        ("S0085", "E-PL10"),
        ("S0088", "E-M10MarkIV"),
        ("S0089", "E-M5MarkIII"),
        ("S0092", "E-M1MarkIII"),
        ("S0093", "E-P7"),
        ("S0094", "E-M10MarkIIIS"),
        ("S0095", "OM-1"),
        ("S0101", "OM-5"),
        ("S0121", "OM-1MarkII"),
        ("S0123", "OM-3"),
        ("S0130", "OM-5MarkII"),
        ("SR45", "D220"),
        ("SR55", "D320L"),
        ("SR83", "D340L"),
        ("SR85", "C830L,D340R"),
        ("SR852", "C860L,D360L"),
        ("SR872", "C900Z,D400Z"),
        ("SR874", "C960Z,D460Z"),
        ("SR951", "C2000Z"),
        ("SR952", "C21"),
        ("SR953", "C21T.commu"),
        ("SR954", "C2020Z"),
        ("SR955", "C990Z,D490Z"),
        ("SR956", "C211Z"),
        ("SR959", "C990ZS,D490Z"),
        ("SR95A", "C2100UZ"),
        ("SR971", "C100,D370"),
        ("SR973", "C2,D230"),
        ("SX151", "E100RS"),
        ("SX351", "C3000Z / C3030Z"),
        ("SX354", "C3040Z"),
        ("SX355", "C2040Z"),
        ("SX357", "C700UZ"),
        ("SX358", "C200Z,D510Z"),
        ("SX374", "C3100Z,C3020Z"),
        ("SX552", "C4040Z"),
        ("SX553", "C40Z,D40Z"),
        ("SX556", "C730UZ"),
        ("SX558", "C5050Z"),
        ("SX571", "C120,D380"),
        ("SX574", "C300Z,D550Z"),
        ("SX575", "C4100Z,C4000Z"),
        ("SX751", "X200,D560Z,C350Z"),
        ("SX752", "X300,D565Z,C450Z"),
        ("SX753", "C750UZ"),
        ("SX754", "C740UZ"),
        ("SX755", "C755UZ"),
        ("SX756", "C5060WZ"),
        ("SX757", "C8080WZ"),
        ("SX758", "X350,D575Z,C360Z"),
        ("SX759", "X400,D580Z,C460Z"),
        ("SX75A", "AZ-2ZOOM"),
        ("SX75B", "D595Z,C500Z"),
        ("SX75C", "X550,D545Z,C480Z"),
        ("SX75D", "IR-300"),
        ("SX75F", "C55Z,C5500Z"),
        ("SX75G", "C170,D425"),
        ("SX75J", "C180,D435"),
        ("SX771", "C760UZ"),
        ("SX772", "C770UZ"),
        ("SX773", "C745UZ"),
        ("SX774", "X250,D560Z,C350Z"),
        ("SX775", "X100,D540Z,C310Z"),
        ("SX776", "C460ZdelSol"),
        ("SX777", "C765UZ"),
        ("SX77A", "D555Z,C315Z"),
        ("SX851", "C7070WZ"),
        ("SX852", "C70Z,C7000Z"),
        ("SX853", "SP500UZ"),
        ("SX854", "SP310"),
        ("SX855", "SP350"),
        ("SX873", "SP320"),
        ("SX875", "FE180/X745"),
        ("SX876", "FE190/X750"),
    ];
    TABLE
        .binary_search_by_key(&code, |&(k, _)| k)
        .ok()
        .map(|i| TABLE[i].1.to_string())
}

/// Decode an Olympus sub-IFD (Equipment, CameraSettings, ImageProcessing, FocusInfo, etc.)
///
/// Decode Olympus TextInfo (tag 0x0208): space/LF-separated "key value" pairs.
fn decode_olympus_text_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // TextInfo contains sections like "[pictureInfo] Resolution 1 [Camera Info] Type SR951"
    // Parse key-value pairs - skip section headers in brackets
    let mut tokens: Vec<&str> = Vec::new();
    for part in text.split([' ', '\n', '\r']) {
        let part = part.trim_matches('\0').trim();
        if !part.is_empty() {
            tokens.push(part);
        }
    }
    let mut i = 0;
    while i < tokens.len() {
        // Skip section headers like "[pictureInfo]"
        if tokens[i].starts_with('[') {
            // skip until closing bracket
            while i < tokens.len() && !tokens[i].ends_with(']') {
                i += 1;
            }
            i += 1;
            continue;
        }
        // Key followed by value
        if i + 1 < tokens.len() {
            let key = tokens[i];
            let val = tokens[i + 1];
            // Map known tag names
            let name = match key {
                "Type" => "CameraType",
                _ => key,
            };
            tags.push(DecodedTag {
                name: name.to_string(),
                value: val.to_string(),
            });
            i += 2;
        } else {
            i += 1;
        }
    }
}

/// Extract Olympus camera model from Equipment sub-IFD (0x2010), tag CameraType2 (0x0100).
fn extract_olympus_model(
    ifd: &crate::tiff::Ifd<'_>,
    mn_data: &[u8],
    tiff_data: &[u8],
    be: bool,
) -> String {
    let equip_entry = ifd.entries.iter().find(|e| e.tag == 0x2010);
    let Some(equip_entry) = equip_entry else {
        return String::new();
    };
    let oly_new_style = mn_data.starts_with(b"OLYMPUS\0");
    let sub_ifd_data = if oly_new_style || tiff_data.is_empty() {
        mn_data
    } else {
        tiff_data
    };
    // Parse Equipment sub-IFD to find CameraType2
    let sub_ifd = if equip_entry.data.len() >= 4
        && (equip_entry.data_type == crate::tiff::DataType::Long
            || equip_entry.data_type == crate::tiff::DataType::Ifd
            || (equip_entry.count == 1 && !equip_entry.inline))
    {
        let offset = if be {
            u32::from_be_bytes([
                equip_entry.data[0],
                equip_entry.data[1],
                equip_entry.data[2],
                equip_entry.data[3],
            ])
        } else {
            u32::from_le_bytes([
                equip_entry.data[0],
                equip_entry.data[1],
                equip_entry.data[2],
                equip_entry.data[3],
            ])
        } as u64;
        tiff::parse_ifd_tolerant(sub_ifd_data, offset, be, false)
    } else {
        // Old style inline
        let sub_be = if equip_entry.data.len() > 4 {
            let be_count = u16::from_be_bytes([equip_entry.data[0], equip_entry.data[1]]) as usize;
            let le_count = u16::from_le_bytes([equip_entry.data[0], equip_entry.data[1]]) as usize;
            if (1..=200).contains(&be_count) && !(1..=200).contains(&le_count) {
                true
            } else {
                be
            }
        } else {
            be
        };
        let mn_start = sub_ifd_data.as_ptr() as usize;
        let entry_start = equip_entry.data.as_ptr() as usize;
        if entry_start >= mn_start && entry_start < mn_start + sub_ifd_data.len() {
            let off_in_mn = entry_start - mn_start;
            tiff::parse_ifd_tolerant(sub_ifd_data, off_in_mn as u64, sub_be, false)
        } else {
            tiff::parse_ifd_tolerant(equip_entry.data, 0, sub_be, false)
        }
    };
    if let Some(sub) = sub_ifd {
        if let Some(ct2) = sub.entries.iter().find(|e| e.tag == 0x0100) {
            let code = std::str::from_utf8(ct2.data)
                .unwrap_or("")
                .trim_end_matches('\0')
                .trim();
            return olympus_camera_type(code).unwrap_or_else(|| code.to_string());
        }
    }
    String::new()
}

/// Sub-IFD tags (0x2010, 0x2020, 0x2040, 0x2050) are IFD/LONG offsets into the MakerNote data.
/// For old-style (UNDEFINED type), the data is an inline IFD blob.
fn decode_olympus_sub_ifd(
    entry: &IfdEntry<'_>,
    mn_data: &[u8],
    parent_be: bool,
    olympus_model: &str,
    tags: &mut Vec<DecodedTag>,
) {
    // Determine sub-IFD name and tag table
    let (sub_name, tag_table) = match entry.tag {
        0x2010 => ("Equipment", &OLYMPUS_EQUIPMENT_TAGS[..]),
        0x2020 => ("CameraSettings", &OLYMPUS_CAMERA_SETTINGS_TAGS[..]),
        0x2030 | 0x2031 => ("RawDevelopment", &OLYMPUS_RAW_DEVELOPMENT_TAGS[..]),
        0x2040 => ("ImageProcessing", &OLYMPUS_IMAGE_PROCESSING_TAGS[..]),
        0x2050 => ("FocusInfo", &OLYMPUS_FOCUS_INFO_TAGS[..]),
        _ => return,
    };

    // Try to get the sub-IFD, tracking effective byte order
    let (sub_ifd, eff_be) = if entry.data.len() >= 4
        && (entry.data_type == crate::tiff::DataType::Long
            || entry.data_type == crate::tiff::DataType::Ifd
            || (entry.count == 1 && !entry.inline))
    {
        // New style: offset pointer into mn_data
        let offset = if parent_be {
            u32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
        } else {
            u32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
        } as u64;
        let ifd = if !mn_data.is_empty() {
            tiff::parse_ifd_tolerant(mn_data, offset, parent_be, false)
        } else {
            None
        };
        (ifd, parent_be)
    } else {
        // Old style: inline IFD blob in entry.data
        // Detect byte order from the data
        let sub_be = if entry.data.len() > 4 {
            let le_count = u16::from_le_bytes([entry.data[0], entry.data[1]]) as usize;
            let be_count = u16::from_be_bytes([entry.data[0], entry.data[1]]) as usize;
            if (1..=200).contains(&be_count) && !(1..=200).contains(&le_count) {
                true
            } else {
                parent_be
            }
        } else {
            parent_be
        };
        // For old-style OLYMP\0, entry.data is a slice of tiff_data.
        // Sub-IFD entries may use absolute offsets that extend beyond the blob.
        let mn_start = mn_data.as_ptr() as usize;
        let entry_start = entry.data.as_ptr() as usize;
        let ifd = if entry_start >= mn_start && entry_start < mn_start + mn_data.len() {
            let off_in_mn = entry_start - mn_start;
            tiff::parse_ifd_tolerant(mn_data, off_in_mn as u64, sub_be, false)
        } else {
            tiff::parse_ifd_tolerant(entry.data, 0, sub_be, false)
        };
        (ifd, sub_be)
    };

    let Some(sub_ifd) = sub_ifd else {
        return;
    };

    // Decode each tag in the sub-IFD using the effective byte order
    for sub_entry in &sub_ifd.entries {
        let tag_name = tag_table
            .iter()
            .find(|&&(id, _)| id == sub_entry.tag)
            .map(|&(_, n)| n);
        let Some(tag_name) = tag_name else {
            continue;
        };

        let val = match entry.tag {
            0x2010 => format_olympus_equipment(sub_entry, tag_name, eff_be),
            0x2020 => format_olympus_camera_settings(sub_entry, tag_name, eff_be, olympus_model),
            0x2030 | 0x2031 => format_olympus_raw_development(sub_entry, tag_name, eff_be),
            0x2040 => format_olympus_image_processing(sub_entry, tag_name, eff_be),
            0x2050 => format_olympus_focus_info(sub_entry, tag_name, eff_be),
            _ => format_ifd_value(sub_entry, eff_be),
        };
        tags.push(DecodedTag {
            name: tag_name.to_string(),
            value: val,
        });
    }
    let _ = sub_name;
}

/// Format an Olympus Equipment sub-IFD tag value
fn format_olympus_equipment(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    match name {
        "EquipmentVersion"
        | "LensSerialNumber"
        | "ExtenderSerialNumber"
        | "FlashSerialNumber"
        | "InternalSerialNumber"
        | "SerialNumber" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        "CameraType2" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            let s = s.trim_end_matches('\0').trim();
            olympus_camera_type(s).unwrap_or_else(|| s.to_string())
        }
        "LensModel" | "ExtenderModel" | "ConversionLens" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        "BodyFirmwareVersion"
        | "LensFirmwareVersion"
        | "ExtenderFirmwareVersion"
        | "FlashFirmwareVersion" => {
            // Encoded as u32 where hex representation gives version: 0x1300 -> "1.300"
            if let Some(v) = entry_u32(entry, be) {
                if v == 0 {
                    return "0".into();
                }
                let hex = format!("{v:X}");
                if hex.len() > 1 {
                    format!(
                        "{}.{}",
                        &hex[..hex.len() - 3.min(hex.len())],
                        &hex[hex.len() - 3.min(hex.len())..]
                    )
                } else {
                    hex
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "MaxApertureAtMinFocal" | "MaxApertureAtMaxFocal" | "MaxAperture" => {
            if let Some(v) = entry_u16(entry, be) {
                if v == 0 {
                    return "0".into();
                }
                let fnum = 2.0_f64.sqrt().powf(v as f64 / 256.0);
                format!("{fnum:.1}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "MinFocalLength" | "MaxFocalLength" => {
            if let Some(v) = entry_u16(entry, be) {
                format!("{v}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "LensType" => {
            // 6 bytes: [Make, ?, Model, SubModel, ?, ?]
            // Key format: "{make:x} {model:02x} {submodel:02x}"
            if entry.data.len() >= 6 {
                let key = format!(
                    "{:x} {:02x} {:02x}",
                    entry.data[0], entry.data[2], entry.data[3]
                );
                olympus_lens_type(&key).unwrap_or(key)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FlashType" => match entry_u16(entry, be) {
            Some(0) => "None".into(),
            Some(2) => "Simple E-System".into(),
            Some(3) => "E-System".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FocalPlaneDiagonal" => {
            // Rational - display as mm
            if entry.data.len() >= 8 {
                let num = if be {
                    u32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                } else {
                    u32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                };
                let den = if be {
                    u32::from_be_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                } else {
                    u32::from_le_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                };
                if den == 0 {
                    format!("{num}")
                } else {
                    let mm = num as f64 / den as f64;
                    // Trim trailing zeros
                    let s = format!("{mm:.3}");
                    let s = s.trim_end_matches('0');
                    let s = s.trim_end_matches('.');
                    format!("{s} mm")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "LensProperties" => {
            if let Some(v) = entry_u16(entry, be) {
                format!("{v:#06x}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "Extender" => {
            // 6 bytes: [Make, ?, Model, SubModel, ?, ?] - all zeros = None
            if entry.data.len() >= 6 && entry.data[..6].iter().all(|&b| b == 0) {
                "None".into()
            } else if entry.data.len() >= 6 {
                format!(
                    "{} {:02} {:02}",
                    entry.data[0], entry.data[2], entry.data[3]
                )
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FlashModel" => match entry_u16(entry, be) {
            Some(0) => "None".into(),
            Some(1) => "FL-20".into(),
            Some(2) => "FL-50".into(),
            Some(3) => "RF-11".into(),
            Some(4) => "TF-22".into(),
            Some(5) => "FL-36".into(),
            Some(6) => "FL-50R".into(),
            Some(7) | Some(9) => "FL-36R".into(),
            Some(11) => "FL-14".into(),
            Some(12) => "FL-600R".into(),
            Some(13) => "FL-LM3".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        _ => format_ifd_value(entry, be),
    }
}

/// Format an Olympus CameraSettings sub-IFD tag value
fn format_olympus_camera_settings(
    entry: &IfdEntry<'_>,
    name: &str,
    be: bool,
    model: &str,
) -> String {
    match name {
        "CameraSettingsVersion" | "ImageProcessingVersion" | "FocusInfoVersion" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        "PreviewImageValid" => match entry_u32(entry, be) {
            Some(0) => "No".into(),
            Some(1) => "Yes".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "PreviewImageStart" | "PreviewImageLength" => {
            if let Some(v) = entry_u32(entry, be) {
                format!("{v}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ExposureMode" => match entry_u16(entry, be) {
            Some(1) => "Manual".into(),
            Some(2) => "Program".into(),
            Some(3) => "Aperture-priority AE".into(),
            Some(4) => "Shutter speed priority AE".into(),
            Some(5) => "Program-shift".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "AELock" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "MeteringMode" => match entry_u16(entry, be) {
            Some(2) => "Center-weighted average".into(),
            Some(3) => "Spot".into(),
            Some(5) => "ESP".into(),
            Some(261) => "Pattern+AF".into(),
            Some(515) => "Spot+Highlight control".into(),
            Some(1027) => "Spot+Shadow control".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "MacroMode" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(2) => "Super Macro".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FocusMode" => {
            if let Some(v) = entry_u16(entry, be) {
                let mode = match v {
                    0 => "Single AF",
                    1 => "Sequential shooting AF",
                    2 => "Continuous AF",
                    3 => "Multi AF",
                    4 => "Face detect",
                    10 => "MF",
                    _ => return format!("{v}"),
                };
                // Second u16 is bitmask of AF modes
                if entry.data.len() >= 4 {
                    let v2 = if be {
                        u16::from_be_bytes([entry.data[2], entry.data[3]])
                    } else {
                        u16::from_le_bytes([entry.data[2], entry.data[3]])
                    };
                    if v2 != 0 {
                        let mut parts = Vec::new();
                        if v2 & 0x01 != 0 {
                            parts.push("S-AF");
                        }
                        if v2 & 0x04 != 0 {
                            parts.push("C-AF");
                        }
                        if v2 & 0x10 != 0 {
                            parts.push("MF");
                        }
                        if v2 & 0x20 != 0 {
                            parts.push("Face Detect");
                        }
                        if v2 & 0x40 != 0 {
                            parts.push("Imager AF");
                        }
                        if v2 & 0x80 != 0 {
                            parts.push("Live View Magnification Frame");
                        }
                        if v2 & 0x100 != 0 {
                            parts.push("AF sensor");
                        }
                        if v2 & 0x200 != 0 {
                            parts.push("Starry Sky AF");
                        }
                        if !parts.is_empty() {
                            return format!("{}; {}", mode, parts.join(", "));
                        }
                    }
                }
                mode.to_string()
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FocusProcess" => {
            let first = entry_u16(entry, be);
            let label = match first {
                Some(0) => "AF Not Used",
                Some(1) => "AF Used",
                Some(v) => {
                    return {
                        // Append second value if present
                        if entry.data.len() >= 4 {
                            let v2 = if be {
                                u16::from_be_bytes([entry.data[2], entry.data[3]])
                            } else {
                                u16::from_le_bytes([entry.data[2], entry.data[3]])
                            };
                            format!("{v}; {v2}")
                        } else {
                            format!("{v}")
                        }
                    };
                }
                None => return format_ifd_value(entry, be),
            };
            // Append second value if present
            if entry.data.len() >= 4 {
                let v2 = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                format!("{label}; {v2}")
            } else {
                label.into()
            }
        }
        "AFSearch" => match entry_u16(entry, be) {
            Some(0) => "Not Ready".into(),
            Some(1) => "Ready".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FlashMode" => {
            if let Some(v) = entry_u16(entry, be) {
                if v == 0 {
                    return "Off".into();
                }
                let mut parts = Vec::new();
                if v & 0x01 != 0 {
                    parts.push("On");
                }
                if v & 0x02 != 0 {
                    parts.push("Fill-in");
                }
                if v & 0x04 != 0 {
                    parts.push("Red-eye");
                }
                if v & 0x08 != 0 {
                    parts.push("Slow-sync");
                }
                if v & 0x10 != 0 {
                    parts.push("Forced On");
                }
                if v & 0x20 != 0 {
                    parts.push("2nd Curtain");
                }
                if parts.is_empty() {
                    format!("{v}")
                } else {
                    parts.join(", ")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FlashExposureComp" => {
            if let Some(v) = olympus_read_rational(entry, be) {
                if v == 0.0 {
                    "0".into()
                } else {
                    format!("{v:+.1}")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "WhiteBalance2" => match entry_u16(entry, be) {
            Some(0) => "Auto".into(),
            Some(1) => "Auto (Keep Warm Color Off)".into(),
            Some(16) => "7500K (Fine Weather with Shade)".into(),
            Some(17) => "6000K (Cloudy)".into(),
            Some(18) => "5300K (Fine Weather)".into(),
            Some(20) => "3000K (Tungsten light)".into(),
            Some(21) => "3600K (Tungsten light-like scene)".into(),
            Some(22) => "Auto Setup".into(),
            Some(23) => "5500K (Flash, studio use)".into(),
            Some(33) => "6600K (Daylight fluorescent)".into(),
            Some(34) => "4500K (Neutral white fluorescent)".into(),
            Some(35) => "4000K (Cool white fluorescent)".into(),
            Some(36) => "White Fluorescent".into(),
            Some(48) => "3600K (Tungsten light-like scene)".into(),
            Some(67) => "Underwater".into(),
            Some(256) => "One Touch WB 1".into(),
            Some(257) => "One Touch WB 2".into(),
            Some(258) => "One Touch WB 3".into(),
            Some(259) => "One Touch WB 4".into(),
            Some(512) => "Custom WB 1".into(),
            Some(513) => "Custom WB 2".into(),
            Some(514) => "Custom WB 3".into(),
            Some(515) => "Custom WB 4".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "WhiteBalanceTemperature" => {
            if let Some(v) = entry_u16(entry, be) {
                if v == 0 {
                    "Auto".into()
                } else {
                    format!("{v}")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "CustomSaturation"
        | "ContrastSetting"
        | "SharpnessSetting"
        | "PictureModeSaturation"
        | "PictureModeContrast"
        | "PictureModeSharpness" => {
            // [value, min, max] - 3 shorts
            if entry.data.len() >= 6 {
                let v0 = if be {
                    i16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    i16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let v1 = if be {
                    i16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    i16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                let v2 = if be {
                    i16::from_be_bytes([entry.data[4], entry.data[5]])
                } else {
                    i16::from_le_bytes([entry.data[4], entry.data[5]])
                };
                if name == "CustomSaturation" && model.starts_with("E-1") {
                    // E-1: offset by min value and prefix with "CS"
                    let a = v0 - v1;
                    let c = v2 - v1;
                    format!("CS{a} (min CS0, max CS{c})")
                } else {
                    format!("{v0} (min {v1}, max {v2})")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ColorSpace" => match entry_u16(entry, be) {
            Some(0) => "sRGB".into(),
            Some(1) => "Adobe RGB".into(),
            Some(2) => "Pro Photo RGB".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "SceneMode" => match entry_u16(entry, be) {
            Some(0) => "Standard".into(),
            Some(6) => "Auto".into(),
            Some(7) => "Sport".into(),
            Some(8) => "Portrait".into(),
            Some(9) => "Landscape+Portrait".into(),
            Some(10) => "Landscape".into(),
            Some(11) => "Night Scene".into(),
            Some(12) => "Self Portrait".into(),
            Some(13) => "Panorama".into(),
            Some(14) => "2 in 1".into(),
            Some(15) => "Movie".into(),
            Some(16) => "Landscape+Portrait".into(),
            Some(17) => "Night+Portrait".into(),
            Some(18) => "Indoor".into(),
            Some(19) => "Fireworks".into(),
            Some(20) => "Sunset".into(),
            Some(22) => "Macro".into(),
            Some(23) => "Super Macro".into(),
            Some(24) => "Food".into(),
            Some(25) => "Documents".into(),
            Some(26) => "Museum".into(),
            Some(27) => "Shoot & Select".into(),
            Some(28) => "Beach & Snow".into(),
            Some(29) => "Self Portrait+Timer".into(),
            Some(30) => "Candle".into(),
            Some(35) => "Underwater Wide1".into(),
            Some(36) => "Underwater Macro".into(),
            Some(39) => "High Key".into(),
            Some(40) => "Digital Image Stabilization".into(),
            Some(44) => "Underwater Wide2".into(),
            Some(45) => "Low Key".into(),
            Some(46) => "Children".into(),
            Some(48) => "Nature Macro".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "NoiseReduction" => {
            if let Some(v) = entry_u16(entry, be) {
                if v == 0 {
                    return "(none)".into();
                }
                let mut parts = Vec::new();
                if v & 0x01 != 0 {
                    parts.push("Noise Reduction");
                }
                if v & 0x02 != 0 {
                    parts.push("Noise Filter");
                }
                if v & 0x04 != 0 {
                    parts.push("Noise Filter (ISO Boost)");
                }
                if v & 0x08 != 0 {
                    parts.push("Auto");
                }
                if parts.is_empty() {
                    format!("{v}")
                } else {
                    parts.join(", ")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "DistortionCorrection" | "ShadingCompensation" | "NDFilter" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ImageQuality2" => match entry_u16(entry, be) {
            Some(1) => "SQ".into(),
            Some(2) => "HQ".into(),
            Some(3) => "SHQ".into(),
            Some(4) => "RAW".into(),
            Some(5) => "SQ (5)".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ImageStabilization" => match entry_u32(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "On, S-IS1 (All Direction Shake IS)".into(),
            Some(2) => "On, S-IS2 (Vertical Shake IS)".into(),
            Some(3) => "On, S-IS3 (Horizontal Shake IS)".into(),
            Some(4) => "On, S-IS Auto".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "DriveMode" => {
            // Multi-value: mode, shot number, mode bits, ...
            if let Some(v) = entry_u16(entry, be) {
                let mode: String = match v {
                    0 => "Single Shot".into(),
                    1 => "Continuous Shooting".into(),
                    2 => "Exposure Bracketing".into(),
                    3 => "White Balance Bracketing".into(),
                    4 => "Exposure+WB Bracketing".into(),
                    v => format!("{v}"),
                };
                // Second value is shot number
                if entry.data.len() >= 4 {
                    let shot = if be {
                        u16::from_be_bytes([entry.data[2], entry.data[3]])
                    } else {
                        u16::from_le_bytes([entry.data[2], entry.data[3]])
                    };
                    if shot > 0 {
                        return format!("{mode}, Shot {shot}");
                    }
                }
                mode
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PictureMode" => {
            let label = match entry_u16(entry, be) {
                Some(1) => "Vivid",
                Some(2) => "Natural",
                Some(3) => "Muted",
                Some(4) => "Portrait",
                Some(5) => "i-Enhance",
                Some(6) => "e-Portrait",
                Some(7) => "Color Creator",
                Some(8) => "Underwater",
                Some(17) => "Art Mode",
                Some(256) => "Monotone",
                Some(512) => "Sepia",
                Some(v) => {
                    return {
                        if entry.data.len() >= 4 {
                            let v2 = if be {
                                u16::from_be_bytes([entry.data[2], entry.data[3]])
                            } else {
                                u16::from_le_bytes([entry.data[2], entry.data[3]])
                            };
                            format!("{v}; {v2}")
                        } else {
                            format!("{v}")
                        }
                    };
                }
                None => return format_ifd_value(entry, be),
            };
            if entry.data.len() >= 4 {
                let v2 = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                format!("{label}; {v2}")
            } else {
                label.into()
            }
        }
        "ModifiedSaturation" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "CM1 (Red Enhance)".into(),
            Some(2) => "CM2 (Green Enhance)".into(),
            Some(3) => "CM3 (Blue Enhance)".into(),
            Some(4) => "CM4 (Skin Tones)".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "PanoramaMode" => {
            // First u16 is mode
            match entry_u16(entry, be) {
                Some(0) => "Off".into(),
                Some(1) => "On".into(),
                Some(v) => format!("{v}"),
                None => format_ifd_value(entry, be),
            }
        }
        "AFFineTune" => match entry.data.first() {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "MultipleExposureMode" => {
            let label = match entry_u16(entry, be) {
                Some(0) => "Off",
                Some(1) => "Live Composite",
                Some(2) => "On (2 frames)",
                Some(3) => "On (3 frames)",
                Some(v) => {
                    return {
                        if entry.data.len() >= 4 {
                            let v2 = if be {
                                u16::from_be_bytes([entry.data[2], entry.data[3]])
                            } else {
                                u16::from_le_bytes([entry.data[2], entry.data[3]])
                            };
                            format!("{v}; {v2}")
                        } else {
                            format!("{v}")
                        }
                    };
                }
                None => return format_ifd_value(entry, be),
            };
            if entry.data.len() >= 4 {
                let v2 = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                format!("{label}; {v2}")
            } else {
                label.into()
            }
        }
        "Gradation" => {
            // 3 or 4 int16s values: first 3 are gradation type, 4th is selection mode
            if entry.data.len() >= 6 {
                let v0 = if be {
                    i16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    i16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let v1 = if be {
                    i16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    i16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                let v2 = if be {
                    i16::from_be_bytes([entry.data[4], entry.data[5]])
                } else {
                    i16::from_le_bytes([entry.data[4], entry.data[5]])
                };
                let label = match (v0, v1, v2) {
                    (0, 0, 0) => "n/a",
                    (-1, -1, 1) => "Low Key",
                    (0, -1, 1) => "Normal",
                    (1, -1, 1) => "High Key",
                    _ => return format!("{v0} {v1} {v2}"),
                };
                // 4th value: selection mode
                if entry.data.len() >= 8 {
                    let v3 = if be {
                        i16::from_be_bytes([entry.data[6], entry.data[7]])
                    } else {
                        i16::from_le_bytes([entry.data[6], entry.data[7]])
                    };
                    let sel = match v3 {
                        0 => "User-Selected",
                        1 => "Auto-Override",
                        _ => return format!("{label}; {v3}"),
                    };
                    format!("{label}; {sel}")
                } else {
                    label.into()
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "CompressionFactor" => {
            if let Some(v) = olympus_read_rational(entry, be) {
                // Format as decimal, stripping trailing zeros
                let s = format!("{v:.1}");
                s
            } else {
                format_ifd_value(entry, be)
            }
        }
        "WhiteBalanceBracket" | "ExposureShift" | "AFFineTuneAdj" | "FocusBracketStepSize" => {
            format_ifd_value(entry, be)
        }
        "FlashRemoteControl" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "Channel 1, Low".into(),
            Some(2) => "Channel 2, Low".into(),
            Some(3) => "Channel 3, Low".into(),
            Some(4) => "Channel 4, Low".into(),
            Some(9) => "Channel 1, Mid".into(),
            Some(10) => "Channel 2, Mid".into(),
            Some(11) => "Channel 3, Mid".into(),
            Some(12) => "Channel 4, Mid".into(),
            Some(17) => "Channel 1, High".into(),
            Some(18) => "Channel 2, High".into(),
            Some(19) => "Channel 3, High".into(),
            Some(20) => "Channel 4, High".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FlashControlMode" => {
            // int16u[3 or 4]: mode values separated by "; "
            let vals = read_u16_array(entry.data, be);
            if vals.is_empty() {
                return format_ifd_value(entry, be);
            }
            let mode = match vals[0] {
                0 => "Off",
                3 => "TTL",
                4 => "Auto",
                5 => "Manual",
                _ => {
                    return vals
                        .iter()
                        .map(|v| format!("{v}"))
                        .collect::<Vec<_>>()
                        .join("; ");
                }
            };
            let rest: Vec<String> = vals[1..].iter().map(|v| format!("{v}")).collect();
            if rest.is_empty() {
                mode.into()
            } else {
                format!("{}; {}", mode, rest.join("; "))
            }
        }
        "FlashIntensity" | "ManualFlashStrength" => {
            // rational64s[3 or 4], "n/a" when all zeros, or "n/a (x4)" for 4 zeros
            if entry.data.len() >= 24 {
                let count = entry.data.len() / 8;
                let mut all_zero = true;
                for i in 0..count {
                    let off = i * 8;
                    let num = if be {
                        i32::from_be_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    } else {
                        i32::from_le_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    };
                    let den = if be {
                        i32::from_be_bytes([
                            entry.data[off + 4],
                            entry.data[off + 5],
                            entry.data[off + 6],
                            entry.data[off + 7],
                        ])
                    } else {
                        i32::from_le_bytes([
                            entry.data[off + 4],
                            entry.data[off + 5],
                            entry.data[off + 6],
                            entry.data[off + 7],
                        ])
                    };
                    if num != 0 || den != 0 {
                        all_zero = false;
                        break;
                    }
                }
                if all_zero {
                    if count == 4 {
                        "n/a (x4)".into()
                    } else {
                        "n/a".into()
                    }
                } else {
                    format_ifd_value(entry, be)
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FacesDetected" => {
            // int32u, 2 or 3 values, space-separated
            let mut vals = Vec::new();
            let mut off = 0;
            while off + 4 <= entry.data.len() {
                let v = if be {
                    u32::from_be_bytes([
                        entry.data[off],
                        entry.data[off + 1],
                        entry.data[off + 2],
                        entry.data[off + 3],
                    ])
                } else {
                    u32::from_le_bytes([
                        entry.data[off],
                        entry.data[off + 1],
                        entry.data[off + 2],
                        entry.data[off + 3],
                    ])
                };
                vals.push(format!("{v}"));
                off += 4;
            }
            vals.join(" ")
        }
        "NoiseFilter" => {
            // int16s[3]: special values map to named modes
            if entry.data.len() >= 6 {
                let vals: Vec<i16> = read_u16_array(entry.data, be)
                    .iter()
                    .map(|&v| v as i16)
                    .collect();
                let s = vals
                    .iter()
                    .map(|v| format!("{v}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                match s.as_str() {
                    "0 0 0" => "n/a".into(),
                    "-2 -2 1" => "Off".into(),
                    "-1 -2 1" => "Low".into(),
                    "0 -2 1" => "Standard".into(),
                    "1 -2 1" => "High".into(),
                    _ => s,
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ExtendedWBDetect" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "RollAngle" => {
            // int16s[2]: second value 1 means valid, negate and /10 for degrees
            if entry.data.len() >= 4 {
                let v0 = if be {
                    i16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    i16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let v1 = if be {
                    i16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    i16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                if v1 == 1 {
                    let angle = -(v0 as f64) / 10.0;
                    format!("{angle}")
                } else {
                    "n/a".into()
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ManometerPressure" => {
            if let Some(v) = entry_u16(entry, be) {
                let kpa = v as f64 / 10.0;
                format!("{kpa} kPa")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ManometerReading" => {
            // int32s[2]: meters*10, feet*10
            if entry.data.len() >= 8 {
                let m = if be {
                    i32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                } else {
                    i32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                };
                let ft = if be {
                    i32::from_be_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                } else {
                    i32::from_le_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                };
                let m_val = m as f64 / 10.0;
                let ft_val = ft as f64 / 10.0;
                format!("{m_val} m, {ft_val} ft")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PictureModeBWFilter" => match entry_i16(entry, be) {
            Some(0) => "n/a".into(),
            Some(1) => "Neutral".into(),
            Some(2) => "Yellow".into(),
            Some(3) => "Orange".into(),
            Some(4) => "Red".into(),
            Some(5) => "Green".into(),
            _ => format_ifd_value(entry, be),
        },
        "PictureModeTone" => match entry_i16(entry, be) {
            Some(0) => "n/a".into(),
            Some(1) => "Neutral".into(),
            Some(2) => "Sepia".into(),
            Some(3) => "Blue".into(),
            Some(4) => "Purple".into(),
            Some(5) => "Green".into(),
            _ => format_ifd_value(entry, be),
        },
        "ArtFilter" => {
            // int16u array: first value is filter ID, rest are parameters
            let vals = read_u16_array(entry.data, be);
            if vals.is_empty() {
                return format_ifd_value(entry, be);
            }
            let filter_name = olympus_art_filter_name(vals[0]);
            let mut parts = vec![filter_name];
            for &v in &vals[1..] {
                parts.push(format!("{v}"));
            }
            parts.join("; ")
        }
        "ArtFilterEffect" => {
            // int16u array with per-element conversions
            let vals = read_u16_array(entry.data, be);
            if vals.is_empty() {
                return format_ifd_value(entry, be);
            }
            let mut parts: Vec<String> = Vec::new();
            for (i, &v) in vals.iter().enumerate() {
                let s = match i {
                    0 => olympus_art_filter_name(v),
                    3 => format!("Partial Color {v}"),
                    4 => match v {
                        0x0000 => "No Effect".into(),
                        0x8010 => "Star Light".into(),
                        0x8020 => "Pin Hole".into(),
                        0x8030 => "Frame".into(),
                        0x8040 => "Soft Focus".into(),
                        0x8050 => "White Edge".into(),
                        0x8060 => "B&W".into(),
                        0x8080 => "Blur Top and Bottom".into(),
                        0x8081 => "Blur Left and Right".into(),
                        _ => format!("{v}"),
                    },
                    6 => match v {
                        0 => "No Color Filter".into(),
                        1 => "Yellow".into(),
                        2 => "Orange".into(),
                        3 => "Red".into(),
                        4 => "Magenta".into(),
                        5 => "Blue".into(),
                        6 => "Cyan".into(),
                        7 => "Green".into(),
                        _ => format!("{v}"),
                    },
                    _ => format!("{v}"),
                };
                parts.push(s);
            }
            parts.join("; ")
        }
        "PictureModeEffect" => {
            // int16s[3]: compound value mapped as string
            let vals = read_i16_array(entry.data, be);
            if vals.len() >= 3 {
                let key = format!("{} {} {}", vals[0], vals[1], vals[2]);
                match key.as_str() {
                    "0 0 0" => "n/a".into(),
                    "-1 -1 1" => "Low".into(),
                    "0 -1 1" => "Standard".into(),
                    "1 -1 1" => "High".into(),
                    _ => format_ifd_value(entry, be),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ToneLevel" => {
            // int16s array: groups of 4 (type, value, min, max)
            // type: -31999=Highlights, -31998=Shadows, 0=plain
            let vals = read_i16_array(entry.data, be);
            let mut parts = Vec::new();
            for v in &vals {
                let s = match *v {
                    -31999 => "Highlights".to_string(),
                    -31998 => "Shadows".to_string(),
                    -31997 => "Midtones".to_string(),
                    other => format!("{other}"),
                };
                parts.push(s);
            }
            parts.join("; ")
        }
        "AFPointSelected" => {
            // 5 x rational (SRATIONAL = int32s pairs): first is selector, then 2 (x,y) pairs
            // Each rational is 8 bytes (num:i32 + den:i32), 5 rationals = 40 bytes
            if entry.data.len() >= 40 {
                let read_i32 = |off: usize| -> i32 {
                    if be {
                        i32::from_be_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    } else {
                        i32::from_le_bytes([
                            entry.data[off],
                            entry.data[off + 1],
                            entry.data[off + 2],
                            entry.data[off + 3],
                        ])
                    }
                };
                // Rationals: [selector, x1, y1, x2, y2] - each 8 bytes
                // Skip first rational (selector), format remaining as (x%,y%) pairs
                let mut parts = Vec::new();
                for i in 0..2 {
                    let base = 8 + i * 16; // skip 8 bytes (selector), each pair is 16 bytes
                    let x_num = read_i32(base) as f64;
                    let x_den = read_i32(base + 4) as f64;
                    let y_num = read_i32(base + 8) as f64;
                    let y_den = read_i32(base + 12) as f64;
                    let xp = if x_den != 0.0 {
                        (x_num * 100.0 / x_den + 0.5) as i32
                    } else {
                        0
                    };
                    let yp = if y_den != 0.0 {
                        (y_num * 100.0 / y_den + 0.5) as i32
                    } else {
                        0
                    };
                    parts.push(format!("({xp}%,{yp}%)"));
                }
                parts.join(" ")
            } else {
                format_ifd_value(entry, be)
            }
        }
        _ => format_ifd_value(entry, be),
    }
}

/// Format an Olympus ImageProcessing sub-IFD tag value
fn format_olympus_image_processing(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    match name {
        "ImageProcessingVersion" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        "NoiseReduction2" => {
            if let Some(v) = entry_u16(entry, be) {
                if v == 0 {
                    return "(none)".into();
                }
                let mut parts = Vec::new();
                if v & 0x01 != 0 {
                    parts.push("Noise Reduction");
                }
                if v & 0x02 != 0 {
                    parts.push("Noise Filter");
                }
                if v & 0x04 != 0 {
                    parts.push("Noise Filter (ISO Boost)");
                }
                if parts.is_empty() {
                    format!("{v}")
                } else {
                    parts.join(", ")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "DistortionCorrection2" | "ShadingCompensation2" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ColorMatrix" => {
            // Array of int16u but ExifTool renders them as int16s
            let count = entry.count as usize;
            let vals: Vec<String> = (0..count)
                .take_while(|&i| i * 2 + 2 <= entry.data.len())
                .map(|i| {
                    let v = if be {
                        i16::from_be_bytes([entry.data[i * 2], entry.data[i * 2 + 1]])
                    } else {
                        i16::from_le_bytes([entry.data[i * 2], entry.data[i * 2 + 1]])
                    };
                    format!("{v}")
                })
                .collect();
            vals.join(" ")
        }
        "MultipleExposureMode" => {
            let label = match entry_u16(entry, be) {
                Some(0) => "Off",
                Some(1) => "Live Composite",
                Some(2) => "On (2 frames)",
                Some(3) => "On (3 frames)",
                Some(v) => {
                    return {
                        if entry.data.len() >= 4 {
                            let v2 = if be {
                                u16::from_be_bytes([entry.data[2], entry.data[3]])
                            } else {
                                u16::from_le_bytes([entry.data[2], entry.data[3]])
                            };
                            format!("{v}; {v2}")
                        } else {
                            format!("{v}")
                        }
                    };
                }
                None => return format_ifd_value(entry, be),
            };
            if entry.data.len() >= 4 {
                let v2 = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                format!("{label}; {v2}")
            } else {
                label.into()
            }
        }
        "AspectRatio" => {
            // int8u[2]: first is type, second is ratio code
            if entry.data.len() >= 2 {
                let ratio = match entry.data[0] {
                    1 => "4:3",
                    2 => "3:2",
                    3 => "16:9",
                    4 => "6:6",
                    5 => "5:4",
                    6 => "7:6",
                    7 => "6:5",
                    8 => "7:5",
                    9 => "3:4",
                    _ => return format_ifd_value(entry, be),
                };
                ratio.into()
            } else {
                format_ifd_value(entry, be)
            }
        }
        "AspectFrame"
        | "FaceDetectFrameSize"
        | "FaceDetectFrameCrop"
        | "MaxFaces"
        | "SensorCalibration" => format_ifd_value(entry, be),
        _ => format_ifd_value(entry, be),
    }
}

/// Format an Olympus FocusInfo sub-IFD tag value
fn format_olympus_focus_info(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    match name {
        "FocusInfoVersion" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        "AutoFocus" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "FocusDistance" => {
            // Stored as rational but ExifTool ignores denominator (inconsistent between models)
            // Just read numerator as mm and convert to meters
            if entry.data.len() >= 4 {
                let num = if be {
                    u32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                } else {
                    u32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                };
                if num == 0xFFFFFFFF || num == 0 {
                    "inf".into()
                } else {
                    let m = num as f64 / 1000.0;
                    // Format with appropriate precision
                    let s = format!("{m:.3}");
                    let s = s.trim_end_matches('0');
                    let s = s.trim_end_matches('.');
                    format!("{s} m")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "SensorTemperature" => {
            if let Some(v) = entry_i16(entry, be) {
                if v == 0 || v == -32768 {
                    return "n/a".into();
                }
                // E-1/E-M5 style: count=1, raw value is direct temperature
                // Other models: count!=1 or count=1 with large values needing conversion
                if entry.count == 1 && v < 100 && v > -50 {
                    // Direct temperature (E-1, E-M5 style)
                    format!("{v} C")
                } else {
                    // Conversion formula for other models
                    let temp = 84.0 - 3.0 * v as f64 / 26.0;
                    format!("{temp:.1} C")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ZoomStepCount" | "FocusStepCount" | "FocusStepInfinity" | "FocusStepNear" => {
            if let Some(v) = entry_u16(entry, be) {
                format!("{v}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "AFPoint" => {
            if let Some(v) = entry_u16(entry, be) {
                match v {
                    0 => "Left (or n/a)".into(),
                    1 => "Center (horizontal)".into(),
                    2 => "Right".into(),
                    3 => "Center (vertical)".into(),
                    255 => "None".into(),
                    v => format!("{v}"),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ExternalFlash" | "InternalFlash" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ExternalFlashBounce" => match entry_u16(entry, be) {
            Some(0) => "Bounce or Off".into(),
            Some(1) => "Direct".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ManualFlash" => {
            // Count=2: first value 0=Off, 1=On; second value is flash strength
            let vals = read_u16_array(entry.data, be);
            if vals.is_empty() {
                return format_ifd_value(entry, be);
            }
            let a = vals[0];
            if a == 0 {
                "Off".into()
            } else {
                let b = vals.get(1).copied().unwrap_or(0);
                let strength = if b == 1 {
                    "Full".into()
                } else {
                    format!("1/{b}")
                };
                format!("On ({strength} strength)")
            }
        }
        "MacroLED" => match entry_u16(entry, be) {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ManometerPressure" => {
            if let Some(v) = entry_u16(entry, be) {
                let kpa = v as f64 / 10.0;
                format!("{kpa} kPa")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ManometerReading" => {
            // int32s[2]: meters*10, feet*10
            if entry.data.len() >= 8 {
                let m = if be {
                    i32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                } else {
                    i32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
                };
                let ft = if be {
                    i32::from_be_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                } else {
                    i32::from_le_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
                };
                let m_val = m as f64 / 10.0;
                let ft_val = ft as f64 / 10.0;
                // Format: remove trailing zeros
                let ms = format!("{m_val}");
                let fts = format!("{ft_val}");
                format!("{ms} m, {fts} ft")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "AFResult" => match entry_u16(entry, be) {
            Some(0) => "Not Ready".into(),
            Some(1) => "Ready".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "AFAreas" => {
            // 64 x int32u: each non-zero entry packs one AF area. The value is
            // read as an int32u in the file's byte order, then its big-endian
            // bytes are the coordinates (x1,y1,x2,y2).
            let count = entry.data.len() / 4;
            let mut parts = Vec::new();
            for i in 0..count {
                let off = i * 4;
                // Read as native byte order to get the u32 value
                let v = if be {
                    u32::from_be_bytes([
                        entry.data[off],
                        entry.data[off + 1],
                        entry.data[off + 2],
                        entry.data[off + 3],
                    ])
                } else {
                    u32::from_le_bytes([
                        entry.data[off],
                        entry.data[off + 1],
                        entry.data[off + 2],
                        entry.data[off + 3],
                    ])
                };
                if v == 0 {
                    continue;
                }
                // Coordinates are the u32's big-endian bytes, whatever the
                // file's byte order was.
                let bytes = v.to_be_bytes();
                let label = match v {
                    0x36794285 => Some("Left"),
                    0x79798585 => Some("Center"),
                    0xBD79C985 => Some("Right"),
                    _ => None,
                };
                let coords = format!("({},{})-({},{})", bytes[0], bytes[1], bytes[2], bytes[3]);
                if let Some(name) = label {
                    parts.push(format!("{name} {coords}"));
                } else {
                    parts.push(coords);
                }
            }
            if parts.is_empty() {
                "none".into()
            } else {
                parts.join(", ")
            }
        }
        _ => format_ifd_value(entry, be),
    }
}

/// Format an Olympus RawDevelopment sub-IFD tag value
fn format_olympus_raw_development(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    match name {
        "RawDevVersion" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        "RawDevColorSpace" => match entry_u16(entry, be) {
            Some(0) => "sRGB".into(),
            Some(1) => "Adobe RGB".into(),
            Some(2) => "Pro Photo RGB".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "RawDevEngine" => match entry_u16(entry, be) {
            Some(0) => "High Speed".into(),
            Some(1) => "High Function".into(),
            Some(2) => "Advanced High Speed".into(),
            Some(3) => "Advanced High Function".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "RawDevNoiseReduction" => {
            if let Some(v) = entry_u16(entry, be) {
                if v == 0 {
                    return "(none)".into();
                }
                let mut parts = Vec::new();
                if v & 0x01 != 0 {
                    parts.push("Noise Reduction");
                }
                if v & 0x02 != 0 {
                    parts.push("Noise Filter");
                }
                if v & 0x04 != 0 {
                    parts.push("Noise Filter (ISO Boost)");
                }
                if parts.is_empty() {
                    format!("{v}")
                } else {
                    parts.join(", ")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "RawDevEditStatus" => match entry_u16(entry, be) {
            Some(0) => "Original".into(),
            Some(1) => "Edited (Olympus Viewer)".into(),
            Some(6) => "Edited (Silkypix)".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "RawDevSettings" => {
            if let Some(v) = entry_u16(entry, be) {
                if v == 0 {
                    return "(none)".into();
                }
                let mut parts = Vec::new();
                if v & 0x01 != 0 {
                    parts.push("WB Color Temp");
                }
                if v & 0x02 != 0 {
                    parts.push("WB Gray Point");
                }
                if v & 0x04 != 0 {
                    parts.push("Saturation");
                }
                if v & 0x08 != 0 {
                    parts.push("Contrast");
                }
                if v & 0x10 != 0 {
                    parts.push("Sharpness");
                }
                if v & 0x20 != 0 {
                    parts.push("Color Space");
                }
                if v & 0x40 != 0 {
                    parts.push("High Function");
                }
                if v & 0x80 != 0 {
                    parts.push("Noise Reduction");
                }
                if parts.is_empty() {
                    format!("{v}")
                } else {
                    parts.join(", ")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        _ => format_ifd_value(entry, be),
    }
}

/// Read a signed 16-bit integer from an IFD entry.
fn entry_i16(entry: &IfdEntry<'_>, be: bool) -> Option<i16> {
    if entry.data.len() < 2 {
        return None;
    }
    Some(if be {
        i16::from_be_bytes([entry.data[0], entry.data[1]])
    } else {
        i16::from_le_bytes([entry.data[0], entry.data[1]])
    })
}

/// Read a RATIONAL or SRATIONAL value from an IFD entry as f64.
fn olympus_read_rational(entry: &IfdEntry<'_>, be: bool) -> Option<f64> {
    if entry.data.len() < 8 {
        return None;
    }
    if entry.data_type == crate::tiff::DataType::SRational {
        let n = if be {
            i32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
        } else {
            i32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
        };
        let d = if be {
            i32::from_be_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
        } else {
            i32::from_le_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
        };
        if d == 0 {
            None
        } else {
            Some(n as f64 / d as f64)
        }
    } else {
        let n = if be {
            u32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
        } else {
            u32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
        };
        let d = if be {
            u32::from_be_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
        } else {
            u32::from_le_bytes([entry.data[4], entry.data[5], entry.data[6], entry.data[7]])
        };
        if d == 0 {
            None
        } else {
            Some(n as f64 / d as f64)
        }
    }
}

/// Format a float with approximately `sig` significant digits, trimming trailing zeros.
fn format_sig_digits(val: f64, sig: usize) -> String {
    if val == 0.0 {
        return "0".to_string();
    }
    let magnitude = val.abs().log10().floor() as i32;
    let decimal_places = (sig as i32 - magnitude - 1).max(0) as usize;
    let s = format!("{val:.decimal_places$}");
    // Trim trailing zeros
    if s.contains('.') {
        let s = s.trim_end_matches('0');
        s.strip_suffix('.').unwrap_or(s).to_string()
    } else {
        s
    }
}

/// Format Nikon EV-encoded 3-4 byte value: byte[0] (signed) / byte[2]
fn nikon_ev_format(data: &[u8], _decimal: bool) -> String {
    if data.len() >= 3 && data[2] != 0 {
        let val = data[0] as i8 as f64 / data[2] as f64;
        if val == 0.0 {
            "0".into()
        } else {
            format!("{val}")
        }
    } else if !data.is_empty() {
        format!("{}", data[0] as i8)
    } else {
        "0".into()
    }
}

fn gcd_i32(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

/// Format unsigned RATIONAL values as space-separated decimals.
fn format_urational_decimals(data: &[u8], be: bool) -> Option<String> {
    if data.len() < 8 || data.len() % 8 != 0 {
        return None;
    }
    let count = data.len() / 8;
    let mut parts = Vec::with_capacity(count);
    for i in 0..count {
        let off = i * 8;
        let num = if be {
            u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        };
        let den = if be {
            u32::from_be_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
        } else {
            u32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
        };
        if den == 0 {
            parts.push("0".to_string());
        } else {
            let val = num as f64 / den as f64;
            // Match ExifTool precision
            let s = format!("{val}");
            parts.push(s);
        }
    }
    Some(parts.join(" "))
}

/// Apply ExifTool's Nikon FormatString logic to a raw string entry.
/// Converts runs of uppercase letters containing vowels to title-case,
/// then patches known exceptions like "Raw" -> "RAW", "Af" -> "AF".
fn nikon_format_string(entry: &IfdEntry<'_>) -> Option<String> {
    let raw = entry_string(entry)?;
    let bytes = raw.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Find runs of uppercase ASCII letters
        if bytes[i].is_ascii_uppercase() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_uppercase() {
                i += 1;
            }
            let run = &raw[start..i];
            let has_vowel = run
                .chars()
                .any(|c| matches!(c, 'A' | 'E' | 'I' | 'O' | 'U' | 'Y'));
            if run.len() > 1 && has_vowel {
                // Title-case: keep first letter, lowercase rest
                result.push(bytes[start]);
                for &b in &bytes[start + 1..i] {
                    result.push(b.to_ascii_lowercase());
                }
            } else {
                result.extend_from_slice(&bytes[start..i]);
            }
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    let mut s = String::from_utf8(result).unwrap_or(raw);
    // Trim trailing whitespace
    let trimmed = s.trim_end().to_string();
    s = trimmed;
    // Patch known exceptions
    // "Af" at word boundary -> "AF"
    s = s.replace("Af-", "AF-"); // AF-S, AF-C, AF-A
    if s == "Af" {
        s = "AF".to_string();
    }
    s = s.replace(" Af ", " AF ").replace(" Af,", " AF,");
    if s.starts_with("Af ") {
        s = format!("AF {}", &s[3..]);
    }
    if s.ends_with(" Af") {
        let l = s.len();
        s.replace_range(l - 2.., "AF");
    }
    // "Raw" at word boundary -> "RAW"
    if s == "Raw" {
        s = "RAW".to_string();
    }
    if s.contains("Raw") {
        // Only at word boundaries (before non-lowercase or end)
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        let mut buf = String::new();
        while let Some(c) = chars.next() {
            buf.push(c);
            if buf.ends_with("Raw") {
                let next = chars.peek();
                let at_boundary = next.is_none() || !next.unwrap().is_ascii_lowercase();
                if at_boundary {
                    let prefix_len = buf.len() - 3;
                    out.push_str(&buf[..prefix_len]);
                    out.push_str("RAW");
                    buf.clear();
                }
            }
        }
        out.push_str(&buf);
        s = out;
    }
    // Remove trailing garbage after VR-OFF/VR-ON
    if s.contains("Vr-") {
        if let Some(pos) = s.find("Vr-off") {
            s = s[..pos + 6].trim_end().to_string();
        } else if let Some(pos) = s.find("Vr-on") {
            s = s[..pos + 5].trim_end().to_string();
        }
    }
    Some(s)
}

/// Format a distance in meters. Show integer when whole, else "X.XX m".
fn format_distance_m(m: f64) -> String {
    if m == 0.0 {
        "0 m".into()
    } else if m == m.floor() {
        format!("{} m", m as u32)
    } else {
        let s = format!("{m:.2}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        format!("{s} m")
    }
}

/// Read a 16-bit signed value from a byte slice.
fn read_i16(data: &[u8], index: usize, big_endian: bool) -> Option<i16> {
    let off = index * 2;
    if off + 2 > data.len() {
        return None;
    }
    Some(if big_endian {
        i16::from_be_bytes([data[off], data[off + 1]])
    } else {
        i16::from_le_bytes([data[off], data[off + 1]])
    })
}

/// Read a 16-bit unsigned value from a byte slice.
fn read_u16(data: &[u8], index: usize, big_endian: bool) -> Option<u16> {
    read_i16(data, index, big_endian).map(|v| v as u16)
}

// -- Canon sub-array decoding ------------------------------------------

/// Canon EV conversion: convert Canon's internal EV encoding to a real value.
/// Handles 1/3 and 2/3 EV correction codes in the low 5 bits.
fn canon_ev(val: i16) -> f64 {
    let sign: f64 = if val < 0 { -1.0 } else { 1.0 };
    let abs_val = val.unsigned_abs();
    let frac = abs_val & 0x1F;
    let base = abs_val - frac;
    let frac_adj: f64 = match frac {
        0x0C => 32.0 / 3.0, // 1/3 EV
        0x14 => 64.0 / 3.0, // 2/3 EV
        _ => frac as f64,
    };
    sign * (base as f64 + frac_adj) / 32.0
}

/// Format a Canon EV value as an f-stop using %.2g (2 significant digits).
fn canon_ev_aperture(val: i16) -> String {
    if val == 0 {
        return "0".to_string();
    }
    let ev = canon_ev(val);
    let fstop = (ev * 2.0_f64.ln() / 2.0).exp();
    format_sig_digits(fstop, 2)
}

/// Format a Canon EV value as exposure time.
fn canon_ev_time(val: i16) -> String {
    if val == 0 {
        return "0".to_string();
    }
    let ev = canon_ev(val);
    let t = (-ev * 2.0_f64.ln()).exp();
    if t < 0.25 && t > 0.0 {
        let recip = (1.0 / t).round() as u32;
        format!("1/{recip}")
    } else if t >= 0.25 {
        format!("{t:.1}")
    } else {
        format!("{t}")
    }
}

/// Format a Canon EV value as exposure compensation.
fn canon_ev_comp(val: i16) -> String {
    if val == 0 {
        return "0".to_string();
    }
    let ev = canon_ev(val);
    if (ev - ev.round()).abs() < 0.01 {
        format!("{}", ev.round() as i32)
    } else {
        format!("{ev:+.2}")
    }
}

/// Format a focal length in mm. Shows integer when whole, otherwise full precision.
fn format_focal_mm(mm: f64) -> String {
    if mm == mm.floor() {
        format!("{} mm", mm as u32)
    } else {
        // Remove trailing zeros but keep meaningful decimals
        let s = format!("{mm} mm");
        s
    }
}

/// Decode Canon CameraInfo (tag 0x000D).
/// Model-specific binary structure. Falls back to Unknown32 (int32s) for CameraTemperature.
fn decode_canon_camera_info(
    entry: &crate::tiff::IfdEntry<'_>,
    be: bool,
    model: &str,
    tags: &mut Vec<DecodedTag>,
) {
    let data = entry.data;
    if data.is_empty() {
        return;
    }

    // Check if int32 format (raw_type 4=Long or 9=SLong) -> CameraInfoUnknown32
    let is_int32 = entry.raw_type == 4 || entry.raw_type == 9;

    if is_int32 {
        // CameraInfoUnknown32: CameraTemperature at known indices based on count
        let count = data.len() / 4;
        let get32s = |idx: usize| -> i32 {
            let off = idx * 4;
            if off + 4 > data.len() {
                return 0;
            }
            if be {
                i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            } else {
                i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            }
        };
        let temp_idx = match count {
            72 => Some(71),              // S1
            85 => Some(83),              // S2
            93 | 94 => Some(91),         // A410, A610, etc.
            96 => Some(92),              // S3
            104 => Some(100),            // A420, A430, etc.
            c if c > 400 => Some(c - 3), // Most newer models
            _ => None,
        };
        if let Some(idx) = temp_idx {
            let temp = get32s(idx);
            tags.push(DecodedTag {
                name: "CameraTemperature".to_string(),
                value: format!("{temp} C"),
            });
        }
        return;
    }

    // int8u format: model-specific CameraInfo
    // CameraTemperature: raw_byte - 128, displayed as "N C"
    let temp_offset: Option<usize>;
    let fw_offset: Option<usize>;
    let lens_type_offset: Option<usize>;
    let ps_info_offset: Option<usize>; // PictureStyleInfo sub-directory offset
    let ps_info_v2: bool; // true = PSInfo2 (has Auto style, shifted UserDef offsets)
    let file_index_offset: Option<usize>; // int32u, value + 1
    let dir_index_offset: Option<usize>; // int32u, value - 1
    let orientation_offset: Option<usize>; // int8u, 0=horiz, 1=90CW, 2=270CW

    if model.contains("650D") || model.contains("REBEL T4i") || model.contains("Kiss X6i") {
        temp_offset = Some(0x1b);
        lens_type_offset = Some(0x127);
        fw_offset = Some(0x21b);
        ps_info_offset = Some(0x390);
        ps_info_v2 = true;
        file_index_offset = Some(0x270);
        dir_index_offset = Some(0x27c);
        orientation_offset = Some(0x7d);
    } else if model.contains("700D") || model.contains("REBEL T5i") || model.contains("Kiss X7i") {
        temp_offset = Some(0x1b);
        lens_type_offset = Some(0x127);
        fw_offset = Some(0x220);
        ps_info_offset = Some(0x390);
        ps_info_v2 = true;
        file_index_offset = Some(0x270);
        dir_index_offset = Some(0x27c);
        orientation_offset = Some(0x7d);
    } else if model.contains("1D Mark III")
        || model.contains("1Ds Mark III")
        || model.contains("1DmkIII")
        || model.contains("1DSmkIII")
    {
        temp_offset = Some(0x18);
        lens_type_offset = Some(0x111);
        fw_offset = Some(0x136);
        ps_info_offset = Some(0x2aa);
        ps_info_v2 = false;
        file_index_offset = Some(0x172);
        dir_index_offset = Some(0x17e);
        orientation_offset = Some(0x30);
    } else if model.contains("1D Mark IV") || model.contains("1DmkIV") {
        temp_offset = Some(0x1e);
        lens_type_offset = Some(0x11c);
        fw_offset = Some(0x190);
        ps_info_offset = Some(0x368);
        ps_info_v2 = false;
        file_index_offset = Some(0x22c);
        dir_index_offset = Some(0x238);
        orientation_offset = Some(0x35);
    } else if model.contains("5D Mark II") || model.contains("5DmkII") {
        temp_offset = Some(0x1e);
        lens_type_offset = Some(0x116);
        fw_offset = Some(0x190);
        ps_info_offset = Some(0x2f7);
        ps_info_v2 = false;
        file_index_offset = Some(0x1bb);
        dir_index_offset = Some(0x1c7);
        orientation_offset = Some(0x31);
    } else if model.contains("5D Mark III") || model.contains("5DmkIII") {
        temp_offset = Some(0x1b);
        lens_type_offset = Some(0x13e);
        fw_offset = Some(0x22c);
        ps_info_offset = Some(0x3b0);
        ps_info_v2 = true;
        file_index_offset = Some(0x28c);
        dir_index_offset = Some(0x298);
        orientation_offset = Some(0x7d);
    } else if model.contains("6D") && !model.contains("6D Mark II") {
        temp_offset = Some(0x1b);
        lens_type_offset = Some(0x13e);
        fw_offset = Some(0x22c);
        ps_info_offset = Some(0x3c6);
        ps_info_v2 = true;
        file_index_offset = Some(0x2aa);
        dir_index_offset = Some(0x2b6);
        orientation_offset = Some(0x83);
    } else if model.contains("7D") && !model.contains("7D Mark II") {
        temp_offset = Some(0x1e);
        lens_type_offset = Some(0x119);
        fw_offset = Some(0x1d0);
        ps_info_offset = Some(0x327);
        ps_info_v2 = false;
        file_index_offset = Some(0x1eb);
        dir_index_offset = Some(0x1f7);
        orientation_offset = Some(0x35);
    } else if model.contains("70D") {
        temp_offset = Some(0x1b);
        lens_type_offset = Some(0x127);
        fw_offset = Some(0x220);
        ps_info_offset = Some(0x3cf);
        ps_info_v2 = true;
        file_index_offset = Some(0x2b3);
        dir_index_offset = Some(0x2bf);
        orientation_offset = Some(0x84);
    } else if model.contains("40D") {
        temp_offset = Some(0x18);
        lens_type_offset = Some(0xd3);
        fw_offset = Some(0x107);
        ps_info_offset = Some(0x25b);
        ps_info_v2 = false;
        file_index_offset = Some(0xd0);
        dir_index_offset = Some(0x133);
        orientation_offset = Some(0x30);
    } else if model.contains("50D") {
        temp_offset = Some(0x1e);
        lens_type_offset = Some(0xf6);
        fw_offset = Some(0x148);
        ps_info_offset = Some(0x2d7);
        ps_info_v2 = false;
        file_index_offset = Some(0x19b);
        dir_index_offset = Some(0x1a7);
        orientation_offset = Some(0x31);
    } else if model.contains("60D") {
        temp_offset = Some(0x1e);
        lens_type_offset = Some(0x116);
        fw_offset = Some(0x1b4);
        ps_info_offset = Some(0x321);
        ps_info_v2 = true;
        file_index_offset = Some(0x1d9);
        dir_index_offset = Some(0x1e5);
        orientation_offset = Some(0x36);
    } else if model.contains("450D") || model.contains("REBEL XSi") || model.contains("Kiss X2") {
        temp_offset = Some(0x18);
        lens_type_offset = Some(0xd6);
        fw_offset = Some(0x10b);
        ps_info_offset = Some(0x263);
        ps_info_v2 = false;
        file_index_offset = Some(0x13f);
        dir_index_offset = Some(0x133);
        orientation_offset = Some(0x30);
    } else if model.contains("500D") || model.contains("REBEL T1i") || model.contains("Kiss X3") {
        temp_offset = Some(0x1e);
        lens_type_offset = Some(0xf6);
        fw_offset = Some(0x14b);
        ps_info_offset = Some(0x30b);
        ps_info_v2 = false;
        file_index_offset = Some(0x1d3);
        dir_index_offset = Some(0x1df);
        orientation_offset = Some(0x31);
    } else if model.contains("550D") || model.contains("REBEL T2i") || model.contains("Kiss X4") {
        temp_offset = Some(0x1e);
        lens_type_offset = Some(0x114);
        fw_offset = Some(0x178);
        ps_info_offset = Some(0x31c);
        ps_info_v2 = false;
        file_index_offset = Some(0x1e4);
        dir_index_offset = Some(0x1f0);
        orientation_offset = Some(0x35);
    } else if model.contains("600D") || model.contains("REBEL T3i") || model.contains("Kiss X5") {
        temp_offset = Some(0x1e);
        lens_type_offset = Some(0x116);
        fw_offset = Some(0x1b0);
        ps_info_offset = Some(0x2fb);
        ps_info_v2 = true;
        file_index_offset = Some(0x1db);
        dir_index_offset = Some(0x1e7);
        orientation_offset = Some(0x38);
    } else if model.contains("1000D") || model.contains("REBEL XS") || model.contains("Kiss F") {
        temp_offset = Some(0x18);
        lens_type_offset = Some(0xd4);
        fw_offset = Some(0x109);
        ps_info_offset = Some(0x267);
        ps_info_v2 = false;
        file_index_offset = Some(0x143);
        dir_index_offset = Some(0x137);
        orientation_offset = Some(0x30);
    } else if model.contains("1100D") || model.contains("REBEL T3") || model.contains("Kiss X50") {
        temp_offset = Some(0x1e);
        lens_type_offset = Some(0x116);
        fw_offset = Some(0x17c);
        ps_info_offset = Some(0x2f9);
        ps_info_v2 = true;
        // Shares CameraInfo600D layout
        file_index_offset = Some(0x1db);
        dir_index_offset = Some(0x1e7);
        orientation_offset = Some(0x38);
    } else if model.contains("1D X") && !model.contains("1D X Mark II") {
        temp_offset = Some(0x1b);
        lens_type_offset = Some(0x13e);
        fw_offset = Some(0x270);
        ps_info_offset = Some(0x3f4);
        ps_info_v2 = true;
        file_index_offset = Some(0x2d0);
        dir_index_offset = Some(0x2dc);
        orientation_offset = Some(0x7d);
    } else {
        // Unknown model - skip
        return;
    }

    // Extract CameraTemperature (int8u - 128)
    if let Some(off) = temp_offset {
        if let Some(&raw) = data.get(off) {
            let temp = raw as i32 - 128;
            tags.push(DecodedTag {
                name: "CameraTemperature".to_string(),
                value: format!("{temp} C"),
            });
        }
    }

    // Extract FirmwareVersion (string[6])
    if let Some(off) = fw_offset {
        if data.len() > off + 6 {
            let fw_bytes = &data[off..off + 6];
            let fw = std::str::from_utf8(fw_bytes)
                .unwrap_or("")
                .trim_end_matches('\0');
            if !fw.is_empty() && fw.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                tags.push(DecodedTag {
                    name: "FirmwareVersion".to_string(),
                    value: fw.to_string(),
                });
            }
        }
    }

    // Extract LensType (int16u big-endian at offset)
    if let Some(off) = lens_type_offset {
        if data.len() > off + 1 {
            // int16uRev = big-endian regardless of file byte order
            let lens_id = u16::from_be_bytes([data[off], data[off + 1]]);
            if lens_id > 0 {
                if let Some(name) = canon_lens_name(lens_id) {
                    tags.push(DecodedTag {
                        name: "LensType".to_string(),
                        value: name.to_string(),
                    });
                }
            }
        }
    }

    // Extract FileIndex (int32u, displayed as raw + 1)
    if let Some(off) = file_index_offset {
        if off + 4 <= data.len() {
            let raw = if be {
                u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            } else {
                u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            };
            tags.push(DecodedTag {
                name: "FileIndex".into(),
                value: format!("{}", raw + 1),
            });
        }
    }

    // Extract DirectoryIndex (int32u, displayed as raw - 1)
    if let Some(off) = dir_index_offset {
        if off + 4 <= data.len() {
            let raw = if be {
                u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            } else {
                u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            };
            if raw > 0 {
                tags.push(DecodedTag {
                    name: "DirectoryIndex".into(),
                    value: format!("{}", raw - 1),
                });
            } else {
                tags.push(DecodedTag {
                    name: "DirectoryIndex".into(),
                    value: "0".into(),
                });
            }
        }
    }

    // Extract CameraOrientation
    if let Some(off) = orientation_offset {
        if let Some(&raw) = data.get(off) {
            tags.push(DecodedTag {
                name: "CameraOrientation".into(),
                value: match raw {
                    0 => "Horizontal (normal)".into(),
                    1 => "Rotate 90 CW".into(),
                    2 => "Rotate 270 CW".into(),
                    _ => format!("{raw}"),
                },
            });
        }
    }

    // Extract TimeStamp1 / TimeStamp for 1DmkIII / 1DSmkIII
    if model.contains("1D Mark III")
        || model.contains("1Ds Mark III")
        || model.contains("1DmkIII")
        || model.contains("1DSmkIII")
    {
        let get32u_at = |off: usize| -> Option<u32> {
            if off + 4 > data.len() {
                return None;
            }
            Some(if be {
                u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            } else {
                u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            })
        };
        // ShutterCount at offset 0x176 (int32u, val + 1)
        if let Some(sc) = get32u_at(0x176) {
            tags.push(DecodedTag {
                name: "ShutterCount".into(),
                value: format!("{}", sc + 1),
            });
        }
        // TimeStamp1 at offset 0x45a
        if let Some(ts) = get32u_at(0x45a) {
            if ts > 0 {
                let dt = unix_timestamp_to_string(ts as i64);
                tags.push(DecodedTag {
                    name: "TimeStamp1".into(),
                    value: dt,
                });
            }
        }
        // TimeStamp at offset 0x45e
        if let Some(ts) = get32u_at(0x45e) {
            if ts > 0 {
                let dt = unix_timestamp_to_string(ts as i64);
                tags.push(DecodedTag {
                    name: "TimeStamp".into(),
                    value: dt,
                });
            }
        }
    }

    // Extract PictureStyleInfo (PSInfo / PSInfo2)
    if let Some(ps_off) = ps_info_offset {
        decode_canon_ps_info(data, ps_off, ps_info_v2, be, tags);
    }
}

/// Decode Canon PictureStyleInfo sub-directory from CameraInfo binary data.
/// PSInfo: int32s values at 4-byte intervals. 0xDEADBEEF = "n/a".
/// PSInfo2 adds "Auto" style between Monochrome and UserDef, shifting offsets.
fn decode_canon_ps_info(data: &[u8], base: usize, v2: bool, be: bool, tags: &mut Vec<DecodedTag>) {
    let get32s = |off: usize| -> Option<i32> {
        let abs = base + off;
        if abs + 4 > data.len() {
            return None;
        }
        Some(if be {
            i32::from_be_bytes([data[abs], data[abs + 1], data[abs + 2], data[abs + 3]])
        } else {
            i32::from_le_bytes([data[abs], data[abs + 1], data[abs + 2], data[abs + 3]])
        })
    };
    let get16u = |off: usize| -> Option<u16> {
        let abs = base + off;
        if abs + 2 > data.len() {
            return None;
        }
        Some(if be {
            u16::from_be_bytes([data[abs], data[abs + 1]])
        } else {
            u16::from_le_bytes([data[abs], data[abs + 1]])
        })
    };

    let ps_val = |v: i32| -> String {
        if v as u32 == 0xDEADBEEF {
            "n/a".into()
        } else {
            format!("{v}")
        }
    };

    let filter_effect = |v: i32| -> String {
        match v as u32 {
            0xDEADBEEF => "n/a".into(),
            _ => match v {
                0 => "None".into(),
                1 => "Yellow".into(),
                2 => "Orange".into(),
                3 => "Red".into(),
                4 => "Green".into(),
                _ => format!("{v}"),
            },
        }
    };

    let toning_effect = |v: i32| -> String {
        match v as u32 {
            0xDEADBEEF => "n/a".into(),
            _ => match v {
                0 => "None".into(),
                1 => "Sepia".into(),
                2 => "Blue".into(),
                3 => "Purple".into(),
                4 => "Green".into(),
                _ => format!("{v}"),
            },
        }
    };

    // Shared layout for Standard/Portrait/Landscape/Neutral/Faithful/Monochrome (0x00-0x8c)
    // Each style: Contrast, Sharpness, Saturation, ColorTone, FilterEffect*, ToningEffect*
    // (* = Unknown/skipped for Standard-Faithful; printed for Monochrome)
    let styles: &[(&str, usize)] = &[
        ("Standard", 0x00),
        ("Portrait", 0x18),
        ("Landscape", 0x30),
        ("Neutral", 0x48),
        ("Faithful", 0x60),
        ("Monochrome", 0x78),
    ];

    for &(style, off) in styles {
        if let Some(v) = get32s(off) {
            tags.push(DecodedTag {
                name: format!("Contrast{style}"),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x04) {
            tags.push(DecodedTag {
                name: format!("Sharpness{style}"),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x08) {
            tags.push(DecodedTag {
                name: format!("Saturation{style}"),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x0c) {
            tags.push(DecodedTag {
                name: format!("ColorTone{style}"),
                value: ps_val(v),
            });
        }
        // FilterEffect/ToningEffect: only output for Monochrome (others are Unknown in ExifTool)
        if style == "Monochrome" {
            if let Some(v) = get32s(off + 0x10) {
                tags.push(DecodedTag {
                    name: "FilterEffectMonochrome".into(),
                    value: filter_effect(v),
                });
            }
            if let Some(v) = get32s(off + 0x14) {
                tags.push(DecodedTag {
                    name: "ToningEffectMonochrome".into(),
                    value: toning_effect(v),
                });
            }
        }
    }

    // Auto style (PSInfo2 only, at 0x90)
    if v2 {
        let off = 0x90;
        if let Some(v) = get32s(off) {
            tags.push(DecodedTag {
                name: "ContrastAuto".into(),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x04) {
            tags.push(DecodedTag {
                name: "SharpnessAuto".into(),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x08) {
            tags.push(DecodedTag {
                name: "SaturationAuto".into(),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x0c) {
            tags.push(DecodedTag {
                name: "ColorToneAuto".into(),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x10) {
            tags.push(DecodedTag {
                name: "FilterEffectAuto".into(),
                value: filter_effect(v),
            });
        }
        if let Some(v) = get32s(off + 0x14) {
            tags.push(DecodedTag {
                name: "ToningEffectAuto".into(),
                value: toning_effect(v),
            });
        }
    }

    // UserDef1-3: offset depends on PSInfo vs PSInfo2
    let ud_base = if v2 { 0xa8 } else { 0x90 };
    for i in 0..3u8 {
        let n = i + 1;
        let off = ud_base + (i as usize) * 0x18;
        if let Some(v) = get32s(off) {
            tags.push(DecodedTag {
                name: format!("ContrastUserDef{n}"),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x04) {
            tags.push(DecodedTag {
                name: format!("SharpnessUserDef{n}"),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x08) {
            tags.push(DecodedTag {
                name: format!("SaturationUserDef{n}"),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x0c) {
            tags.push(DecodedTag {
                name: format!("ColorToneUserDef{n}"),
                value: ps_val(v),
            });
        }
        if let Some(v) = get32s(off + 0x10) {
            tags.push(DecodedTag {
                name: format!("FilterEffectUserDef{n}"),
                value: filter_effect(v),
            });
        }
        if let Some(v) = get32s(off + 0x14) {
            tags.push(DecodedTag {
                name: format!("ToningEffectUserDef{n}"),
                value: toning_effect(v),
            });
        }
    }

    // UserDefPictureStyle: int16u at end
    let ud_style_base = if v2 { 0xf0 } else { 0xd8 };
    let user_def_style = |v: u16| -> String {
        match v {
            0x41 => "PC 1".into(),
            0x42 => "PC 2".into(),
            0x43 => "PC 3".into(),
            0x81 => "Standard".into(),
            0x82 => "Portrait".into(),
            0x83 => "Landscape".into(),
            0x84 => "Neutral".into(),
            0x85 => "Faithful".into(),
            0x86 => "Monochrome".into(),
            0x87 => "Auto".into(),
            _ => format!("{v}"),
        }
    };
    for i in 0..3u8 {
        let n = i + 1;
        if let Some(v) = get16u(ud_style_base + (i as usize) * 2) {
            tags.push(DecodedTag {
                name: format!("UserDef{n}PictureStyle"),
                value: user_def_style(v),
            });
        }
    }
}

/// Decode Canon ColorBalance (tag 0x00a9) - older cameras (10D, 300D).
/// FORMAT=int16s, FIRST_ENTRY=0. WB RGGB levels at known array indices.
/// Decode Canon SensorInfo (tag 0x00E0) - sensor dimensions and borders.
/// FORMAT=int16s, FIRST_ENTRY=1.
fn decode_canon_sensor_info(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let get = |idx: usize| -> Option<i16> { read_i16(data, idx, be) };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };
    if let Some(v) = get(1) {
        push(tags, "SensorWidth", format!("{v}"));
    }
    if let Some(v) = get(2) {
        push(tags, "SensorHeight", format!("{v}"));
    }
    if let Some(v) = get(5) {
        push(tags, "SensorLeftBorder", format!("{v}"));
    }
    if let Some(v) = get(6) {
        push(tags, "SensorTopBorder", format!("{v}"));
    }
    if let Some(v) = get(7) {
        push(tags, "SensorRightBorder", format!("{v}"));
    }
    if let Some(v) = get(8) {
        push(tags, "SensorBottomBorder", format!("{v}"));
    }
    if let Some(v) = get(9) {
        push(tags, "BlackMaskLeftBorder", format!("{v}"));
    }
    if let Some(v) = get(10) {
        push(tags, "BlackMaskTopBorder", format!("{v}"));
    }
    if let Some(v) = get(11) {
        push(tags, "BlackMaskRightBorder", format!("{v}"));
    }
    if let Some(v) = get(12) {
        push(tags, "BlackMaskBottomBorder", format!("{v}"));
    }
}

/// Decode Canon ColorBalance (tag 0x00a9) - older cameras (10D, 300D).
/// FORMAT=int16s, FIRST_ENTRY=0. WB RGGB levels at known array indices.
fn decode_canon_color_balance(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let get_rggb = |idx: usize| -> Option<String> {
        let off = idx * 2;
        if off + 8 > data.len() {
            return None;
        }
        let r = if be {
            i16::from_be_bytes([data[off], data[off + 1]])
        } else {
            i16::from_le_bytes([data[off], data[off + 1]])
        };
        let g1 = if be {
            i16::from_be_bytes([data[off + 2], data[off + 3]])
        } else {
            i16::from_le_bytes([data[off + 2], data[off + 3]])
        };
        let g2 = if be {
            i16::from_be_bytes([data[off + 4], data[off + 5]])
        } else {
            i16::from_le_bytes([data[off + 4], data[off + 5]])
        };
        let b = if be {
            i16::from_be_bytes([data[off + 6], data[off + 7]])
        } else {
            i16::from_le_bytes([data[off + 6], data[off + 7]])
        };
        Some(format!("{r} {g1} {g2} {b}"))
    };
    // Index 1: Auto, 5: Daylight, 9: Shade, 13: Cloudy, 17: Tungsten,
    // 21: Fluorescent, 25: Flash, 29: Custom, 33: Kelvin, 37: BlackLevels
    let entries: &[(usize, &str)] = &[
        (1, "WB_RGGBLevelsAuto"),
        (5, "WB_RGGBLevelsDaylight"),
        (9, "WB_RGGBLevelsShade"),
        (13, "WB_RGGBLevelsCloudy"),
        (17, "WB_RGGBLevelsTungsten"),
        (21, "WB_RGGBLevelsFluorescent"),
        (25, "WB_RGGBLevelsFlash"),
        (29, "WB_RGGBLevelsCustom"),
        (33, "WB_RGGBLevelsKelvin"),
        (37, "WB_RGGBBlackLevels"),
    ];
    for &(idx, name) in entries {
        if let Some(val) = get_rggb(idx) {
            tags.push(DecodedTag {
                name: name.to_string(),
                value: val,
            });
        }
    }
}

fn decode_canon_color_data(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let count = data.len() / 2;
    let get16s = |idx: usize| -> Option<i16> {
        let off = idx * 2;
        if off + 2 <= data.len() {
            Some(if be {
                i16::from_be_bytes([data[off], data[off + 1]])
            } else {
                i16::from_le_bytes([data[off], data[off + 1]])
            })
        } else {
            None
        }
    };
    let get_rggb = |idx: usize| -> Option<String> {
        let r = get16s(idx)?;
        let g1 = get16s(idx + 1)?;
        let g2 = get16s(idx + 2)?;
        let b = get16s(idx + 3)?;
        Some(format!("{r} {g1} {g2} {b}"))
    };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Determine version from count (number of int16u values)
    // ColorData4: 674, 692, 702, 1227, 1250, 1251, 1337, 1338, 1346
    // ColorCoefs sub-directory starts at index 0x3f (63), offsets relative to sub-dir start
    let is_data4 = matches!(
        count,
        674 | 692 | 702 | 1227 | 1250 | 1251 | 1337 | 1338 | 1346
    );
    // ColorData7: 1312, 1313, 1316, 1506
    // WB fields at direct offsets
    let is_data7 = matches!(count, 1312 | 1313 | 1316 | 1506);
    // ColorData6: 1273, 1275
    let is_data6 = matches!(count, 1273 | 1275);
    // ColorData8: 1560, 1592, 1353, 1602
    let is_data8 = matches!(count, 1560 | 1592 | 1353 | 1602);

    // WB field offsets: (name, rggb_offset, color_temp_offset)
    struct WbEntry {
        name: &'static str,
        rggb: usize,
        temp: usize,
    }

    let wb_entries: &[WbEntry] = &[
        WbEntry {
            name: "AsShot",
            rggb: 0,
            temp: 4,
        },
        WbEntry {
            name: "Auto",
            rggb: 5,
            temp: 9,
        },
        WbEntry {
            name: "Measured",
            rggb: 10,
            temp: 14,
        },
        WbEntry {
            name: "Daylight",
            rggb: 20,
            temp: 24,
        },
        WbEntry {
            name: "Shade",
            rggb: 25,
            temp: 29,
        },
        WbEntry {
            name: "Cloudy",
            rggb: 30,
            temp: 34,
        },
        WbEntry {
            name: "Tungsten",
            rggb: 35,
            temp: 39,
        },
        WbEntry {
            name: "Fluorescent",
            rggb: 40,
            temp: 44,
        },
        WbEntry {
            name: "Kelvin",
            rggb: 45,
            temp: 49,
        },
        WbEntry {
            name: "Flash",
            rggb: 50,
            temp: 54,
        },
    ];

    if is_data4 {
        // ColorCoefs sub-directory at index 0x3f (63)
        let base = 0x3f;
        for entry in wb_entries {
            if let Some(rggb) = get_rggb(base + entry.rggb) {
                push(tags, &format!("WB_RGGBLevels{}", entry.name), rggb);
            }
            if let Some(temp) = get16s(base + entry.temp) {
                push(tags, &format!("ColorTemp{}", entry.name), format!("{temp}"));
            }
        }
        // FlashBatteryLevel at index 0x26c (for count >= 674)
        if let Some(v) = get16s(0x26c) {
            if v != 0 {
                push(
                    tags,
                    "FlashBatteryLevel",
                    format!("{:.2}V", v as f64 * 5.0 / 186.0),
                );
            } else {
                push(tags, "FlashBatteryLevel", "n/a".into());
            }
        }
    } else if is_data7 {
        // Direct offsets in ColorData7
        let wb7: &[(usize, usize, &str)] = &[
            (0x3f, 0x43, "AsShot"),
            (0x44, 0x48, "Auto"),
            (0x49, 0x4d, "Measured"),
            (0x80, 0x84, "Daylight"),
            (0x85, 0x89, "Shade"),
            (0x8a, 0x8e, "Cloudy"),
            (0x8f, 0x93, "Tungsten"),
            (0x94, 0x98, "Fluorescent"),
            (0x99, 0x9d, "Kelvin"),
            (0x9e, 0xa2, "Flash"),
        ];
        for &(rggb_idx, temp_idx, name) in wb7 {
            if let Some(rggb) = get_rggb(rggb_idx) {
                push(tags, &format!("WB_RGGBLevels{name}"), rggb);
            }
            if let Some(temp) = get16s(temp_idx) {
                push(tags, &format!("ColorTemp{name}"), format!("{temp}"));
            }
        }
    } else if is_data6 {
        // ColorData6: direct offsets, Daylight+ at 0x67
        let wb6: &[(usize, usize, &str)] = &[
            (0x3f, 0x43, "AsShot"),
            (0x44, 0x48, "Auto"),
            (0x49, 0x4d, "Measured"),
            (0x67, 0x6b, "Daylight"),
            (0x6c, 0x70, "Shade"),
            (0x71, 0x75, "Cloudy"),
            (0x76, 0x7a, "Tungsten"),
            (0x7b, 0x7f, "Fluorescent"),
            (0x80, 0x84, "Kelvin"),
            (0x85, 0x89, "Flash"),
        ];
        for &(rggb_idx, temp_idx, name) in wb6 {
            if let Some(rggb) = get_rggb(rggb_idx) {
                push(tags, &format!("WB_RGGBLevels{name}"), rggb);
            }
            if let Some(temp) = get16s(temp_idx) {
                push(tags, &format!("ColorTemp{name}"), format!("{temp}"));
            }
        }
    } else if is_data8 {
        // ColorData8: same layout as ColorData7 but Daylight+ shifted to 0x85
        let wb8: &[(usize, usize, &str)] = &[
            (0x3f, 0x43, "AsShot"),
            (0x44, 0x48, "Auto"),
            (0x49, 0x4d, "Measured"),
            (0x85, 0x89, "Daylight"),
            (0x8a, 0x8e, "Shade"),
            (0x8f, 0x93, "Cloudy"),
            (0x94, 0x98, "Tungsten"),
            (0x99, 0x9d, "Fluorescent"),
            (0x9e, 0xa2, "Kelvin"),
            (0xa3, 0xa7, "Flash"),
        ];
        for &(rggb_idx, temp_idx, name) in wb8 {
            if let Some(rggb) = get_rggb(rggb_idx) {
                push(tags, &format!("WB_RGGBLevels{name}"), rggb);
            }
            if let Some(temp) = get16s(temp_idx) {
                push(tags, &format!("ColorTemp{name}"), format!("{temp}"));
            }
        }
    }
    // FlashOutput: exp((val-200)/16*ln(2)), displayed as percentage
    let flash_offset = if is_data4 {
        Some(0x26b)
    } else if is_data7 || is_data8 {
        Some(0x198)
    }
    // same as Data7
    else if is_data6 {
        Some(0x1a7)
    } else {
        None
    };
    if let Some(fo) = flash_offset {
        if let Some(raw) = get16s(fo) {
            let raw = raw as i32;
            if raw >= 255 {
                push(tags, "FlashOutput", "Strobe or Misfire".into());
            } else {
                let pct = ((raw as f64 - 200.0) / 16.0 * std::f64::consts::LN_2).exp() * 100.0;
                push(tags, "FlashOutput", format!("{pct:.0}%"));
            }
        }
    }
    // ColorDataVersion (index 0x00) - present in ColorData3+
    if is_data4 || is_data6 || is_data7 || is_data8 {
        if let Some(v) = get16s(0) {
            let desc = if is_data4 {
                match v {
                    2 => "2 (1DmkIII)".into(),
                    3 => "3 (40D)".into(),
                    4 => "4 (1DSmkIII)".into(),
                    5 => "5 (450D/1000D)".into(),
                    6 => "6 (50D/5DmkII)".into(),
                    7 => "7 (500D/550D/7D/1DmkIV)".into(),
                    9 => "9 (60D/1100D)".into(),
                    _ => format!("{v}"),
                }
            } else if is_data7 {
                match v {
                    10 => "10 (1DX/5DmkIII/6D/70D/100D/650D/700D/M/M2)".into(),
                    11 => "11 (7DmkII/750D/760D/8000D)".into(),
                    _ => format!("{v}"),
                }
            } else {
                format!("{v}")
            };
            push(tags, "ColorDataVersion", desc);
        }
    }

    // AverageBlackLevel (int16u[4]) at index 0x0e7 in ColorData4
    if is_data4 {
        let idx = 0x0e7;
        if idx + 3 < count {
            let get_u16 = |i: usize| -> u16 {
                let off = i * 2;
                if off + 2 <= data.len() {
                    if be {
                        u16::from_be_bytes([data[off], data[off + 1]])
                    } else {
                        u16::from_le_bytes([data[off], data[off + 1]])
                    }
                } else {
                    0
                }
            };
            let vals = format!(
                "{} {} {} {}",
                get_u16(idx),
                get_u16(idx + 1),
                get_u16(idx + 2),
                get_u16(idx + 3)
            );
            push(tags, "AverageBlackLevel", vals);
        }
    }

    // ColorData7 additional tags (version-dependent offsets)
    if is_data7 {
        let get_u16 = |i: usize| -> Option<u16> {
            let off = i * 2;
            if off + 2 <= data.len() {
                Some(if be {
                    u16::from_be_bytes([data[off], data[off + 1]])
                } else {
                    u16::from_le_bytes([data[off], data[off + 1]])
                })
            } else {
                None
            }
        };
        // Determine version for offset selection
        let version = get16s(0).unwrap_or(0);
        let (avg_bl, raw_rggb, pcbl, nwl, swl, lum, fbl) = if version == 10 {
            (
                0x114usize, 0x1adusize, 0x1f8usize, 0x1fcusize, 0x1fdusize, 0x1feusize, 0x199usize,
            )
        } else {
            // version 11 (7DmkII etc.)
            (
                0x146usize, 0x26busize, 0x2b0usize, 0x2b4usize, 0x2b5usize, 0x2b6usize, 0x199usize,
            )
        };
        // AverageBlackLevel (int16u[4])
        if avg_bl + 3 < count {
            if let (Some(a), Some(b), Some(c), Some(d)) = (
                get_u16(avg_bl),
                get_u16(avg_bl + 1),
                get_u16(avg_bl + 2),
                get_u16(avg_bl + 3),
            ) {
                push(tags, "AverageBlackLevel", format!("{a} {b} {c} {d}"));
            }
        }
        // FlashBatteryLevel
        if let Some(v) = get16s(fbl) {
            if v != 0 {
                push(
                    tags,
                    "FlashBatteryLevel",
                    format!("{:.2}V", v as f64 * 5.0 / 186.0),
                );
            } else {
                push(tags, "FlashBatteryLevel", "n/a".into());
            }
        }
        // RawMeasuredRGGB (int32u[4], word-swapped)
        {
            let byte_off = raw_rggb * 2;
            if byte_off + 16 <= data.len() {
                let get_swapped_u32 = |off: usize| -> u32 {
                    let raw = if be {
                        u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    } else {
                        u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    };
                    raw.rotate_left(16)
                };
                let vals = format!(
                    "{} {} {} {}",
                    get_swapped_u32(byte_off),
                    get_swapped_u32(byte_off + 4),
                    get_swapped_u32(byte_off + 8),
                    get_swapped_u32(byte_off + 12)
                );
                push(tags, "RawMeasuredRGGB", vals);
            }
        }
        // PerChannelBlackLevel (int16u[4])
        if pcbl + 3 < count {
            if let (Some(a), Some(b), Some(c), Some(d)) = (
                get_u16(pcbl),
                get_u16(pcbl + 1),
                get_u16(pcbl + 2),
                get_u16(pcbl + 3),
            ) {
                push(tags, "PerChannelBlackLevel", format!("{a} {b} {c} {d}"));
            }
        }
        // NormalWhiteLevel
        if let Some(v) = get_u16(nwl) {
            if v > 0 {
                push(tags, "NormalWhiteLevel", format!("{v}"));
            }
        }
        // SpecularWhiteLevel
        if let Some(v) = get_u16(swl) {
            push(tags, "SpecularWhiteLevel", format!("{v}"));
        }
        // LinearityUpperMargin
        if let Some(v) = get_u16(lum) {
            push(tags, "LinearityUpperMargin", format!("{v}"));
        }
    }

    // RawMeasuredRGGB (int32u[4], word-swapped) at effective index 0x280 in ColorData4
    if is_data4 {
        let byte_off = 0x280 * 2; // convert int16 index to byte offset
        if byte_off + 16 <= data.len() {
            // Read as int32u then swap the two 16-bit halves: (val >> 16) | (val << 16)
            let get_swapped_u32 = |off: usize| -> u32 {
                let raw = if be {
                    u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                } else {
                    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                };
                raw.rotate_left(16)
            };
            let vals = format!(
                "{} {} {} {}",
                get_swapped_u32(byte_off),
                get_swapped_u32(byte_off + 4),
                get_swapped_u32(byte_off + 8),
                get_swapped_u32(byte_off + 12)
            );
            push(tags, "RawMeasuredRGGB", vals);
        }
    }
}

fn decode_canon_camera_settings(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // Array indices start at 1 (FIRST_ENTRY = 1)
    let get = |idx: usize| -> Option<i16> { read_i16(data, idx, be) };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    if let Some(v) = get(1) {
        push(
            tags,
            "MacroMode",
            match v {
                1 => "Macro".into(),
                2 => "Normal".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(2) {
        push(
            tags,
            "SelfTimer",
            if v == 0 {
                "Off".into()
            } else {
                format!("{:.1} s", v as f64 / 10.0)
            },
        );
    }
    if let Some(v) = get(3) {
        push(
            tags,
            "Quality",
            match v {
                -1 => "n/a".into(),
                1 => "Economy".into(),
                2 => "Normal".into(),
                3 => "Fine".into(),
                4 => "RAW".into(),
                5 => "Superfine".into(),
                7 => "CRAW".into(),
                130 => "Normal Movie".into(),
                131 => "Movie (2)".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(4) {
        push(
            tags,
            "CanonFlashMode",
            match v {
                0 => "Off".into(),
                1 => "Auto".into(),
                2 => "On".into(),
                3 => "Red-eye reduction".into(),
                4 => "Slow-sync".into(),
                5 => "Red-eye reduction (Auto)".into(),
                6 => "Red-eye reduction (On)".into(),
                16 => "External flash".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(5) {
        push(
            tags,
            "ContinuousDrive",
            match v {
                0 => "Single".into(),
                1 => "Continuous".into(),
                2 => "Movie".into(),
                3 => "Continuous, Speed Priority".into(),
                4 => "Continuous, Low".into(),
                5 => "Continuous, High".into(),
                6 => "Silent Single".into(),
                9 => "Single, Silent".into(),
                10 => "Continuous, Silent".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(7) {
        push(
            tags,
            "FocusMode",
            match v {
                0 => "One-shot AF".into(),
                1 => "AI Servo AF".into(),
                2 => "AI Focus AF".into(),
                3 => "Manual Focus (3)".into(),
                4 => "Single".into(),
                5 => "Continuous".into(),
                6 => "Manual Focus (6)".into(),
                16 => "Pan Focus".into(),
                256 => "AF + MF".into(),
                512 => "Movie Snap Focus".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(9) {
        push(
            tags,
            "RecordMode",
            match v {
                1 => "JPEG".into(),
                2 => "CRW+THM".into(),
                3 => "AVI+THM".into(),
                4 => "TIF".into(),
                5 => "TIF+JPEG".into(),
                6 => "CR2".into(),
                7 => "CR2+JPEG".into(),
                9 => "MOV".into(),
                10 => "MP4".into(),
                11 => "CRM".into(),
                12 => "CR3".into(),
                13 => "CR3+JPEG".into(),
                14 => "HIF".into(),
                15 => "CR3+HIF".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(10) {
        push(
            tags,
            "CanonImageSize",
            match v {
                -1 => "n/a".into(),
                0 => "Large".into(),
                1 => "Medium".into(),
                2 => "Small".into(),
                5 => "Medium 1".into(),
                6 => "Medium 2".into(),
                7 => "Medium 3".into(),
                8 => "Postcard".into(),
                9 => "Widescreen".into(),
                10 => "Medium Widescreen".into(),
                14 => "Small 1".into(),
                15 => "Small 2".into(),
                16 => "Small 3".into(),
                128 => "640x480 Movie".into(),
                129 => "Medium Movie".into(),
                130 => "Small Movie".into(),
                137 => "1280x720 Movie".into(),
                142 => "1920x1080 Movie".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(11) {
        push(
            tags,
            "EasyMode",
            match v {
                0 => "Full auto".into(),
                1 => "Manual".into(),
                2 => "Landscape".into(),
                3 => "Fast shutter".into(),
                4 => "Slow shutter".into(),
                5 => "Night".into(),
                6 => "Gray Scale".into(),
                7 => "Sepia".into(),
                8 => "Portrait".into(),
                9 => "Sports".into(),
                10 => "Macro".into(),
                11 => "Black & White".into(),
                12 => "Pan focus".into(),
                13 => "Vivid".into(),
                14 => "Neutral".into(),
                15 => "Flash Off".into(),
                16 => "Long Shutter".into(),
                17 => "Super Macro".into(),
                18 => "Foliage".into(),
                19 => "Indoor".into(),
                20 => "Fireworks".into(),
                21 => "Beach".into(),
                22 => "Underwater".into(),
                23 => "Snow".into(),
                24 => "Kids & Pets".into(),
                25 => "Night SnapShot".into(),
                26 => "Digital Macro".into(),
                27 => "My Colors".into(),
                28 => "Movie Snap".into(),
                29 => "Super Macro 2".into(),
                30 => "Color Accent".into(),
                31 => "Color Swap".into(),
                32 => "Aquarium".into(),
                33 => "ISO 3200".into(),
                38 => "Creative Auto".into(),
                39 => "Zoom Blur".into(),
                40 => "Low Light".into(),
                41 => "Nostalgic".into(),
                42 => "Super Vivid".into(),
                43 => "Poster Effect".into(),
                44 => "Face Self-timer".into(),
                45 => "Smile".into(),
                46 => "Wink Self-timer".into(),
                47 => "Fisheye Effect".into(),
                48 => "Miniature Effect".into(),
                49 => "High-speed Burst".into(),
                50 => "Best Image Selection".into(),
                51 => "High Dynamic Range".into(),
                52 => "Handheld Night Scene".into(),
                53 => "Movie Digest".into(),
                54 => "Live View Control".into(),
                55 => "Discreet".into(),
                56 => "Blur Reduction".into(),
                57 => "Monochrome".into(),
                58 => "Toy Camera Effect".into(),
                59 => "Scene Intelligent Auto".into(),
                60 => "High-speed Burst HQ".into(),
                61 => "Smooth Skin".into(),
                62 => "Soft Focus".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(12) {
        push(
            tags,
            "DigitalZoom",
            match v {
                0 => "None".into(),
                1 => "2x".into(),
                2 => "4x".into(),
                3 => "Other".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(13) {
        push(
            tags,
            "Contrast",
            if v == 0x7FFF {
                "n/a".into()
            } else if v == 0 {
                "Normal".into()
            } else {
                format!("{v:+}")
            },
        );
    }
    if let Some(v) = get(14) {
        push(
            tags,
            "Saturation",
            if v == 0x7FFF {
                "n/a".into()
            } else if v == 0 {
                "Normal".into()
            } else {
                format!("{v:+}")
            },
        );
    }
    if let Some(v) = get(15) {
        // 0x7FFF means undefined (suppress, like ExifTool)
        if v != 0x7FFF {
            push(tags, "Sharpness", format!("{v}"));
        }
    }
    if let Some(v) = get(16) {
        push(
            tags,
            "CameraISO",
            if v == 0x7FFF || v == 0 {
                "n/a".into()
            }
            // Special encoding for PowerShot models
            else if (14..=17).contains(&v) {
                match v {
                    14 => "Auto High".into(),
                    15 => "Auto".into(),
                    16 => "50".into(),
                    17 => "100".into(),
                    _ => format!("{v}"),
                }
            } else {
                format!("{v}")
            },
        );
    }
    if let Some(v) = get(17) {
        push(
            tags,
            "MeteringMode",
            match v {
                0 => "Default".into(),
                1 => "Spot".into(),
                2 => "Average".into(),
                3 => "Evaluative".into(),
                4 => "Partial".into(),
                5 => "Center-weighted average".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(18) {
        push(
            tags,
            "FocusRange",
            match v {
                0 => "Manual".into(),
                1 => "Auto".into(),
                2 => "Not Known".into(),
                3 => "Macro".into(),
                4 => "Very Close".into(),
                5 => "Close".into(),
                6 => "Middle Range".into(),
                7 => "Far Range".into(),
                8 => "Pan Focus".into(),
                9 => "Super Macro".into(),
                10 => "Infinity".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(19) {
        let v = v as u16;
        push(
            tags,
            "AFPoint",
            match v {
                0x2005 => "Manual AF point selection".into(),
                0x3000 => "None (MF)".into(),
                0x3001 => "Auto AF point selection".into(),
                0x3002 => "Right".into(),
                0x3003 => "Center".into(),
                0x3004 => "Left".into(),
                0x4001 => "Auto AF point selection".into(),
                0x4006 => "Face Detect".into(),
                _ => format!("Unknown (0x{v:04X})"),
            },
        );
    }
    if let Some(v) = get(20) {
        push(
            tags,
            "CanonExposureMode",
            match v {
                0 => "Easy".into(),
                1 => "Program AE".into(),
                2 => "Shutter speed priority AE".into(),
                3 => "Aperture-priority AE".into(),
                4 => "Manual".into(),
                5 => "Depth-of-field AE".into(),
                6 => "M-Dep".into(),
                7 => "Bulb".into(),
                8 => "Flexible-priority AE".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(22) {
        let v = v as u16;
        if v == 0xFFFF {
            push(tags, "LensType", "n/a".into());
        } else if v == 0 {
            // ExifTool shows "n/a" for LensType=0 on compact cameras
            push(tags, "LensType", "n/a".into());
        } else {
            push(
                tags,
                "LensType",
                if let Some(name) = canon_lens_name(v) {
                    name.to_string()
                } else {
                    format!("Unknown ({v})")
                },
            );
        }
    }
    // MaxFocalLength, MinFocalLength, FocalUnits
    let focal_units = get(25).unwrap_or(1).max(1) as f64;
    if let Some(v) = get(23) {
        let mm = v as u16 as f64 / focal_units;
        push(tags, "MaxFocalLength", format_focal_mm(mm));
    }
    if let Some(v) = get(24) {
        let mm = v as u16 as f64 / focal_units;
        push(tags, "MinFocalLength", format_focal_mm(mm));
    }
    if let Some(v) = get(25) {
        push(tags, "FocalUnits", format!("{v}/mm"));
    }
    if let Some(v) = get(26) {
        push(tags, "MaxAperture", canon_ev_aperture(v));
    }
    if let Some(v) = get(27) {
        push(tags, "MinAperture", canon_ev_aperture(v));
    }
    if let Some(v) = get(28) {
        let v = (v as u16) & 0x7F;
        if v != 127 {
            push(
                tags,
                "FlashModel",
                match v {
                    0 => "n/a".into(),
                    4 => "Speedlite 540EZ".into(),
                    5 => "Speedlite 380EX".into(),
                    6 => "Speedlite 550EX".into(),
                    8 => "Speedlite ST-E2".into(),
                    9 => "Speedlite MR-14EX".into(),
                    12 => "Speedlite 580EX".into(),
                    13 => "Speedlite 430EX".into(),
                    17 => "Speedlite 580EX II".into(),
                    18 => "Speedlite 430EX II".into(),
                    22 => "Speedlite 600EX-RT".into(),
                    23 => "Speedlite 600EX II-RT".into(),
                    24 => "Speedlite 90EX".into(),
                    25 => "Speedlite 430EX III-RT".into(),
                    31 => "Speedlite EL-1 ver2".into(),
                    33 => "Speedlite EL-5".into(),
                    34 => "Speedlite EL-10".into(),
                    _ => format!("{v}"),
                },
            );
        }
    }
    if let Some(v) = get(29) {
        push(tags, "FlashBits", format_canon_flash_bits(v));
    }
    if let Some(v) = get(32) {
        if v != -1 {
            push(
                tags,
                "FocusContinuous",
                match v {
                    0 => "Single".into(),
                    1 => "Continuous".into(),
                    8 => "Manual".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
    if let Some(v) = get(33) {
        push(
            tags,
            "AESetting",
            match v {
                0 => "Normal AE".into(),
                1 => "Exposure Compensation".into(),
                2 => "AE Lock".into(),
                3 => "AE Lock + Exposure Comp.".into(),
                4 => "No AE".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(34) {
        if v != -1 {
            push(
                tags,
                "ImageStabilization",
                match v {
                    0 => "Off".into(),
                    1 => "On".into(),
                    2 => "On (2)".into(),
                    3 => "On (3)".into(),
                    4 => "On (4)".into(),
                    256 => "Off (2)".into(),
                    257 => "On (2)".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
    if let Some(v) = get(35) {
        if v != 0 {
            push(tags, "DisplayAperture", format!("{}", v as f64 / 10.0));
        }
    }
    if let Some(v) = get(36) {
        push(tags, "ZoomSourceWidth", format!("{v}"));
    }
    if let Some(v) = get(37) {
        push(tags, "ZoomTargetWidth", format!("{v}"));
    }
    if let Some(v) = get(39) {
        push(
            tags,
            "SpotMeteringMode",
            match v {
                0 => "Center".into(),
                1 => "AF Point".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(40) {
        push(
            tags,
            "PhotoEffect",
            match v {
                0 => "Off".into(),
                1 => "Vivid".into(),
                2 => "Neutral".into(),
                3 => "Smooth".into(),
                4 => "Sepia".into(),
                5 => "B&W".into(),
                6 => "Custom".into(),
                100 => "My Color Data".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(41) {
        push(
            tags,
            "ManualFlashOutput",
            match v {
                0 | 0x7FFF => "n/a".into(),
                0x500 => "Full".into(),
                0x502 => "Medium".into(),
                0x504 => "Low".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(42) {
        push(
            tags,
            "ColorTone",
            if v == 0x7FFF {
                "n/a".into()
            } else if v == 0 {
                "Normal".into()
            } else {
                format!("{v}")
            },
        );
    }
    if let Some(v) = get(46) {
        if v != -1 {
            push(
                tags,
                "SRAWQuality",
                match v {
                    0 => "n/a".into(),
                    1 => "sRAW1 (mRAW)".into(),
                    2 => "sRAW2 (sRAW)".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
}

fn format_canon_flash_bits(v: i16) -> String {
    let v = v as u16;
    if v == 0 {
        return "(none)".into();
    }
    let mut parts = Vec::new();
    if v & 0x0001 != 0 {
        parts.push("Manual");
    }
    if v & 0x0002 != 0 {
        parts.push("TTL");
    }
    if v & 0x0004 != 0 {
        parts.push("A-TTL");
    }
    if v & 0x0008 != 0 {
        parts.push("E-TTL");
    }
    if v & 0x0010 != 0 {
        parts.push("FP sync enabled");
    }
    if v & 0x0080 != 0 {
        parts.push("2nd-curtain sync");
    }
    if v & 0x0800 != 0 {
        parts.push("FP sync used");
    }
    if v & 0x2000 != 0 {
        parts.push("Built-in");
    }
    if v & 0x4000 != 0 {
        parts.push("External");
    }
    if parts.is_empty() {
        format!("0x{v:04X}")
    } else {
        parts.join(", ")
    }
}

fn decode_canon_focal_length(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // Array indices start at 0 (FIRST_ENTRY = 0)
    let get = |idx: usize| -> Option<u16> { read_u16(data, idx, be) };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    if let Some(v) = get(0) {
        push(
            tags,
            "FocalType",
            match v {
                1 => "Fixed".into(),
                2 => "Zoom".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(1) {
        // FocalLength in raw units; divide by FocalUnits if known (default 1)
        push(tags, "FocalLength", format!("{v} mm"));
    }
    if let Some(v) = get(2) {
        if v > 0 {
            let mm = v as f64 * 25.4 / 1000.0;
            push(tags, "FocalPlaneXSize", format!("{mm:.2} mm"));
        }
    }
    if let Some(v) = get(3) {
        if v > 0 {
            let mm = v as f64 * 25.4 / 1000.0;
            push(tags, "FocalPlaneYSize", format!("{mm:.2} mm"));
        }
    }
}

fn decode_canon_shot_info(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // Array indices start at 1 (FIRST_ENTRY = 1)
    let get = |idx: usize| -> Option<i16> { read_i16(data, idx, be) };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    if let Some(v) = get(1) {
        let iso = ((v as f64 / 32.0) * 2.0_f64.ln()).exp() * 100.0;
        push(tags, "AutoISO", format!("{}", iso.round() as u32));
    }
    if let Some(v) = get(2) {
        if v != 0 {
            let iso = ((v as f64 / 32.0) * 2.0_f64.ln()).exp() * 100.0 / 32.0;
            push(tags, "BaseISO", format!("{}", iso.round() as u32));
        }
    }
    if let Some(v) = get(3) {
        let ev = v as f64 / 32.0 + 5.0;
        push(tags, "MeasuredEV", format!("{ev:.2}"));
    }
    if let Some(v) = get(4) {
        if v > 0 {
            push(tags, "TargetAperture", canon_ev_aperture(v));
        }
    }
    if let Some(v) = get(5) {
        // Values <= -1000 are the "not available" fill (-32768 in practice),
        // not exposure times.
        if v != 0 && v > -1000 {
            push(tags, "TargetExposureTime", canon_ev_time(v));
        }
    }
    if let Some(v) = get(6) {
        push(tags, "ExposureCompensation", canon_ev_comp(v));
    }
    if let Some(v) = get(7) {
        push(
            tags,
            "WhiteBalance",
            match v {
                0 => "Auto".into(),
                1 => "Daylight".into(),
                2 => "Cloudy".into(),
                3 => "Tungsten".into(),
                4 => "Fluorescent".into(),
                5 => "Flash".into(),
                6 => "Custom".into(),
                7 => "Black & White".into(),
                8 => "Shade".into(),
                9 => "Manual Temperature (Kelvin)".into(),
                10 => "PC Set1".into(),
                11 => "PC Set2".into(),
                12 => "PC Set3".into(),
                14 => "Daylight Fluorescent".into(),
                15 => "Custom 1".into(),
                16 => "Custom 2".into(),
                17 => "Underwater".into(),
                18 => "Custom 3".into(),
                19 => "Custom 4".into(),
                20 => "PC Set4".into(),
                21 => "PC Set5".into(),
                23 => "Auto (ambience priority)".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(8) {
        push(
            tags,
            "SlowShutter",
            match v {
                -1 => "n/a".into(),
                0 => "Off".into(),
                1 => "Night Scene".into(),
                2 => "On".into(),
                3 => "None".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(9) {
        push(tags, "SequenceNumber", format!("{v}"));
    }
    if let Some(v) = get(10) {
        push(
            tags,
            "OpticalZoomCode",
            match v {
                8 => "n/a".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(12) {
        if v != 0 {
            push(tags, "CameraTemperature", format!("{} C", v - 128));
        }
    }
    if let Some(v) = get(13) {
        push(
            tags,
            "FlashGuideNumber",
            if v == -1 {
                "n/a".into()
            } else {
                format!("{:.1}", v as f64 / 32.0)
            },
        );
    }
    if let Some(v) = get(14) {
        let v = v as u16;
        if v != 0 {
            push(
                tags,
                "AFPointsInFocus",
                match v {
                    0x3000 => "None (MF)".into(),
                    0x3001 => "Right".into(),
                    0x3002 => "Center".into(),
                    0x3003 => "Center+Right".into(),
                    0x3004 => "Left".into(),
                    0x3005 => "Left+Right".into(),
                    0x3006 => "Left+Center".into(),
                    0x3007 => "All".into(),
                    _ => format!("0x{v:04X}"),
                },
            );
        }
    }
    if let Some(v) = get(15) {
        push(tags, "FlashExposureComp", canon_ev_comp(v));
    }
    if let Some(v) = get(16) {
        push(
            tags,
            "AutoExposureBracketing",
            match v {
                -1 => "On".into(),
                0 => "Off".into(),
                1 => "On (shot 1)".into(),
                2 => "On (shot 2)".into(),
                3 => "On (shot 3)".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(17) {
        push(tags, "AEBBracketValue", canon_ev_comp(v));
    }
    if let Some(v) = get(18) {
        push(
            tags,
            "ControlMode",
            match v {
                0 => "n/a".into(),
                1 => "Camera Local Control".into(),
                3 => "Computer Remote Control".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(19) {
        let v = v as u16;
        if v != 0 {
            let dist = v as f64 / 100.0;
            push(
                tags,
                "FocusDistanceUpper",
                if dist > 655.345 {
                    "inf".into()
                } else {
                    format_distance_m(dist)
                },
            );
        }
    }
    if let Some(upper) = get(19) {
        if upper != 0 {
            if let Some(v) = get(20) {
                let v = v as u16;
                push(
                    tags,
                    "FocusDistanceLower",
                    format_distance_m(v as f64 / 100.0),
                );
            }
        }
    }
    if let Some(v) = get(23) {
        if v != 0 {
            let ev = v as f64 / 8.0 - 6.0;
            // Use enough precision (v/8 gives at most 3 decimal places)
            let s = format!("{ev:.3}");
            let s = s.trim_end_matches('0');
            let s = s.strip_suffix('.').unwrap_or(s);
            push(tags, "MeasuredEV2", s.to_string());
        }
    }
    if let Some(v) = get(24) {
        push(tags, "BulbDuration", format!("{}", v as f64 / 10.0));
    }
    if let Some(v) = get(26) {
        push(
            tags,
            "CameraType",
            match v {
                0 => "n/a".into(),
                248 => "EOS High-end".into(),
                250 => "Compact".into(),
                252 => "EOS Mid-range".into(),
                255 => "DV Camera".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(27) {
        push(
            tags,
            "AutoRotate",
            match v {
                -1 => "n/a".into(),
                0 => "None".into(),
                1 => "Rotate 90 CW".into(),
                2 => "Rotate 180".into(),
                3 => "Rotate 270 CW".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(28) {
        push(
            tags,
            "NDFilter",
            match v {
                -1 => "n/a".into(),
                0 => "Off".into(),
                1 => "On".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(29) {
        push(tags, "SelfTimer2", format!("{v}"));
    }
    if let Some(v) = get(33) {
        push(tags, "FlashOutput", format!("{v}"));
    }
    if let Some(v) = get(41) {
        let v = v as u16;
        push(
            tags,
            "ManualFlashOutput",
            match v {
                0 => "n/a".into(),
                0x500 => "Full".into(),
                0x502 => "Medium".into(),
                0x504 => "Low".into(),
                0x7FFF => "n/a".into(),
                _ => format!("{v}"),
            },
        );
    }
}

/// Decode Canon AFInfo (tag 0x0012) - older format with single-value area widths/heights.
///
/// Layout: NumAFPoints(u16), ValidAFPoints(u16), CanonImageWidth(u16), CanonImageHeight(u16),
/// AFImageWidth(u16), AFImageHeight(u16), AFAreaWidth(u16), AFAreaHeight(u16),
/// then NumAFPoints × [AFAreaXPositions, AFAreaYPositions](i16),
/// then ceil(NumAFPoints/16) × AFPointsInFocus(i16) bitmask.
fn decode_canon_af_info(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    if data.len() < 16 {
        return;
    }

    let u = |off: usize| -> u16 {
        if be {
            u16::from_be_bytes([data[off], data[off + 1]])
        } else {
            u16::from_le_bytes([data[off], data[off + 1]])
        }
    };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    let num_af = u(0) as usize;
    push(tags, "NumAFPoints", format!("{num_af}"));

    let valid_af = u(2) as usize;
    push(tags, "ValidAFPoints", format!("{valid_af}"));

    push(tags, "CanonImageWidth", format!("{}", u(4)));
    push(tags, "CanonImageHeight", format!("{}", u(6)));
    push(tags, "AFImageWidth", format!("{}", u(8)));
    push(tags, "AFImageHeight", format!("{}", u(10)));
    push(tags, "AFAreaWidth", format!("{}", u(12)));
    push(tags, "AFAreaHeight", format!("{}", u(14)));

    // Variable-length arrays: each has num_af i16 values
    let arrays_start = 16;
    let array_size = num_af * 2;

    let x_off = arrays_start;
    let y_off = x_off + array_size;
    let focus_off = y_off + array_size;
    let bitmask_words = num_af.div_ceil(16);

    let min_len = focus_off + bitmask_words * 2;
    if data.len() < min_len || num_af == 0 {
        return;
    }

    let fmt_i16_array = |off: usize, count: usize| -> String {
        (0..count)
            .map(|i| {
                let o = off + i * 2;
                let v = if be {
                    i16::from_be_bytes([data[o], data[o + 1]])
                } else {
                    i16::from_le_bytes([data[o], data[o + 1]])
                };
                format!("{v}")
            })
            .collect::<Vec<_>>()
            .join(" ")
    };

    push(tags, "AFAreaXPositions", fmt_i16_array(x_off, num_af));
    push(tags, "AFAreaYPositions", fmt_i16_array(y_off, num_af));

    // Decode AFPointsInFocus bitmask
    let mut in_focus = Vec::new();
    for word_idx in 0..bitmask_words {
        let o = focus_off + word_idx * 2;
        let mask = if be {
            i16::from_be_bytes([data[o], data[o + 1]]) as u16
        } else {
            i16::from_le_bytes([data[o], data[o + 1]]) as u16
        };
        for bit in 0..16 {
            let point = word_idx * 16 + bit;
            if point >= num_af {
                break;
            }
            if mask & (1 << bit) != 0 {
                in_focus.push(format!("{point}"));
            }
        }
    }
    push(
        tags,
        "AFPointsInFocus",
        if in_focus.is_empty() {
            "(none)".into()
        } else {
            in_focus.join(",")
        },
    );

    // PrimaryAFPoint follows the bitmask (index 11 in serial layout)
    let primary_off = focus_off + bitmask_words * 2;
    if primary_off + 2 <= data.len() {
        let primary = u(primary_off);
        push(tags, "PrimaryAFPoint", format!("{primary}"));
    }
}

/// Decode Canon AFInfo2 (tag 0x0026) - sequential binary format with variable-length arrays.
///
/// Layout: AFInfoSize(u16), AFAreaMode(u16), NumAFPoints(u16), ValidAFPoints(u16),
/// Decode Canon FaceDetect1 (tag 0x0024) - binary data, FORMAT=int16u.
/// Index 2 = FacesDetected.
fn decode_canon_face_detect1(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let get = |idx: usize| -> Option<u16> { read_u16(data, idx, be) };
    if let Some(v) = get(2) {
        tags.push(DecodedTag {
            name: "FacesDetected".to_string(),
            value: format!("{v}"),
        });
    }
}

/// Decode Canon FaceDetect3 (tag 0x002f) - binary data, FORMAT=int16u.
/// Index 3 = FacesDetected -> byte offset 6.
fn decode_canon_face_detect3(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    if data.len() >= 8 {
        let v = if be {
            u16::from_be_bytes([data[6], data[7]])
        } else {
            u16::from_le_bytes([data[6], data[7]])
        };
        tags.push(DecodedTag {
            name: "FacesDetected".to_string(),
            value: format!("{v}"),
        });
    }
}

/// CanonImageWidth(u16), CanonImageHeight(u16), AFImageWidth(u16), AFImageHeight(u16),
/// then NumAFPoints × [AFAreaWidths, AFAreaHeights, AFAreaXPositions, AFAreaYPositions](i16),
/// then ceil(NumAFPoints/16) × AFPointsInFocus(i16) bitmask.
fn decode_canon_af_info2(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    if data.len() < 16 {
        return;
    }

    let u = |off: usize| -> u16 {
        if be {
            u16::from_be_bytes([data[off], data[off + 1]])
        } else {
            u16::from_le_bytes([data[off], data[off + 1]])
        }
    };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Index 1: AFAreaMode
    let af_area_mode = u(2);
    push(
        tags,
        "AFAreaMode",
        match af_area_mode {
            0 => "Off (Manual Focus)".into(),
            1 => "AF Point Expansion (surround)".into(),
            2 => "Single-point AF".into(),
            4 => "Auto".into(),
            5 => "Face Detect AF".into(),
            6 => "Face + Tracking".into(),
            7 => "Zone AF".into(),
            8 => "AF Point Expansion (4 point)".into(),
            9 => "Spot AF".into(),
            10 => "AF Point Expansion (8 point)".into(),
            11 => "Flexizone Multi (49 point)".into(),
            12 => "Flexizone Multi (9 point)".into(),
            13 => "Flexizone Single".into(),
            14 => "Large Zone AF".into(),
            v => format!("{v}"),
        },
    );

    // Index 2: NumAFPoints
    let num_af = u(4) as usize;
    push(tags, "NumAFPoints", format!("{num_af}"));

    // Index 3: ValidAFPoints
    let valid_af = u(6) as usize;
    push(tags, "ValidAFPoints", format!("{valid_af}"));

    // Index 4-7: Image and AF dimensions
    push(tags, "CanonImageWidth", format!("{}", u(8)));
    push(tags, "CanonImageHeight", format!("{}", u(10)));
    push(tags, "AFImageWidth", format!("{}", u(12)));
    push(tags, "AFImageHeight", format!("{}", u(14)));

    // Variable-length arrays: each has num_af i16 values
    let arrays_start = 16;
    let array_size = num_af * 2; // bytes per array

    // AFAreaWidths (index 8)
    let widths_off = arrays_start;
    // AFAreaHeights (index 9)
    let heights_off = widths_off + array_size;
    // AFAreaXPositions (index 10)
    let x_off = heights_off + array_size;
    // AFAreaYPositions (index 11)
    let y_off = x_off + array_size;
    // AFPointsInFocus bitmask (index 12)
    let focus_off = y_off + array_size;
    let bitmask_words = num_af.div_ceil(16);

    // Check we have enough data for all arrays
    let min_len = focus_off + bitmask_words * 2;
    if data.len() < min_len || num_af == 0 {
        return;
    }

    // Format arrays as space-separated values
    let fmt_i16_array = |off: usize, count: usize| -> String {
        (0..count)
            .map(|i| {
                let o = off + i * 2;
                let v = if be {
                    i16::from_be_bytes([data[o], data[o + 1]])
                } else {
                    i16::from_le_bytes([data[o], data[o + 1]])
                };
                format!("{v}")
            })
            .collect::<Vec<_>>()
            .join(" ")
    };

    push(tags, "AFAreaWidths", fmt_i16_array(widths_off, num_af));
    push(tags, "AFAreaHeights", fmt_i16_array(heights_off, num_af));
    push(tags, "AFAreaXPositions", fmt_i16_array(x_off, num_af));
    push(tags, "AFAreaYPositions", fmt_i16_array(y_off, num_af));

    // Decode AFPointsInFocus bitmask
    let mut in_focus = Vec::new();
    for word_idx in 0..bitmask_words {
        let o = focus_off + word_idx * 2;
        let mask = if be {
            i16::from_be_bytes([data[o], data[o + 1]]) as u16
        } else {
            i16::from_le_bytes([data[o], data[o + 1]]) as u16
        };
        for bit in 0..16 {
            let point = word_idx * 16 + bit;
            if point >= num_af {
                break;
            }
            if mask & (1 << bit) != 0 {
                in_focus.push(format!("{point}"));
            }
        }
    }
    push(
        tags,
        "AFPointsInFocus",
        if in_focus.is_empty() {
            "(none)".into()
        } else {
            in_focus.join(",")
        },
    );

    // AFPointsSelected bitmask (same format, right after AFPointsInFocus)
    let sel_off = focus_off + bitmask_words * 2;
    if data.len() >= sel_off + bitmask_words * 2 {
        let mut selected = Vec::new();
        for word_idx in 0..bitmask_words {
            let o = sel_off + word_idx * 2;
            let mask = if be {
                i16::from_be_bytes([data[o], data[o + 1]]) as u16
            } else {
                i16::from_le_bytes([data[o], data[o + 1]]) as u16
            };
            for bit in 0..16 {
                let point = word_idx * 16 + bit;
                if point >= num_af {
                    break;
                }
                if mask & (1 << bit) != 0 {
                    selected.push(format!("{point}"));
                }
            }
        }
        push(
            tags,
            "AFPointsSelected",
            if selected.is_empty() {
                "(none)".into()
            } else {
                selected.join(",")
            },
        );

        // PrimaryAFPoint follows AFPointsSelected.
        // For non-EOS cameras, AFPointsSelected has bitmask_words+1 entries,
        // then PrimaryAFPoint is the next u16.
        // For EOS cameras, AFPointsSelected has bitmask_words entries and no PrimaryAFPoint.
        // We try the non-EOS layout first.
        let primary_off = sel_off + (bitmask_words + 1) * 2;
        if primary_off + 2 <= data.len() {
            let primary = u(primary_off);
            push(tags, "PrimaryAFPoint", format!("{primary}"));
        }
    }
}

/// Decode Canon TimeInfo (tag 0x0035) - timezone information.
fn decode_canon_time_info(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let get = |idx: usize| -> Option<i32> { read_i32(data, idx, be) };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    if let Some(v) = get(1) {
        let hours = v / 60;
        let mins = (v % 60).abs();
        push(tags, "TimeZone", format!("{hours:+03}:{mins:02}"));
    }
    if let Some(v) = get(2) {
        push(tags, "TimeZoneCity", canon_timezone_city(v));
    }
    if let Some(v) = get(3) {
        push(
            tags,
            "DaylightSavings",
            match v {
                0 => "Off".into(),
                60 => "On".into(),
                _ => format!("{v}"),
            },
        );
    }
}

/// Read an i32 from a Canon sub-array at the given 1-based index.
fn canon_timezone_city(v: i32) -> String {
    match v {
        0 => "n/a".into(),
        1 => "Chatham Islands".into(),
        2 => "Wellington".into(),
        3 => "Solomon Islands".into(),
        4 => "Sydney".into(),
        5 => "Adelaide".into(),
        6 => "Tokyo".into(),
        7 => "Hong Kong".into(),
        8 => "Bangkok".into(),
        9 => "Yangon".into(),
        10 => "Dhaka".into(),
        11 => "Kathmandu".into(),
        12 => "Delhi".into(),
        13 => "Karachi".into(),
        14 => "Kabul".into(),
        15 => "Dubai".into(),
        16 => "Tehran".into(),
        17 => "Moscow".into(),
        18 => "Cairo".into(),
        19 => "Paris".into(),
        20 => "London".into(),
        21 => "Azores".into(),
        22 => "Fernando de Noronha".into(),
        23 => "Sao Paulo".into(),
        24 => "Newfoundland".into(),
        25 => "Santiago".into(),
        26 => "Caracas".into(),
        27 => "New York".into(),
        28 => "Chicago".into(),
        29 => "Denver".into(),
        30 => "Los Angeles".into(),
        31 => "Anchorage".into(),
        32 => "Honolulu".into(),
        33 => "Samoa".into(),
        32766 => "(not set)".into(),
        _ => format!("{v}"),
    }
}

fn read_i32(data: &[u8], idx: usize, be: bool) -> Option<i32> {
    let off = idx * 4;
    if off + 4 > data.len() {
        return None;
    }
    Some(if be {
        i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    } else {
        i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    })
}

/// Decode Canon FileInfo (tag 0x0093) - file-related metadata.
/// FORMAT is int16s, FIRST_ENTRY=1, but index 1 occupies 4 bytes (int32u).
fn decode_canon_file_info(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // Note: FORMAT=int16s, so indices are in 2-byte units from FIRST_ENTRY=1
    let get = |idx: usize| -> Option<i16> { read_i16(data, idx, be) };
    let getu = |idx: usize| -> Option<u16> { read_i16(data, idx, be).map(|v| v as u16) };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Index 3: BracketMode
    if let Some(v) = getu(3) {
        push(
            tags,
            "BracketMode",
            match v {
                0 => "Off".into(),
                1 => "AEB".into(),
                2 => "FEB".into(),
                3 => "ISO".into(),
                4 => "WB".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    // Index 4: BracketValue (EV compensation)
    if let Some(v) = get(4) {
        push(tags, "BracketValue", canon_ev_comp(v));
    }
    // Index 5: BracketShotNumber
    if let Some(v) = get(5) {
        push(tags, "BracketShotNumber", format!("{v}"));
    }
    // Index 6: RawJpgQuality (suppress if <=0)
    if let Some(v) = get(6) {
        if v > 0 {
            push(
                tags,
                "RawJpgQuality",
                match v {
                    1 => "Economy".into(),
                    2 => "Normal".into(),
                    3 => "Fine".into(),
                    4 => "RAW".into(),
                    5 => "Superfine".into(),
                    7 => "CRAW".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
    // Index 7: RawJpgSize (suppress if <0)
    if let Some(v) = get(7) {
        if v >= 0 {
            push(
                tags,
                "RawJpgSize",
                match v {
                    0 => "Large".into(),
                    1 => "Medium".into(),
                    2 => "Small".into(),
                    5 => "Medium 1".into(),
                    6 => "Medium 2".into(),
                    7 => "Medium 3".into(),
                    8 => "Postcard".into(),
                    9 => "Widescreen".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
    // Index 8: LongExposureNoiseReduction2 (suppress if <0)
    if let Some(v) = get(8) {
        if v >= 0 {
            push(
                tags,
                "LongExposureNoiseReduction2",
                match v {
                    0 => "Off".into(),
                    1 => "On (1)".into(),
                    3 => "On".into(),
                    4 => "Auto".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
    // Index 9: WBBracketMode (suppress if <0)
    if let Some(v) = get(9) {
        if v >= 0 {
            push(
                tags,
                "WBBracketMode",
                match v {
                    0 => "Off".into(),
                    1 => "On (shift AB)".into(),
                    2 => "On (shift GM)".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
    // Index 12: WBBracketValueAB
    if let Some(v) = get(12) {
        push(tags, "WBBracketValueAB", format!("{v}"));
    }
    // Index 13: WBBracketValueGM
    if let Some(v) = get(13) {
        push(tags, "WBBracketValueGM", format!("{v}"));
    }
    // Index 14: FilterEffect (suppress if <0)
    if let Some(v) = get(14) {
        if v >= 0 {
            push(
                tags,
                "FilterEffect",
                match v {
                    0 => "None".into(),
                    1 => "Yellow".into(),
                    2 => "Orange".into(),
                    3 => "Red".into(),
                    4 => "Green".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
    // Index 15: ToningEffect (suppress if <0)
    if let Some(v) = get(15) {
        if v >= 0 {
            push(
                tags,
                "ToningEffect",
                match v {
                    0 => "None".into(),
                    1 => "Sepia".into(),
                    2 => "Blue".into(),
                    3 => "Purple".into(),
                    4 => "Green".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
    // Index 16: MacroMagnification
    if let Some(v) = get(16) {
        if v > 0 {
            push(tags, "MacroMagnification", format!("{v}"));
        }
    }
    // Index 19: LiveViewShooting
    if let Some(v) = getu(19) {
        push(
            tags,
            "LiveViewShooting",
            match v {
                0 => "Off".into(),
                1 => "On".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    // Index 20: FocusDistanceUpper (suppress if 0)
    if let Some(v) = getu(20) {
        if v != 0 {
            let dist = v as f64 / 100.0;
            push(
                tags,
                "FocusDistanceUpper",
                if dist > 655.345 {
                    "inf".into()
                } else {
                    format_distance_m(dist)
                },
            );
        }
    }
    // Index 21: FocusDistanceLower (suppress if upper was 0)
    if let Some(upper) = getu(20) {
        if upper != 0 {
            if let Some(v) = getu(21) {
                push(
                    tags,
                    "FocusDistanceLower",
                    format_distance_m(v as f64 / 100.0),
                );
            }
        }
    }
    // Index 23: ShutterMode
    if let Some(v) = get(23) {
        push(
            tags,
            "ShutterMode",
            match v {
                0 => "Mechanical".into(),
                1 => "Electronic First Curtain".into(),
                2 => "Electronic".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    // Index 25: FlashExposureLock
    if let Some(v) = getu(25) {
        push(
            tags,
            "FlashExposureLock",
            match v {
                0 => "Off".into(),
                1 => "On".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    // Index 26: MyColorMode
    if let Some(v) = getu(26) {
        push(
            tags,
            "MyColorMode",
            match v {
                0 => "Off".into(),
                1 => "Positive Film".into(),
                2 => "Light Skin Tone".into(),
                3 => "Dark Skin Tone".into(),
                4 => "Vivid Blue".into(),
                5 => "Vivid Green".into(),
                6 => "Vivid Red".into(),
                7 => "Color Accent".into(),
                8 => "Color Swap".into(),
                9 => "Custom Theme".into(),
                12 => "Vivid".into(),
                13 => "Neutral".into(),
                14 => "Faithful".into(),
                15 => "Landscape".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    // Index 27: FirmwareRevision (4-byte string of version)
    if let Some(v) = getu(27) {
        if v != 0 {
            let major = (v >> 8) as u8;
            let minor = (v & 0xFF) as u8;
            push(tags, "FirmwareRevision", format!("{major}.{minor:02}"));
        }
    }
    // Index 33: Categories bitmap
    if let Some(v) = getu(33) {
        if v != 0 {
            let mut cats = Vec::new();
            if v & 0x0001 != 0 {
                cats.push("People");
            }
            if v & 0x0002 != 0 {
                cats.push("Scenery");
            }
            if v & 0x0004 != 0 {
                cats.push("Events");
            }
            if v & 0x0008 != 0 {
                cats.push("User 1");
            }
            if v & 0x0010 != 0 {
                cats.push("User 2");
            }
            if v & 0x0020 != 0 {
                cats.push("User 3");
            }
            if v & 0x0040 != 0 {
                cats.push("To Do");
            }
            push(
                tags,
                "Categories",
                if cats.is_empty() {
                    "(none)".into()
                } else {
                    cats.join(", ")
                },
            );
        }
    }
    // Index 35: ImageUniqueID
    if let Some(v) = getu(35) {
        if v != 0 {
            push(tags, "ImageUniqueID", format!("{v:#06X}"));
        }
    }
}

/// Decode Canon AspectInfo (tag 0x009A) - crop/aspect ratio information.
fn decode_canon_aspect_info(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let getu = |idx: usize| -> Option<u32> {
        let off = idx * 4;
        if off + 4 > data.len() {
            return None;
        }
        Some(if be {
            u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        })
    };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    if let Some(v) = getu(0) {
        push(
            tags,
            "AspectRatio",
            match v {
                0 => "3:2".into(),
                1 => "1:1".into(),
                2 => "4:3".into(),
                7 => "16:9".into(),
                8 => "4:5".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = getu(1) {
        push(tags, "CroppedImageWidth", format!("{v}"));
    }
    if let Some(v) = getu(2) {
        push(tags, "CroppedImageHeight", format!("{v}"));
    }
    if let Some(v) = getu(3) {
        push(tags, "CroppedImageLeft", format!("{v}"));
    }
    if let Some(v) = getu(4) {
        push(tags, "CroppedImageTop", format!("{v}"));
    }
}

/// Decode Canon ProcessingInfo (tag 0x00A0) - image processing settings.
fn decode_canon_processing_info(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let get = |idx: usize| -> Option<i16> { read_i16(data, idx, be) };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    if let Some(v) = get(1) {
        push(
            tags,
            "ToneCurve",
            match v {
                0 => "Standard".into(),
                1 => "Manual".into(),
                2 => "Custom".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(2) {
        push(tags, "Sharpness", format!("{v}"));
    }
    if let Some(v) = get(3) {
        push(
            tags,
            "SharpnessFrequency",
            match v {
                0 => "n/a".into(),
                1 => "Lowest".into(),
                2 => "Low".into(),
                3 => "Standard".into(),
                4 => "High".into(),
                5 => "Highest".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(4) {
        push(tags, "SensorRedLevel", format!("{v}"));
    }
    if let Some(v) = get(5) {
        push(tags, "SensorBlueLevel", format!("{v}"));
    }
    if let Some(v) = get(6) {
        push(tags, "WhiteBalanceRed", format!("{v}"));
    }
    if let Some(v) = get(7) {
        push(tags, "WhiteBalanceBlue", format!("{v}"));
    }
    if let Some(v) = get(8) {
        if v >= 0 {
            push(
                tags,
                "WhiteBalance",
                match v {
                    0 => "Auto".into(),
                    1 => "Daylight".into(),
                    2 => "Cloudy".into(),
                    3 => "Tungsten".into(),
                    4 => "Fluorescent".into(),
                    5 => "Flash".into(),
                    6 => "Custom".into(),
                    7 => "Black & White".into(),
                    8 => "Shade".into(),
                    9 => "Manual Temperature (Kelvin)".into(),
                    10 => "PC Set1".into(),
                    11 => "PC Set2".into(),
                    12 => "PC Set3".into(),
                    14 => "Daylight Fluorescent".into(),
                    _ => format!("Unknown ({v})"),
                },
            );
        }
    }
    if let Some(v) = get(9) {
        if v > 0 {
            push(tags, "ColorTemperature", format!("{v}"));
        }
    }
    if let Some(v) = get(10) {
        push(
            tags,
            "PictureStyle",
            match v {
                0 => "None".into(),
                1 => "Standard".into(),
                2 => "Portrait".into(),
                3 => "High Saturation".into(),
                4 => "Adobe RGB".into(),
                5 => "Low Saturation".into(),
                6 => "CM Set 1".into(),
                7 => "CM Set 2".into(),
                0x21 => "User Def. 1".into(),
                0x22 => "User Def. 2".into(),
                0x23 => "User Def. 3".into(),
                0x41 => "PC 1".into(),
                0x42 => "PC 2".into(),
                0x43 => "PC 3".into(),
                0x81 => "Standard".into(),
                0x82 => "Portrait".into(),
                0x83 => "Landscape".into(),
                0x84 => "Neutral".into(),
                0x85 => "Faithful".into(),
                0x86 => "Monochrome".into(),
                0x87 => "Auto".into(),
                0x88 => "Fine Detail".into(),
                _ => format!("Unknown ({v})"),
            },
        );
    }
    if let Some(v) = get(11) {
        push(tags, "DigitalGain", format!("{v}"));
    }
    if let Some(v) = get(12) {
        push(tags, "WBShiftAB", format!("{v}"));
    }
    if let Some(v) = get(13) {
        push(tags, "WBShiftGM", format!("{v}"));
    }
}

/// Decode Canon CropInfo (tag 0x0098) - FORMAT=int16u, FIRST_ENTRY=0.
fn decode_canon_crop_info(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let get_u16 = |idx: usize| -> Option<u16> {
        let off = idx * 2;
        if off + 2 > data.len() {
            return None;
        }
        Some(if be {
            u16::from_be_bytes([data[off], data[off + 1]])
        } else {
            u16::from_le_bytes([data[off], data[off + 1]])
        })
    };
    if let Some(v) = get_u16(0) {
        tags.push(DecodedTag {
            name: "CropLeftMargin".into(),
            value: format!("{v}"),
        });
    }
    if let Some(v) = get_u16(1) {
        tags.push(DecodedTag {
            name: "CropRightMargin".into(),
            value: format!("{v}"),
        });
    }
    if let Some(v) = get_u16(2) {
        tags.push(DecodedTag {
            name: "CropTopMargin".into(),
            value: format!("{v}"),
        });
    }
    if let Some(v) = get_u16(3) {
        tags.push(DecodedTag {
            name: "CropBottomMargin".into(),
            value: format!("{v}"),
        });
    }
}

/// Decode Canon LightingOpt (tag 0x4018) - FORMAT=int32s, FIRST_ENTRY=1.
/// Contains HighISONoiseReduction, HighlightTonePriority, LongExposureNoiseReduction.
fn decode_canon_vignetting_corr(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // Format: int16s, FIRST_ENTRY=1
    if data.len() < 14 {
        return;
    }
    // Version at index 0 (int8u format)
    tags.push(DecodedTag {
        name: "VignettingCorrVersion".into(),
        value: format!("{}", data[0]),
    });
    // Skip if data is all zeros after version
    if data[2..].iter().all(|&b| b == 0) {
        return;
    }
    let get16s = |idx: usize| -> Option<i16> {
        let off = idx * 2;
        if off + 2 > data.len() {
            return None;
        }
        Some(if be {
            i16::from_be_bytes([data[off], data[off + 1]])
        } else {
            i16::from_le_bytes([data[off], data[off + 1]])
        })
    };
    let off_on = |v: i16| -> String {
        match v {
            0 => "Off".into(),
            1 => "On".into(),
            _ => format!("{v}"),
        }
    };
    if let Some(v) = get16s(2) {
        tags.push(DecodedTag {
            name: "PeripheralLighting".into(),
            value: off_on(v),
        });
    }
    if let Some(v) = get16s(3) {
        tags.push(DecodedTag {
            name: "DistortionCorrection".into(),
            value: off_on(v),
        });
    }
    if let Some(v) = get16s(4) {
        tags.push(DecodedTag {
            name: "ChromaticAberrationCorr".into(),
            value: off_on(v),
        });
    }
    if let Some(v) = get16s(6) {
        tags.push(DecodedTag {
            name: "PeripheralLightingValue".into(),
            value: format!("{v}"),
        });
    }
    if let Some(v) = get16s(9) {
        tags.push(DecodedTag {
            name: "DistortionCorrectionValue".into(),
            value: format!("{v}"),
        });
    }
    if let Some(v) = get16s(11) {
        if v > 0 {
            tags.push(DecodedTag {
                name: "OriginalImageWidth".into(),
                value: format!("{v}"),
            });
        }
    }
    if let Some(v) = get16s(12) {
        if v > 0 {
            tags.push(DecodedTag {
                name: "OriginalImageHeight".into(),
                value: format!("{v}"),
            });
        }
    }
}

fn decode_canon_vignetting_corr2(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // Format: int32s, FIRST_ENTRY=1
    if data.len() < 28 {
        return;
    }
    let get32s = |idx: usize| -> Option<i32> {
        let off = idx * 4;
        if off + 4 > data.len() {
            return None;
        }
        Some(if be {
            i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        })
    };
    let off_on = |v: i32| -> String {
        match v {
            0 => "Off".into(),
            1 => "On".into(),
            _ => format!("{v}"),
        }
    };
    if let Some(v) = get32s(5) {
        tags.push(DecodedTag {
            name: "PeripheralLightingSetting".into(),
            value: off_on(v),
        });
    }
    if let Some(v) = get32s(6) {
        tags.push(DecodedTag {
            name: "ChromaticAberrationSetting".into(),
            value: off_on(v),
        });
    }
    if let Some(v) = get32s(7) {
        tags.push(DecodedTag {
            name: "DistortionCorrectionSetting".into(),
            value: off_on(v),
        });
    }
}

fn decode_canon_ambience(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // Format: int32s, FIRST_ENTRY=1
    if data.len() < 8 {
        return;
    }
    let off = 4; // index 1
    if off + 4 > data.len() {
        return;
    }
    let v = if be {
        i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    } else {
        i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    };
    tags.push(DecodedTag {
        name: "AmbienceSelection".into(),
        value: match v {
            0 => "Standard".into(),
            1 => "Vivid".into(),
            2 => "Warm".into(),
            3 => "Soft".into(),
            4 => "Cool".into(),
            5 => "Intense".into(),
            6 => "Brighter".into(),
            7 => "Darker".into(),
            8 => "Monochrome".into(),
            _ => format!("{v}"),
        },
    });
}

fn decode_canon_hdr_info(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // Format: int32s, FIRST_ENTRY=1
    if data.len() < 12 {
        return;
    }
    let get32s = |idx: usize| -> Option<i32> {
        let off = idx * 4;
        if off + 4 > data.len() {
            return None;
        }
        Some(if be {
            i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        })
    };
    if let Some(v) = get32s(1) {
        tags.push(DecodedTag {
            name: "HDR".into(),
            value: match v {
                0 => "Off".into(),
                1 => "Auto".into(),
                2 => "On".into(),
                _ => format!("{v}"),
            },
        });
    }
    if let Some(v) = get32s(2) {
        tags.push(DecodedTag {
            name: "HDREffect".into(),
            value: match v {
                0 => "Natural".into(),
                1 => "Art (standard)".into(),
                2 => "Art (vivid)".into(),
                3 => "Art (bold)".into(),
                4 => "Art (embossed)".into(),
                _ => format!("{v}"),
            },
        });
    }
}

fn decode_canon_lighting_opt(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    // Validate: first 2 bytes = size, must match data length
    if data.len() < 4 {
        return;
    }
    let size = if be {
        u16::from_be_bytes([data[0], data[1]]) as usize
    } else {
        u16::from_le_bytes([data[0], data[1]]) as usize
    };
    if size != data.len() && data.len() < 24 {
        return;
    }

    let get32s = |idx: usize| -> Option<i32> {
        let off = idx * 4;
        if off + 4 > data.len() {
            return None;
        }
        Some(if be {
            i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        })
    };

    if let Some(v) = get32s(1) {
        tags.push(DecodedTag {
            name: "PeripheralIlluminationCorr".into(),
            value: match v {
                0 => "Off".into(),
                1 => "On".into(),
                _ => format!("{v}"),
            },
        });
    }
    if let Some(v) = get32s(2) {
        tags.push(DecodedTag {
            name: "AutoLightingOptimizer".into(),
            value: match v {
                0 => "Standard".into(),
                1 => "Low".into(),
                2 => "Strong".into(),
                3 => "Off".into(),
                _ => format!("{v}"),
            },
        });
    }
    if let Some(v) = get32s(3) {
        tags.push(DecodedTag {
            name: "HighlightTonePriority".into(),
            value: match v {
                0 => "Off".into(),
                1 => "On".into(),
                2 => "Enhanced".into(),
                _ => format!("{v}"),
            },
        });
    }
    if let Some(v) = get32s(4) {
        tags.push(DecodedTag {
            name: "LongExposureNoiseReduction".into(),
            value: match v {
                0 => "Off".into(),
                1 => "Auto".into(),
                2 => "On".into(),
                _ => format!("{v}"),
            },
        });
    }
    if let Some(v) = get32s(5) {
        tags.push(DecodedTag {
            name: "HighISONoiseReduction".into(),
            value: match v {
                0 => "Standard".into(),
                1 => "Low".into(),
                2 => "Strong".into(),
                3 => "Off".into(),
                _ => format!("{v}"),
            },
        });
    }
}

/// Decode Canon AFMicroAdj (tag 0x4013) - FORMAT=int32s, FIRST_ENTRY=1.
fn decode_canon_af_micro_adj(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let get32s = |idx: usize| -> Option<i32> {
        let off = idx * 4;
        if off + 4 > data.len() {
            return None;
        }
        Some(if be {
            i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        })
    };

    if let Some(v) = get32s(1) {
        tags.push(DecodedTag {
            name: "AFMicroAdjMode".into(),
            value: match v {
                0 => "Disable".into(),
                1 => "Adjust all by the same amount".into(),
                2 => "Adjust by lens".into(),
                _ => format!("{v}"),
            },
        });
    }
    // Index 2: AFMicroAdjValue as rational64s (two int32s: numerator, denominator)
    if let (Some(num), Some(den)) = (get32s(2), get32s(3)) {
        let value = if den != 0 && den != 1 {
            format!("{}", num as f64 / den as f64)
        } else {
            format!("{num}")
        };
        tags.push(DecodedTag {
            name: "AFMicroAdjValue".into(),
            value,
        });
    }
}

/// Decode Canon CustomFunctions2 (tag 0x0099) - grouped binary structure.
/// Layout: u16 total_size, u16 pad, u32 group_count,
///   then groups: u32 group_num, u32 group_len, u32 record_count,
///     then records: u32 tag_id, u32 value_count, value_count × i32 values.
fn decode_canon_custom_functions2(data: &[u8], be: bool, model: &str, tags: &mut Vec<DecodedTag>) {
    if data.len() < 8 {
        return;
    }
    let get16u = |off: usize| -> u16 {
        if be {
            u16::from_be_bytes([data[off], data[off + 1]])
        } else {
            u16::from_le_bytes([data[off], data[off + 1]])
        }
    };
    let get32u = |off: usize| -> u32 {
        if be {
            u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        }
    };
    let get32s = |off: usize| -> i32 {
        if be {
            i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        }
    };

    // Model classification
    let is_1d = model.contains("1D");
    let is_hires_model = model.contains("50D")
        || model.contains("60D")
        || model.contains("5D Mark II")
        || model.contains("5DmkII")
        || model.contains("7D")
        || model.contains("500D")
        || model.contains("T1i")
        || model.contains("Kiss X3")
        || model.contains("550D")
        || model.contains("T2i")
        || model.contains("Kiss X4")
        || model.contains("600D")
        || model.contains("T3i")
        || model.contains("Kiss X5")
        || model.contains("1100D")
        || model.contains("T3")
        || model.contains("Kiss X50");

    let total_size = get16u(0) as usize;
    if total_size != data.len() || total_size < 8 {
        return;
    }
    let group_count = get32u(4) as usize;

    let mut pos = 8usize;
    for _ in 0..group_count {
        if pos + 12 > data.len() {
            break;
        }
        let _group_num = get32u(pos);
        let group_len = get32u(pos + 4) as usize;
        let rec_count = get32u(pos + 8) as usize;
        if group_len < 8 {
            break;
        }
        pos += 12;
        let rec_end = pos + group_len - 8;
        if rec_end > data.len() {
            break;
        }

        for _ in 0..rec_count {
            if pos + 8 > rec_end {
                break;
            }
            let tag_id = get32u(pos);
            let num = get32u(pos + 4) as usize;
            pos += 8;
            let next_pos = pos + num * 4;
            if next_pos > rec_end {
                break;
            }

            // Read first value (most tags are single-value)
            let val = if num > 0 { get32s(pos) } else { 0 };

            let (name, formatted) = match tag_id {
                0x0101 => (
                    "ExposureLevelIncrements",
                    if is_1d {
                        match val {
                            0 => "1/3-stop set, 1/3-stop comp.".into(),
                            1 => "1-stop set, 1/3-stop comp.".into(),
                            2 => "1/2-stop set, 1/2-stop comp.".into(),
                            _ => format!("{val}"),
                        }
                    } else {
                        match val {
                            0 => "1/3 Stop".into(),
                            1 => "1/2 Stop".into(),
                            _ => format!("{val}"),
                        }
                    },
                ),
                0x0102 => (
                    "ISOSpeedIncrements",
                    match val {
                        0 => "1/3 Stop".into(),
                        1 => "1 Stop".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0103 if is_1d && num >= 3 => {
                    // ISOSpeedRange for 1D models: 3 values
                    let enable = if get32s(pos) == 0 {
                        "Disable"
                    } else {
                        "Enable"
                    };
                    let iso_conv = |v: i32| -> String {
                        if v < 2 {
                            format!("{v}")
                        } else {
                            format!(
                                "{}",
                                ((((v as f64) / 8.0 - 9.0) * 2.0f64.ln()).exp() * 100.0 + 0.5)
                                    as u32
                            )
                        }
                    };
                    let max_iso = iso_conv(get32s(pos + 4));
                    let min_iso = iso_conv(get32s(pos + 8));
                    (
                        "ISOSpeedRange",
                        format!("{enable}; Max {max_iso}; Min {min_iso}"),
                    )
                }
                0x0103 => (
                    "ISOExpansion",
                    match val {
                        0 => "Off".into(),
                        1 => "On".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0104 => (
                    "AEBAutoCancel",
                    match val {
                        0 => "On".into(),
                        1 => "Off".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0105 => (
                    "AEBSequence",
                    match val {
                        0 => "0,-,+".into(),
                        1 => "-,0,+".into(),
                        2 => "+,0,-".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0107 => (
                    "SpotMeterLinkToAFPoint",
                    match val {
                        0 => "Disable (use center AF point)".into(),
                        1 => "Enable (use active AF point)".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0108 => (
                    "SafetyShift",
                    match val {
                        0 => "Disable".into(),
                        1 => "Enable (Tv/Av)".into(),
                        2 => "Enable (ISO speed)".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x010b => (
                    "ExposureModeInManual",
                    match val {
                        0 => "Specified metering mode".into(),
                        1 => "Evaluative metering".into(),
                        2 => "Partial metering".into(),
                        3 => "Spot metering".into(),
                        4 => "Center-weighted average".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0106 => (
                    "AEBShotCount",
                    if is_1d {
                        match val {
                            0 => "3 shots".into(),
                            1 => "2 shots".into(),
                            2 => "5 shots".into(),
                            3 => "7 shots".into(),
                            _ => format!("{val}"),
                        }
                    } else {
                        match val {
                            0 => "3 shots".into(),
                            1 => "2 shots".into(),
                            _ => format!("{val}"),
                        }
                    },
                ),
                0x010f => (
                    "FlashSyncSpeedAv",
                    match val {
                        0 => "Auto".into(),
                        1 => "1/250 Fixed".into(),
                        2 => "1/200 Fixed".into(),
                        _ => format!("{val}"),
                    },
                ),
                // 2) Image
                0x0201 => (
                    "LongExposureNoiseReduction",
                    match val {
                        0 => "Off".into(),
                        1 => "Auto".into(),
                        2 => "On".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0202 => (
                    "HighISONoiseReduction",
                    if is_hires_model {
                        match val {
                            0 => "Standard".into(),
                            1 => "Low".into(),
                            2 => "Strong".into(),
                            3 => "Off".into(),
                            _ => format!("{val}"),
                        }
                    } else {
                        match val {
                            0 => "Off".into(),
                            1 => "On".into(),
                            _ => format!("{val}"),
                        }
                    },
                ),
                0x0203 => (
                    "HighlightTonePriority",
                    match val {
                        0 => "Disable".into(),
                        1 => "Enable".into(),
                        _ => format!("{val}"),
                    },
                ),
                // 3) Flash
                0x0304 => (
                    "ETTLII",
                    match val {
                        0 => "Evaluative".into(),
                        1 => "Average".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0305 => (
                    "ShutterCurtainSync",
                    match val {
                        0 => "1st-curtain sync".into(),
                        1 => "2nd-curtain sync".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0306 => (
                    "FlashFiring",
                    match val {
                        0 => "Fires".into(),
                        1 => "Does not fire".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0407 => (
                    "ViewInfoDuringExposure",
                    match val {
                        0 => "Disable".into(),
                        1 => "Enable".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0408 => (
                    "LCDIlluminationDuringBulb",
                    match val {
                        0 => "Off".into(),
                        1 => "On".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0409 => (
                    "InfoButtonWhenShooting",
                    match val {
                        0 => "Displays camera settings".into(),
                        1 => "Displays shooting functions".into(),
                        _ => format!("{val}"),
                    },
                ),
                // 5) AF
                0x0501 => (
                    "USMLensElectronicMF",
                    match val {
                        0 => "Enable after one-shot AF".into(),
                        1 => "Disable after one-shot AF".into(),
                        2 => "Disable in AF mode".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0502 => (
                    "AIServoTrackingSensitivity",
                    match val {
                        -2 => "Slow".into(),
                        -1 => "Medium Slow".into(),
                        0 => "Standard".into(),
                        1 => "Medium Fast".into(),
                        2 => "Fast".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0503 => (
                    "AIServoImagePriority",
                    match val {
                        0 => "1: AF, 2: Tracking".into(),
                        1 => "1: AF, 2: Drive speed".into(),
                        2 => "1: Release, 2: Drive speed".into(),
                        3 => "1: Release, 2: Tracking".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0504 => (
                    "AIServoTrackingMethod",
                    match val {
                        0 => "Main focus point priority".into(),
                        1 => "Continuous AF track priority".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0505 => (
                    "LensDriveNoAF",
                    match val {
                        0 => "Focus search on".into(),
                        1 => "Focus search off".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0506 => (
                    "LensAFStopButton",
                    match val {
                        0 => "AF stop".into(),
                        1 => "AF start".into(),
                        2 => "AE lock".into(),
                        3 => "AF point: M->Auto/Auto->ctr".into(),
                        4 => "ONE SHOT <-> AI SERVO".into(),
                        5 => "IS start".into(),
                        6 => "Switch to registered AF point".into(),
                        7 => "Spot AF".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0508 => (
                    "AFPointAreaExpansion",
                    match val {
                        0 => "Disable".into(),
                        1 => "Enable".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x050a => (
                    "SwitchToRegisteredAFPoint",
                    match val {
                        0 => "Disable".into(),
                        1 => "Enable".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x050b => (
                    "AFPointAutoSelection",
                    match val {
                        0 => "Control-direct:disable/Main:enable".into(),
                        1 => "Control-direct:disable/Main:disable".into(),
                        2 => "Control-direct:enable/Main:enable".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x050c => (
                    "AFPointDisplayDuringFocus",
                    match val {
                        0 => "On".into(),
                        1 => "Off".into(),
                        2 => "On (when focus achieved)".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x050d => (
                    "AFPointBrightness",
                    match val {
                        0 => "Normal".into(),
                        1 => "Brighter".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x050e => (
                    "AFAssistBeam",
                    match val {
                        0 => "Emits".into(),
                        1 => "Does not emit".into(),
                        2 => "Only ext. flash emits".into(),
                        3 => "IR AF assist beam only".into(),
                        _ => format!("{val}"),
                    },
                ),
                // 6) Drive
                0x060f => (
                    "MirrorLockup",
                    match val {
                        0 => "Disable".into(),
                        1 => "Enable".into(),
                        2 => "Enable: Down with Set".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0610 => {
                    // ContinuousShootingSpeed: multi-value for 1D models
                    let mut parts = Vec::new();
                    for i in 0..num {
                        let v = get32s(pos + i * 4);
                        if i == 0 {
                            parts.push(if v == 0 {
                                "Disable".to_string()
                            } else {
                                format!("{v}")
                            });
                        } else if i == 1 {
                            parts.push(format!("Hi {v}"));
                        } else if i == 2 {
                            parts.push(format!("Lo {v}"));
                        }
                    }
                    ("ContinuousShootingSpeed", parts.join("; "))
                }
                0x0611 => {
                    // ContinuousShotLimit: may have 2 values
                    let mut parts = Vec::new();
                    for i in 0..num {
                        let v = get32s(pos + i * 4);
                        if i == 0 {
                            parts.push(if v == 0 {
                                "Disable".to_string()
                            } else {
                                format!("{v}")
                            });
                        } else {
                            parts.push(format!("{v} shots"));
                        }
                    }
                    ("ContinuousShotLimit", parts.join("; "))
                }
                // 7) Operation
                0x0701 => (
                    "ShutterButtonAFOnButton",
                    match val {
                        0 => "Metering + AF start".into(),
                        1 => "Metering + AF start/AF stop".into(),
                        2 => "Metering start/Meter + AF start".into(),
                        3 => "AE lock/Metering + AF start".into(),
                        4 => "Metering + AF start/disable".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0702 => (
                    "AFOnAELockButtonSwitch",
                    match val {
                        0 => "Disable".into(),
                        1 => "Enable".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0704 => (
                    "SetButtonWhenShooting",
                    match val {
                        0 => "Normal (disabled)".into(),
                        1 => "Image quality".into(),
                        2 => "Picture style".into(),
                        3 => "Menu display".into(),
                        4 => "Image playback".into(),
                        5 => "Quick control screen".into(),
                        6 => "Record func. + card/folder sel.".into(),
                        7 => "ISO speed".into(),
                        8 => "Flash exposure compensation".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0703 => (
                    "QuickControlDialInMeter",
                    match val {
                        0 => "Exposure comp/Aperture".into(),
                        1 => "AF point selection".into(),
                        2 => "ISO speed".into(),
                        3 => "AF point selection swapped with exposure comp.".into(),
                        4 => "ISO speed swapped with AF point selection".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0705 => (
                    "ManualTv",
                    match val {
                        0 => "Tv=Main/Av=Control".into(),
                        1 => "Tv=Control/Av=Main".into(),
                        2 => "Tv=Main/Av=Main w/o lens".into(),
                        3 => "Tv=Control/Av=Main w/o lens".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0706 => (
                    "DialDirectionTvAv",
                    match val {
                        0 => "Normal".into(),
                        1 => "Reversed".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0707 => (
                    "AvSettingWithoutLens",
                    match val {
                        0 => "Disable".into(),
                        1 => "Enable".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0708 => (
                    "WBMediaImageSizeSetting",
                    match val {
                        0 => "Rear LCD panel".into(),
                        1 => "LCD monitor".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0709 => (
                    "LockMicrophoneButton",
                    match val {
                        0 => "Protect (hold:record memo)".into(),
                        1 => "Record memo (protect:disabled)".into(),
                        2 => "No function".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x070a => (
                    "ButtonFunctionControlOff",
                    match val {
                        0 => "Normal (enable)".into(),
                        1 => "Disable main, control, multi-controller".into(),
                        _ => format!("{val}"),
                    },
                ),
                // 8) Others
                0x080b => (
                    "FocusingScreen",
                    if is_1d {
                        match val {
                            0 => "Ec-CIV".into(),
                            1 => "Ec-A,B,C,D,H,I,L".into(),
                            2 => "Ec-S".into(),
                            3 => "Ec-N,R".into(),
                            _ => format!("{val}"),
                        }
                    } else {
                        match val {
                            0 => "Ef-A".into(),
                            1 => "Ef-D".into(),
                            2 => "Ef-S".into(),
                            _ => format!("{val}"),
                        }
                    },
                ),
                0x080d => (
                    "ShortReleaseTimeLag",
                    match val {
                        0 => "Disable".into(),
                        1 => "Enable".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x080e => (
                    "AddAspectRatioInfo",
                    match val {
                        0 => "Off".into(),
                        1 => "On".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x080f => (
                    "AddOriginalDecisionData",
                    match val {
                        0 => "Off".into(),
                        1 => "On".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0810 => (
                    "LiveViewExposureSimulation",
                    match val {
                        0 => "Disable (LCD auto adjust)".into(),
                        1 => "Enable (simulates exposure)".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x0811 => (
                    "LCDDisplayAtPowerOn",
                    match val {
                        0 => "Display".into(),
                        1 => "Retain power off display".into(),
                        _ => format!("{val}"),
                    },
                ),
                // Multi-value tags
                0x0109 => {
                    // UsableShootingModes
                    let enable = if val == 0 { "Disable" } else { "Enable" };
                    if num >= 2 {
                        let flags = get32s(pos + 4);
                        (
                            "UsableShootingModes",
                            format!("{enable}; Flags 0x{flags:x}"),
                        )
                    } else {
                        ("UsableShootingModes", enable.to_string())
                    }
                }
                0x010a => {
                    // UsableMeteringModes
                    let enable = if val == 0 { "Disable" } else { "Enable" };
                    if num >= 2 {
                        let flags = get32s(pos + 4);
                        (
                            "UsableMeteringModes",
                            format!("{enable}; Flags 0x{flags:x}"),
                        )
                    } else {
                        ("UsableMeteringModes", enable.to_string())
                    }
                }
                0x010c if num >= 3 => {
                    // ShutterSpeedRange: 3 values
                    let enable = if val == 0 { "Disable" } else { "Enable" };
                    let shutter_conv = |v: i32| -> String {
                        let t = (-(v as f64 / 8.0 - 7.0) * 2.0f64.ln()).exp();
                        // Use ExifTool's PrintExposureTime format
                        if t < 0.25 {
                            let recip = (1.0 / t + 0.5) as u32;
                            format!("1/{recip}")
                        } else {
                            format!("{t}")
                        }
                    };
                    let hi = shutter_conv(get32s(pos + 4));
                    let lo = shutter_conv(get32s(pos + 8));
                    ("ShutterSpeedRange", format!("{enable}; Hi {hi}; Lo {lo}"))
                }
                0x010d if num >= 3 => {
                    // ApertureRange: 3 values
                    let enable = if val == 0 { "Disable" } else { "Enable" };
                    let aperture_conv = |v: i32| -> String {
                        let x = (((v as f64) / 8.0 - 1.0) * 2.0f64.ln() / 2.0).exp();
                        // Mimic C's %.2g: 2 significant digits
                        if x >= 10.0 {
                            format!("{}", x.round() as u32)
                        } else if x >= 1.0 {
                            let s = format!("{x:.1}");
                            s.trim_end_matches('0').trim_end_matches('.').to_string()
                        } else {
                            format!("{x:.2}")
                        }
                    };
                    let closed = aperture_conv(get32s(pos + 4));
                    let open = aperture_conv(get32s(pos + 8));
                    (
                        "ApertureRange",
                        format!("{enable}; Closed {closed}; Open {open}"),
                    )
                }
                0x010e => {
                    // ApplyShootingMeteringMode: first value is enable/disable, rest raw
                    let enable = if val == 0 { "Disable" } else { "Enable" };
                    let mut parts = vec![enable.to_string()];
                    for i in 1..num {
                        parts.push(format!("{}", get32s(pos + i * 4)));
                    }
                    ("ApplyShootingMeteringMode", parts.join("; "))
                }
                0x0507 => {
                    // AFMicroadjustment
                    let mode = match val {
                        0 => "Disable".to_string(),
                        1 => "Adjust all by same amount".to_string(),
                        2 => "Adjust by lens".to_string(),
                        _ => format!("{val}"),
                    };
                    let mut parts = vec![mode];
                    for i in 1..num {
                        parts.push(format!("{}", get32s(pos + i * 4)));
                    }
                    ("AFMicroadjustment", parts.join("; "))
                }
                0x0509 => (
                    "SelectableAFPoint",
                    match val {
                        0 => "19 points".into(),
                        1 => "Inner 9 points".into(),
                        2 => "Outer 9 points".into(),
                        3 => "19 Points, Multi-controller selectable".into(),
                        4 => "Inner 9 Points, Multi-controller selectable".into(),
                        5 => "Outer 9 Points, Multi-controller selectable".into(),
                        _ => format!("{val}"),
                    },
                ),
                0x080c => {
                    // TimerLength: 4 values
                    let enable = if val == 0 { "Disable" } else { "Enable" };
                    let mut parts = vec![enable.to_string()];
                    if num >= 2 {
                        parts.push(format!("6 s: {}", get32s(pos + 4)));
                    }
                    if num >= 3 {
                        parts.push(format!("16 s: {}", get32s(pos + 8)));
                    }
                    if num >= 4 {
                        parts.push(format!("After release: {}", get32s(pos + 12)));
                    }
                    ("TimerLength", parts.join("; "))
                }
                _ => {
                    pos = next_pos;
                    continue;
                }
            };

            tags.push(DecodedTag {
                name: name.to_string(),
                value: formatted,
            });
            pos = next_pos;
        }
        pos = rec_end;
    }
}

/// Decode Nikon PictureControlData (tag 0x0023 / 0x00BD).
///
/// Versioned binary format: V1 (starts "01"), V2 ("02"), V3 ("03").
/// Each version has fixed-offset fields for picture control settings.
/// Decode Nikon AFInfo (tag 0x0088) - 4-byte binary format.
///
/// Byte 0: AFAreaMode, Byte 1: AFPoint, Bytes 2-3: AFPointsInFocus bitmask.
fn decode_nikon_af_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 4 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // AFAreaMode (byte 0)
    push(
        tags,
        "AFAreaMode",
        match data[0] {
            0 => "Single Area".into(),
            1 => "Dynamic Area".into(),
            2 => "Dynamic Area (closest subject)".into(),
            3 => "Group Dynamic".into(),
            4 => "Single Area (wide)".into(),
            5 => "Dynamic Area (wide)".into(),
            v => format!("{v}"),
        },
    );

    // AFPoint (byte 1)
    push(
        tags,
        "AFPoint",
        match data[1] {
            0 => "Center".into(),
            1 => "Top".into(),
            2 => "Bottom".into(),
            3 => "Mid-left".into(),
            4 => "Mid-right".into(),
            5 => "Upper-left".into(),
            6 => "Upper-right".into(),
            7 => "Lower-left".into(),
            8 => "Lower-right".into(),
            9 => "Far Left".into(),
            10 => "Far Right".into(),
            v => format!("{v}"),
        },
    );

    // AFPointsInFocus bitmask (bytes 2-3, big-endian u16)
    let mask = u16::from_be_bytes([data[2], data[3]]);
    static POINT_NAMES: &[&str] = &[
        "Center",
        "Top",
        "Bottom",
        "Mid-left",
        "Mid-right",
        "Upper-left",
        "Upper-right",
        "Lower-left",
        "Lower-right",
        "Far Left",
        "Far Right",
    ];
    let mut in_focus = Vec::new();
    for (i, name) in POINT_NAMES.iter().enumerate() {
        if mask & (1 << i) != 0 {
            in_focus.push(name.to_string());
        }
    }
    push(
        tags,
        "AFPointsInFocus",
        if in_focus.is_empty() {
            "(none)".into()
        } else {
            in_focus.join(", ")
        },
    );
}

/// Decode Nikon FaceDetect (tag 0x0021) - binary structure.
///
/// Byte 0: version, bytes 2-5: FaceDetectFrameSize (2×u16),
/// then face bounding boxes.
fn decode_nikon_face_detect(data: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    if data.len() < 6 {
        return;
    }

    // FaceDetectFrameSize at offset 2: two u16 values (width, height)
    let w = if be {
        u16::from_be_bytes([data[2], data[3]])
    } else {
        u16::from_le_bytes([data[2], data[3]])
    };
    let h = if be {
        u16::from_be_bytes([data[4], data[5]])
    } else {
        u16::from_le_bytes([data[4], data[5]])
    };
    if w > 0 && h > 0 {
        tags.push(DecodedTag {
            name: "FaceDetectFrameSize".to_string(),
            value: format!("{w} {h}"),
        });
    }

    // FacesDetected at offset 6 (u16)
    if data.len() >= 8 {
        let faces = if be {
            u16::from_be_bytes([data[6], data[7]])
        } else {
            u16::from_le_bytes([data[6], data[7]])
        };
        tags.push(DecodedTag {
            name: "FacesDetected".to_string(),
            value: format!("{faces}"),
        });
    }
}

/// Decode Nikon PictureControlData (tag 0x0023 / 0x00BD).
fn decode_nikon_picture_control(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 48 {
        return;
    }

    // Every field in this block is named for the block it came from.
    //
    // Not cosmetic: the Nikon IFD ALSO carries a top-level Sharpness (0x0006)
    // and Saturation (0x00AA), so without the prefix one MakerNotes group holds
    // two rows called "Saturation" with different values and no way to tell
    // which is which (measured on a COOLPIX P6000: "Normal" from the IFD, "0"
    // from here). ExifTool separates them with a sub-group; sift's tag list is
    // flat, so the name carries it - which is what the block's own
    // PictureControlName/Base/Adjust/QuickAdjust fields were already doing.
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        let name = if name.starts_with("PictureControl") {
            name.to_string()
        } else {
            format!("PictureControl{name}")
        };
        tags.push(DecodedTag { name, value: val });
    };

    // Detect version from first bytes - version string is e.g. "0100", "0200", "0300"
    // Major version is the second character (data[1]), not the first (which is always '0')
    let version = if data.len() >= 4 { data[1] - b'0' } else { 0 };

    // PictureControlVersion (first 4 bytes)
    let ver_len = if version >= 3 { 8 } else { 4 };
    if data.len() >= ver_len {
        let s = std::str::from_utf8(&data[..ver_len]).unwrap_or("");
        let s = s.trim_end_matches('\0').trim();
        if !s.is_empty() {
            push(tags, "PictureControlVersion", s.to_string());
        }
    }

    // Name and Base offsets depend on version
    let (name_off, base_off) = if version >= 3 { (8, 28) } else { (4, 24) };

    // PictureControlName
    if data.len() > name_off + 20 {
        let s = std::str::from_utf8(&data[name_off..name_off + 20]).unwrap_or("");
        let s = s.trim_end_matches('\0').trim();
        if !s.is_empty() {
            push(tags, "PictureControlName", titlecase_str(s));
        }
    }

    // PictureControlBase
    if data.len() > base_off + 20 {
        let s = std::str::from_utf8(&data[base_off..base_off + 20]).unwrap_or("");
        let s = s.trim_end_matches('\0').trim();
        if !s.is_empty() {
            push(tags, "PictureControlBase", titlecase_str(s));
        }
    }

    // Adjust offset is version-dependent
    let adj_off = if version >= 3 { 54 } else { 48 };
    if data.len() <= adj_off + 2 {
        return;
    }

    // PictureControlAdjust
    push(
        tags,
        "PictureControlAdjust",
        match data[adj_off] {
            0 => "Default Settings".into(),
            1 => "Quick Adjust".into(),
            2 => "Full Control".into(),
            v => format!("{v}"),
        },
    );

    // PictureControlQuickAdjust: 0x80 = 0 ("Normal"), 0x7E = -2, 0x82 = +2
    let qa = data[adj_off + 1];
    if qa != 0xFF {
        let val = qa as i32 - 0x80;
        let s = if val == 0 {
            "Normal".into()
        } else {
            format!("{val:+}")
        };
        push(tags, "PictureControlQuickAdjust", s);
    }

    // Version-specific field offsets
    match version {
        1 => {
            // V1: compact layout starting at adj_off+2
            if data.len() > adj_off + 9 {
                let sharpness = data[adj_off + 2];
                let contrast = data[adj_off + 3];
                let brightness = data[adj_off + 4];
                let saturation = data[adj_off + 5];
                let hue = data[adj_off + 6];
                let filter = data[adj_off + 7];
                let toning = data[adj_off + 8];
                let toning_sat = data[adj_off + 9];

                if sharpness != 0xFF {
                    push(tags, "Sharpness", nikon_pc_val(sharpness));
                }
                if contrast != 0xFF {
                    push(tags, "Contrast", nikon_pc_val(contrast));
                }
                push(tags, "Brightness", nikon_pc_brightness(brightness));
                if saturation != 0xFF {
                    push(tags, "Saturation", nikon_pc_val(saturation));
                }
                push(tags, "HueAdjustment", nikon_pc_hue(hue));
                push(tags, "FilterEffect", nikon_pc_filter(filter));
                push(tags, "ToningEffect", nikon_pc_toning(toning));
                if toning_sat == 0x7F || toning_sat == 0xFF {
                    push(tags, "ToningSaturation", "n/a".into());
                } else {
                    push(tags, "ToningSaturation", nikon_pc_val(toning_sat));
                }
            }
        }
        2 => {
            // V2: wider spacing (every other byte)
            if data.len() > adj_off + 17 {
                let sharpness = data[adj_off + 3];
                let clarity = data[adj_off + 5];
                let contrast = data[adj_off + 7];
                let brightness = data[adj_off + 9];
                let saturation = data[adj_off + 11];
                let hue = data[adj_off + 13];
                let filter = data[adj_off + 15];
                let toning = data[adj_off + 16];
                let toning_sat = data[adj_off + 17];

                if sharpness != 0xFF {
                    push(tags, "Sharpness", nikon_pc_val(sharpness));
                }
                if clarity != 0xFF {
                    push(tags, "Clarity", nikon_pc_val(clarity));
                }
                if contrast != 0xFF {
                    push(tags, "Contrast", nikon_pc_val(contrast));
                }
                push(tags, "Brightness", nikon_pc_brightness(brightness));
                if saturation != 0xFF {
                    push(tags, "Saturation", nikon_pc_val(saturation));
                }
                push(tags, "HueAdjustment", nikon_pc_hue(hue));
                push(tags, "FilterEffect", nikon_pc_filter(filter));
                push(tags, "ToningEffect", nikon_pc_toning(toning));
                if toning_sat == 0x7F || toning_sat == 0xFF {
                    push(tags, "ToningSaturation", "n/a".into());
                } else {
                    push(tags, "ToningSaturation", nikon_pc_val(toning_sat));
                }
            }
        }
        3
            // V3: wider spacing, extra MidRangeSharpness
            if data.len() > adj_off + 19 => {
                let sharpness = data[adj_off + 3];
                let mid_sharp = data[adj_off + 5];
                let clarity = data[adj_off + 7];
                let contrast = data[adj_off + 9];
                let brightness = data[adj_off + 11];
                let saturation = data[adj_off + 13];
                let hue = data[adj_off + 15];
                let filter = data[adj_off + 17];
                let toning = data[adj_off + 18];
                let toning_sat = data[adj_off + 19];

                if sharpness != 0xFF {
                    push(tags, "Sharpness", nikon_pc_val(sharpness));
                }
                if mid_sharp != 0xFF {
                    push(tags, "MidRangeSharpness", nikon_pc_val(mid_sharp));
                }
                if clarity != 0xFF {
                    push(tags, "Clarity", nikon_pc_val(clarity));
                }
                if contrast != 0xFF {
                    push(tags, "Contrast", nikon_pc_val(contrast));
                }
                push(tags, "Brightness", nikon_pc_brightness(brightness));
                if saturation != 0xFF {
                    push(tags, "Saturation", nikon_pc_val(saturation));
                }
                push(tags, "HueAdjustment", nikon_pc_hue(hue));
                push(tags, "FilterEffect", nikon_pc_filter(filter));
                push(tags, "ToningEffect", nikon_pc_toning(toning));
                if toning_sat == 0x7F || toning_sat == 0xFF {
                    push(tags, "ToningSaturation", "n/a".into());
                } else {
                    push(tags, "ToningSaturation", nikon_pc_val(toning_sat));
                }
            }
        _ => {}
    }
}

/// Format a Nikon PictureControl numeric value (stored as u8, 0x80 = 0).
/// 0x00 and 0xFF are treated as "n/a".
fn nikon_pc_val(v: u8) -> String {
    if v == 0xFF || v == 0x00 {
        return "n/a".into();
    }
    let val = v as i32 - 0x80;
    format!("{val}")
}

/// Format PictureControl Brightness: 0x80=Normal, 0xFF/0x00=n/a
fn nikon_pc_brightness(v: u8) -> String {
    if v == 0xFF || v == 0x00 {
        return "n/a".into();
    }
    let val = v as i32 - 0x80;
    if val == 0 {
        "Normal".into()
    } else {
        format!("{val}")
    }
}

///// Format PictureControl HueAdjustment: 0x80=None, 0xFF=n/a
fn nikon_pc_hue(v: u8) -> String {
    if v == 0xFF {
        return "n/a".into();
    }
    let val = v as i32 - 0x80;
    if val == 0 {
        "None".into()
    } else {
        format!("{val}")
    }
}

/// Format a Nikon PictureControl FilterEffect.
fn nikon_pc_filter(v: u8) -> String {
    match v {
        0x80 => "Off".into(),
        0x81 => "Yellow".into(),
        0x82 => "Orange".into(),
        0x83 => "Red".into(),
        0x84 => "Green".into(),
        0xFF => "n/a".into(),
        v => format!("{v}"),
    }
}

/// Format a Nikon PictureControl ToningEffect.
fn nikon_pc_toning(v: u8) -> String {
    match v {
        0x80 => "B&W".into(),
        0x81 => "Sepia".into(),
        0x82 => "Cyanotype".into(),
        0x83 => "Red".into(),
        0x84 => "Yellow".into(),
        0x85 => "Green".into(),
        0x86 => "Blue-green".into(),
        0x87 => "Blue".into(),
        0x88 => "Purple-blue".into(),
        0x89 => "Red-purple".into(),
        0xFF => "n/a".into(),
        v => format!("{v}"),
    }
}

/// Nikon XOR cipher for encrypted sub-structures (LensData, ShotInfo).
///
/// Keys are derived from serial number and shutter count. Data bytes from
/// `start` onward are decrypted in place.
fn nikon_decrypt(data: &mut [u8], serial: u32, shutter_count: u32) {
    #[rustfmt::skip]
    const XLAT0: [u8; 256] = [
        0xc1,0xbf,0x6d,0x0d,0x59,0xc5,0x13,0x9d,0x83,0x61,0x6b,0x4f,0xc7,0x7f,0x3d,0x3d,
        0x53,0x59,0xe3,0xc7,0xe9,0x2f,0x95,0xa7,0x95,0x1f,0xdf,0x7f,0x2b,0x29,0xc7,0x0d,
        0xdf,0x07,0xef,0x71,0x89,0x3d,0x13,0x3d,0x3b,0x13,0xfb,0x0d,0x89,0xc1,0x65,0x1f,
        0xb3,0x0d,0x6b,0x29,0xe3,0xfb,0xef,0xa3,0x6b,0x47,0x7f,0x95,0x35,0xa7,0x47,0x4f,
        0xc7,0xf1,0x59,0x95,0x35,0x11,0x29,0x61,0xf1,0x3d,0xb3,0x2b,0x0d,0x43,0x89,0xc1,
        0x9d,0x9d,0x89,0x65,0xf1,0xe9,0xdf,0xbf,0x3d,0x7f,0x53,0x97,0xe5,0xe9,0x95,0x17,
        0x1d,0x3d,0x8b,0xfb,0xc7,0xe3,0x67,0xa7,0x07,0xf1,0x71,0xa7,0x53,0xb5,0x29,0x89,
        0xe5,0x2b,0xa7,0x17,0x29,0xe9,0x4f,0xc5,0x65,0x6d,0x6b,0xef,0x0d,0x89,0x49,0x2f,
        0xb3,0x43,0x53,0x65,0x1d,0x49,0xa3,0x13,0x89,0x59,0xef,0x6b,0xef,0x65,0x1d,0x0b,
        0x59,0x13,0xe3,0x4f,0x9d,0xb3,0x29,0x43,0x2b,0x07,0x1d,0x95,0x59,0x59,0x47,0xfb,
        0xe5,0xe9,0x61,0x47,0x2f,0x35,0x7f,0x17,0x7f,0xef,0x7f,0x95,0x95,0x71,0xd3,0xa3,
        0x0b,0x71,0xa3,0xad,0x0b,0x3b,0xb5,0xfb,0xa3,0xbf,0x4f,0x83,0x1d,0xad,0xe9,0x2f,
        0x71,0x65,0xa3,0xe5,0x07,0x35,0x3d,0x0d,0xb5,0xe9,0xe5,0x47,0x3b,0x9d,0xef,0x35,
        0xa3,0xbf,0xb3,0xdf,0x53,0xd3,0x97,0x53,0x49,0x71,0x07,0x35,0x61,0x71,0x2f,0x43,
        0x2f,0x11,0xdf,0x17,0x97,0xfb,0x95,0x3b,0x7f,0x6b,0xd3,0x25,0xbf,0xad,0xc7,0xc5,
        0xc5,0xb5,0x8b,0xef,0x2f,0xd3,0x07,0x6b,0x25,0x49,0x95,0x25,0x49,0x6d,0x71,0xc7,
    ];
    #[rustfmt::skip]
    const XLAT1: [u8; 256] = [
        0xa7,0xbc,0xc9,0xad,0x91,0xdf,0x85,0xe5,0xd4,0x78,0xd5,0x17,0x46,0x7c,0x29,0x4c,
        0x4d,0x03,0xe9,0x25,0x68,0x11,0x86,0xb3,0xbd,0xf7,0x6f,0x61,0x22,0xa2,0x26,0x34,
        0x2a,0xbe,0x1e,0x46,0x14,0x68,0x9d,0x44,0x18,0xc2,0x40,0xf4,0x7e,0x5f,0x1b,0xad,
        0x0b,0x94,0xb6,0x67,0xb4,0x0b,0xe1,0xea,0x95,0x9c,0x66,0xdc,0xe7,0x5d,0x6c,0x05,
        0xda,0xd5,0xdf,0x7a,0xef,0xf6,0xdb,0x1f,0x82,0x4c,0xc0,0x68,0x47,0xa1,0xbd,0xee,
        0x39,0x50,0x56,0x4a,0xdd,0xdf,0xa5,0xf8,0xc6,0xda,0xca,0x90,0xca,0x01,0x42,0x9d,
        0x8b,0x0c,0x73,0x43,0x75,0x05,0x94,0xde,0x24,0xb3,0x80,0x34,0xe5,0x2c,0xdc,0x9b,
        0x3f,0xca,0x33,0x45,0xd0,0xdb,0x5f,0xf5,0x52,0xc3,0x21,0xda,0xe2,0x22,0x72,0x6b,
        0x3e,0xd0,0x5b,0xa8,0x87,0x8c,0x06,0x5d,0x0f,0xdd,0x09,0x19,0x93,0xd0,0xb9,0xfc,
        0x8b,0x0f,0x84,0x60,0x33,0x1c,0x9b,0x45,0xf1,0xf0,0xa3,0x94,0x3a,0x12,0x77,0x33,
        0x4d,0x44,0x78,0x28,0x3c,0x9e,0xfd,0x65,0x57,0x16,0x94,0x6b,0xfb,0x59,0xd0,0xc8,
        0x22,0x36,0xdb,0xd2,0x63,0x98,0x43,0xa1,0x04,0x87,0x86,0xf7,0xa6,0x26,0xbb,0xd6,
        0x59,0x4d,0xbf,0x6a,0x2e,0xaa,0x2b,0xef,0xe6,0x78,0xb6,0x4e,0xe0,0x2f,0xdc,0x7c,
        0xbe,0x57,0x19,0x32,0x7e,0x2a,0xd0,0xb8,0xba,0x29,0x00,0x3c,0x52,0x7d,0xa8,0x49,
        0x3b,0x2d,0xeb,0x25,0x49,0xfa,0xa3,0xaa,0x39,0xa7,0xc5,0xa7,0x50,0x11,0x36,0xfb,
        0xc6,0x67,0x4a,0xf5,0xa5,0x12,0x65,0x7e,0xb0,0xdf,0xaf,0x4e,0xb3,0x61,0x7f,0x2f,
    ];

    // Fold shutter count into a single byte by XORing all 4 bytes
    let key = ((shutter_count & 0xFF)
        ^ ((shutter_count >> 8) & 0xFF)
        ^ ((shutter_count >> 16) & 0xFF)
        ^ ((shutter_count >> 24) & 0xFF)) as u8;

    let ci = XLAT0[(serial & 0xFF) as usize];
    let mut cj = XLAT1[key as usize];
    let mut ck: u8 = 0x60;

    for byte in data.iter_mut() {
        cj = cj.wrapping_add(ci.wrapping_mul(ck));
        ck = ck.wrapping_add(1);
        *byte ^= cj;
    }
}

/// Nikon LensData (tag 0x0098) - decodes lens identification and optical parameters.
///
/// Version 0100: unencrypted, simple layout.
/// Version 0101: unencrypted, extended layout (D70, D70s).
/// Versions 02xx/04xx/08xx: same field layout, encrypted from offset 4 (decrypted by caller).
fn decode_nikon_lens_data(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 7 {
        return;
    }

    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Version string (first 4 bytes)
    let ver_str = std::str::from_utf8(&data[..4]).unwrap_or("????");
    push(
        tags,
        "LensDataVersion",
        ver_str.trim_end_matches('\0').to_string(),
    );

    match ver_str {
        "0100" => {
            // LensData00: unencrypted, compact layout
            if data.len() < 0x0d {
                return;
            }
            push(tags, "LensIDNumber", format!("{}", data[0x06]));
            push(tags, "LensFStops", nikon_lens_fstops(data[0x07]));
            push(tags, "MinFocalLength", nikon_focal_length(data[0x08]));
            push(tags, "MaxFocalLength", nikon_focal_length(data[0x09]));
            push(tags, "MaxApertureAtMinFocal", nikon_aperture(data[0x0a]));
            push(tags, "MaxApertureAtMaxFocal", nikon_aperture(data[0x0b]));
            push(tags, "MCUVersion", format!("{}", data[0x0c]));
        }
        "0101" => {
            // LensData01: unencrypted, extended layout (D70, D70s)
            if data.len() < 0x13 {
                return;
            }
            push(tags, "ExitPupilPosition", nikon_exit_pupil(data[0x04]));
            push(tags, "AFAperture", nikon_aperture(data[0x05]));
            push(tags, "FocusPosition", format!("0x{:02x}", data[0x08]));
            push(tags, "FocusDistance", nikon_focus_distance(data[0x09]));
            push(tags, "FocalLength", nikon_focal_length(data[0x0a]));
            push(tags, "LensIDNumber", format!("{}", data[0x0b]));
            push(tags, "LensFStops", nikon_lens_fstops(data[0x0c]));
            push(tags, "MinFocalLength", nikon_focal_length(data[0x0d]));
            push(tags, "MaxFocalLength", nikon_focal_length(data[0x0e]));
            push(tags, "MaxApertureAtMinFocal", nikon_aperture(data[0x0f]));
            push(tags, "MaxApertureAtMaxFocal", nikon_aperture(data[0x10]));
            push(tags, "MCUVersion", format!("{}", data[0x11]));
            push(tags, "EffectiveMaxAperture", nikon_aperture(data[0x12]));
        }
        v if v.starts_with("02") => {
            // LensData 0201/0202: same layout as 0101 (decrypted by caller)
            if data.len() < 0x13 {
                return;
            }
            push(tags, "ExitPupilPosition", nikon_exit_pupil(data[0x04]));
            push(tags, "AFAperture", nikon_aperture(data[0x05]));
            push(tags, "FocusPosition", format!("0x{:02x}", data[0x08]));
            push(tags, "FocusDistance", nikon_focus_distance(data[0x09]));
            push(tags, "FocalLength", nikon_focal_length(data[0x0a]));
            push(tags, "LensIDNumber", format!("{}", data[0x0b]));
            push(tags, "LensFStops", nikon_lens_fstops(data[0x0c]));
            push(tags, "MinFocalLength", nikon_focal_length(data[0x0d]));
            push(tags, "MaxFocalLength", nikon_focal_length(data[0x0e]));
            push(tags, "MaxApertureAtMinFocal", nikon_aperture(data[0x0f]));
            push(tags, "MaxApertureAtMaxFocal", nikon_aperture(data[0x10]));
            push(tags, "MCUVersion", format!("{}", data[0x11]));
            push(tags, "EffectiveMaxAperture", nikon_aperture(data[0x12]));
        }
        v if v.starts_with("04") => {
            // LensData 0204: shifted by +1 byte vs 0201 (extra byte at 0x09)
            if data.len() < 0x14 {
                return;
            }
            push(tags, "ExitPupilPosition", nikon_exit_pupil(data[0x04]));
            push(tags, "AFAperture", nikon_aperture(data[0x05]));
            push(tags, "FocusPosition", format!("0x{:02x}", data[0x08]));
            push(tags, "FocusDistance", nikon_focus_distance(data[0x0a]));
            push(tags, "FocalLength", nikon_focal_length(data[0x0b]));
            push(tags, "LensIDNumber", format!("{}", data[0x0c]));
            push(tags, "LensFStops", nikon_lens_fstops(data[0x0d]));
            push(tags, "MinFocalLength", nikon_focal_length(data[0x0e]));
            push(tags, "MaxFocalLength", nikon_focal_length(data[0x0f]));
            push(tags, "MaxApertureAtMinFocal", nikon_aperture(data[0x10]));
            push(tags, "MaxApertureAtMaxFocal", nikon_aperture(data[0x11]));
            push(tags, "MCUVersion", format!("{}", data[0x12]));
            push(tags, "EffectiveMaxAperture", nikon_aperture(data[0x13]));
        }
        v if v.starts_with("08") => {
            // LensData 0800 (Z-series): has OldLensData compat section at 0x04-0x14
            if data.len() < 0x14 {
                return;
            }
            push(tags, "ExitPupilPosition", nikon_exit_pupil(data[0x04]));
            push(tags, "AFAperture", nikon_aperture(data[0x05]));
            push(tags, "FocusPosition", format!("0x{:02x}", data[0x08]));
            push(tags, "FocusDistance", nikon_focus_distance(data[0x0a]));
            push(tags, "FocalLength", nikon_focal_length(data[0x0b]));
            push(tags, "LensIDNumber", format!("{}", data[0x0c]));
            push(tags, "LensFStops", nikon_lens_fstops(data[0x0d]));
            push(tags, "MinFocalLength", nikon_focal_length(data[0x0e]));
            push(tags, "MaxFocalLength", nikon_focal_length(data[0x0f]));
            push(tags, "MaxApertureAtMinFocal", nikon_aperture(data[0x10]));
            push(tags, "MaxApertureAtMaxFocal", nikon_aperture(data[0x11]));
            push(tags, "MCUVersion", format!("{}", data[0x12]));
            push(tags, "EffectiveMaxAperture", nikon_aperture(data[0x13]));
        }
        _ => {}
    }
}

/// Nikon FlashInfo (tag 0x00A8) - decodes flash settings and group control.
///
/// Version 0100/0101: D2H/D2Hs/D2X/D50/D70/D70s/D80/D200.
fn decode_nikon_flash_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 9 {
        return;
    }

    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Version string
    let ver_str = std::str::from_utf8(&data[..4]).unwrap_or("????");
    push(
        tags,
        "FlashInfoVersion",
        ver_str.trim_end_matches('\0').to_string(),
    );

    // FlashSource
    push(
        tags,
        "FlashSource",
        match data[4] {
            0 => "None".into(),
            1 => "External".into(),
            2 => "Internal".into(),
            v => format!("{v}"),
        },
    );

    // ExternalFlashFirmware (2-byte key)
    if data.len() > 7 {
        let fw = nikon_flash_firmware(data[6], data[7]);
        push(tags, "ExternalFlashFirmware", fw);
    }

    // ExternalFlashFlags
    if data.len() > 8 {
        let flags = data[8];
        let mut parts = Vec::new();
        if flags & 0x01 != 0 {
            parts.push("Fired");
        }
        if flags & 0x04 != 0 {
            parts.push("Bounce Flash");
        }
        if flags & 0x10 != 0 {
            parts.push("Wide Flash Adapter");
        }
        if flags & 0x20 != 0 {
            parts.push("Dome Diffuser");
        }
        if parts.is_empty() {
            parts.push("(none)");
        }
        push(tags, "ExternalFlashFlags", parts.join(", "));
    }

    // Version-specific fields
    let major = if data.len() >= 4 && data[2].is_ascii_digit() && data[3].is_ascii_digit() {
        (data[2] - b'0') * 10 + (data[3] - b'0')
    } else {
        0
    };
    match major {
        0..=2 => {
            // FlashInfo0100/0102: basic layout
            if data.len() > 10 {
                // FlashCommanderMode
                push(
                    tags,
                    "FlashCommanderMode",
                    if data[9] & 0x80 != 0 {
                        "On".into()
                    } else {
                        "Off".into()
                    },
                );
                // FlashControlMode
                push(
                    tags,
                    "FlashControlMode",
                    nikon_flash_control_mode(data[9] & 0x7F),
                );
                // FlashOutput or FlashCompensation based on control mode
                let ctrl = data[9] & 0x7F;
                if ctrl >= 6 {
                    push(tags, "FlashOutput", nikon_flash_output(data[10]));
                } else {
                    push(
                        tags,
                        "FlashCompensation",
                        nikon_flash_compensation(data[10]),
                    );
                }
            }
            if data.len() > 15 {
                push(
                    tags,
                    "FlashFocalLength",
                    if data[12] == 0 || data[12] == 255 {
                        "n/a".into()
                    } else {
                        format!("{} mm", data[12])
                    },
                );
                push(
                    tags,
                    "RepeatingFlashRate",
                    if data[13] == 0 || data[13] == 255 {
                        "n/a".into()
                    } else {
                        format!("{} Hz", data[13])
                    },
                );
                push(
                    tags,
                    "RepeatingFlashCount",
                    if data[14] == 0 || data[14] == 255 {
                        "n/a".into()
                    } else {
                        format!("{}", data[14])
                    },
                );
                push(tags, "FlashGNDistance", nikon_flash_gn_distance(data[15]));
            }
            // Group control modes
            if major == 0 && data.len() > 18 {
                // FlashInfo0100: GroupA at 15(lo), GroupB at 16(lo)
                let ctrl_a = data[15] & 0x0F;
                let ctrl_b = data[16] & 0x0F;
                push(
                    tags,
                    "FlashGroupAControlMode",
                    nikon_flash_control_mode(ctrl_a),
                );
                push(
                    tags,
                    "FlashGroupBControlMode",
                    nikon_flash_control_mode(ctrl_b),
                );
                if ctrl_a >= 6 {
                    push(tags, "FlashGroupAOutput", nikon_flash_output(data[17]));
                } else {
                    push(
                        tags,
                        "FlashGroupACompensation",
                        nikon_flash_compensation(data[17]),
                    );
                }
                if ctrl_b >= 6 {
                    push(tags, "FlashGroupBOutput", nikon_flash_output(data[18]));
                } else {
                    push(
                        tags,
                        "FlashGroupBCompensation",
                        nikon_flash_compensation(data[18]),
                    );
                }
            } else if major == 2 && data.len() > 20 {
                // FlashInfo0102: GroupA at 16(lo), GroupB at 17(hi), GroupC at 17(lo)
                let ctrl_a = data[16] & 0x0F;
                let ctrl_b = (data[17] & 0xF0) >> 4;
                let ctrl_c = data[17] & 0x0F;
                push(
                    tags,
                    "FlashGroupAControlMode",
                    nikon_flash_control_mode(ctrl_a),
                );
                push(
                    tags,
                    "FlashGroupBControlMode",
                    nikon_flash_control_mode(ctrl_b),
                );
                push(
                    tags,
                    "FlashGroupCControlMode",
                    nikon_flash_control_mode(ctrl_c),
                );
                if ctrl_a >= 6 {
                    push(tags, "FlashGroupAOutput", nikon_flash_output(data[18]));
                } else {
                    push(
                        tags,
                        "FlashGroupACompensation",
                        nikon_flash_compensation(data[18]),
                    );
                }
                if ctrl_b >= 6 {
                    push(tags, "FlashGroupBOutput", nikon_flash_output(data[19]));
                } else {
                    push(
                        tags,
                        "FlashGroupBCompensation",
                        nikon_flash_compensation(data[19]),
                    );
                }
                if ctrl_c >= 6 {
                    push(tags, "FlashGroupCOutput", nikon_flash_output(data[20]));
                } else {
                    push(
                        tags,
                        "FlashGroupCCompensation",
                        nikon_flash_compensation(data[20]),
                    );
                }
            }
        }
        3..=5 => {
            // FlashInfo0103/0104/0105: D3 fw2, D3X, D3S, D4, D90, D300 fw1.10,
            // D300S, D600, D700, D800, D3000-D5200, D7000
            if data.len() > 10 {
                push(
                    tags,
                    "FlashCommanderMode",
                    if data[9] & 0x80 != 0 {
                        "On".into()
                    } else {
                        "Off".into()
                    },
                );
                push(
                    tags,
                    "FlashControlMode",
                    nikon_flash_control_mode(data[9] & 0x7F),
                );
                let ctrl = data[9] & 0x7F;
                if ctrl >= 6 {
                    push(tags, "FlashOutput", nikon_flash_output(data[10]));
                } else {
                    push(
                        tags,
                        "FlashCompensation",
                        nikon_flash_compensation(data[10]),
                    );
                }
            }
            if data.len() > 16 {
                push(
                    tags,
                    "FlashFocalLength",
                    if data[12] == 0 || data[12] == 255 {
                        "n/a".into()
                    } else {
                        format!("{} mm", data[12])
                    },
                );
                push(
                    tags,
                    "RepeatingFlashRate",
                    if data[13] == 0 || data[13] == 255 {
                        "n/a".into()
                    } else {
                        format!("{} Hz", data[13])
                    },
                );
                push(
                    tags,
                    "RepeatingFlashCount",
                    if data[14] == 0 || data[14] == 255 {
                        "n/a".into()
                    } else {
                        format!("{}", data[14])
                    },
                );
                push(tags, "FlashGNDistance", nikon_flash_gn_distance(data[15]));
                push(tags, "FlashColorFilter", nikon_flash_color_filter(data[16]));
            }
            // Group control modes at bytes 17-18
            if data.len() > 21 {
                let ctrl_a = data[17] & 0x0F;
                let ctrl_b = (data[18] >> 4) & 0x0F;
                let ctrl_c = data[18] & 0x0F;
                push(
                    tags,
                    "FlashGroupAControlMode",
                    nikon_flash_control_mode(ctrl_a),
                );
                push(
                    tags,
                    "FlashGroupBControlMode",
                    nikon_flash_control_mode(ctrl_b),
                );
                push(
                    tags,
                    "FlashGroupCControlMode",
                    nikon_flash_control_mode(ctrl_c),
                );
                // Compensation/Output at bytes 0x13-0x15
                if ctrl_a >= 6 {
                    push(tags, "FlashGroupAOutput", nikon_flash_output(data[0x13]));
                } else {
                    push(
                        tags,
                        "FlashGroupACompensation",
                        nikon_flash_compensation(data[0x13]),
                    );
                }
                if ctrl_b >= 6 {
                    push(tags, "FlashGroupBOutput", nikon_flash_output(data[0x14]));
                } else {
                    push(
                        tags,
                        "FlashGroupBCompensation",
                        nikon_flash_compensation(data[0x14]),
                    );
                }
                if ctrl_c >= 6 {
                    push(tags, "FlashGroupCOutput", nikon_flash_output(data[0x15]));
                } else {
                    push(
                        tags,
                        "FlashGroupCCompensation",
                        nikon_flash_compensation(data[0x15]),
                    );
                }
            }
        }
        _ => {
            // Newer versions (06xx+) - skip for now
        }
    }
}

/// Nikon exit pupil position: 2048/val mm, 0 = 0
fn nikon_exit_pupil(v: u8) -> String {
    if v == 0 {
        "0".into()
    } else {
        format!("{:.1} mm", 2048.0 / v as f64)
    }
}

/// Nikon aperture encoding: 2^(val/24)
fn nikon_aperture(v: u8) -> String {
    if v == 0 {
        "0".into()
    } else {
        format!("{:.1}", 2.0_f64.powf(v as f64 / 24.0))
    }
}

/// Nikon focal length encoding: 5 * 2^(val/24) mm
fn nikon_focal_length(v: u8) -> String {
    if v == 0 {
        "0".into()
    } else {
        format!("{:.1} mm", 5.0 * 2.0_f64.powf(v as f64 / 24.0))
    }
}

/// Nikon focus distance: 0.01 * 10^(val/40) m; 0 = inf
fn nikon_focus_distance(v: u8) -> String {
    if v == 0 {
        "inf".into()
    } else {
        format!("{:.2} m", 0.01 * 10.0_f64.powf(v as f64 / 40.0))
    }
}

/// Nikon lens f-stops: val/12
fn nikon_lens_fstops(v: u8) -> String {
    if v == 0 {
        "0".into()
    } else {
        let fstops = v as f64 / 12.0;
        if fstops == fstops.round() {
            format!("{fstops:.0}")
        } else {
            format!("{fstops:.2}")
        }
    }
}

/// Nikon flash control mode
fn nikon_flash_control_mode(v: u8) -> String {
    match v {
        0 => "Off".into(),
        1 => "iTTL-BL".into(),
        2 => "iTTL".into(),
        3 => "Auto Aperture".into(),
        4 => "Automatic".into(),
        5 => "GN (distance priority)".into(),
        6 => "Manual".into(),
        7 => "Repeating Flash".into(),
        v => format!("{v}"),
    }
}

/// Nikon flash output: 2^(-val/6)
fn nikon_flash_output(v: u8) -> String {
    if v == 0 {
        "Full".into()
    } else {
        let frac = 2.0_f64.powf(-(v as f64) / 6.0);
        if frac >= 0.5 {
            format!("1/{:.0}", 1.0 / frac)
        } else {
            format!("{frac:.4}")
        }
    }
}

/// Nikon flash compensation: -val/6 EV
fn nikon_flash_compensation(v: u8) -> String {
    let val = -(v as i8) as f64 / 6.0;
    if val == 0.0 {
        "0".into()
    } else {
        format!("{val:+.1} EV")
    }
}

/// Nikon flash GN distance lookup
fn nikon_flash_gn_distance(v: u8) -> String {
    match v {
        0 => "0".into(),
        1 => "0.1 m".into(),
        2 => "0.2 m".into(),
        3 => "0.3 m".into(),
        4 => "0.4 m".into(),
        5 => "0.5 m".into(),
        6 => "0.6 m".into(),
        7 => "0.7 m".into(),
        8 => "0.8 m".into(),
        9 => "0.9 m".into(),
        10 => "1.0 m".into(),
        11 => "1.1 m".into(),
        12 => "1.3 m".into(),
        13 => "1.4 m".into(),
        14 => "1.6 m".into(),
        15 => "1.8 m".into(),
        16 => "2.0 m".into(),
        17 => "2.2 m".into(),
        18 => "2.5 m".into(),
        19 => "2.8 m".into(),
        20 => "3.2 m".into(),
        21 => "3.6 m".into(),
        22 => "4.0 m".into(),
        23 => "4.5 m".into(),
        24 => "5.0 m".into(),
        25 => "5.6 m".into(),
        26 => "6.3 m".into(),
        27 => "7.1 m".into(),
        28 => "8.0 m".into(),
        29 => "9.0 m".into(),
        30 => "10.0 m".into(),
        31 => "11.0 m".into(),
        32 => "13.0 m".into(),
        33 => "14.0 m".into(),
        34 => "16.0 m".into(),
        35 => "18.0 m".into(),
        36 => "20.0 m".into(),
        255 => "n/a".into(),
        v => format!("{v}"),
    }
}

/// Nikon flash color filter
fn nikon_flash_color_filter(v: u8) -> String {
    match v {
        0 => "None".into(),
        1 => "FL-GL1 or SZ-2FL Fluorescent".into(),
        2 => "FL-GL2".into(),
        9 => "TN-A1 or SZ-2TN Incandescent".into(),
        10 => "TN-A2".into(),
        65 => "Red".into(),
        66 => "Blue".into(),
        67 => "Yellow".into(),
        68 => "Amber".into(),
        128 => "Incandescent".into(),
        v => format!("{v}"),
    }
}

/// Nikon external flash firmware identifier
fn nikon_flash_firmware(major: u8, minor: u8) -> String {
    match (major, minor) {
        (0, 0) => "n/a".into(),
        (1, 1) => "1.01 (SB-800 or Metz different different different 58 AF-1)".into(),
        (1, 3) => "1.03 (SB-800)".into(),
        (2, 1) => "2.01 (SB-800)".into(),
        (2, 4) => "2.04 (SB-600)".into(),
        (2, 5) => "2.05 (SB-600)".into(),
        (3, 1) => "3.01 (SU-800 Remote)".into(),
        (4, 1) => "4.01 (SB-400)".into(),
        (4, 2) => "4.02 (SB-400)".into(),
        (4, 4) => "4.04 (SB-400)".into(),
        (5, 1) => "5.01 (SB-900)".into(),
        (5, 2) => "5.02 (SB-900)".into(),
        (6, 1) => "6.01 (SB-700)".into(),
        (7, 1) => "7.01 (SB-910)".into(),
        _ => format!("{major}.{minor:02}"),
    }
}

/// Decode Nikon Preview sub-IFD (tag 0x0011) - contains PreviewImageStart/Length.
fn decode_nikon_preview_ifd(
    entry: &IfdEntry<'_>,
    mn_data: &[u8],
    nikon_tiff_offset: usize,
    mn_file_offset: usize,
    be: bool,
    tags: &mut Vec<DecodedTag>,
) {
    // The entry value is an offset to a sub-IFD within the embedded TIFF
    if entry.data.len() < 4 {
        return;
    }
    let offset = if be {
        u32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
    } else {
        u32::from_le_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
    } as u64;

    // The sub-IFD offset is relative to the embedded TIFF header
    let tiff_slice = if nikon_tiff_offset < mn_data.len() {
        &mn_data[nikon_tiff_offset..]
    } else {
        mn_data
    };

    let sub_ifd = if !tiff_slice.is_empty() {
        tiff::parse_ifd_tolerant(tiff_slice, offset, be, false)
    } else {
        None
    };

    let Some(sub_ifd) = sub_ifd else {
        return;
    };

    for sub_entry in &sub_ifd.entries {
        match sub_entry.tag {
            0x0103 => {
                if let Some(v) = entry_u16(sub_entry, be) {
                    tags.push(DecodedTag {
                        name: "Compression".into(),
                        value: match v {
                            1 => "Uncompressed".into(),
                            6 => "JPEG (old-style)".into(),
                            7 => "JPEG".into(),
                            _ => format!("{v}"),
                        },
                    });
                }
            }
            0x011A | 0x011B => {
                // XResolution, YResolution (rational)
                let name = if sub_entry.tag == 0x011A {
                    "PreviewXResolution"
                } else {
                    "PreviewYResolution"
                };
                let val = format_ifd_value(sub_entry, be);
                tags.push(DecodedTag {
                    name: name.into(),
                    value: val,
                });
            }
            0x0128 => {
                if let Some(v) = entry_u16(sub_entry, be) {
                    tags.push(DecodedTag {
                        name: "PreviewResolutionUnit".into(),
                        value: match v {
                            1 => "None".into(),
                            2 => "inches".into(),
                            3 => "cm".into(),
                            _ => format!("{v}"),
                        },
                    });
                }
            }
            0x0201 => {
                if let Some(v) = entry_u32(sub_entry, be) {
                    // Offset is relative to the Nikon embedded TIFF header.
                    // Convert to file-relative offset. Prefix with '!' to skip blanket tiff_base adjustment.
                    let file_offset =
                        (v as u64).wrapping_add((mn_file_offset + nikon_tiff_offset) as u64);
                    tags.push(DecodedTag {
                        name: "PreviewImageStart".into(),
                        value: format!("!{file_offset}"),
                    });
                }
            }
            0x0202 => {
                if let Some(v) = entry_u32(sub_entry, be) {
                    if v > 0 {
                        tags.push(DecodedTag {
                            name: "PreviewImageLength".into(),
                            value: format!("{v}"),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Decode Nikon ColorBalance (tag 0x0097).
/// Extracts WB_RGGBLevels (or WB_RBGGLevels/WB_RGBGLevels/WB_GRBGLevels) from
/// version-dependent binary data. Versions 01xx are unencrypted; 02xx+ are encrypted.
fn decode_nikon_color_balance(
    data: &[u8],
    serial: u32,
    shutter_count: u32,
    be: bool,
    tags: &mut Vec<DecodedTag>,
) {
    if data.len() < 4 {
        return;
    }
    let ver = std::str::from_utf8(&data[..4]).unwrap_or("");

    // Determine: (decrypt_start, wb_byte_offset, tag_name)
    // wb_byte_offset = DecryptStart + DirOffset (both in bytes), then index 0 means offset 0
    let params: Option<(Option<usize>, usize, &str)> = match ver {
        "0100" => Some((None, 72, "WB_RBGGLevels")),
        "0102" => Some((None, 10, "WB_RGGBLevels")),
        "0103" => Some((None, 20, "WB_RGBGLevels")),
        "0205" => Some((Some(4), 4 + 14, "WB_RGGBLevels")),
        v if v.starts_with("02") => {
            let num: u16 = v[2..4].parse().unwrap_or(0);
            if num == 9 || num == 12 || num == 14 {
                // D300/D3/D700, D3100, D5100 - ColorBalance4 = WB_GRBGLevels
                Some((Some(284), 284 + 10, "WB_GRBGLevels"))
            } else if num < 11 {
                // D2X/D2Hs/D200/D40/D80/D60 - ColorBalance2 = WB_RGGBLevels
                Some((Some(284), 284 + 6, "WB_RGGBLevels"))
            } else if num == 11 {
                // D90/D5000 - ColorBalance4 = WB_GRBGLevels
                Some((Some(284), 284 + 16, "WB_GRBGLevels"))
            } else if num == 13 {
                // D3000 - ColorBalance2 = WB_RGGBLevels
                Some((Some(284), 284 + 10, "WB_RGGBLevels"))
            } else if num == 15 || num == 16 || num == 17 {
                // D7000/D5200/D3200 - ColorBalance4 = WB_GRBGLevels
                Some((Some(284), 284 + 4, "WB_GRBGLevels"))
            } else if num == 19 || (21..=24).contains(&num) {
                // D4/D800/D3300/D7100/D5300 - ColorBalance2 = WB_RGGBLevels
                Some((Some(4), 4 + 0x7c, "WB_RGGBLevels"))
            } else {
                None
            }
        }
        _ => None,
    };

    let Some((decrypt_start, wb_offset, tag_name)) = params else {
        return;
    };

    // Prepare data - decrypt if needed
    let work: Vec<u8>;
    let buf: &[u8] = if let Some(ds) = decrypt_start {
        if data.len() <= ds {
            return;
        }
        work = {
            let mut d = data.to_vec();
            nikon_decrypt(&mut d[ds..], serial, shutter_count);
            d
        };
        &work
    } else {
        data
    };

    // Read 4 × int16u at wb_offset
    if buf.len() < wb_offset + 8 {
        return;
    }
    let get16 = |off: usize| -> u16 {
        if be {
            u16::from_be_bytes([buf[off], buf[off + 1]])
        } else {
            u16::from_le_bytes([buf[off], buf[off + 1]])
        }
    };
    let v0 = get16(wb_offset);
    let v1 = get16(wb_offset + 2);
    let v2 = get16(wb_offset + 4);
    let v3 = get16(wb_offset + 6);

    tags.push(DecodedTag {
        name: tag_name.to_string(),
        value: format!("{v0} {v1} {v2} {v3}"),
    });
}

/// Decode Nikon ShotInfo (tag 0x0091) - version-dependent shot info.
/// Many versions are encrypted (02xx+). We extract the unencrypted header fields
/// and the unencrypted 01xx version fields.
fn decode_nikon_shot_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 4 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // ShotInfoVersion is always at offset 0, 4 bytes ASCII
    let ver = std::str::from_utf8(&data[..4]).unwrap_or("????");
    push(
        tags,
        "ShotInfoVersion",
        ver.trim_end_matches('\0').to_string(),
    );

    // Version 0100 (D70s/D70) - unencrypted, big-endian
    if data.starts_with(b"0100") && data.len() >= 100 {
        // FirmwareVersion at offset 0x04, 5 bytes ASCII
        if data.len() > 9 {
            let fw = std::str::from_utf8(&data[4..9]).unwrap_or("");
            let fw = fw.trim_end_matches('\0').trim();
            if !fw.is_empty() {
                push(tags, "FirmwareVersion", fw.to_string());
            }
        }
    }
    // Version 0103 (D70s) - unencrypted
    if data.starts_with(b"0103") && data.len() >= 100 && data.len() > 9 {
        let fw = std::str::from_utf8(&data[4..9]).unwrap_or("");
        let fw = fw.trim_end_matches('\0').trim();
        if !fw.is_empty() {
            push(tags, "FirmwareVersion", fw.to_string());
        }
    }
    // P6000 and similar: ShotInfoVersion all zeros, has DistortionControl at offset 0x10
    if data.starts_with(&[0, 0, 0, 0]) && data.len() > 0x10 {
        push(
            tags,
            "DistortionControl",
            match data[0x10] {
                0 => "Off".into(),
                1 => "On".into(),
                v => format!("{v}"),
            },
        );
    }
    // Versions 02xx - decrypted by caller. FirmwareVersion at offset 4 (5 bytes).
    if data.len() > 9 && data[0..4].iter().all(|&b| b.is_ascii_digit()) {
        let ver_prefix = data[0] - b'0';
        if ver_prefix == 0 && data[1] == b'2' {
            // 02xx version - extract FirmwareVersion
            let fw = std::str::from_utf8(&data[4..9]).unwrap_or("");
            let fw = fw.trim_end_matches('\0').trim();
            if !fw.is_empty() && fw.bytes().all(|b| b.is_ascii_graphic() || b == b' ') {
                push(tags, "FirmwareVersion", fw.to_string());
            }

            // ShutterCount at version-specific offsets (big-endian int32u)
            let sc_offset = match ver {
                "0208" => Some(0x24a_usize), // D80
                "0209" => Some(0x246),       // D40/D40X
                "0210" => {
                    // D3/D300 variants - offset depends on data length
                    match data.len() {
                        5399 | 5408 | 5412 => Some(0x276), // D3
                        5291 | 5303 => Some(0x276),        // D300
                        _ => None,
                    }
                }
                "0211" => Some(0x2d5),          // D5000/D90
                "0212" => Some(0x27c),          // D700
                "0213" => Some(0x2d5),          // D90
                "0214" => Some(0x276),          // D3X
                "0215" | "0216" => Some(0x2d5), // D300S
                "0218" => Some(0x2d5),          // D3S
                _ => None,
            };
            if let Some(off) = sc_offset {
                if data.len() > off + 3 {
                    let sc = u32::from_be_bytes([
                        data[off],
                        data[off + 1],
                        data[off + 2],
                        data[off + 3],
                    ]);
                    if sc > 0 && sc < 10_000_000 {
                        push(tags, "ShutterCount", format!("{sc}"));
                    }
                }
            }

            // CustomSettings sub-directory at version-specific offsets
            match ver {
                "0208" => {
                    // D80: CustomSettings at offset 748, 17 bytes
                    if data.len() >= 748 + 17 {
                        decode_nikon_custom_d80(&data[748..748 + 17], tags);
                    }
                }
                "0209" => {
                    // D40/D40X: CustomSettings at offset 729, 12 bytes
                    if data.len() >= 729 + 12 {
                        decode_nikon_custom_d40(&data[729..729 + 12], tags);
                    }
                }
                "0210" => {
                    // D300/D300S: CustomSettings at offset 790 (fw 0.25/1.00) or 802 (fw 1.10)
                    let cs_offset = if data.len() == 5303 { 802 } else { 790 };
                    if data.len() >= cs_offset + 24 {
                        decode_nikon_custom_d300(&data[cs_offset..cs_offset + 24], tags);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Decode Nikon CustomSettings for D80 (17 bytes).
fn decode_nikon_custom_d80(cs: &[u8], tags: &mut Vec<DecodedTag>) {
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Byte 0 - %onOff polarity: 0=On, 1=Off
    push(
        tags,
        "Beep",
        if cs[0] & 0x80 != 0 { "Off" } else { "On" }.into(),
    );
    push(
        tags,
        "AFAssist",
        if cs[0] & 0x40 != 0 { "Off" } else { "On" }.into(),
    );
    push(
        tags,
        "NoMemoryCard",
        if cs[0] & 0x20 != 0 {
            "Enable Release"
        } else {
            "Release Locked"
        }
        .into(),
    );
    push(
        tags,
        "ImageReview",
        if cs[0] & 0x10 != 0 { "Off" } else { "On" }.into(),
    );

    // Byte 1
    push(
        tags,
        "AutoISO",
        if cs[1] & 0x40 != 0 { "On" } else { "Off" }.into(),
    );
    push(
        tags,
        "AutoISOMax",
        match (cs[1] & 0x30) >> 4 {
            0 => "200".into(),
            1 => "400".into(),
            2 => "800".into(),
            3 => "1600".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "AutoISOMinShutterSpeed",
        match cs[1] & 0x0f {
            0 => "1/125 s".into(),
            1 => "1/100 s".into(),
            2 => "1/80 s".into(),
            3 => "1/60 s".into(),
            4 => "1/40 s".into(),
            5 => "1/30 s".into(),
            6 => "1/15 s".into(),
            7 => "1/8 s".into(),
            8 => "1/4 s".into(),
            9 => "1/2 s".into(),
            10 => "1 s".into(),
            v => format!("{v}"),
        },
    );

    // Byte 3
    push(
        tags,
        "MonitorOffTime",
        match (cs[3] & 0xe0) >> 5 {
            0 => "5 s".into(),
            1 => "10 s".into(),
            2 => "20 s".into(),
            3 => "1 min".into(),
            4 => "5 min".into(),
            5 => "10 min".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "MeteringTime",
        match (cs[3] & 0x1c) >> 2 {
            0 => "4 s".into(),
            1 => "6 s".into(),
            2 => "8 s".into(),
            3 => "16 s".into(),
            4 => "30 s".into(),
            5 => "30 min".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "SelfTimerTime",
        match cs[3] & 0x03 {
            0 => "2 s".into(),
            1 => "5 s".into(),
            2 => "10 s".into(),
            3 => "20 s".into(),
            v => format!("{v}"),
        },
    );

    // Byte 4
    push(
        tags,
        "RemoteOnDuration",
        match (cs[4] & 0xc0) >> 6 {
            0 => "1 min".into(),
            1 => "5 min".into(),
            2 => "10 min".into(),
            3 => "15 min".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "AELockButton",
        match (cs[4] & 0x1e) >> 1 {
            0 => "AE/AF Lock".into(),
            1 => "AE Lock Only".into(),
            2 => "AF Lock Only".into(),
            3 => "AE Lock (hold)".into(),
            4 => "AF-ON".into(),
            5 => "FV Lock".into(),
            6 => "Focus Area Selection".into(),
            7 => "AE-L/AF-L/AF Area".into(),
            8 => "AE-L/AF Area".into(),
            9 => "AF-L/AF Area".into(),
            10 => "AF-ON/AF Area".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "AELock",
        if cs[4] & 0x01 != 0 { "On" } else { "Off" }.into(),
    );

    // Byte 8
    push(
        tags,
        "InternalFlash",
        match (cs[8] & 0xc0) >> 6 {
            0 => "TTL".into(),
            1 => "Manual".into(),
            2 => "Repeating Flash".into(),
            3 => "Commander Mode".into(),
            v => format!("{v}"),
        },
    );

    // Byte 16
    push(
        tags,
        "AFAreaModeSetting",
        match (cs[16] & 0xc0) >> 6 {
            0 => "Single Area".into(),
            1 => "Dynamic Area".into(),
            2 => "Auto-area".into(),
            v => format!("{v}"),
        },
    );
}

/// Decode Nikon CustomSettings for D40/D40X (12 bytes).
fn decode_nikon_custom_d40(cs: &[u8], tags: &mut Vec<DecodedTag>) {
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Byte 0 - %onOff polarity: 0=On, 1=Off
    push(
        tags,
        "Beep",
        if cs[0] & 0x80 != 0 { "Off" } else { "On" }.into(),
    );
    push(
        tags,
        "AFAssist",
        if cs[0] & 0x40 != 0 { "Off" } else { "On" }.into(),
    );
    push(
        tags,
        "NoMemoryCard",
        if cs[0] & 0x20 != 0 {
            "Enable Release"
        } else {
            "Release Locked"
        }
        .into(),
    );
    push(
        tags,
        "ImageReview",
        if cs[0] & 0x10 != 0 { "Off" } else { "On" }.into(),
    );

    // Byte 1
    push(
        tags,
        "AutoISO",
        if cs[1] & 0x80 != 0 { "On" } else { "Off" }.into(),
    );
    push(
        tags,
        "AutoISOMax",
        match (cs[1] & 0x30) >> 4 {
            1 => "400".into(),
            2 => "800".into(),
            3 => "1600".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "AutoISOMinShutterSpeed",
        match cs[1] & 0x07 {
            0 => "1/125 s".into(),
            1 => "1/60 s".into(),
            2 => "1/30 s".into(),
            3 => "1/15 s".into(),
            4 => "1/8 s".into(),
            5 => "1/4 s".into(),
            6 => "1/2 s".into(),
            7 => "1 s".into(),
            v => format!("{v}"),
        },
    );

    // Byte 2
    push(
        tags,
        "ImageReviewTime",
        match cs[2] & 0x07 {
            0 => "4 s".into(),
            1 => "8 s".into(),
            2 => "20 s".into(),
            3 => "1 min".into(),
            4 => "10 min".into(),
            v => format!("{v}"),
        },
    );

    // Byte 3
    push(
        tags,
        "MonitorOffTime",
        match (cs[3] & 0xe0) >> 5 {
            0 => "4 s".into(),
            1 => "8 s".into(),
            2 => "20 s".into(),
            3 => "1 min".into(),
            4 => "10 min".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "MeteringTime",
        match (cs[3] & 0x1c) >> 2 {
            0 => "4 s".into(),
            1 => "8 s".into(),
            2 => "20 s".into(),
            3 => "1 min".into(),
            4 => "30 min".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "SelfTimerTime",
        match cs[3] & 0x03 {
            0 => "2 s".into(),
            1 => "5 s".into(),
            2 => "10 s".into(),
            3 => "20 s".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "RemoteOnDuration",
        match (cs[3] & 0xc0) >> 6 {
            0 => "1 min".into(),
            1 => "5 min".into(),
            2 => "10 min".into(),
            3 => "15 min".into(),
            v => format!("{v}"),
        },
    );

    // Byte 4
    push(
        tags,
        "AELockButton",
        match (cs[4] & 0x0e) >> 1 {
            0 => "AE/AF Lock".into(),
            1 => "AE Lock Only".into(),
            2 => "AF Lock Only".into(),
            3 => "AE Lock (hold)".into(),
            4 => "AF-ON".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "AELock",
        if cs[4] & 0x01 != 0 { "On" } else { "Off" }.into(),
    );

    // Byte 5
    push(
        tags,
        "ShootingModeSetting",
        match (cs[5] & 0x70) >> 4 {
            0 => "Single Frame".into(),
            1 => "Continuous".into(),
            2 => "Self-timer".into(),
            3 => "Delayed Remote".into(),
            4 => "Quick-response Remote".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "TimerFunctionButton",
        match cs[5] & 0x07 {
            0 => "Shooting Mode".into(),
            1 => "Image Quality/Size".into(),
            2 => "ISO".into(),
            3 => "White Balance".into(),
            4 => "Self-timer".into(),
            v => format!("{v}"),
        },
    );

    // Byte 6
    push(
        tags,
        "Metering",
        match cs[6] & 0x03 {
            0 => "Matrix".into(),
            1 => "Center-weighted".into(),
            2 => "Spot".into(),
            v => format!("{v}"),
        },
    );

    // Byte 8
    push(
        tags,
        "InternalFlash",
        match (cs[8] & 0x10) >> 4 {
            0 => "TTL".into(),
            1 => "Manual".into(),
            v => format!("{v}"),
        },
    );
    let mfo_val = cs[8] & 0x07;
    let mfo = if mfo_val == 0 {
        "Full".into()
    } else {
        let v = 2.0_f64.powi(-(mfo_val as i32));
        if v >= 0.25 {
            format!("{v}")
        } else {
            format!("1/{}", (1.0 / v) as u32)
        }
    };
    push(tags, "ManualFlashOutput", mfo);

    // Byte 9: FlashLevel (int8s, value/6)
    let flash_raw = cs[9] as i8;
    let flash_val = flash_raw as f64 / 6.0;
    push(tags, "FlashLevel", format!("{flash_val:+.1}"));

    // Byte 10
    push(
        tags,
        "FocusModeSetting",
        match (cs[10] & 0xc0) >> 6 {
            0 => "Manual".into(),
            1 => "AF-S".into(),
            2 => "AF-C".into(),
            3 => "AF-A".into(),
            v => format!("{v}"),
        },
    );

    // Byte 11
    push(
        tags,
        "AFAreaModeSetting",
        match (cs[11] & 0x30) >> 4 {
            0 => "Single Area".into(),
            1 => "Dynamic Area".into(),
            2 => "Closest Subject".into(),
            v => format!("{v}"),
        },
    );
}

/// Decode Nikon CustomSettings for D300/D300S/D3/D3S/D3X (24 bytes, SettingsD3 table).
fn decode_nikon_custom_d300(cs: &[u8], tags: &mut Vec<DecodedTag>) {
    if cs.len() < 24 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };
    let fine_tune = |nibble: u8| -> String {
        let v = if nibble > 7 {
            nibble as i8 - 16
        } else {
            nibble as i8
        };
        if v == 0 {
            "0".into()
        } else {
            format!("{:+.2}", v as f64 / 6.0)
        }
    };

    // Byte 0
    push(
        tags,
        "CustomSettingsBank",
        match cs[0] & 0x03 {
            0 => "A".into(),
            1 => "B".into(),
            2 => "C".into(),
            3 => "D".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "CustomSettingsAllDefault",
        if cs[0] & 0x80 != 0 { "No" } else { "Yes" }.into(),
    );

    // Byte 1
    push(
        tags,
        "AF-CPrioritySelection",
        match (cs[1] & 0xc0) >> 6 {
            0 => "Release".into(),
            1 => "Release + Focus".into(),
            2 => "Focus".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "AF-SPrioritySelection",
        if cs[1] & 0x20 != 0 {
            "Release"
        } else {
            "Focus"
        }
        .into(),
    );
    push(
        tags,
        "AFPointSelection",
        if cs[1] & 0x10 != 0 {
            "11 Points"
        } else {
            "51 Points"
        }
        .into(),
    );
    push(
        tags,
        "DynamicAFArea",
        match (cs[1] & 0x0c) >> 2 {
            0 => "9 Points".into(),
            1 => "21 Points".into(),
            2 => "51 Points".into(),
            3 => "51 Points (3D-tracking)".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "FocusTrackingLockOn",
        match cs[1] & 0x03 {
            0 => "Long".into(),
            1 => "Normal".into(),
            2 => "Short".into(),
            3 => "Off".into(),
            v => format!("{v}"),
        },
    );

    // Byte 2
    push(
        tags,
        "AFActivation",
        if cs[2] & 0x80 != 0 {
            "AF-On Only"
        } else {
            "Shutter/AF-On"
        }
        .into(),
    );
    push(
        tags,
        "FocusPointWrap",
        if cs[2] & 0x08 != 0 { "Wrap" } else { "No Wrap" }.into(),
    );
    push(
        tags,
        "AFPointIllumination",
        match (cs[2] & 0x06) >> 1 {
            0 => "Auto".into(),
            1 => "Off".into(),
            2 => "On".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "AFAssist",
        if cs[2] & 0x01 != 0 { "Off" } else { "On" }.into(),
    );

    // Byte 3
    push(
        tags,
        "AF-OnForMB-D10",
        match (cs[3] & 0x70) >> 4 {
            0 => "AF-On".into(),
            1 => "AE/AF Lock".into(),
            2 => "AE Lock Only".into(),
            3 => "AE Lock (reset on release)".into(),
            4 => "AE Lock (hold)".into(),
            5 => "AF Lock Only".into(),
            6 => "Same as FUNC Button".into(),
            v => format!("{v}"),
        },
    );

    // Byte 6
    push(
        tags,
        "ISOStepSize",
        match (cs[6] & 0xc0) >> 6 {
            0 => "1/3 EV".into(),
            1 => "1/2 EV".into(),
            2 => "1 EV".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "ExposureControlStepSize",
        match (cs[6] & 0x30) >> 4 {
            0 => "1/3 EV".into(),
            1 => "1/2 EV".into(),
            2 => "1 EV".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "ExposureCompStepSize",
        match (cs[6] & 0x0c) >> 2 {
            0 => "1/3 EV".into(),
            1 => "1/2 EV".into(),
            2 => "1 EV".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "EasyExposureCompensation",
        match cs[6] & 0x03 {
            0 => "Off".into(),
            1 => "On".into(),
            2 => "On (auto reset)".into(),
            v => format!("{v}"),
        },
    );

    // Byte 7
    push(
        tags,
        "CenterWeightedAreaSize",
        match (cs[7] & 0xe0) >> 5 {
            0 => "6 mm".into(),
            1 => "8 mm".into(),
            2 => "10 mm".into(),
            3 => "13 mm".into(),
            4 => "Average".into(),
            v => format!("{v}"),
        },
    );
    push(tags, "FineTuneOptCenterWeighted", fine_tune(cs[7] & 0x0f));

    // Byte 8
    push(
        tags,
        "FineTuneOptMatrixMetering",
        fine_tune((cs[8] & 0xf0) >> 4),
    );
    push(tags, "FineTuneOptSpotMetering", fine_tune(cs[8] & 0x0f));

    // Byte 9
    push(
        tags,
        "MultiSelectorShootMode",
        match (cs[9] & 0xc0) >> 6 {
            0 => "Select Center Focus Point".into(),
            1 => "Highlight Active Focus Point".into(),
            2 => "Not Used".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "MultiSelectorPlaybackMode",
        match (cs[9] & 0x30) >> 4 {
            0 => "Thumbnail On/Off".into(),
            1 => "View Histograms".into(),
            2 => "Zoom On/Off".into(),
            3 => "Choose Folder".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "InitialZoomSetting",
        match (cs[9] & 0x0c) >> 2 {
            0 => "Low Magnification".into(),
            1 => "Medium Magnification".into(),
            2 => "High Magnification".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "MultiSelector",
        if cs[9] & 0x01 != 0 {
            "Reset Meter-off Delay"
        } else {
            "Do Nothing"
        }
        .into(),
    );

    // Byte 10
    push(
        tags,
        "ExposureDelayMode",
        if cs[10] & 0x40 != 0 { "On" } else { "Off" }.into(),
    );
    push(
        tags,
        "CLModeShootingSpeed",
        format!("{} fps", cs[10] & 0x07),
    );

    // Byte 11
    push(tags, "MaxContinuousRelease", format!("{}", cs[11]));

    // Byte 12
    push(
        tags,
        "ReverseIndicators",
        if cs[12] & 0x20 != 0 { "- 0 +" } else { "+ 0 -" }.into(),
    );
    push(
        tags,
        "FileNumberSequence",
        if cs[12] & 0x08 != 0 { "Off" } else { "On" }.into(),
    );
    push(
        tags,
        "BatteryOrder",
        if cs[12] & 0x04 != 0 {
            "Camera Battery First"
        } else {
            "MB-D10 First"
        }
        .into(),
    );
    push(
        tags,
        "MB-D10Batteries",
        match cs[12] & 0x03 {
            0 => "LR6 (AA alkaline)".into(),
            1 => "HR6 (AA Ni-MH)".into(),
            2 => "FR6 (AA lithium)".into(),
            3 => "ZR6 (AA Ni-Mn)".into(),
            v => format!("{v}"),
        },
    );

    // Byte 13
    push(
        tags,
        "Beep",
        match (cs[13] & 0xc0) >> 6 {
            0 => "High".into(),
            1 => "Low".into(),
            2 => "Off".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "ShootingInfoDisplay",
        match (cs[13] & 0x30) >> 4 {
            0 | 1 => "Auto".into(),
            2 => "Manual (dark on light)".into(),
            3 => "Manual (light on dark)".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "GridDisplay",
        if cs[13] & 0x02 != 0 { "On" } else { "Off" }.into(),
    );
    push(
        tags,
        "ViewfinderWarning",
        if cs[13] & 0x01 != 0 { "Off" } else { "On" }.into(),
    );

    // Byte 14 - FuncButton / FuncButtonPlusDials
    let btn_name = |v: u8| -> String {
        match v {
            0 => "None".into(),
            1 => "Preview".into(),
            2 => "FV Lock".into(),
            3 => "AE/AF Lock".into(),
            4 => "AE Lock Only".into(),
            5 => "AE Lock (reset on release)".into(),
            6 => "AE Lock (hold)".into(),
            7 => "AF Lock Only".into(),
            8 => "AF On".into(),
            9 => "Flash Off".into(),
            10 => "Bracketing Burst".into(),
            11 => "Matrix Metering".into(),
            12 => "Center-weighted Metering".into(),
            13 => "Spot Metering".into(),
            14 => "Playback".into(),
            15 => "My Menu Top".into(),
            16 => "+ NEF (RAW)".into(),
            v => format!("{v}"),
        }
    };
    let dials_name = |v: u8| -> String {
        match v {
            0 => "None".into(),
            2 => "One Step Speed/Aperture".into(),
            3 => "Choose Non-CPU Lens Number".into(),
            5 => "Auto Bracketing".into(),
            6 => "Dynamic AF Area".into(),
            v => format!("{v}"),
        }
    };
    push(tags, "FuncButton", btn_name((cs[14] & 0xf8) >> 3));
    push(tags, "FuncButtonPlusDials", dials_name(cs[14] & 0x07));

    // Byte 15 - PreviewButton / PreviewButtonPlusDials
    push(tags, "PreviewButton", btn_name((cs[15] & 0xf8) >> 3));
    push(tags, "PreviewButtonPlusDials", dials_name(cs[15] & 0x07));

    // Byte 16 - AELockButton / AELockButtonPlusDials
    push(tags, "AELockButton", btn_name((cs[16] & 0xf8) >> 3));
    let ae_dials = |v: u8| -> String {
        match v {
            0 => "None".into(),
            3 => "Choose Non-CPU Lens Number".into(),
            5 => "Auto Bracketing".into(),
            6 => "Dynamic AF Area".into(),
            v => format!("{v}"),
        }
    };
    push(tags, "AELockButtonPlusDials", ae_dials(cs[16] & 0x07));

    // Byte 17
    push(
        tags,
        "CommandDialsReverseRotation",
        if cs[17] & 0x80 != 0 { "Yes" } else { "No" }.into(),
    );
    push(
        tags,
        "CommandDialsChangeMainSub",
        if cs[17] & 0x40 != 0 { "On" } else { "Off" }.into(),
    );
    push(
        tags,
        "CommandDialsApertureSetting",
        if cs[17] & 0x20 != 0 {
            "Aperture Ring"
        } else {
            "Sub-command Dial"
        }
        .into(),
    );
    push(
        tags,
        "CommandDialsMenuAndPlayback",
        if cs[17] & 0x10 != 0 { "On" } else { "Off" }.into(),
    );
    push(
        tags,
        "LCDIllumination",
        if cs[17] & 0x08 != 0 { "On" } else { "Off" }.into(),
    );
    push(
        tags,
        "PhotoInfoPlayback",
        if cs[17] & 0x04 != 0 {
            "Info Left-right, Playback Up-down"
        } else {
            "Info Up-down, Playback Left-right"
        }
        .into(),
    );
    push(
        tags,
        "ShutterReleaseButtonAE-L",
        if cs[17] & 0x02 != 0 { "On" } else { "Off" }.into(),
    );
    push(
        tags,
        "ReleaseButtonToUseDial",
        if cs[17] & 0x01 != 0 { "Yes" } else { "No" }.into(),
    );

    // Byte 18
    push(
        tags,
        "SelfTimerTime",
        match (cs[18] & 0x18) >> 3 {
            0 => "2 s".into(),
            1 => "5 s".into(),
            2 => "10 s".into(),
            3 => "20 s".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "MonitorOffTime",
        match cs[18] & 0x07 {
            0 => "10 s".into(),
            1 => "20 s".into(),
            2 => "1 min".into(),
            3 => "5 min".into(),
            4 => "10 min".into(),
            v => format!("{v}"),
        },
    );

    // Byte 20
    push(
        tags,
        "FlashSyncSpeed",
        match (cs[20] & 0xf0) >> 4 {
            0 => "1/320 s (auto FP)".into(),
            1 => "1/250 s (auto FP)".into(),
            2 => "1/250 s".into(),
            3 => "1/200 s".into(),
            4 => "1/160 s".into(),
            5 => "1/125 s".into(),
            6 => "1/100 s".into(),
            7 => "1/80 s".into(),
            8 => "1/60 s".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "FlashShutterSpeed",
        match cs[20] & 0x0f {
            0 => "1/60 s".into(),
            1 => "1/30 s".into(),
            2 => "1/15 s".into(),
            3 => "1/8 s".into(),
            4 => "1/4 s".into(),
            5 => "1/2 s".into(),
            6 => "1 s".into(),
            7 => "2 s".into(),
            8 => "4 s".into(),
            9 => "8 s".into(),
            10 => "15 s".into(),
            11 => "30 s".into(),
            v => format!("{v}"),
        },
    );

    // Byte 21
    push(
        tags,
        "AutoBracketSet",
        match (cs[21] & 0xc0) >> 6 {
            0 => "AE & Flash".into(),
            1 => "AE Only".into(),
            2 => "Flash Only".into(),
            3 => "WB Bracketing".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "AutoBracketModeM",
        match (cs[21] & 0x30) >> 4 {
            0 => "Flash/Speed".into(),
            1 => "Flash/Speed/Aperture".into(),
            2 => "Flash/Aperture".into(),
            3 => "Flash Only".into(),
            v => format!("{v}"),
        },
    );
    push(
        tags,
        "AutoBracketOrder",
        if cs[21] & 0x08 != 0 { "-,0,+" } else { "0,-,+" }.into(),
    );
    push(
        tags,
        "ModelingFlash",
        if cs[21] & 0x01 != 0 { "Off" } else { "On" }.into(),
    );

    // Byte 22
    push(
        tags,
        "NoMemoryCard",
        if cs[22] & 0x80 != 0 {
            "Enable Release"
        } else {
            "Release Locked"
        }
        .into(),
    );
    push(
        tags,
        "MeteringTime",
        match cs[22] & 0x0f {
            0 => "4 s".into(),
            1 => "6 s".into(),
            2 => "8 s".into(),
            3 => "16 s".into(),
            4 => "30 s".into(),
            5 => "1 min".into(),
            6 => "5 min".into(),
            7 => "10 min".into(),
            8 => "30 min".into(),
            9 => "No Limit".into(),
            v => format!("{v}"),
        },
    );

    // Byte 23
    push(
        tags,
        "InternalFlash",
        match (cs[23] & 0xc0) >> 6 {
            0 => "TTL".into(),
            1 => "Manual".into(),
            2 => "Repeating Flash".into(),
            3 => "Commander Mode".into(),
            v => format!("{v}"),
        },
    );
}

/// Decode Nikon VRInfo (tag 0x001F) - vibration reduction info.
fn decode_nikon_vr_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 7 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    let ver = std::str::from_utf8(&data[..4]).unwrap_or("????");
    push(
        tags,
        "VRInfoVersion",
        ver.trim_end_matches('\0').to_string(),
    );

    push(
        tags,
        "VibrationReduction",
        match data[4] {
            1 => "On".into(),
            2 => "Off".into(),
            v => format!("{v}"),
        },
    );

    if data.len() > 6 {
        push(
            tags,
            "VRMode",
            match data[6] {
                0 => "Normal".into(),
                1 => "Active".into(),
                3 => "Sport".into(),
                v => format!("{v}"),
            },
        );
    }
}

/// Decode Nikon WorldTime (tag 0x0024) - timezone/DST info.
/// Nikon DistortInfo (tag 0x002B) - distortion correction settings.
fn decode_nikon_distort_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 5 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };
    // Offset 4: AutoDistortionControl / DistortionControl
    push(
        tags,
        "DistortionControl",
        match data[4] {
            0 => "Off".into(),
            1 => "On".into(),
            2 => "On (underwater)".into(),
            v => format!("{v}"),
        },
    );
}

fn decode_nikon_world_time(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 4 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Timezone is stored as i16 big-endian (minutes offset from UTC)
    let tz = i16::from_be_bytes([data[0], data[1]]);
    let hours = tz / 60;
    let mins = (tz % 60).abs();
    push(tags, "TimeZone", format!("{hours:+03}:{mins:02}"));

    push(
        tags,
        "DaylightSavings",
        match data[2] {
            0 => "No".into(),
            1 => "Yes".into(),
            v => format!("{v}"),
        },
    );

    push(
        tags,
        "DateDisplayFormat",
        match data[3] {
            0 => "Y/M/D".into(),
            1 => "M/D/Y".into(),
            2 => "D/M/Y".into(),
            v => format!("{v}"),
        },
    );
}

/// Decode Nikon ISOInfo (tag 0x0025) - ISO-related data.
fn decode_nikon_iso_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 7 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Index 2 (bytes 2-3): ISOExpansion
    if data.len() >= 4 {
        let exp = u16::from_be_bytes([data[2], data[3]]);
        let exp_str = match exp {
            0x000 => "Off".into(),
            0x101 => "Hi 0.3".into(),
            0x102 => "Hi 0.5".into(),
            0x103 => "Hi 0.7".into(),
            0x104 => "Hi 1.0".into(),
            0x105 => "Hi 1.3".into(),
            0x106 => "Hi 1.5".into(),
            0x107 => "Hi 1.7".into(),
            0x108 => "Hi 2.0".into(),
            0x109 => "Hi 2.3".into(),
            0x10a => "Hi 2.5".into(),
            0x10b => "Hi 2.7".into(),
            0x10c => "Hi 3.0".into(),
            0x10d => "Hi 3.3".into(),
            0x10e => "Hi 3.5".into(),
            0x10f => "Hi 3.7".into(),
            0x110 => "Hi 4.0".into(),
            0x111 => "Hi 4.3".into(),
            0x112 => "Hi 4.5".into(),
            0x113 => "Hi 4.7".into(),
            0x114 => "Hi 5.0".into(),
            0x201 => "Lo 0.3".into(),
            0x202 => "Lo 0.5".into(),
            0x203 => "Lo 0.7".into(),
            0x204 => "Lo 1.0".into(),
            _ => String::new(),
        };
        if !exp_str.is_empty() {
            push(tags, "ISOExpansion", exp_str);
        }
    }

    // Index 6 (byte 6): ISO2 - val = 100 * 2^(raw/12 - 5)
    let iso2_raw = data[6];
    if iso2_raw > 0 {
        let iso2 = 100.0 * (2.0f64).powf(iso2_raw as f64 / 12.0 - 5.0);
        push(tags, "ISO2", format!("{}", (iso2 + 0.5) as u32));
    }

    // Index 10 (bytes 10-11): ISOExpansion2
    if data.len() >= 12 {
        let exp2 = u16::from_be_bytes([data[10], data[11]]);
        let exp2_str = match exp2 {
            0x000 => "Off".into(),
            0x101 => "Hi 0.3".into(),
            0x102 => "Hi 0.5".into(),
            0x103 => "Hi 0.7".into(),
            0x104 => "Hi 1.0".into(),
            0x105 => "Hi 1.3".into(),
            0x106 => "Hi 1.5".into(),
            0x107 => "Hi 1.7".into(),
            0x108 => "Hi 2.0".into(),
            0x201 => "Lo 0.3".into(),
            0x202 => "Lo 0.5".into(),
            0x203 => "Lo 0.7".into(),
            0x204 => "Lo 1.0".into(),
            _ => String::new(),
        };
        if !exp2_str.is_empty() {
            push(tags, "ISOExpansion2", exp2_str);
        }
    }
}

/// Decode Nikon MultiExposure (tag 0x00B0) - multi-exposure settings.
fn decode_nikon_multi_exposure(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 16 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    let ver = std::str::from_utf8(&data[..4]).unwrap_or("????");
    push(
        tags,
        "MultiExposureVersion",
        ver.trim_end_matches('\0').to_string(),
    );

    let mode = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    push(
        tags,
        "MultiExposureMode",
        match mode {
            0 => "Off".into(),
            1 => "Multiple Exposure".into(),
            2 => "Image Overlay".into(),
            3 => "HDR".into(),
            v => format!("{v}"),
        },
    );

    let shots = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    push(tags, "MultiExposureShots", format!("{shots}"));

    let gain = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    push(
        tags,
        "MultiExposureAutoGain",
        match gain {
            0 => "Off".into(),
            1 => "On".into(),
            v => format!("{v}"),
        },
    );
}

/// Decode Nikon AFInfo2 (tag 0x00B7) - advanced AF info.
fn decode_nikon_af_info2(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 8 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    let ver = std::str::from_utf8(&data[..4]).unwrap_or("????");
    push(
        tags,
        "AFInfo2Version",
        ver.trim_end_matches('\0').to_string(),
    );

    push(
        tags,
        "ContrastDetectAF",
        match data[4] {
            0 => "Off".into(),
            1 => "On".into(),
            2 => "On (2)".into(),
            v => format!("{v}"),
        },
    );

    push(
        tags,
        "AFAreaMode",
        match data[5] {
            0 => "Single Area".into(),
            1 => "Dynamic Area".into(),
            2 => "Dynamic Area (closest subject)".into(),
            3 => "Group Dynamic".into(),
            4 => "Dynamic Area (9 points)".into(),
            5 => "Dynamic Area (21 points)".into(),
            6 => "Dynamic Area (51 points)".into(),
            7 => "Dynamic Area (51 points, 3D-tracking)".into(),
            8 => "Auto-area".into(),
            9 => "Dynamic Area (3D-tracking)".into(),
            10 => "Single Area (wide)".into(),
            11 => "Dynamic Area (wide)".into(),
            12 => "Dynamic Area (wide, 3D-tracking)".into(),
            13 => "Group Area".into(),
            14 => "Dynamic Area (25 points)".into(),
            15 => "Dynamic Area (72 points)".into(),
            16 => "Group Area (HL)".into(),
            17 => "Group Area (VL)".into(),
            18 => "Dynamic Area (49 points)".into(),
            v => format!("{v}"),
        },
    );

    push(
        tags,
        "PhaseDetectAF",
        match data[6] {
            0 => "Off".into(),
            1 => "On (51-point)".into(),
            2 => "On (11-point)".into(),
            3 => "On (39-point)".into(),
            4 => "On (73-point)".into(),
            5 => "On (5)".into(),
            6 => "On (105-point)".into(),
            7 => "On (153-point)".into(),
            8 => "On (81-point)".into(),
            9 => "On (105-point)".into(),
            v => format!("{v}"),
        },
    );

    if data.len() > 7 {
        let af_val = data[7] as u32;
        let phase = data[6];
        let af_str = if af_val == 0 {
            "(none)".into()
        } else {
            match phase {
                1 => nikon_af_point_51(af_val),
                3 => nikon_af_point_39(af_val),
                _ => format!("{af_val}"),
            }
        };
        push(tags, "PrimaryAFPoint", af_str);
    }

    // AFPointsUsed bitmask at offset 10+
    let phase = data[6]; // FocusPointSchema
    if data.len() > 10 {
        let num_bytes = data.len() - 10;
        let mut points = Vec::new();
        for byte_idx in 0..num_bytes.min(7) {
            let byte = data[10 + byte_idx];
            for bit in 0..8 {
                if byte & (1 << bit) != 0 {
                    let idx = byte_idx * 8 + bit + 1; // 1-based
                    let name = match phase {
                        1 => nikon_af_point_51(idx as u32).replace(" (Center)", ""),
                        3 => nikon_af_point_39(idx as u32).replace(" (Center)", ""),
                        _ => format!("{}", idx - 1),
                    };
                    points.push(name);
                }
            }
        }
        if !points.is_empty() {
            // Sort using ExifTool's convention: letter+number, short names get 0-padded for comparison
            points.sort_by(|a, b| {
                if a.len() == b.len() {
                    return a.cmp(b);
                }
                let a_pad = if a.len() == 2 {
                    format!("{}0{}", &a[..1], &a[1..])
                } else {
                    a.clone()
                };
                let b_pad = if b.len() == 2 {
                    format!("{}0{}", &b[..1], &b[1..])
                } else {
                    b.clone()
                };
                a_pad.cmp(&b_pad)
            });
            push(tags, "AFPointsUsed", points.join(","));
        }
    }
}

/// Decode Nikon FileInfo (tag 0x00B8) - file numbering info.
fn decode_nikon_file_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 10 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    let ver = std::str::from_utf8(&data[..4]).unwrap_or("????");
    push(
        tags,
        "FileInfoVersion",
        ver.trim_end_matches('\0').to_string(),
    );

    // FORMAT => int16u, so indices are in 2-byte units
    // Index 2 = byte offset 4
    if data.len() >= 6 {
        let mem = u16::from_be_bytes([data[4], data[5]]);
        push(tags, "MemoryCardNumber", format!("{mem}"));
    }
    if data.len() >= 8 {
        let dir = u16::from_be_bytes([data[6], data[7]]);
        push(tags, "DirectoryNumber", format!("{dir}"));
    }
    if data.len() >= 10 {
        let file = u16::from_be_bytes([data[8], data[9]]);
        push(tags, "FileNumber", format!("{file}"));
    }
}

/// Decode Nikon AFTune (tag 0x00B9) - AF fine-tune settings.
fn decode_nikon_af_tune(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 3 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    push(
        tags,
        "AFFineTune",
        match data[0] {
            0 => "Off".into(),
            1 => "On (saved value)".into(),
            2 => "On (adjust all by same amount)".into(),
            3 => "On (saved value for each lens)".into(),
            v => format!("{v}"),
        },
    );

    let idx = data[1] as i8;
    push(
        tags,
        "AFFineTuneIndex",
        if idx == -1 {
            "n/a".into()
        } else {
            format!("{idx}")
        },
    );

    if data.len() > 2 {
        push(tags, "AFFineTuneAdj", format!("{}", data[2] as i8));
    }
    if data.len() > 3 {
        push(tags, "AFFineTuneAdjTele", format!("{}", data[3] as i8));
    }
}

/// Decode Apple RunTime binary plist (tag 0x0003) - CMTime structure.
///
/// Standard binary plist containing a dict with keys: timescale, value, epoch, flags.
fn decode_apple_runtime(data: &[u8], tags: &mut Vec<DecodedTag>) {
    // Binary plist: "bplist00" magic
    if data.len() < 40 || &data[..6] != b"bplist" {
        return;
    }

    // Read trailer (last 32 bytes; we use last 26 for the fields we need)
    let trailer_off = data.len() - 32;
    // Byte 6 of trailer: offset int size; byte 7: object ref size
    let offset_size = data[trailer_off + 6] as usize;
    let ref_size = data[trailer_off + 7] as usize;

    // Number of objects (bytes 8-15 of trailer, big-endian u64)
    let num_objects = {
        let b = &data[trailer_off + 8..trailer_off + 16];
        u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as usize
    };

    // Top object index (bytes 16-23 of trailer)
    let top_object = {
        let b = &data[trailer_off + 16..trailer_off + 24];
        u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as usize
    };

    // Offset table position (bytes 24-31 of trailer)
    let table_off = {
        let b = &data[trailer_off + 24..trailer_off + 32];
        u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as usize
    };

    if offset_size == 0 || offset_size > 8 || ref_size == 0 || ref_size > 8 {
        return;
    }
    if table_off + num_objects * offset_size > data.len() {
        return;
    }

    // Read offset table
    let read_int_be = |off: usize, size: usize| -> usize {
        let mut val: u64 = 0;
        for i in 0..size {
            val = (val << 8) | data[off + i] as u64;
        }
        val as usize
    };

    let object_offset =
        |idx: usize| -> usize { read_int_be(table_off + idx * offset_size, offset_size) };

    // Parse an object, returning (key_string, integer_value)
    let parse_object = |idx: usize| -> BplistValue {
        if idx >= num_objects {
            return BplistValue::None;
        }
        let off = object_offset(idx);
        if off >= data.len() {
            return BplistValue::None;
        }
        let header = data[off];
        let obj_type = header >> 4;
        let size_nibble = (header & 0x0F) as usize;

        match obj_type {
            0x01 => {
                // Integer: size = 1 << size_nibble
                let byte_count = 1 << size_nibble;
                if off + 1 + byte_count > data.len() {
                    return BplistValue::None;
                }
                let mut val: u64 = 0;
                for i in 0..byte_count {
                    val = (val << 8) | data[off + 1 + i] as u64;
                }
                BplistValue::Int(val)
            }
            0x05 => {
                // ASCII string
                let len = size_nibble;
                if off + 1 + len > data.len() {
                    return BplistValue::None;
                }
                let s = std::str::from_utf8(&data[off + 1..off + 1 + len]).unwrap_or("");
                BplistValue::Str(s.to_string())
            }
            _ => BplistValue::None,
        }
    };

    // Top object should be a dict (type 0x0D)
    let top_off = object_offset(top_object);
    if top_off >= data.len() {
        return;
    }
    let top_header = data[top_off];
    if top_header >> 4 != 0x0D {
        return;
    }
    let dict_count = (top_header & 0x0F) as usize;

    // Dict has dict_count key refs followed by dict_count value refs
    let keys_start = top_off + 1;
    let vals_start = keys_start + dict_count * ref_size;
    if vals_start + dict_count * ref_size > data.len() {
        return;
    }

    for i in 0..dict_count {
        let key_idx = read_int_be(keys_start + i * ref_size, ref_size);
        let val_idx = read_int_be(vals_start + i * ref_size, ref_size);

        let key = parse_object(key_idx);
        let val = parse_object(val_idx);

        if let BplistValue::Str(ref k) = key {
            if let BplistValue::Int(v) = val {
                let tag_name = match k.as_str() {
                    "timescale" => "RunTimeScale",
                    "value" => "RunTimeValue",
                    "epoch" => "RunTimeEpoch",
                    "flags" => "RunTimeFlags",
                    _ => continue,
                };
                let formatted = if k == "flags" {
                    // Bitmask: bit 0 = Valid, bit 1 = Has been rounded, etc.
                    let mut parts = Vec::new();
                    if v & 1 != 0 {
                        parts.push("Valid");
                    }
                    if v & 2 != 0 {
                        parts.push("Has been rounded");
                    }
                    if v & 4 != 0 {
                        parts.push("Positive infinity");
                    }
                    if v & 8 != 0 {
                        parts.push("Negative infinity");
                    }
                    if v & 16 != 0 {
                        parts.push("Indefinite");
                    }
                    if parts.is_empty() {
                        format!("{v}")
                    } else {
                        parts.join(", ")
                    }
                } else {
                    format!("{v}")
                };
                tags.push(DecodedTag {
                    name: tag_name.to_string(),
                    value: formatted,
                });
            }
        }
    }
}

/// Internal helper for binary plist parsing.
enum BplistValue {
    Int(u64),
    Str(String),
    None,
}

/// Get the tag name for a maker note tag ID, given the vendor.
pub fn maker_tag_name(tag: u16, vendor: Vendor) -> &'static str {
    let table: &[(u16, &str)] = match vendor {
        Vendor::Canon => &CANON_TAGS,
        Vendor::Nikon => &NIKON_TAGS,
        Vendor::Sony => &SONY_TAGS,
        Vendor::Fujifilm => &FUJI_TAGS,
        Vendor::Panasonic => &PANASONIC_TAGS,
        Vendor::Olympus => &OLYMPUS_TAGS,
        Vendor::Samsung => &SAMSUNG_TAGS,
        Vendor::Apple => &APPLE_TAGS,
        Vendor::Pentax => &PENTAX_TAGS,
        Vendor::Casio => &CASIO_TAGS,
        Vendor::Minolta => &MINOLTA_TAGS,
        Vendor::Kodak => &KODAK_TAGS,
        Vendor::Sanyo => &SANYO_TAGS,
        Vendor::Ricoh => &RICOH_TAGS,
        Vendor::Sigma => &SIGMA_TAGS,
        Vendor::Motorola => &MOTOROLA_TAGS,
        Vendor::Jvc => &JVC_TAGS,
        Vendor::Reconyx => return "Unknown", // binary format, no IFD tag table
        Vendor::Flir => &FLIR_TAGS,
        Vendor::Ge => &GE_TAGS,
        Vendor::Unknown => return "Unknown",
    };
    lookup_tag(tag, table)
}

fn lookup_tag(tag: u16, table: &'static [(u16, &'static str)]) -> &'static str {
    table
        .iter()
        .find(|&&(id, _)| id == tag)
        .map_or("Unknown", |&(_, name)| name)
}

// -- MN2: Canon tag table ------------------------------------------------

static CANON_TAGS: [(u16, &str); 57] = [
    (0x0001, "CanonCameraSettings"),
    (0x0002, "CanonFocalLength"),
    (0x0003, "CanonFlashInfo"),
    (0x0004, "CanonShotInfo"),
    (0x0005, "Panorama"),
    (0x0006, "CanonImageType"),
    (0x0007, "CanonFirmwareVersion"),
    (0x0008, "FileNumber"),
    (0x0009, "OwnerName"),
    (0x000C, "SerialNumber"),
    (0x000D, "CanonCameraInfo"),
    (0x000E, "CanonFileLength"),
    (0x000F, "CustomFunctions"),
    (0x0010, "CanonModelID"),
    (0x0012, "CanonAFInfo"),
    (0x0013, "ThumbnailImageValidArea"),
    (0x0015, "SerialNumberFormat"),
    (0x0019, "LensInfo"),
    (0x001A, "SuperMacro"),
    (0x001C, "DateStampMode"),
    (0x001D, "MyColors"),
    (0x001E, "FirmwareRevision"),
    (0x0023, "Categories"),
    (0x0024, "FaceDetect1"),
    (0x0026, "AFInfo2"),
    (0x0027, "ContrastInfo"),
    (0x0028, "ImageUniqueID"),
    (0x0029, "WBInfo"),
    (0x002F, "FaceDetect3"),
    (0x0034, "BracketMode"),
    (0x0035, "TimeInfo"),
    (0x0038, "BatteryType"),
    (0x003C, "AFInfo3"),
    (0x0083, "OriginalDecisionDataOffset"),
    (0x0093, "CanonFileInfo"),
    (0x0095, "LensModel"),
    (0x0096, "InternalSerialNumber"),
    (0x0098, "CropInfo"),
    (0x0099, "CustomFunctions2"),
    (0x009A, "AspectInfo"),
    (0x00A0, "ProcessingInfo"),
    (0x00A9, "ColorBalance"),
    (0x00AA, "MeasuredColor"),
    (0x00AE, "ColorTemperature"),
    (0x00B4, "ColorSpace"),
    (0x00D0, "VRDOffset"),
    (0x00E0, "SensorInfo"),
    (0x4001, "ColorData"),
    (0x4013, "AFMicroAdj"),
    (0x4015, "VignettingCorr"),
    (0x4016, "VignettingCorr2"),
    (0x4018, "LightingOpt"),
    (0x4020, "Ambience"),
    (0x4025, "HDRInfo"),
    (0x4008, "PictureStyleUserDef"),
    (0x4009, "PictureStylePC"),
    (0x4010, "CustomPictureStyleFileName"),
];

// -- MN3: Nikon tag table ------------------------------------------------

// Nikon Type 2 (old Coolpix: E950, E880, etc.) - "Nikon\0\x01\0" header
static NIKON_TYPE2_TAGS: [(u16, &str); 8] = [
    (0x0003, "Quality"),
    (0x0004, "ColorMode"),
    (0x0005, "ImageAdjustment"),
    (0x0006, "CCDSensitivity"),
    (0x0007, "WhiteBalance"),
    (0x0008, "Focus"),
    (0x000A, "DigitalZoom"),
    (0x000B, "Converter"),
];

// Nikon Type 3 (modern: "Nikon\0\x02\0" + TIFF header)
static NIKON_TAGS: [(u16, &str); 99] = [
    (0x0001, "MakerNoteVersion"),
    (0x0002, "ISO"),
    (0x0003, "ColorMode"),
    (0x0004, "Quality"),
    (0x0005, "WhiteBalance"),
    (0x0006, "Sharpness"),
    (0x0007, "FocusMode"),
    (0x0008, "FlashSetting"),
    (0x0009, "FlashType"),
    (0x000B, "WhiteBalanceFineTune"),
    (0x000C, "WB_RBLevels"),
    (0x000D, "ProgramShift"),
    (0x000E, "ExposureDifference"),
    (0x000F, "ISOSelection"),
    (0x0010, "DataDump"),
    (0x0011, "PreviewIFD"),
    (0x0012, "FlashExposureComp"),
    (0x0013, "ISOSetting"),
    (0x0016, "ImageBoundary"),
    (0x0017, "ExternalFlashExposureComp"),
    (0x0018, "FlashExposureBracketValue"),
    (0x0019, "ExposureBracketValue"),
    (0x001A, "ImageProcessing"),
    (0x001B, "CropHiSpeed"),
    (0x001C, "ExposureTuning"),
    (0x001D, "SerialNumber"),
    (0x001E, "ColorSpace"),
    (0x001F, "VRInfo"),
    (0x0020, "ImageAuthentication"),
    (0x0021, "FaceDetect"),
    (0x0022, "ActiveD-Lighting"),
    (0x0023, "PictureControlData"),
    (0x0024, "WorldTime"),
    (0x0025, "ISOInfo"),
    (0x002A, "VignetteControl"),
    (0x002B, "DistortInfo"),
    (0x002C, "UnknownInfo"),
    (0x0032, "UnknownInfo2"),
    (0x0034, "ShutterMode"),
    (0x0035, "HDRInfo"),
    (0x0037, "MechanicalShutterCount"),
    (0x0039, "LocationInfo"),
    (0x003D, "BlackLevel"),
    (0x0045, "CropArea"),
    (0x004E, "NikonSettings"),
    (0x004F, "ColorTemperatureAuto"),
    (0x0080, "ImageAdjustment"),
    (0x0081, "ToneComp"),
    (0x0082, "AuxiliaryLens"),
    (0x0083, "LensType"),
    (0x0084, "Lens"),
    (0x0085, "ManualFocusDistance"),
    (0x0086, "DigitalZoom"),
    (0x0087, "FlashMode"),
    (0x0088, "AFInfo"),
    (0x0089, "ShootingMode"),
    (0x008B, "LensFStops"),
    (0x008C, "ContrastCurve"),
    (0x008D, "ColorHue"),
    (0x008F, "SceneMode"),
    (0x0090, "LightSource"),
    (0x0091, "ShotInfo"),
    (0x0092, "HueAdjustment"),
    (0x0093, "NEFCompression"),
    (0x0094, "SaturationAdj"),
    (0x0095, "NoiseReduction"),
    (0x0096, "LinearizationTable"),
    (0x0097, "ColorBalance"),
    (0x0098, "LensData"),
    (0x0099, "RawImageCenter"),
    (0x009A, "SensorPixelSize"),
    (0x009C, "SceneAssist"),
    (0x009D, "DateStampMode"),
    (0x009E, "RetouchHistory"),
    (0x00A0, "SerialNumber"),
    (0x00A2, "ImageDataSize"),
    (0x00A5, "ImageCount"),
    (0x00A6, "DeletedImageCount"),
    (0x00A7, "ShutterCount"),
    (0x00A8, "FlashInfo"),
    (0x00A9, "ImageOptimization"),
    (0x00AA, "Saturation"),
    (0x00AB, "VariProgram"),
    (0x00AC, "ImageStabilization"),
    (0x00AD, "AFResponse"),
    (0x00B0, "MultiExposure"),
    (0x00B1, "HighISONoiseReduction"),
    (0x00B3, "ToningEffect"),
    (0x00B6, "PowerUpTime"),
    (0x00B7, "AFInfo2"),
    (0x00B8, "FileInfo"),
    (0x00B9, "AFTune"),
    (0x00BB, "RetouchInfo"),
    (0x00BD, "PictureControlData2"),
    (0x00C3, "BarometerInfo"),
    (0x0E00, "PrintIM"),
    (0x0E09, "NikonCaptureVersion"),
    (0x0E0E, "NikonCaptureOffsets"),
    (0x0E22, "NEFBitDepth"),
];

// -- MN4: Sony tag table -------------------------------------------------

static SONY_TAGS: [(u16, &str); 34] = [
    (0x0102, "Quality"),
    (0x0104, "FlashExposureComp"),
    (0x0105, "Teleconverter"),
    (0x0112, "WhiteBalanceFineTune"),
    (0x0114, "CameraSettings"),
    (0x0115, "WhiteBalance"),
    (0x0116, "ExtraInfo"),
    (0x0E00, "PrintIM"),
    (0x1000, "MultiBurstMode"),
    (0x1001, "MultiBurstImageWidth"),
    (0x1002, "MultiBurstImageHeight"),
    (0x1003, "Panorama"),
    (0x2000, "AutoHDR"),
    (0x2001, "MultiFrameNoiseReduction"),
    (0x200A, "AutoPortraitFramed"),
    (0x200B, "DistortionCorrParams"),
    (0x200E, "SoftSkinEffect"),
    (0x2027, "FocusLocation"),
    (0x2037, "FocusFrameSize"),
    (0x3000, "ShotInfo"),
    (0xB000, "FileFormat"),
    (0xB001, "SonyModelID"),
    (0xB020, "ColorReproduction"),
    (0xB021, "ColorTemperature"),
    (0xB022, "ColorCompensationFilter"),
    (0xB023, "SceneMode"),
    (0xB024, "ZoneMatching"),
    (0xB025, "DynamicRangeOptimizer"),
    (0xB026, "ImageStabilization"),
    (0xB027, "LensType"),
    (0xB028, "MinFocalLength"),
    (0xB029, "MaxFocalLength"),
    // Continuous-shooting (burst) tags. Values stay raw: `ReleaseMode` is
    // 0 = Normal, 2 = Continuous, 5 = Exposure Bracketing, 6 = White Balance
    // Bracketing, 8 = DRO Bracketing, 65535 = n/a, and `SequenceNumber` is the
    // 1-based frame index inside the burst (0 = single shot, 65535 = n/a).
    (0xB049, "ReleaseMode"),
    (0xB04A, "SequenceNumber"),
];

// -- MN5: Fujifilm tag table ---------------------------------------------

static FUJI_TAGS: [(u16, &str); 35] = [
    (0x0000, "Version"),
    (0x0010, "InternalSerialNumber"),
    (0x1000, "Quality"),
    (0x1001, "Sharpness"),
    (0x1002, "WhiteBalance"),
    (0x1003, "Saturation"),
    (0x1004, "Contrast"),
    (0x1005, "ColorTemperature"),
    (0x100A, "WhiteBalanceFineTune"),
    (0x1010, "FujiFlashMode"),
    (0x1011, "FlashExposureComp"),
    (0x1020, "Macro"),
    (0x1021, "FocusMode"),
    (0x1022, "AFMode"),
    (0x1023, "FocusPixel"),
    (0x1030, "SlowSync"),
    (0x1031, "PictureMode"),
    (0x1032, "ExposureCount"),
    (0x1033, "EXRAuto"),
    (0x1040, "ShadowTone"),
    (0x1041, "HighlightTone"),
    (0x1044, "DigitalZoom"),
    (0x1045, "LensModulationOptimizer"),
    (0x1047, "GrainEffect"),
    (0x1100, "AutoBracketing"),
    (0x1101, "SequenceNumber"),
    (0x1210, "ColorMode"),
    (0x1300, "BlurWarning"),
    (0x1301, "FocusWarning"),
    (0x1302, "ExposureWarning"),
    (0x1400, "DynamicRange"),
    (0x1401, "FilmMode"),
    (0x1402, "DynamicRangeSetting"),
    (0x1404, "MinFocalLength"),
    (0x1405, "MaxFocalLength"),
];

// -- MN6: Panasonic tag table --------------------------------------------

static PANASONIC_TAGS: [(u16, &str); 91] = [
    (0x0001, "ImageQuality"),
    (0x0002, "FirmwareVersion"),
    (0x0003, "WhiteBalance"),
    (0x0007, "FocusMode"),
    (0x000F, "AFAreaMode"),
    (0x001A, "ImageStabilization"),
    (0x001C, "MacroMode"),
    (0x001F, "ShootingMode"),
    (0x0020, "Audio"),
    (0x0023, "WhiteBalanceBias"),
    (0x0024, "FlashBias"),
    (0x0025, "InternalSerialNumber"),
    (0x0026, "PanasonicExifVersion"),
    (0x0027, "VideoFrameRate"),
    (0x0028, "ColorEffect"),
    (0x0029, "TimeSincePowerOn"),
    (0x002A, "BurstMode"),
    (0x002B, "SequenceNumber"),
    (0x002C, "ContrastMode"),
    (0x002D, "NoiseReduction"),
    (0x002E, "SelfTimer"),
    (0x0030, "Rotation"),
    (0x0031, "AFAssistLamp"),
    (0x0032, "ColorMode"),
    (0x0033, "BabyAge"),
    (0x0034, "OpticalZoomMode"),
    (0x0035, "ConversionLens"),
    (0x0036, "TravelDay"),
    (0x0039, "Contrast"),
    (0x003A, "WorldTimeLocation"),
    (0x003B, "TextStamp"),
    (0x003C, "ProgramISO"),
    (0x003D, "AdvancedSceneType"),
    (0x003E, "TextStamp2"),
    (0x003F, "FacesDetected"),
    (0x0040, "Saturation"),
    (0x0041, "Sharpness"),
    (0x0042, "FilmMode"),
    (0x0043, "JPEGQuality"),
    (0x0044, "ColorTempKelvin"),
    (0x0046, "WBAdjustAB"),
    (0x0047, "WBAdjustGM"),
    (0x0048, "FlashCurtain"),
    (0x004B, "PanasonicImageWidth"),
    (0x004C, "PanasonicImageHeight"),
    (0x004D, "AFPointPosition"),
    (0x0051, "LensType"),
    (0x0052, "LensSerialNumber"),
    (0x0053, "AccessoryType"),
    (0x0054, "AccessorySerialNumber"),
    (0x0059, "Transform"),
    (0x005D, "IntelligentExposure"),
    (0x0060, "LensFirmwareVersion"),
    (0x0061, "FacesRecognized"),
    (0x0065, "Title"),
    (0x0066, "BabyName"),
    (0x0067, "Location"),
    (0x0069, "Country"),
    (0x006B, "State"),
    (0x006D, "City"),
    (0x006F, "Landmark"),
    (0x0070, "IntelligentResolution"),
    (0x0077, "BurstSpeed"),
    (0x0079, "IntelligentDRange"),
    (0x007C, "ClearRetouch"),
    (0x0080, "City2"),
    (0x0086, "ManometerPressure"),
    (0x0089, "PhotoStyle"),
    (0x008A, "ShadingCompensation"),
    (0x008C, "AccelerometerZ"),
    (0x008D, "AccelerometerX"),
    (0x008E, "AccelerometerY"),
    (0x008F, "CameraOrientation"),
    (0x0090, "RollAngle"),
    (0x0091, "PitchAngle"),
    (0x0093, "SweepPanoramaDirection"),
    (0x0094, "SweepPanoramaFieldOfView"),
    (0x0096, "TimerRecording"),
    (0x009D, "InternalNDFilter"),
    (0x009E, "HDR"),
    (0x009F, "ShutterType"),
    (0x00A3, "ClearRetouchValue"),
    (0x00AB, "TouchAE"),
    (0x00AF, "TimeStamp"),
    (0x0E00, "PrintIM"),
    (0x8000, "MakerNoteVersion"),
    (0x8001, "SceneMode"),
    (0x8004, "WBRedLevel"),
    (0x8005, "WBGreenLevel"),
    (0x8006, "WBBlueLevel"),
    (0x8012, "Transform"),
];

// -- MN7: Olympus tag table ----------------------------------------------

static OLYMPUS_TAGS: [(u16, &str); 75] = [
    (0x0000, "MakerNoteVersion"),
    (0x0001, "MinoltaCameraSettingsOld"),
    (0x0003, "MinoltaCameraSettings"),
    (0x0040, "CompressedImageSize"),
    (0x0081, "PreviewImageData"),
    (0x0088, "PreviewImageStart"),
    (0x0089, "PreviewImageLength"),
    (0x0100, "ThumbnailImage"),
    (0x0104, "BodyFirmwareVersion"),
    (0x0200, "SpecialMode"),
    (0x0201, "Quality"),
    (0x0202, "Macro"),
    (0x0203, "BWMode"),
    (0x0204, "DigitalZoom"),
    (0x0205, "FocalPlaneDiagonal"),
    (0x0206, "LensDistortionParams"),
    (0x0207, "CameraType"),
    (0x0208, "PictureInfo"),
    (0x0209, "CameraID"),
    (0x020B, "EpsonImageWidth"),
    (0x020C, "EpsonImageHeight"),
    (0x020D, "EpsonSoftware"),
    (0x0300, "PreCaptureFrames"),
    (0x0403, "SceneMode"),
    (0x0404, "SerialNumber"),
    (0x1000, "ShutterSpeedValue"),
    (0x1001, "ISOValue"),
    (0x1002, "ApertureValue"),
    (0x1003, "BrightnessValue"),
    (0x1004, "FlashMode"),
    (0x1005, "FlashDevice"),
    (0x1006, "ExposureCompensation"),
    (0x1007, "SensorTemperature"),
    (0x1008, "LensTemperature"),
    (0x1009, "LightCondition"),
    (0x100A, "FocusRange"),
    (0x100B, "FocusMode"),
    (0x100C, "ManualFocusDistance"),
    (0x100D, "ZoomStepCount"),
    (0x100E, "FocusStepCount"),
    (0x100F, "Sharpness"),
    (0x1010, "FlashChargeLevel"),
    (0x1011, "ColorMatrix"),
    (0x1012, "BlackLevel"),
    (0x1015, "WBMode"),
    (0x1017, "RedBalance"),
    (0x1018, "BlueBalance"),
    (0x1019, "ColorMatrixNumber"),
    (0x101A, "SerialNumber"),
    (0x1023, "FlashExposureComp"),
    (0x1024, "InternalFlashTable"),
    (0x1025, "ExternalFlashGValue"),
    (0x1026, "ExternalFlashBounce"),
    (0x1027, "ExternalFlashZoom"),
    (0x1028, "ExternalFlashMode"),
    (0x1029, "Contrast"),
    (0x102A, "SharpnessFactor"),
    (0x102B, "ColorControl"),
    (0x102C, "ValidBits"),
    (0x102D, "CoringFilter"),
    (0x102E, "OlympusImageWidth"),
    (0x102F, "OlympusImageHeight"),
    (0x1030, "SceneDetect"),
    (0x1038, "AFResult"),
    (0x103B, "FocusStepInfinity"),
    (0x103C, "FocusStepNear"),
    (0x103D, "LightValueCenter"),
    (0x103E, "LightValuePeriphery"),
    (0x2010, "Equipment"),
    (0x2020, "CameraSettings"),
    (0x2030, "RawDevelopment"),
    (0x2031, "RawDevelopment2"),
    (0x2040, "ImageProcessing"),
    (0x2050, "FocusInfo"),
    (0x2100, "Olympus2100"),
];

// Olympus Equipment sub-IFD (0x2010) tags
static OLYMPUS_EQUIPMENT_TAGS: [(u16, &str); 25] = [
    (0x0000, "EquipmentVersion"),
    (0x0100, "CameraType2"),
    (0x0101, "SerialNumber"),
    (0x0102, "InternalSerialNumber"),
    (0x0103, "FocalPlaneDiagonal"),
    (0x0104, "BodyFirmwareVersion"),
    (0x0201, "LensType"),
    (0x0202, "LensSerialNumber"),
    (0x0203, "LensModel"),
    (0x0204, "LensFirmwareVersion"),
    (0x0205, "MaxApertureAtMinFocal"),
    (0x0206, "MaxApertureAtMaxFocal"),
    (0x0207, "MinFocalLength"),
    (0x0208, "MaxFocalLength"),
    (0x020A, "MaxAperture"),
    (0x020B, "LensProperties"),
    (0x0301, "Extender"),
    (0x0302, "ExtenderSerialNumber"),
    (0x0303, "ExtenderModel"),
    (0x0304, "ExtenderFirmwareVersion"),
    (0x0403, "ConversionLens"),
    (0x1000, "FlashType"),
    (0x1001, "FlashModel"),
    (0x1002, "FlashFirmwareVersion"),
    (0x1003, "FlashSerialNumber"),
];

// Olympus CameraSettings sub-IFD (0x2020) tags
static OLYMPUS_CAMERA_SETTINGS_TAGS: [(u16, &str); 58] = [
    (0x0000, "CameraSettingsVersion"),
    (0x0100, "PreviewImageValid"),
    (0x0101, "PreviewImageStart"),
    (0x0102, "PreviewImageLength"),
    (0x0200, "ExposureMode"),
    (0x0201, "AELock"),
    (0x0202, "MeteringMode"),
    (0x0203, "ExposureShift"),
    (0x0204, "NDFilter"),
    (0x0300, "MacroMode"),
    (0x0301, "FocusMode"),
    (0x0302, "FocusProcess"),
    (0x0303, "AFSearch"),
    (0x0305, "AFPointSelected"),
    (0x0306, "AFFineTune"),
    (0x0307, "AFFineTuneAdj"),
    (0x0400, "FlashMode"),
    (0x0401, "FlashExposureComp"),
    (0x0500, "WhiteBalance2"),
    (0x0501, "WhiteBalanceTemperature"),
    (0x0502, "WhiteBalanceBracket"),
    (0x0503, "CustomSaturation"),
    (0x0504, "ModifiedSaturation"),
    (0x0505, "ContrastSetting"),
    (0x0506, "SharpnessSetting"),
    (0x0507, "ColorSpace"),
    (0x0509, "SceneMode"),
    (0x050A, "NoiseReduction"),
    (0x050B, "DistortionCorrection"),
    (0x050C, "ShadingCompensation"),
    (0x050D, "CompressionFactor"),
    (0x0520, "PictureMode"),
    (0x0600, "DriveMode"),
    (0x0601, "PanoramaMode"),
    (0x0603, "ImageQuality2"),
    (0x0403, "FlashRemoteControl"),
    (0x0404, "FlashControlMode"),
    (0x0405, "FlashIntensity"),
    (0x0406, "ManualFlashStrength"),
    (0x0604, "ImageStabilization"),
    (0x0308, "FocusBracketStepSize"),
    (0x050F, "Gradation"),
    (0x0521, "PictureModeSaturation"),
    (0x0522, "PictureModeHue"),
    (0x0523, "PictureModeContrast"),
    (0x0524, "PictureModeSharpness"),
    (0x0525, "PictureModeBWFilter"),
    (0x0526, "PictureModeTone"),
    (0x0527, "NoiseFilter"),
    (0x0529, "ArtFilter"),
    (0x052C, "MagicFilter"),
    (0x052D, "PictureModeEffect"),
    (0x052E, "ToneLevel"),
    (0x052F, "ArtFilterEffect"),
    (0x0900, "ManometerPressure"),
    (0x0901, "ManometerReading"),
    (0x0902, "ExtendedWBDetect"),
    (0x0903, "RollAngle"),
];

// Olympus ImageProcessing sub-IFD (0x2040) tags
static OLYMPUS_IMAGE_PROCESSING_TAGS: [(u16, &str); 36] = [
    (0x0000, "ImageProcessingVersion"),
    (0x0100, "WB_RBLevels"),
    (0x010D, "WB_RBLevels7500K"),
    (0x011F, "WB_GLevel"),
    (0x0200, "ColorMatrix"),
    (0x0300, "Enhancer"),
    (0x0310, "CoringFilter"),
    (0x0500, "OlympusImageWidth"),
    (0x0501, "OlympusImageHeight"),
    (0x050D, "CompressionFactor"),
    (0x050F, "Gradation"),
    (0x0600, "BlackLevel2"),
    (0x0610, "GainBase"),
    (0x0611, "ValidBits"),
    (0x0612, "CropLeft"),
    (0x0613, "CropTop"),
    (0x0614, "CropWidth"),
    (0x0615, "CropHeight"),
    (0x0805, "SensorCalibration"),
    (0x1010, "NoiseReduction2"),
    (0x1011, "DistortionCorrection2"),
    (0x1012, "ShadingCompensation2"),
    (0x101C, "MultipleExposureMode"),
    (0x102A, "SharpnessFactor"),
    (0x103D, "LightValueCenter"),
    (0x103E, "LightValuePeriphery"),
    (0x1112, "AspectRatio"),
    (0x1113, "AspectFrame"),
    (0x1200, "FacesDetected"),
    (0x1202, "MaxFaces"),
    (0x1203, "FaceDetectFrameSize"),
    (0x1207, "FaceDetectFrameCrop"),
    (0x1306, "CameraTemperature"),
    (0x1900, "ColorControl"),
    (0x1906, "ExtendedWBDetect"),
    (0x1908, "LevelGaugeRoll"),
];

// Olympus FocusInfo sub-IFD (0x2050) tags
static OLYMPUS_FOCUS_INFO_TAGS: [(u16, &str); 22] = [
    (0x0000, "FocusInfoVersion"),
    (0x0209, "AutoFocus"),
    (0x0210, "SceneDetect"),
    (0x0300, "ZoomStepCount"),
    (0x0301, "FocusStepCount"),
    (0x0303, "FocusStepInfinity"),
    (0x0304, "FocusStepNear"),
    (0x0305, "FocusDistance"),
    (0x0308, "AFPoint"),
    (0x031B, "AFPointDetails"),
    (0x030D, "AFAreas"),
    (0x1201, "ExternalFlash"),
    (0x1203, "ExternalFlashGuideNumber"),
    (0x1204, "ExternalFlashBounce"),
    (0x1205, "ExternalFlashZoom"),
    (0x1208, "InternalFlash"),
    (0x1209, "ManualFlash"),
    (0x120A, "MacroLED"),
    (0x0900, "ManometerPressure"),
    (0x0901, "ManometerReading"),
    (0x030C, "AFResult"),
    (0x1500, "SensorTemperature"),
];

// Olympus RawDevelopment sub-IFD (0x2030) tags
static OLYMPUS_RAW_DEVELOPMENT_TAGS: [(u16, &str); 14] = [
    (0x0000, "RawDevVersion"),
    (0x0100, "RawDevExposureBiasValue"),
    (0x0101, "RawDevWhiteBalanceValue"),
    (0x0102, "RawDevWBFineAdjustment"),
    (0x0103, "RawDevGrayPoint"),
    (0x0104, "RawDevSaturationEmphasis"),
    (0x0105, "RawDevMemoryColorEmphasis"),
    (0x0106, "RawDevContrastValue"),
    (0x0107, "RawDevSharpnessValue"),
    (0x0108, "RawDevColorSpace"),
    (0x0109, "RawDevEngine"),
    (0x010A, "RawDevNoiseReduction"),
    (0x010B, "RawDevEditStatus"),
    (0x010C, "RawDevSettings"),
];

// -- MN8: Samsung tag table ----------------------------------------------

static SAMSUNG_TAGS: [(u16, &str); 12] = [
    (0x0001, "MakerNoteVersion"),
    (0x0021, "PictureWizard"),
    (0x0030, "LocalLocationName"),
    (0x0031, "LocationName"),
    (0x0035, "PreviewIFD"),
    (0x0040, "RawDataByteOrder"),
    (0x0043, "CameraTemperature"),
    (0x0050, "RawDataCFAPattern"),
    (0x0100, "FaceDetect"),
    (0x0120, "FaceRecognition"),
    (0x0A01, "FirmwareName"),
    (0x0A02, "SerialNumber"),
];

// -- MN9: Apple tag table ------------------------------------------------

static APPLE_TAGS: [(u16, &str); 37] = [
    (0x0001, "MakerNoteVersion"),
    (0x0002, "AEMatrix"),
    (0x0003, "RunTime"),
    (0x0004, "AEStable"),
    (0x0005, "AETarget"),
    (0x0006, "AEAverage"),
    (0x0007, "AFStable"),
    (0x0008, "AccelerationVector"),
    (0x000A, "HDRImageType"),
    (0x000B, "BurstUUID"),
    (0x000C, "FocusDistanceRange"),
    (0x000F, "OISMode"),
    (0x0011, "ContentIdentifier"),
    (0x0014, "ImageCaptureType"),
    (0x0015, "ImageUniqueID"),
    (0x0017, "LivePhotoVideoIndex"),
    (0x001A, "QualityHint"),
    (0x001D, "LuminanceNoiseAmplitude"),
    (0x001F, "PhotosAppFeatureFlags"),
    (0x0020, "ImageCaptureRequestID"),
    (0x0021, "HDRHeadroom"),
    (0x0023, "AFPerformance"),
    (0x0025, "SceneFlags"),
    (0x0026, "SignalToNoiseRatioType"),
    (0x0027, "SignalToNoiseRatio"),
    (0x002B, "PhotoIdentifier"),
    (0x002D, "ColorTemperature"),
    (0x002E, "CameraType"),
    (0x002F, "FocusPosition"),
    (0x0030, "HDRGain"),
    (0x0038, "AFMeasuredDepth"),
    (0x003D, "AFConfidence"),
    (0x0040, "SemanticStyle"),
    (0x0041, "SemanticStyleRenderingVer"),
    (0x0042, "SemanticStylePreset"),
    (0x0045, "FrontFacingCamera"),
    (0x004E, "CameraIdentifier"),
];

// -- Pentax tag table ----------------------------------------------------

static PENTAX_TAGS: [(u16, &str); 95] = [
    (0x0000, "PentaxVersion"),
    (0x0001, "PentaxModelType"),
    (0x0002, "PreviewImageSize"),
    (0x0003, "PreviewImageLength"),
    (0x0004, "PreviewImageStart"),
    (0x0005, "PentaxModelID"),
    (0x0006, "Date"),
    (0x0007, "Time"),
    (0x0008, "Quality"),
    (0x0009, "PentaxImageSize"),
    (0x000B, "PictureMode"),
    (0x000C, "FlashMode"),
    (0x000D, "FocusMode"),
    (0x000E, "AFPointSelected"),
    (0x000F, "AFPointsInFocus"),
    (0x0010, "FocusPosition"),
    (0x0012, "ExposureTime"),
    (0x0013, "FNumber"),
    (0x0014, "ISO"),
    (0x0015, "LightReading"),
    (0x0016, "ExposureCompensation"),
    (0x0017, "MeteringMode"),
    (0x0018, "AutoBracketing"),
    (0x0019, "WhiteBalance"),
    (0x001A, "WhiteBalanceMode"),
    (0x001B, "BlueBalance"),
    (0x001C, "RedBalance"),
    (0x001D, "FocalLength"),
    (0x001E, "DigitalZoom"),
    (0x001F, "Saturation"),
    (0x0020, "Contrast"),
    (0x0021, "Sharpness"),
    (0x0022, "WorldTimeLocation"),
    (0x0023, "HometownCity"),
    (0x0024, "DestinationCity"),
    (0x0025, "HometownDST"),
    (0x0026, "DestinationDST"),
    (0x0027, "DSPFirmwareVersion"),
    (0x0028, "CPUFirmwareVersion"),
    (0x0029, "FrameNumber"),
    (0x002D, "EffectiveLV"),
    (0x0032, "ImageEditing"),
    (0x0033, "PictureMode"),
    (0x0034, "DriveMode"),
    (0x0037, "ColorSpace"),
    (0x0038, "ImageAreaOffset"),
    (0x0039, "RawImageSize"),
    (0x003C, "AFPointsInFocus"),
    (0x003D, "DataScaling"),
    (0x003E, "PreviewImageBorders"),
    (0x003F, "LensRec"),
    (0x0040, "SensitivityAdjust"),
    (0x0041, "ImageEditCount"),
    (0x0047, "CameraTemperature"),
    (0x0048, "AELock"),
    (0x0049, "NoiseReduction"),
    (0x004D, "FlashExposureComp"),
    (0x004F, "ImageTone"),
    (0x0050, "ColorTemperature"),
    (0x005C, "ShakeReductionInfo"),
    (0x005D, "ShutterCount"),
    (0x005E, "LensInfo2"),
    (0x0060, "FaceInfo"),
    (0x0062, "RawDevelopmentProcess"),
    (0x0067, "Hue"),
    (0x0069, "DynamicRangeExpansion"),
    (0x006B, "TimeInfo"),
    (0x006D, "ContrastHighlight"),
    (0x006F, "ContrastHighlightShadowAdj"),
    (0x0071, "HighISONoiseReduction"),
    (0x0073, "MonochromeFilterEffect"),
    (0x0077, "FaceDetectFrameSize"),
    (0x0079, "ShadowCorrection"),
    (0x007F, "BleachBypassToning"),
    (0x0200, "BlackPoint"),
    (0x0201, "WhitePoint"),
    (0x0205, "CameraSettings"),
    (0x0206, "AEInfo"),
    (0x0207, "LensInfo"),
    (0x0208, "FlashInfo"),
    (0x0209, "AEMeteringSegments"),
    (0x020A, "FlashMeteringSegments"),
    (0x020B, "SlaveFlashMeteringSegments"),
    (0x020D, "WB_RGGBLevelsDaylight"),
    (0x020E, "WB_RGGBLevelsShade"),
    (0x020F, "WB_RGGBLevelsCloudy"),
    (0x0210, "WB_RGGBLevelsTungsten"),
    (0x0211, "WB_RGGBLevelsFluorescentD"),
    (0x0212, "WB_RGGBLevelsFluorescentN"),
    (0x0213, "WB_RGGBLevelsFluorescentW"),
    (0x0214, "WB_RGGBLevelsFlash"),
    (0x0215, "CameraInfo"),
    (0x0216, "BatteryInfo"),
    (0x021f, "AFInfo"),
    (0x0222, "ColorInfo"),
];

// -- Casio tag table -----------------------------------------------------

static CASIO_TAGS: [(u16, &str); 47] = [
    (0x0001, "RecordingMode"),
    (0x0002, "Quality"),
    (0x0003, "FocusMode"),
    (0x0004, "FlashMode"),
    (0x0005, "FlashIntensity"),
    (0x0006, "ObjectDistance"),
    (0x0007, "WhiteBalance"),
    (0x000A, "DigitalZoom"),
    (0x000B, "Sharpness"),
    (0x000C, "Contrast"),
    (0x000D, "Saturation"),
    (0x0014, "ISO"),
    (0x0015, "Color"),
    (0x0016, "Enhancement"),
    (0x0017, "Filter"),
    (0x0019, "CameraVersion"),
    (0x0E00, "PrintIM"),
    (0x2000, "PreviewThumbnailDimensions"),
    (0x2001, "FirmwareDate"),
    (0x2002, "PreviewThumbnailOffset"),
    (0x2003, "QualityMode"),
    (0x2004, "ImageSize"),
    (0x2011, "WhiteBalanceBias"),
    (0x2012, "WhiteBalance"),
    (0x2021, "AFPointPosition"),
    (0x2022, "ObjectDistance"),
    (0x2034, "FlashDistance"),
    (0x3000, "RecordMode"),
    (0x3001, "ReleaseMode"),
    (0x3002, "Quality"),
    (0x3003, "FocusMode"),
    (0x3006, "HometownCity"),
    (0x3007, "BestShotMode"),
    (0x3008, "AutoISO"),
    (0x3009, "AFMode"),
    (0x3014, "ISO"),
    (0x3015, "ColorMode"),
    (0x3016, "Enhancement"),
    (0x3017, "ColorFilter"),
    (0x301C, "SequenceNumber"),
    (0x301D, "BracketSequence"),
    (0x3020, "ImageStabilization"),
    (0x3103, "DriveMode"),
    (0x301B, "ArtMode"),
    (0x310B, "ArtModeParameters"),
    (0x4001, "CaptureFrameRate"),
    (0x4003, "VideoQuality"),
];

// -- Minolta tag table ---------------------------------------------------

static MINOLTA_TAGS: [(u16, &str); 25] = [
    (0x0000, "MakerNoteVersion"),
    (0x0001, "MinoltaCameraSettingsOld"),
    (0x0003, "MinoltaCameraSettings"),
    (0x0004, "MinoltaCameraSettings7D"),
    (0x0018, "ImageStabilization"),
    (0x0040, "CompressedImageSize"),
    (0x0081, "PreviewImage"),
    (0x0088, "PreviewImageStart"),
    (0x0089, "PreviewImageLength"),
    (0x0100, "SceneMode"),
    (0x0101, "ColorMode"),
    (0x0102, "MinoltaQuality"),
    (0x0103, "MinoltaImageSize"),
    (0x0104, "FlashExposureComp"),
    (0x0105, "Teleconverter"),
    (0x0107, "ImageStabilization2"),
    (0x0109, "RawAndJpgRecording"),
    (0x010A, "ZoneMatching"),
    (0x010B, "ColorTemperature"),
    (0x010C, "LensType"),
    (0x0112, "WhiteBalanceFineTune"),
    (0x0113, "ImageStabilization3"),
    (0x0114, "MinoltaCameraSettings5D"),
    (0x0115, "WhiteBalance"),
    (0x0E00, "PrintIM"),
];

// -- Kodak binary maker notes --------------------------------------------

/// Detect Kodak binary maker note type and decode tags.
/// Types: 1 (KDK header), 3 (DC240/DC280), 4 (DC200/DC215), others.
fn decode_kodak_binary(data: &[u8], tags: &mut Vec<DecodedTag>) {
    // Type 1: starts with "KDK INFO" or "KDK " - data offset 8, big-endian or little-endian
    if data.len() > 8 && data.starts_with(b"KDK") {
        let be = data.starts_with(b"KDK INFO");
        let d = &data[8..];
        decode_kodak_type1(d, be, tags);
        return;
    }

    // Type 4: DC200/DC215 - "Eastman Kodak" (mixed case Make), bytes[41..44] == "JPG"
    if data.len() > 44 && data.get(41..44) == Some(b"JPG") {
        decode_kodak_type4(data, tags);
        return;
    }

    // Type 3: DC240/DC280/DC3400/DC5000 - byte at offset 12 is 0x07 (year upper byte)
    if data.len() > 0x50 && data.get(12) == Some(&0x07) {
        decode_kodak_type3(data, tags);
        return;
    }

    // Type 9: starts with "IIII\x02\x00" or "IIII\x03\x00"
    if data.len() > 0x36 && data.starts_with(b"IIII") {
        decode_kodak_type9(data, tags);
        return;
    }

    // Type 7: serial number pattern at start
    if data.len() >= 16 {
        let s = &data[..16];
        if s.iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b' ' || b == 0)
        {
            let serial = String::from_utf8_lossy(s)
                .trim_end_matches('\0')
                .trim()
                .to_string();
            if !serial.is_empty() && serial.len() >= 6 {
                tags.push(DecodedTag {
                    name: "SerialNumber".into(),
                    value: serial,
                });
            }
        }
    }
}

fn decode_kodak_type1(d: &[u8], be: bool, tags: &mut Vec<DecodedTag>) {
    let u16at = |off: usize| -> Option<u16> {
        if off + 2 > d.len() {
            return None;
        }
        Some(if be {
            u16::from_be_bytes([d[off], d[off + 1]])
        } else {
            u16::from_le_bytes([d[off], d[off + 1]])
        })
    };
    let i16at = |off: usize| -> Option<i16> {
        if off + 2 > d.len() {
            return None;
        }
        Some(if be {
            i16::from_be_bytes([d[off], d[off + 1]])
        } else {
            i16::from_le_bytes([d[off], d[off + 1]])
        })
    };
    let u32at = |off: usize| -> Option<u32> {
        if off + 4 > d.len() {
            return None;
        }
        Some(if be {
            u32::from_be_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
        } else {
            u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
        })
    };

    // 0x00: KodakModel (string[8])
    if d.len() >= 8 {
        let model = String::from_utf8_lossy(&d[0..8])
            .trim_end_matches('\0')
            .trim()
            .to_string();
        if !model.is_empty() {
            tags.push(DecodedTag {
                name: "KodakModel".into(),
                value: model,
            });
        }
    }
    // 0x09: Quality
    if let Some(&v) = d.get(0x09) {
        tags.push(DecodedTag {
            name: "Quality".into(),
            value: match v {
                1 => "Fine".into(),
                2 => "Normal".into(),
                _ => format!("{v}"),
            },
        });
    }
    // 0x0A: BurstMode
    if let Some(&v) = d.get(0x0A) {
        tags.push(DecodedTag {
            name: "BurstMode".into(),
            value: match v {
                0 => "Off".into(),
                1 => "On".into(),
                _ => format!("{v}"),
            },
        });
    }
    // 0x0C: KodakImageWidth
    if let Some(v) = u16at(0x0C) {
        tags.push(DecodedTag {
            name: "KodakImageWidth".into(),
            value: format!("{v}"),
        });
    }
    // 0x0E: KodakImageHeight
    if let Some(v) = u16at(0x0E) {
        tags.push(DecodedTag {
            name: "KodakImageHeight".into(),
            value: format!("{v}"),
        });
    }
    // 0x10: YearCreated
    if let Some(v) = u16at(0x10) {
        tags.push(DecodedTag {
            name: "YearCreated".into(),
            value: format!("{v}"),
        });
    }
    // 0x12: MonthDayCreated (2 bytes)
    if d.len() > 0x13 {
        tags.push(DecodedTag {
            name: "MonthDayCreated".into(),
            value: format!("{:02}:{:02}", d[0x12], d[0x13]),
        });
    }
    // 0x14: TimeCreated (4 bytes)
    if d.len() > 0x17 {
        tags.push(DecodedTag {
            name: "TimeCreated".into(),
            value: format!(
                "{:02}:{:02}:{:02}.{:02}",
                d[0x14], d[0x15], d[0x16], d[0x17]
            ),
        });
    }
    // 0x1B: ShutterMode
    if let Some(&v) = d.get(0x1B) {
        tags.push(DecodedTag {
            name: "ShutterMode".into(),
            value: match v {
                0 => "Auto".into(),
                8 => "Aperture Priority".into(),
                32 => "Manual".into(),
                _ => format!("{v}"),
            },
        });
    }
    // 0x1C: MeteringMode
    if let Some(&v) = d.get(0x1C) {
        tags.push(DecodedTag {
            name: "MeteringMode".into(),
            value: match v {
                0 => "Multi-segment".into(),
                1 => "Center-weighted average".into(),
                2 => "Spot".into(),
                _ => format!("{v}"),
            },
        });
    }
    // 0x1D: SequenceNumber
    if let Some(&v) = d.get(0x1D) {
        tags.push(DecodedTag {
            name: "SequenceNumber".into(),
            value: format!("{v}"),
        });
    }
    // 0x1E: FNumber (u16 / 100)
    if let Some(v) = u16at(0x1E) {
        let f = v as f64 / 100.0;
        // Format like ExifTool: no trailing zeros beyond one decimal
        let s = format!("{f:.2}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        tags.push(DecodedTag {
            name: "FNumber".into(),
            value: s.to_string(),
        });
    }
    // 0x20: ExposureTime (u32 / 1e5)
    if let Some(v) = u32at(0x20) {
        let t = v as f64 / 100000.0;
        tags.push(DecodedTag {
            name: "ExposureTime".into(),
            value: crate::tiff::tags::format_exposure_time(t).unwrap_or_else(|| format!("{t}")),
        });
    }
    // 0x24: ExposureCompensation (i16 / 1000)
    if let Some(v) = i16at(0x24) {
        let c = v as f64 / 1000.0;
        tags.push(DecodedTag {
            name: "ExposureCompensation".into(),
            value: format!("{c}"),
        });
    }
    // 0x38: FocusMode
    if let Some(&v) = d.get(0x38) {
        tags.push(DecodedTag {
            name: "FocusMode".into(),
            value: match v {
                0 => "Normal".into(),
                2 => "Macro".into(),
                _ => format!("{v}"),
            },
        });
    }
    // 0x40: WhiteBalance
    if let Some(&v) = d.get(0x40) {
        tags.push(DecodedTag {
            name: "WhiteBalance".into(),
            value: match v {
                0 => "Auto".into(),
                1 => "Flash".into(),
                2 => "Tungsten".into(),
                3 => "Daylight".into(),
                _ => format!("{v}"),
            },
        });
    }
    // 0x5C: FlashMode
    if let Some(&v) = d.get(0x5C) {
        tags.push(DecodedTag {
            name: "FlashMode".into(),
            value: match v {
                0x00 => "Auto".into(),
                0x01 | 0x10 => "Fill Flash".into(),
                0x02 | 0x20 => "Off".into(),
                0x03 => "Red-Eye".into(),
                0x40 => "Red-Eye".into(),
                _ => format!("{v}"),
            },
        });
    }
    // 0x5D: FlashFired
    if let Some(&v) = d.get(0x5D) {
        tags.push(DecodedTag {
            name: "FlashFired".into(),
            value: match v {
                0 => "No".into(),
                1 => "Yes".into(),
                _ => format!("{v}"),
            },
        });
    }
    // 0x5E: ISOSetting
    if let Some(v) = u16at(0x5E) {
        tags.push(DecodedTag {
            name: "ISOSetting".into(),
            value: if v == 0 {
                "Auto".into()
            } else {
                format!("{v}")
            },
        });
    }
    // 0x60: ISO
    if let Some(v) = u16at(0x60) {
        tags.push(DecodedTag {
            name: "ISO".into(),
            value: format!("{v}"),
        });
    }
    // 0x62: TotalZoom (u16 / 100)
    if let Some(v) = u16at(0x62) {
        let z = v as f64 / 100.0;
        let s = format!("{z:.1}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        tags.push(DecodedTag {
            name: "TotalZoom".into(),
            value: s.to_string(),
        });
    }
    // 0x64: DateTimeStamp
    if let Some(v) = u16at(0x64) {
        tags.push(DecodedTag {
            name: "DateTimeStamp".into(),
            value: if v == 0 {
                "Off".into()
            } else {
                format!("Mode {v}")
            },
        });
    }
    // 0x66: ColorMode
    if let Some(v) = u16at(0x66) {
        tags.push(DecodedTag {
            name: "ColorMode".into(),
            value: match v {
                0x0001 => "B&W".into(),
                0x0002 => "Sepia".into(),
                0x0003 => "B&W Yellow Filter".into(),
                0x0004 => "B&W Red Filter".into(),
                0x0020 | 0x0100 => "Saturated Color".into(),
                0x0040 | 0x0200 => "Neutral Color".into(),
                0x2000 => "B&W".into(),
                0x4000 => "Sepia".into(),
                _ => format!("{v}"),
            },
        });
    }
    // 0x68: DigitalZoom (u16 / 100)
    if let Some(v) = u16at(0x68) {
        let z = v as f64 / 100.0;
        let s = format!("{z:.1}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        tags.push(DecodedTag {
            name: "DigitalZoom".into(),
            value: s.to_string(),
        });
    }
    // 0x6B: Sharpness (int8s)
    if let Some(&raw) = d.get(0x6B) {
        let v = raw as i8;
        tags.push(DecodedTag {
            name: "Sharpness".into(),
            value: match v {
                i if i < 0 => "Soft".into(),
                0 => "Normal".into(),
                _ => "Hard".into(),
            },
        });
    }
}

fn decode_kodak_type3(d: &[u8], tags: &mut Vec<DecodedTag>) {
    // Type 3: DC240, DC280, DC3400, DC5000 - big-endian
    let u16be = |off: usize| -> Option<u16> {
        if off + 2 > d.len() {
            return None;
        }
        Some(u16::from_be_bytes([d[off], d[off + 1]]))
    };
    let u32be = |off: usize| -> Option<u32> {
        if off + 4 > d.len() {
            return None;
        }
        Some(u32::from_be_bytes([
            d[off],
            d[off + 1],
            d[off + 2],
            d[off + 3],
        ]))
    };

    // 0x0C: YearCreated
    if let Some(v) = u16be(0x0C) {
        tags.push(DecodedTag {
            name: "YearCreated".into(),
            value: format!("{v}"),
        });
    }
    // 0x0E: MonthDayCreated
    if d.len() > 0x0F {
        tags.push(DecodedTag {
            name: "MonthDayCreated".into(),
            value: format!("{:02}:{:02}", d[0x0E], d[0x0F]),
        });
    }
    // 0x10: TimeCreated (4 bytes - note first field is "%2d" not "%02d")
    if d.len() > 0x13 {
        tags.push(DecodedTag {
            name: "TimeCreated".into(),
            value: format!("{:2}:{:02}:{:02}.{:02}", d[0x10], d[0x11], d[0x12], d[0x13]),
        });
    }
    // 0x1E: OpticalZoom (u16 / 100)
    if let Some(v) = u16be(0x1E) {
        let z = v as f64 / 100.0;
        let s = format!("{z:.1}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        tags.push(DecodedTag {
            name: "OpticalZoom".into(),
            value: s.to_string(),
        });
    }
    // 0x37: Sharpness (int8s)
    if let Some(&raw) = d.get(0x37) {
        let v = raw as i8;
        tags.push(DecodedTag {
            name: "Sharpness".into(),
            value: match v {
                i if i < 0 => "Soft".into(),
                0 => "Normal".into(),
                _ => "Hard".into(),
            },
        });
    }
    // 0x38: ExposureTime (u32 / 1e5)
    if let Some(v) = u32be(0x38) {
        let t = v as f64 / 100000.0;
        tags.push(DecodedTag {
            name: "ExposureTime".into(),
            value: crate::tiff::tags::format_exposure_time(t).unwrap_or_else(|| format!("{t}")),
        });
    }
    // 0x3C: FNumber (u16 / 100)
    if let Some(v) = u16be(0x3C) {
        let f = v as f64 / 100.0;
        let s = format!("{f:.2}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        tags.push(DecodedTag {
            name: "FNumber".into(),
            value: s.to_string(),
        });
    }
    // 0x4E: ISO
    if let Some(v) = u16be(0x4E) {
        tags.push(DecodedTag {
            name: "ISO".into(),
            value: format!("{v}"),
        });
    }
}

fn decode_kodak_type4(d: &[u8], tags: &mut Vec<DecodedTag>) {
    // Type 4: DC200, DC215 - big-endian
    // 0x20: OriginalFileName (string[12])
    if d.len() >= 0x2C {
        let name = String::from_utf8_lossy(&d[0x20..0x2C])
            .trim_end_matches('\0')
            .trim()
            .to_string();
        if !name.is_empty() {
            tags.push(DecodedTag {
                name: "OriginalFileName".into(),
                value: name,
            });
        }
    }
}

fn decode_kodak_type9(d: &[u8], tags: &mut Vec<DecodedTag>) {
    // Type 9: C140, C180, C913 etc. - little-endian, starts with "IIII"
    let u16le = |off: usize| -> Option<u16> {
        if off + 2 > d.len() {
            return None;
        }
        Some(u16::from_le_bytes([d[off], d[off + 1]]))
    };
    let u32le = |off: usize| -> Option<u32> {
        if off + 4 > d.len() {
            return None;
        }
        Some(u32::from_le_bytes([
            d[off],
            d[off + 1],
            d[off + 2],
            d[off + 3],
        ]))
    };

    // 0x0C: FNumber (u16 / 100)
    if let Some(v) = u16le(0x0C) {
        let f = v as f64 / 100.0;
        let s = format!("{f:.2}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        tags.push(DecodedTag {
            name: "FNumber".into(),
            value: s.to_string(),
        });
    }
    // 0x10: ExposureTime (u32 / 1e6 for type 9)
    if let Some(v) = u32le(0x10) {
        let t = v as f64 / 1000000.0;
        tags.push(DecodedTag {
            name: "ExposureTime".into(),
            value: crate::tiff::tags::format_exposure_time(t).unwrap_or_else(|| format!("{t}")),
        });
    }
    // 0x14: DateTimeOriginal (string[20], / replaced with :)
    if d.len() >= 0x28 {
        let raw = String::from_utf8_lossy(&d[0x14..0x28])
            .trim_end_matches('\0')
            .to_string();
        let dt = raw.replacen('/', ":", 2);
        if !dt.trim().is_empty() {
            tags.push(DecodedTag {
                name: "DateTimeOriginal".into(),
                value: dt,
            });
        }
    }
    // 0x34: ISO
    if let Some(v) = u16le(0x34) {
        tags.push(DecodedTag {
            name: "ISO".into(),
            value: format!("{v}"),
        });
    }
    // 0x57: FirmwareVersion (string[16])
    if d.len() >= 0x67 {
        let fw = String::from_utf8_lossy(&d[0x57..0x67])
            .trim_end_matches('\0')
            .trim()
            .to_string();
        if !fw.is_empty() {
            tags.push(DecodedTag {
                name: "FirmwareVersion".into(),
                value: fw,
            });
        }
    }
}

// -- Reconyx HyperFire binary decoder ------------------------------------

fn decode_reconyx_hyperfire(d: &[u8], tags: &mut Vec<DecodedTag>) {
    let u16le = |off: usize| -> u16 { u16::from_le_bytes([d[off], d[off + 1]]) };
    let i16le = |off: usize| -> i16 { i16::from_le_bytes([d[off], d[off + 1]]) };

    // 0x00: MakerNoteVersion (int16u, format as 0xNNNN)
    let ver = u16le(0x00);
    tags.push(DecodedTag {
        name: "MakerNoteVersion".into(),
        value: format!("0x{ver:04x}"),
    });

    // 0x02: FirmwareVersion (3 x int16u -> "X.Y.Z")
    if d.len() > 0x07 {
        let v0 = u16le(0x02);
        let v1 = u16le(0x04);
        let v2 = u16le(0x06);
        tags.push(DecodedTag {
            name: "FirmwareVersion".into(),
            value: format!("{v0}.{v1}.{v2}"),
        });
    }

    // 0x08: FirmwareDate (2 x int16u -> formatted as hex: "%.4x:%.2x:%.2x")
    if d.len() > 0x0B {
        let w0 = u16le(0x08);
        let w1 = u16le(0x0A);
        tags.push(DecodedTag {
            name: "FirmwareDate".into(),
            value: format!("{:04x}:{:02x}:{:02x}", w0, w1 >> 8, w1 & 0xFF),
        });
    }

    // 0x0C: TriggerMode (string[2])
    if d.len() > 0x0D {
        let mode = match &d[0x0C..0x0E] {
            b"C\0" | [b'C', _] => "CodeLoc Not Entered",
            b"E\0" | [b'E', _] => "External Sensor",
            b"M\0" | [b'M', _] => "Motion Detection",
            b"T\0" | [b'T', _] => "Time Lapse",
            _ => {
                let s = String::from_utf8_lossy(&d[0x0C..0x0E])
                    .trim_end_matches('\0')
                    .to_string();
                tags.push(DecodedTag {
                    name: "TriggerMode".into(),
                    value: s,
                });
                ""
            }
        };
        if !mode.is_empty() {
            tags.push(DecodedTag {
                name: "TriggerMode".into(),
                value: mode.into(),
            });
        }
    }

    // 0x0E: Sequence (2 x int16u -> "X of Y")
    if d.len() > 0x11 {
        let seq = u16le(0x0E);
        let total = u16le(0x10);
        tags.push(DecodedTag {
            name: "Sequence".into(),
            value: format!("{seq} of {total}"),
        });
    }

    // 0x12: EventNumber (2 x int16u -> combined 32-bit)
    if d.len() > 0x15 {
        let hi = u16le(0x12) as u32;
        let lo = u16le(0x14) as u32;
        let ev = (hi << 16) + lo;
        tags.push(DecodedTag {
            name: "EventNumber".into(),
            value: format!("{ev}"),
        });
    }

    // 0x16: DateTimeOriginal (6 x int16u, stored in reverse order:
    // sec, min, hour, month, day, year). Printed "YYYY:MM:DD HH:MM:SS" with
    // every field but the year zero-padded to two digits.
    if d.len() > 0x21 {
        let mut vals = [0u16; 6];
        for (i, item) in vals.iter_mut().enumerate() {
            let mut v = u16le(0x16 + i * 2);
            // Byte-swap fix: if high byte set but low byte zero
            if (v & 0xFF) == 0 && (v >> 8) != 0 {
                v = (v >> 8) | ((v & 0xFF) << 8);
            }
            *item = v;
        }
        // vals = [sec, min, hour, month, day, year]
        let (sec, min, hour, month, day, year) =
            (vals[0], vals[1], vals[2], vals[3], vals[4], vals[5]);
        tags.push(DecodedTag {
            name: "DateTimeOriginal".into(),
            value: format!("{year:04}:{month:02}:{day:02} {hour:02}:{min:02}:{sec:02}"),
        });
    }

    // 0x24: MoonPhase
    if d.len() > 0x25 {
        let v = u16le(0x24);
        tags.push(DecodedTag {
            name: "MoonPhase".into(),
            value: match v {
                0 => "New".into(),
                1 => "New Crescent".into(),
                2 => "First Quarter".into(),
                3 => "Waxing Gibbous".into(),
                4 => "Full".into(),
                5 => "Waning Gibbous".into(),
                6 => "Last Quarter".into(),
                7 => "Old Crescent".into(),
                _ => format!("{v}"),
            },
        });
    }

    // 0x26: AmbientTemperatureFahrenheit (int16s)
    if d.len() > 0x27 {
        let v = i16le(0x26);
        tags.push(DecodedTag {
            name: "AmbientTemperatureFahrenheit".into(),
            value: format!("{v} F"),
        });
    }
    // 0x28: AmbientTemperature (int16s)
    if d.len() > 0x29 {
        let v = i16le(0x28);
        tags.push(DecodedTag {
            name: "AmbientTemperature".into(),
            value: format!("{v} C"),
        });
    }

    // 0x2A: SerialNumber (unicode[15] = 30 bytes, UTF-16LE)
    if d.len() > 0x47 {
        let utf16: Vec<u16> = (0..15).map(|i| u16le(0x2A + i * 2)).collect();
        let s = String::from_utf16_lossy(&utf16)
            .trim_end_matches('\0')
            .to_string();
        if !s.is_empty() {
            tags.push(DecodedTag {
                name: "SerialNumber".into(),
                value: s,
            });
        }
    }

    // 0x48: Contrast
    if d.len() > 0x49 {
        tags.push(DecodedTag {
            name: "Contrast".into(),
            value: format!("{}", u16le(0x48)),
        });
    }
    // 0x4A: Brightness
    if d.len() > 0x4B {
        tags.push(DecodedTag {
            name: "Brightness".into(),
            value: format!("{}", u16le(0x4A)),
        });
    }
    // 0x4C: Sharpness
    if d.len() > 0x4D {
        tags.push(DecodedTag {
            name: "Sharpness".into(),
            value: format!("{}", u16le(0x4C)),
        });
    }
    // 0x4E: Saturation
    if d.len() > 0x4F {
        tags.push(DecodedTag {
            name: "Saturation".into(),
            value: format!("{}", u16le(0x4E)),
        });
    }

    // 0x50: InfraredIlluminator
    if d.len() > 0x51 {
        let v = u16le(0x50);
        tags.push(DecodedTag {
            name: "InfraredIlluminator".into(),
            value: match v {
                0 => "Off".into(),
                1 => "On".into(),
                _ => format!("{v}"),
            },
        });
    }

    // 0x52: MotionSensitivity
    if d.len() > 0x53 {
        tags.push(DecodedTag {
            name: "MotionSensitivity".into(),
            value: format!("{}", u16le(0x52)),
        });
    }

    // 0x54: BatteryVoltage (millivolts -> volts)
    if d.len() > 0x55 {
        let mv = u16le(0x54);
        let v = mv as f64 / 1000.0;
        tags.push(DecodedTag {
            name: "BatteryVoltage".into(),
            value: format!("{v:.2} V"),
        });
    }

    // 0x56: UserLabel (string[22])
    if d.len() > 0x6B {
        let label = String::from_utf8_lossy(&d[0x56..0x6C])
            .trim_end_matches('\0')
            .trim()
            .to_string();
        if !label.is_empty() {
            tags.push(DecodedTag {
                name: "UserLabel".into(),
                value: label,
            });
        }
    }
}

// -- Kodak tag table -----------------------------------------------------

static KODAK_TAGS: [(u16, &str); 21] = [
    (0x0000, "KodakModel"),
    (0x0001, "Quality"),
    (0x0005, "BurstMode"),
    (0x0009, "ImageWidth"),
    (0x000A, "ImageHeight"),
    (0x000C, "YearCreated"),
    (0x000D, "MonthDayCreated"),
    (0x000E, "TimeCreated"),
    (0x0010, "BurstMode2"),
    (0x001C, "ShutterMode"),
    (0x001D, "MeteringMode"),
    (0x001E, "SequenceNumber"),
    (0x001F, "FNumber"),
    (0x0020, "ExposureTime"),
    (0x0021, "ExposureCompensation"),
    (0x0022, "FocusMode"),
    (0x0024, "WhiteBalance"),
    (0x005C, "FocusDistance"),
    (0x0068, "ISO"),
    (0x00FC, "FirmwareVersion"),
    (0x00FE, "SerialNumber"),
];

// -- Sanyo tag table -----------------------------------------------------

static SANYO_TAGS: [(u16, &str); 26] = [
    (0x00FF, "MakerNoteOffset"),
    (0x0100, "SanyoThumbnail"),
    (0x0200, "SpecialMode"),
    (0x0201, "SanyoQuality"),
    (0x0202, "Macro"),
    (0x0204, "DigitalZoom"),
    (0x0207, "SoftwareVersion"),
    (0x0208, "PictInfo"),
    (0x0209, "CameraID"),
    (0x020E, "SequentialShot"),
    (0x020F, "WideRange"),
    (0x0210, "ColorAdjustmentMode"),
    (0x0213, "QuickShot"),
    (0x0214, "SelfTimer"),
    (0x0216, "VoiceMemo"),
    (0x0217, "RecordShutterRelease"),
    (0x0218, "FlickerReduce"),
    (0x0219, "OpticalZoomOn"),
    (0x021B, "DigitalZoomOn"),
    (0x021D, "LightSourceSpecial"),
    (0x021E, "Resaved"),
    (0x021F, "SceneSelect"),
    (0x0223, "ManualFocusDistance"),
    (0x0224, "SequenceShotInterval"),
    (0x0225, "FlashMode"),
    (0x0E00, "PrintIM"),
];

// -- Ricoh tag table -----------------------------------------------------

static RICOH_TAGS: [(u16, &str); 22] = [
    (0x0001, "MakerNoteType"),
    (0x0002, "FirmwareVersion"),
    (0x0005, "InternalSerialNumber"),
    (0x000E, "ImageInfo"),
    (0x1001, "RecordingMode"),
    (0x1002, "Quality"),
    (0x1003, "FocusMode"),
    (0x1004, "FlashMode"),
    (0x1006, "FocusPoint"),
    (0x1007, "Sharpness"),
    (0x1008, "WhiteBalance"),
    (0x100B, "ISO"),
    (0x100D, "Contrast"),
    (0x100E, "Saturation"),
    (0x1011, "MacroMode"),
    (0x1014, "ImageSize"),
    (0x1017, "ColorFilter"),
    (0x101C, "NDFilter"),
    (0x1200, "AFStatus"),
    (0x1201, "AFAreaXPosition"),
    (0x1202, "AFAreaYPosition"),
    (0x1203, "AFAreaMode"),
];

// -- Sigma tag table -----------------------------------------------------

static SIGMA_TAGS: [(u16, &str); 23] = [
    (0x0002, "SerialNumber"),
    (0x0003, "DriveMode"),
    (0x0004, "ResolutionMode"),
    (0x0005, "AFMode"),
    (0x0006, "FocusSetting"),
    (0x0007, "WhiteBalance"),
    (0x0008, "ExposureMode"),
    (0x0009, "MeteringMode"),
    (0x000A, "LensFocalRange"),
    (0x000B, "ColorSpace"),
    (0x000C, "ExposureCompensation"),
    (0x000D, "Contrast"),
    (0x000E, "Shadow"),
    (0x000F, "Highlight"),
    (0x0010, "Saturation"),
    (0x0011, "Sharpness"),
    (0x0012, "X3FillLight"),
    (0x0014, "ColorAdjustment"),
    (0x0015, "AdjustmentMode"),
    (0x0016, "Quality"),
    (0x0017, "Firmware"),
    (0x0018, "Software"),
    (0x0019, "AutoBracket"),
];

// -- Motorola tag table --------------------------------------------------

static MOTOROLA_TAGS: [(u16, &str); 4] = [
    (0x5500, "BuildNumber"),
    (0x5501, "SerialNumber"),
    (0x665E, "Sensor"),
    (0x6705, "ManufactureDate"),
];

// -- JVC tag table -------------------------------------------------------

static JVC_TAGS: [(u16, &str); 2] = [(0x0002, "CPUVersions"), (0x0003, "Quality")];

/// Decode JVC text-format maker notes: "VER:0100QTY:FINE"
fn decode_jvc_text(text: &str, tags: &mut Vec<DecodedTag>) {
    // Parse [A-Z]+:[value]{3,4} pairs
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        // Find key: uppercase letters
        if !bytes[i].is_ascii_uppercase() {
            i += 1;
            continue;
        }
        let key_start = i;
        while i < bytes.len() && bytes[i].is_ascii_uppercase() {
            i += 1;
        }
        let key = &text[key_start..i];
        // Expect ':'
        if i >= bytes.len() || bytes[i] != b':' {
            continue;
        }
        i += 1;
        // Value: 3-4 characters
        let val_start = i;
        let val_end = (i + 4).min(bytes.len());
        i = val_end;
        let val = text[val_start..val_end].trim_end_matches('\0');

        match key {
            "VER" => {
                tags.push(DecodedTag {
                    name: "MakerNoteVersion".into(),
                    value: val.to_string(),
                });
            }
            "QTY" => {
                let q = match val {
                    "STND" | "STD" => "Normal",
                    "FINE" => "Fine",
                    _ => val,
                };
                tags.push(DecodedTag {
                    name: "Quality".into(),
                    value: q.to_string(),
                });
            }
            _ => {}
        }
    }
}

// -- FLIR tag table ------------------------------------------------------

static FLIR_TAGS: [(u16, &str); 2] = [
    (0x0001, "ImageTemperatureMax"),
    (0x0002, "ImageTemperatureMin"),
];

// -- GE (General Imaging) tag table --------------------------------------

static GE_TAGS: [(u16, &str); 3] = [(0x0202, "Macro"), (0x0207, "GEModel"), (0x0300, "GEMake")];

// -- Pentax value formatting ---------------------------------------------

fn format_casio_value(entry: &IfdEntry<'_>, name: &str, be: bool, is_type2: bool) -> String {
    let v = entry_u16(entry, be);
    match name {
        // Casio Type 1 tags (0x0001-0x0019)
        "RecordingMode" => match v {
            Some(1) => "Single Shutter".into(),
            Some(2) => "Panorama".into(),
            Some(3) => "Night Scene".into(),
            Some(4) => "Portrait".into(),
            Some(5) => "Landscape".into(),
            Some(7) => "Panorama".into(),
            Some(10) => "Night Scene".into(),
            Some(15) => "Night Scene".into(),
            Some(16) => "Normal".into(),
            Some(19) => "Fireworks".into(),
            _ => format_ifd_value(entry, be),
        },
        "Quality" => match v {
            Some(1) => "Economy".into(),
            Some(2) => "Normal".into(),
            Some(3) => "Fine".into(),
            _ => format_ifd_value(entry, be),
        },
        "FocusMode" if !is_type2 => match v {
            Some(2) => "Macro".into(),
            Some(3) => "Auto".into(),
            Some(4) => "Manual".into(),
            Some(5) => "Infinity".into(),
            Some(7) => "Pan Focus".into(),
            _ => format_ifd_value(entry, be),
        },
        "FocusMode" if is_type2 => {
            let raw = entry_u16(entry, be);
            if let Some(v) = raw {
                match v & 0xFF {
                    0 => "Manual".into(),
                    1 => "Focus Lock".into(),
                    2 => "Macro".into(),
                    3 => "Single-Area Auto Focus".into(),
                    5 => "Infinity".into(),
                    6 => "Multi-Area Auto Focus".into(),
                    8 => "Super Macro".into(),
                    _ => format_ifd_value(entry, be),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "Enhancement" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            _ => format_ifd_value(entry, be),
        },
        "FlashMode" => match v {
            Some(1) => "Auto".into(),
            Some(2) => "On".into(),
            Some(3) => "Off".into(),
            Some(4) => "Red-eye Reduction".into(),
            Some(5) => "Slow-sync".into(),
            _ => format_ifd_value(entry, be),
        },
        "FlashIntensity" => match v {
            Some(11) => "Weak".into(),
            Some(13) => "Normal".into(),
            Some(15) => "Strong".into(),
            _ => format_ifd_value(entry, be),
        },
        "ObjectDistance" => match entry_u32(entry, be) {
            Some(d) if d >= 0x20000000 => "inf".into(),
            Some(d) if d > 0 => {
                let m = d as f64 / 1000.0;
                // Format: remove trailing zeros but keep at least one decimal if needed
                let s = format!("{m:.2}");
                let s = s.trim_end_matches('0').trim_end_matches('.');
                format!("{s} m")
            }
            _ => format_ifd_value(entry, be),
        },
        "WhiteBalance" if !is_type2 => match v {
            Some(1) => "Auto".into(),
            Some(2) => "Tungsten".into(),
            Some(3) => "Daylight".into(),
            Some(4) => "Fluorescent".into(),
            Some(5) => "Shade".into(),
            Some(129) => "Manual".into(),
            _ => format_ifd_value(entry, be),
        },
        "WhiteBalance" if is_type2 => match v {
            Some(0) => "Manual".into(),
            Some(1) => "Daylight".into(),
            Some(2) => "Cloudy".into(),
            Some(3) => "Shade".into(),
            Some(4) => "Flash?".into(),
            Some(6) => "Fluorescent".into(),
            Some(9) => "Tungsten?".into(),
            Some(10) => "Tungsten".into(),
            Some(12) => "Flash".into(),
            Some(v) => format!("Unknown ({v})"),
            None => format_ifd_value(entry, be),
        },
        "DigitalZoom" => match entry_u32(entry, be) {
            Some(0x10000) => "Off".into(),
            Some(0x10001) => "2x Digital Zoom".into(),
            Some(0x20000) => "2x".into(),
            Some(0x40000) => "4x".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "Sharpness" | "Contrast" | "Saturation" => match v {
            Some(0) => "Normal".into(),
            Some(1) => "Soft".into(),
            Some(2) => "Hard".into(),
            _ => format_ifd_value(entry, be),
        },
        "ISO" if !is_type2 => {
            // Type 1 ISO: raw value IS the ISO
            if let Some(v) = v {
                format!("{v}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        // Casio Type 2 tags (0x2000+, 0x3000+)
        "RecordMode" => match v {
            Some(2) => "Program AE".into(),
            Some(3) => "Shutter Priority".into(),
            Some(4) => "Aperture Priority".into(),
            Some(5) => "Manual".into(),
            Some(6) => "Best Shot".into(),
            Some(17) => "Movie".into(),
            Some(19) => "Movie (19)".into(),
            Some(20) => "YouTube Movie".into(),
            _ => format_ifd_value(entry, be),
        },
        "ReleaseMode" => match v {
            Some(1) => "Normal".into(),
            Some(3) => "AE Bracketing".into(),
            Some(6) => "Multi-frame".into(),
            Some(9) => "Bracketing".into(),
            Some(11) => "Continuous".into(),
            Some(12) => "Continuous".into(),
            Some(18) => "Cont., SpeedShot".into(),
            _ => format_ifd_value(entry, be),
        },
        "Quality2" => match v {
            Some(1) => "Economy".into(),
            Some(2) => "Normal".into(),
            Some(3) => "Fine".into(),
            _ => format_ifd_value(entry, be),
        },
        "FocusMode2" => match v {
            Some(0) => "Manual".into(),
            Some(1) => "Focus Lock".into(),
            Some(2) => "Macro".into(),
            Some(3) => "Single-Area Auto Focus".into(),
            Some(5) => "Infinity".into(),
            Some(6) => "Multi-Area Auto Focus".into(),
            Some(8) => "Super Macro".into(),
            _ => format_ifd_value(entry, be),
        },
        "BestShotMode" => match v {
            Some(0) => "Off".into(),
            Some(v) => format!("{v}"),
            None => format_ifd_value(entry, be),
        },
        "ColorMode" => match v {
            Some(0) => "Off".into(),
            Some(2) => "Black & White".into(),
            Some(3) => "Sepia".into(),
            _ => format_ifd_value(entry, be),
        },
        "ImageStabilization" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(2) => "Best Shot".into(),
            Some(3) => "Movie Anti-Shake".into(),
            _ => format_ifd_value(entry, be),
        },
        "AFPointPosition" => {
            // 4 u16 values: x_num, x_denom, y_num, y_denom
            if entry.data.len() >= 8 {
                let x_num = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                } as f64;
                let x_den = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                } as f64;
                let y_num = if be {
                    u16::from_be_bytes([entry.data[4], entry.data[5]])
                } else {
                    u16::from_le_bytes([entry.data[4], entry.data[5]])
                } as f64;
                let y_den = if be {
                    u16::from_be_bytes([entry.data[6], entry.data[7]])
                } else {
                    u16::from_le_bytes([entry.data[6], entry.data[7]])
                } as f64;
                if x_den == 0.0 || y_den == 0.0 || (x_num == 65535.0 && y_num == 65535.0) {
                    "n/a".into()
                } else {
                    let x = x_num / x_den;
                    let y = y_num / y_den;
                    format!("{x:.1} {y:.1}")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "FirmwareDate" => {
            // Format: YYmm\0\0DDhh\0\0MM\0\0\0\0 (18 bytes, null-padded pairs)
            if entry.data.len() >= 14 {
                let s = std::str::from_utf8(entry.data).unwrap_or("");
                let digits: Vec<u8> = entry
                    .data
                    .iter()
                    .copied()
                    .filter(|&b| b.is_ascii_digit())
                    .collect();
                if digits.len() >= 10 {
                    let yr2 = (digits[0] - b'0') as u32 * 10 + (digits[1] - b'0') as u32;
                    let yr = if yr2 < 70 { 2000 + yr2 } else { 1900 + yr2 };
                    let mon = (digits[2] - b'0') as u32 * 10 + (digits[3] - b'0') as u32;
                    let day = (digits[4] - b'0') as u32 * 10 + (digits[5] - b'0') as u32;
                    let hr = (digits[6] - b'0') as u32 * 10 + (digits[7] - b'0') as u32;
                    let min = (digits[8] - b'0') as u32 * 10 + (digits[9] - b'0') as u32;
                    return format!("{yr:04}:{mon:02}:{day:02} {hr:02}:{min:02}");
                }
                s.trim_end_matches('\0').trim().to_string()
            } else {
                format_ifd_value(entry, be)
            }
        }
        "AFMode" if is_type2 => match v {
            Some(0) => "Off".into(),
            Some(1) => "Spot".into(),
            Some(2) => "Multi".into(),
            Some(3) => "Face Detection".into(),
            Some(4) => "Tracking".into(),
            Some(5) => "Intelligent".into(),
            _ => format_ifd_value(entry, be),
        },
        "BracketSequence" => {
            // int16u[2]
            if entry.data.len() >= 4 {
                let v0 = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let v1 = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                format!("{v0} {v1}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "AutoISO" if is_type2 => match v {
            Some(1) => "On".into(),
            Some(2) => "Off".into(),
            Some(7) => "On (high sensitivity)".into(),
            Some(8) => "On (anti-shake)".into(),
            Some(10) => "High Speed".into(),
            _ => format_ifd_value(entry, be),
        },
        "PreviewImageSize" => {
            // int16u[2]: width x height
            if entry.data.len() >= 4 {
                let w = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let h = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                format!("{w}x{h}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PreviewImageLength" | "PreviewImageStart" => {
            if let Some(v) = entry_u32(entry, be) {
                format!("{v}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ColorFilter" if is_type2 => match v {
            Some(0) => "Off".into(),
            Some(1) => "Blue".into(),
            Some(3) => "Green".into(),
            Some(4) => "Yellow".into(),
            Some(5) => "Red".into(),
            Some(6) => "Purple".into(),
            Some(7) => "Pink".into(),
            _ => format_ifd_value(entry, be),
        },
        "ArtMode" => match v {
            Some(0) => "Normal".into(),
            Some(8) => "Silent Movie".into(),
            Some(39) => "HDR".into(),
            Some(45) => "Premium Auto".into(),
            Some(47) => "Painting".into(),
            Some(49) => "Crayon Drawing".into(),
            Some(51) => "Panorama".into(),
            Some(52) => "Art HDR".into(),
            Some(62) => "High Speed Night Shot".into(),
            Some(64) => "Monochrome".into(),
            Some(67) => "Toy Camera".into(),
            Some(68) => "Pop Art".into(),
            Some(69) => "Light Tone".into(),
            _ => format_ifd_value(entry, be),
        },
        "HometownCity" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        "MakerNoteVersion" | "CameraVersion" | "SoftwareVersion" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        _ => format_ifd_value(entry, be),
    }
}

/// Decode Ricoh sub-directory (tag 0x2001): "[Ricoh Camera Info]\0" + IFD.
/// Contains ManufactureDate1/2 and other tags, always big-endian.
/// Decode Ricoh text-format maker notes: "Rv0207;Rg76;Bg60;Gg42;..."
fn decode_ricoh_text(text: &str, tags: &mut Vec<DecodedTag>) {
    // Parse key-value pairs: [A-Z][a-z]{1,2} followed by [0-9A-F]+ then ;
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        // Find key: uppercase letter + 1-2 lowercase
        if !bytes[i].is_ascii_uppercase() {
            i += 1;
            continue;
        }
        let key_start = i;
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_lowercase() {
            i += 1;
        }
        let key = &text[key_start..i];
        if key.len() < 2 || key.len() > 3 {
            continue;
        }

        // Find value: hex digits until ;
        let val_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_hexdigit()) {
            i += 1;
        }
        let val = &text[val_start..i];

        // Expect semicolon
        if i < bytes.len() && bytes[i] == b';' {
            i += 1;
        } else {
            continue;
        }

        match key {
            "Rv" | "Rev" => {
                // FirmwareVersion: divide by 100, format as X.XX
                if let Ok(n) = val.parse::<u32>() {
                    tags.push(DecodedTag {
                        name: "FirmwareVersion".into(),
                        value: format!("{:.2}", n as f64 / 100.0),
                    });
                }
            }
            "Rg" => {
                tags.push(DecodedTag {
                    name: "RedGain".into(),
                    value: val.to_string(),
                });
            }
            "Gg" => {
                tags.push(DecodedTag {
                    name: "GreenGain".into(),
                    value: val.to_string(),
                });
            }
            "Bg" => {
                tags.push(DecodedTag {
                    name: "BlueGain".into(),
                    value: val.to_string(),
                });
            }
            _ => {} // Unknown keys ignored
        }
    }
}

/// The RR1 model uses Base = start-20, meaning IFD offsets are relative to the
/// beginning of the entire block (including the 20-byte header), so we parse
/// with the header included and IFD starting at offset 20.
fn decode_ricoh_subdir(entry: &IfdEntry<'_>, mn_data: &[u8], tags: &mut Vec<DecodedTag>) {
    let header_prefix = b"[Ricoh Camera Info]";
    let header_len = 20; // header prefix + 1 byte (may be \0 or \xff)
    // The data may be inline or an offset to the sub-directory in mn_data
    let block = if entry.data.len() >= header_len && entry.data.starts_with(header_prefix) {
        entry.data
    } else if entry.data.len() >= 4 {
        // Could be an offset (int32u) into mn_data
        let off = u32::from_be_bytes([entry.data[0], entry.data[1], entry.data[2], entry.data[3]])
            as usize;
        if off + header_len < mn_data.len() && mn_data[off..].starts_with(header_prefix) {
            &mn_data[off..]
        } else {
            return;
        }
    } else {
        return;
    };

    if block.len() <= header_len {
        return;
    }

    // Try parsing from the block first (offsets relative to block start).
    // If that fails or produces entries with empty data, try from the parent data context.
    let sub_ifd = tiff::parse_ifd_tolerant(block, header_len as u64, true, false);
    let sub_ifd = match sub_ifd {
        Some(ifd) if ifd.entries.iter().any(|e| !e.inline && !e.data.is_empty()) => ifd,
        _ => {
            // Sub-IFD offsets may be TIFF-absolute; try resolving from mn_data (which is tiff_data)
            let mn_start = mn_data.as_ptr() as usize;
            let block_start = block.as_ptr() as usize;
            if block_start >= mn_start && block_start < mn_start + mn_data.len() {
                let off_in_mn = block_start - mn_start;
                match tiff::parse_ifd_tolerant(
                    mn_data,
                    (off_in_mn + header_len) as u64,
                    true,
                    false,
                ) {
                    Some(ifd) => ifd,
                    None => return,
                }
            } else {
                return;
            }
        }
    };

    for sub_entry in &sub_ifd.entries {
        match sub_entry.tag {
            0x0004 => {
                // ManufactureDate1 - string
                let s = std::str::from_utf8(sub_entry.data)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                if !s.is_empty() {
                    tags.push(DecodedTag {
                        name: "ManufactureDate1".into(),
                        value: s.to_string(),
                    });
                }
            }
            0x0005 => {
                // ManufactureDate2 - string
                let s = std::str::from_utf8(sub_entry.data)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                if !s.is_empty() {
                    tags.push(DecodedTag {
                        name: "ManufactureDate2".into(),
                        value: s.to_string(),
                    });
                }
            }
            _ => {}
        }
    }
}

fn format_ricoh_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    match name {
        "FirmwareVersion" => {
            // "Rev0104" -> "1.04"
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            let s = s.trim_end_matches('\0').trim();
            if let Some(digits) = s.strip_prefix("Rev") {
                if let Ok(num) = digits.parse::<u32>() {
                    return format!("{:.2}", num as f64 / 100.0);
                }
            }
            s.to_string()
        }
        "FocusMode" if entry.tag == 0x1003 && entry.data_type != crate::tiff::DataType::Short => {
            // Tag 0x1003: int32s -> Sharpness, int16u -> WhiteBalance (ExifTool)
            // When data type is not int16u, this is Sharpness
            match entry_u32(entry, be) {
                Some(0) => "Sharp".into(),
                Some(1) => "Normal".into(),
                Some(2) => "Soft".into(),
                _ => format_ifd_value(entry, be),
            }
        }
        "Sharpness" => match entry_u32(entry, be) {
            Some(0) => "Sharp".into(),
            Some(1) => "Normal".into(),
            Some(2) => "Soft".into(),
            _ => format_ifd_value(entry, be),
        },
        "WhiteBalance" => match entry_u16(entry, be) {
            Some(0) => "Auto".into(),
            Some(1) => "Daylight".into(),
            Some(2) => "Cloudy".into(),
            Some(3) => "Tungsten".into(),
            Some(4) => "Fluorescent".into(),
            Some(5) => "Manual".into(),
            Some(7) => "Shade".into(),
            Some(8) => "Multi-Pattern Auto".into(),
            _ => format_ifd_value(entry, be),
        },
        "MakerNoteType" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        _ => format_ifd_value(entry, be),
    }
}

fn format_sanyo_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    let v = entry_u16(entry, be);
    // Many Sanyo tags use 0=Off pattern
    let off_on = |v: Option<u16>| -> String {
        match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            _ => format_ifd_value(entry, be),
        }
    };
    match name {
        "QuickShot"
        | "VoiceMemo"
        | "FlickerReduce"
        | "WideRange"
        | "LightSourceSpecial"
        | "SceneSelect"
        | "ColorAdjustmentMode"
        | "OpticalZoomOn"
        | "DigitalZoomOn" => off_on(v),
        "Resaved" => match v {
            Some(0) => "No".into(),
            Some(1) => "Yes".into(),
            _ => format_ifd_value(entry, be),
        },
        "SelfTimer" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            Some(2) => "2 sec".into(),
            _ => format_ifd_value(entry, be),
        },
        "RecordShutterRelease" => match v {
            Some(0) => "Record while down".into(),
            Some(1) => "Press start, press stop".into(),
            _ => format_ifd_value(entry, be),
        },
        "SequentialShot" => match v {
            Some(0) => "None".into(),
            Some(1) => "Standard".into(),
            Some(2) => "Best".into(),
            Some(3) => "Adjust Exposure".into(),
            _ => format_ifd_value(entry, be),
        },
        "SequenceShotInterval" => match v {
            Some(0) => "5 frames/s".into(),
            Some(1) => "10 frames/s".into(),
            Some(2) => "15 frames/s".into(),
            Some(3) => "20 frames/s".into(),
            _ => format_ifd_value(entry, be),
        },
        "FlashMode" => match v {
            Some(0) => "Auto".into(),
            Some(1) => "Force".into(),
            Some(2) => "Disabled".into(),
            Some(3) => "Red eye".into(),
            _ => format_ifd_value(entry, be),
        },
        "SanyoQuality" => match v {
            Some(0) => "Normal/Very Low".into(),
            Some(1) => "Normal/Low".into(),
            Some(2) => "Normal/Medium Low".into(),
            Some(3) => "Normal/Medium".into(),
            Some(4) => "Normal/Medium High".into(),
            Some(5) => "Normal/High".into(),
            Some(6) => "Normal/Very High".into(),
            Some(7) => "Normal/Super High".into(),
            Some(256) => "Fine/Very Low".into(),
            Some(257) => "Fine/Low".into(),
            Some(258) => "Fine/Medium Low".into(),
            Some(259) => "Fine/Medium".into(),
            Some(260) => "Fine/Medium High".into(),
            Some(261) => "Fine/High".into(),
            Some(262) => "Fine/Very High".into(),
            Some(263) => "Fine/Super High".into(),
            Some(512) => "Super Fine/Very Low".into(),
            Some(513) => "Super Fine/Low".into(),
            Some(514) => "Super Fine/Medium Low".into(),
            Some(515) => "Super Fine/Medium".into(),
            _ => format_ifd_value(entry, be),
        },
        "Macro" => match v {
            Some(0) => "Normal".into(),
            Some(1) => "Macro".into(),
            Some(2) => "View".into(),
            Some(3) => "Manual".into(),
            _ => format_ifd_value(entry, be),
        },
        "DigitalZoom" => {
            // Sanyo DigitalZoom is rational64u - output raw value
            format_ifd_value(entry, be)
        }
        "SoftwareVersion" | "CameraID" | "MakerNoteVersion" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        _ => format_ifd_value(entry, be),
    }
}

fn decode_minolta_camera_settings(data: &[u8], tags: &mut Vec<DecodedTag>) {
    // FORMAT = int32u, ByteOrder = BigEndian, FIRST_ENTRY = 0
    let get = |idx: usize| -> Option<u32> {
        let off = idx * 4;
        if off + 4 <= data.len() {
            Some(u32::from_be_bytes([
                data[off],
                data[off + 1],
                data[off + 2],
                data[off + 3],
            ]))
        } else {
            None
        }
    };
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    if let Some(v) = get(1) {
        push(
            tags,
            "ExposureMode",
            match v {
                0 => "Program".into(),
                1 => "Aperture Priority".into(),
                2 => "Shutter Priority".into(),
                3 => "Manual".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(2) {
        push(
            tags,
            "FlashMode",
            match v {
                0 => "Fill flash".into(),
                1 => "Red-eye reduction".into(),
                2 => "Rear flash sync".into(),
                3 => "Wireless".into(),
                4 => "Off?".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(3) {
        push(tags, "WhiteBalance", minolta_white_balance(v));
    }
    if let Some(v) = get(4) {
        push(
            tags,
            "MinoltaImageSize",
            match v {
                0 => "Full".into(),
                1 => "1600x1200".into(),
                2 => "1280x960".into(),
                3 => "640x480".into(),
                6 => "2080x1560".into(),
                7 => "2560x1920".into(),
                8 => "3264x2176".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(5) {
        push(
            tags,
            "MinoltaQuality",
            match v {
                0 => "Raw".into(),
                1 => "Super Fine".into(),
                2 => "Fine".into(),
                3 => "Standard".into(),
                4 => "Economy".into(),
                5 => "Extra Fine".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(6) {
        push(
            tags,
            "DriveMode",
            match v {
                0 => "Single".into(),
                1 => "Continuous".into(),
                2 => "Self-timer".into(),
                4 => "Bracketing".into(),
                5 => "Interval".into(),
                6 => "UHS continuous".into(),
                7 => "HS continuous".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(8) {
        // ISO: 2^((val-48)/8) * 100
        let iso = 2.0_f64.powf((v as f64 - 48.0) / 8.0) * 100.0;
        push(tags, "ISO", format!("{}", (iso + 0.5) as u32));
    }
    if let Some(v) = get(9) {
        // ExposureTime: 2^((48-val)/8)
        let t = 2.0_f64.powf((48.0 - v as f64) / 8.0);
        let s = if t > 0.0 && t < 0.25001 {
            let recip = (0.5 + 1.0 / t) as u32;
            format!("1/{recip}")
        } else {
            format!("{t:.1}")
        };
        push(tags, "ExposureTime", s);
    }
    if let Some(v) = get(10) {
        // FNumber: 2^((val-8)/16)
        let fnum = 2.0_f64.powf((v as f64 - 8.0) / 16.0);
        push(tags, "FNumber", format!("{fnum:.1}"));
    }
    if let Some(v) = get(11) {
        push(
            tags,
            "MacroMode",
            match v {
                0 => "Off".into(),
                1 => "On".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(12) {
        push(
            tags,
            "DigitalZoom",
            match v {
                0 => "Off".into(),
                1 => "Electronic magnification".into(),
                2 => "2x".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(13) {
        // ExposureCompensation: val/3 - 2
        let ec = v as f64 / 3.0 - 2.0;
        push(tags, "ExposureCompensation", format_fraction(ec));
    }
    if let Some(v) = get(14) {
        push(
            tags,
            "BracketStep",
            match v {
                0 => "1/3 EV".into(),
                1 => "2/3 EV".into(),
                2 => "1 EV".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(16) {
        push(tags, "IntervalLength", format!("{v}"));
    }
    if let Some(v) = get(17) {
        push(tags, "IntervalNumber", format!("{v}"));
    }
    if let Some(v) = get(18) {
        // FocalLength: val / 256
        let fl = v as f64 / 256.0;
        push(tags, "FocalLength", format!("{fl:.1} mm"));
    }
    if let Some(v) = get(19) {
        // FocusDistance: val / 1000, 0 -> inf
        if v == 0 {
            push(tags, "FocusDistance", "inf".into());
        } else {
            let dist = v as f64 / 1000.0;
            push(tags, "FocusDistance", format!("{dist} m"));
        }
    }
    if let Some(v) = get(20) {
        push(
            tags,
            "FlashFired",
            match v {
                0 => "No".into(),
                1 => "Yes".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(21) {
        // MinoltaDate: val>>16 : (val>>8)&0xff : val&0xff
        let y = v >> 16;
        let m = (v >> 8) & 0xff;
        let d = v & 0xff;
        push(tags, "MinoltaDate", format!("{y:04}:{m:02}:{d:02}"));
    }
    if let Some(v) = get(22) {
        // MinoltaTime: val>>16 : (val>>8)&0xff : val&0xff
        let h = v >> 16;
        let m = (v >> 8) & 0xff;
        let s = v & 0xff;
        push(tags, "MinoltaTime", format!("{h:02}:{m:02}:{s:02}"));
    }
    if let Some(v) = get(23) {
        // MaxAperture: 2^((val-8)/16)
        let ap = 2.0_f64.powf((v as f64 - 8.0) / 16.0);
        push(tags, "MaxAperture", format!("{ap:.1}"));
    }
    if let Some(v) = get(26) {
        push(
            tags,
            "FileNumberMemory",
            match v {
                0 => "Off".into(),
                1 => "On".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(27) {
        push(tags, "LastFileNumber", format!("{v}"));
    }
    if let Some(v) = get(28) {
        // ColorBalanceRed: val / 256
        let bal = v as f64 / 256.0;
        push(tags, "ColorBalanceRed", format_sig_digits(bal, 10));
    }
    if let Some(v) = get(29) {
        let bal = v as f64 / 256.0;
        push(tags, "ColorBalanceGreen", format_sig_digits(bal, 10));
    }
    if let Some(v) = get(30) {
        let bal = v as f64 / 256.0;
        push(tags, "ColorBalanceBlue", format_sig_digits(bal, 10));
    }
    if let Some(v) = get(33) {
        push(
            tags,
            "Sharpness",
            match v {
                0 => "Hard".into(),
                1 => "Normal".into(),
                2 => "Soft".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(34) {
        push(
            tags,
            "SubjectProgram",
            match v {
                0 => "None".into(),
                1 => "Portrait".into(),
                2 => "Text".into(),
                3 => "Night portrait".into(),
                4 => "Sunset".into(),
                5 => "Sports action".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(35) {
        // FlashExposureComp: (val - 6) / 3
        let ec = (v as f64 - 6.0) / 3.0;
        push(tags, "FlashExposureComp", format_fraction(ec));
    }
    if let Some(v) = get(36) {
        push(
            tags,
            "ISOSetting",
            match v {
                0 => "100".into(),
                1 => "200".into(),
                2 => "400".into(),
                3 => "800".into(),
                4 => "Auto".into(),
                5 => "64".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(37) {
        push(
            tags,
            "MinoltaModelID",
            match v {
                0 => "DiMAGE 7, X1, X21 or X31".into(),
                1 => "DiMAGE 5".into(),
                2 => "DiMAGE S304".into(),
                3 => "DiMAGE S404".into(),
                4 => "DiMAGE 7i".into(),
                5 => "DiMAGE 7Hi".into(),
                6 => "DiMAGE A1".into(),
                7 => "DiMAGE A2 or S414".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(38) {
        push(
            tags,
            "IntervalMode",
            match v {
                0 => "Still Image".into(),
                1 => "Time-lapse Movie".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(39) {
        push(
            tags,
            "FolderName",
            match v {
                0 => "Standard Form".into(),
                1 => "Data Form".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(40) {
        push(
            tags,
            "ColorMode",
            match v {
                0 => "Natural color".into(),
                1 => "Black & White".into(),
                2 => "Vivid color".into(),
                3 => "Solarization".into(),
                4 => "Adobe RGB".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(41) {
        // ColorFilter: val - 3 (simplified, ignoring DiMAGE A2 special case)
        let cf = v as i32 - 3;
        push(tags, "ColorFilter", format!("{cf}"));
    }
    if let Some(v) = get(42) {
        push(tags, "BWFilter", format!("{v}"));
    }
    if let Some(v) = get(43) {
        push(
            tags,
            "InternalFlash",
            match v {
                0 => "No".into(),
                1 => "Fired".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(44) {
        // Brightness: val/8 - 6
        let br = v as f64 / 8.0 - 6.0;
        push(tags, "Brightness", format_sig_digits(br, 10));
    }
    if let Some(v) = get(45) {
        push(tags, "SpotFocusPointX", format!("{v}"));
    }
    if let Some(v) = get(46) {
        push(tags, "SpotFocusPointY", format!("{v}"));
    }
    if let Some(v) = get(47) {
        push(
            tags,
            "WideFocusZone",
            match v {
                0 => "No zone".into(),
                1 => "Center zone (horizontal orientation)".into(),
                2 => "Center zone (vertical orientation)".into(),
                3 => "Left zone".into(),
                4 => "Right zone".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(48) {
        push(
            tags,
            "FocusMode",
            match v {
                0 => "AF".into(),
                1 => "MF".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(49) {
        push(
            tags,
            "FocusArea",
            match v {
                0 => "Wide Focus (normal)".into(),
                1 => "Spot Focus".into(),
                _ => format!("{v}"),
            },
        );
    }
    if let Some(v) = get(50) {
        push(
            tags,
            "DECPosition",
            match v {
                0 => "Exposure".into(),
                1 => "Contrast".into(),
                2 => "Saturation".into(),
                3 => "Filter".into(),
                _ => format!("{v}"),
            },
        );
    }
}

/// Decode Apple binary plist (bplist00) to string representation.
/// Returns None if data is not a valid bplist or can't be decoded.
fn decode_bplist(data: &[u8]) -> Option<String> {
    if data.len() < 40 || &data[..8] != b"bplist00" {
        return None;
    }
    // Parse trailer (last 32 bytes)
    let trailer = &data[data.len() - 32..];
    let offset_size = trailer[6] as usize;
    let ref_size = trailer[7] as usize;
    let num_objects = u64::from_be_bytes(trailer[8..16].try_into().ok()?) as usize;
    let top_object = u64::from_be_bytes(trailer[16..24].try_into().ok()?) as usize;
    let offset_table_pos = u64::from_be_bytes(trailer[24..32].try_into().ok()?) as usize;

    if num_objects == 0 || offset_size == 0 || ref_size == 0 {
        return None;
    }
    if offset_table_pos + num_objects * offset_size > data.len() - 32 {
        return None;
    }

    // Build offset table
    let mut offsets = Vec::with_capacity(num_objects);
    for i in 0..num_objects {
        let pos = offset_table_pos + i * offset_size;
        let off = bplist_read_sized(data, pos, offset_size)?;
        offsets.push(off);
    }

    bplist_extract_object(data, &offsets, ref_size, top_object, 0)
}

fn bplist_read_sized(data: &[u8], pos: usize, size: usize) -> Option<usize> {
    if pos + size > data.len() {
        return None;
    }
    let mut val: u64 = 0;
    for i in 0..size {
        val = (val << 8) | data[pos + i] as u64;
    }
    Some(val as usize)
}

fn bplist_extract_object(
    data: &[u8],
    offsets: &[usize],
    ref_size: usize,
    obj_idx: usize,
    depth: usize,
) -> Option<String> {
    if depth > 32 || obj_idx >= offsets.len() {
        return None;
    }
    let pos = offsets[obj_idx];
    if pos >= data.len() {
        return None;
    }
    let marker = data[pos];
    let hi = marker >> 4;
    let lo = (marker & 0x0F) as usize;

    match hi {
        0x0 => {
            // null/bool/fill
            match marker {
                0x08 => Some("True".into()),
                0x09 => Some("False".into()),
                _ => Some("null".into()),
            }
        }
        0x1 => {
            // integer: 2^lo bytes
            let nbytes = 1usize << lo;
            if pos + 1 + nbytes > data.len() {
                return None;
            }
            let mut val: i64 = 0;
            for i in 0..nbytes {
                val = (val << 8) | data[pos + 1 + i] as i64;
            }
            // Sign-extend for 1/2/4/8 byte ints
            if nbytes <= 4 {
                let shift = 64 - nbytes * 8;
                val = (val << shift) >> shift;
            }
            Some(format!("{val}"))
        }
        0x2 => {
            // real/float
            let nbytes = 1usize << lo;
            if pos + 1 + nbytes > data.len() {
                return None;
            }
            if nbytes == 4 {
                let v = f32::from_be_bytes(data[pos + 1..pos + 5].try_into().ok()?);
                Some(format_bplist_float(v as f64))
            } else if nbytes == 8 {
                let v = f64::from_be_bytes(data[pos + 1..pos + 9].try_into().ok()?);
                Some(format_bplist_float(v))
            } else {
                None
            }
        }
        0x5 => {
            // ASCII string
            let len = if lo == 0x0F {
                bplist_read_extended_size(data, pos + 1)?
            } else {
                lo
            };
            let start = if lo == 0x0F {
                pos + 1 + bplist_extended_size_bytes(data, pos + 1)
            } else {
                pos + 1
            };
            if start + len > data.len() {
                return None;
            }
            Some(String::from_utf8_lossy(&data[start..start + len]).into_owned())
        }
        0x6 => {
            // UTF-16BE string
            let count = if lo == 0x0F {
                bplist_read_extended_size(data, pos + 1)?
            } else {
                lo
            };
            let start = if lo == 0x0F {
                pos + 1 + bplist_extended_size_bytes(data, pos + 1)
            } else {
                pos + 1
            };
            let byte_len = count * 2;
            if start + byte_len > data.len() {
                return None;
            }
            let chars: Vec<u16> = (0..count)
                .filter_map(|i| {
                    Some(u16::from_be_bytes(
                        data[start + i * 2..start + i * 2 + 2].try_into().ok()?,
                    ))
                })
                .collect();
            Some(String::from_utf16_lossy(&chars))
        }
        0xA => {
            // array
            let count = if lo == 0x0F {
                bplist_read_extended_size(data, pos + 1)?
            } else {
                lo
            };
            let refs_start = if lo == 0x0F {
                pos + 1 + bplist_extended_size_bytes(data, pos + 1)
            } else {
                pos + 1
            };
            let mut items = Vec::new();
            for i in 0..count {
                let ref_idx = bplist_read_sized(data, refs_start + i * ref_size, ref_size)?;
                let val = bplist_extract_object(data, offsets, ref_size, ref_idx, depth + 1)?;
                items.push(val);
            }
            Some(items.join(","))
        }
        0xD => {
            // dictionary
            let count = if lo == 0x0F {
                bplist_read_extended_size(data, pos + 1)?
            } else {
                lo
            };
            let refs_start = if lo == 0x0F {
                pos + 1 + bplist_extended_size_bytes(data, pos + 1)
            } else {
                pos + 1
            };
            let mut pairs = Vec::new();
            for i in 0..count {
                let key_ref = bplist_read_sized(data, refs_start + i * ref_size, ref_size)?;
                let val_ref =
                    bplist_read_sized(data, refs_start + (count + i) * ref_size, ref_size)?;
                let key = bplist_extract_object(data, offsets, ref_size, key_ref, depth + 1)?;
                let val = bplist_extract_object(data, offsets, ref_size, val_ref, depth + 1)?;
                // ExifTool prefixes numeric keys with '_'
                let display_key = if key.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    format!("_{key}")
                } else {
                    key
                };
                pairs.push(format!("{display_key}={val}"));
            }
            // Sort by key for consistent output
            pairs.sort();
            Some(format!("{{{}}}", pairs.join(",")))
        }
        _ => None,
    }
}

fn bplist_read_extended_size(data: &[u8], pos: usize) -> Option<usize> {
    if pos >= data.len() {
        return None;
    }
    let marker = data[pos];
    if marker >> 4 != 0x1 {
        return None;
    }
    let nbytes = 1usize << (marker & 0x0F);
    if pos + 1 + nbytes > data.len() {
        return None;
    }
    let mut val: usize = 0;
    for i in 0..nbytes {
        val = (val << 8) | data[pos + 1 + i] as usize;
    }
    Some(val)
}

fn bplist_extended_size_bytes(data: &[u8], pos: usize) -> usize {
    if pos >= data.len() {
        return 1;
    }
    let marker = data[pos];
    let nbytes = 1usize << (marker & 0x0F);
    1 + nbytes // marker byte + size bytes
}

fn format_bplist_float(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        // Match ExifTool's output: remove trailing zeros
        let s = format!("{v}");
        s
    }
}

fn olympus_art_filter_name(v: u16) -> String {
    match v {
        0 => "Off",
        1 => "Soft Focus",
        2 => "Pop Art",
        3 => "Pale & Light Color",
        4 => "Light Tone",
        5 => "Pin Hole",
        6 => "Grainy Film",
        8 => "Underwater",
        9 => "Diorama",
        10 => "Cross Process",
        12 => "Fish Eye",
        13 => "Drawing",
        14 => "Gentle Sepia",
        15 => "Pale & Light Color II",
        16 => "Pop Art II",
        17 => "Pin Hole II",
        18 => "Pin Hole III",
        19 => "Grainy Film II",
        20 => "Dramatic Tone",
        21 => "Punk",
        22 => "Soft Focus 2",
        23 => "Sparkle",
        24 => "Watercolor",
        25 => "Key Line",
        26 => "Key Line II",
        27 => "Miniature",
        28 => "Reflection",
        29 => "Fragmented",
        31 => "Cross Process II",
        32 => "Dramatic Tone II",
        33 => "Watercolor I",
        34 => "Watercolor II",
        35 => "Diorama II",
        36 => "Vintage",
        37 => "Vintage II",
        38 => "Vintage III",
        39 => "Partial Color",
        40 => "Partial Color II",
        41 => "Partial Color III",
        42 => "Bleach Bypass",
        43 => "Bleach Bypass II",
        44 => "Instant Film",
        _ => return format!("Unknown ({v})"),
    }
    .to_string()
}

fn nikon_af_point_51(v: u32) -> String {
    static TABLE: &[(u32, &str)] = &[
        (1, "C6 (Center)"),
        (2, "B6"),
        (3, "A5"),
        (4, "D6"),
        (5, "E5"),
        (6, "C7"),
        (7, "B7"),
        (8, "A6"),
        (9, "D7"),
        (10, "E6"),
        (11, "C5"),
        (12, "B5"),
        (13, "A4"),
        (14, "D5"),
        (15, "E4"),
        (16, "C8"),
        (17, "B8"),
        (18, "A7"),
        (19, "D8"),
        (20, "E7"),
        (21, "C9"),
        (22, "B9"),
        (23, "A8"),
        (24, "D9"),
        (25, "E8"),
        (26, "C10"),
        (27, "B10"),
        (28, "A9"),
        (29, "D10"),
        (30, "E9"),
        (31, "C11"),
        (32, "B11"),
        (33, "D11"),
        (34, "C4"),
        (35, "B4"),
        (36, "A3"),
        (37, "D4"),
        (38, "E3"),
        (39, "C3"),
        (40, "B3"),
        (41, "A2"),
        (42, "D3"),
        (43, "E2"),
        (44, "C2"),
        (45, "B2"),
        (46, "A1"),
        (47, "D2"),
        (48, "E1"),
        (49, "C1"),
        (50, "B1"),
        (51, "D1"),
    ];
    TABLE
        .iter()
        .find(|&&(id, _)| id == v)
        .map_or_else(|| format!("{v}"), |&(_, n)| n.to_string())
}

fn nikon_af_point_39(v: u32) -> String {
    static TABLE: &[(u32, &str)] = &[
        (1, "C6 (Center)"),
        (2, "B6"),
        (3, "A2"),
        (4, "D6"),
        (5, "E2"),
        (6, "C7"),
        (7, "B7"),
        (8, "A3"),
        (9, "D7"),
        (10, "E3"),
        (11, "C5"),
        (12, "B5"),
        (13, "A1"),
        (14, "D5"),
        (15, "E1"),
        (16, "C8"),
        (17, "B8"),
        (18, "D8"),
        (19, "C9"),
        (20, "B9"),
        (21, "D9"),
        (22, "C10"),
        (23, "B10"),
        (24, "D10"),
        (25, "C11"),
        (26, "B11"),
        (27, "D11"),
        (28, "C4"),
        (29, "B4"),
        (30, "D4"),
        (31, "C3"),
        (32, "B3"),
        (33, "D3"),
        (34, "C2"),
        (35, "B2"),
        (36, "D2"),
        (37, "C1"),
        (38, "B1"),
        (39, "D1"),
    ];
    TABLE
        .iter()
        .find(|&&(id, _)| id == v)
        .map_or_else(|| format!("{v}"), |&(_, n)| n.to_string())
}

/// Format a fraction value for ExifTool compatibility (e.g., 0 -> "0", 0.333 -> "+1/3")
fn format_fraction(val: f64) -> String {
    if val.abs() < 0.001 {
        return "0".into();
    }
    let sign = if val > 0.0 { "+" } else { "" };
    let abs = val.abs();
    // Try common fractions: 1/3, 2/3, 1/2
    for denom in [3, 2, 6] {
        let numer = (abs * denom as f64 + 0.01) as i32;
        if ((numer as f64 / denom as f64) - abs).abs() < 0.05 {
            if denom == 1 {
                return format!("{sign}{numer}");
            }
            let numer = if val < 0.0 { -numer } else { numer };
            return format!("{numer}/{denom}");
        }
    }
    format!("{sign}{val:.1}")
}

fn minolta_white_balance(v: u32) -> String {
    match v {
        0 => "Auto".into(),
        1 => "Daylight".into(),
        2 => "Cloudy".into(),
        3 => "Tungsten".into(),
        5 => "Custom".into(),
        7 => "Fluorescent".into(),
        8 => "Fluorescent 2".into(),
        11 => "Custom 2".into(),
        12 => "Custom 3".into(),
        _ => format!("{v}"),
    }
}

fn format_minolta_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    // Minolta tags use int32u for many value fields
    let v32 = entry_u32(entry, be);
    let _v = v32.map(|x| x as u16);
    match name {
        "MinoltaQuality" => match v32 {
            Some(0) => "Raw".into(),
            Some(1) => "Super Fine".into(),
            Some(2) => "Fine".into(),
            Some(3) => "Standard".into(),
            Some(4) => "Economy".into(),
            Some(5) => "Extra fine".into(),
            _ => format_ifd_value(entry, be),
        },
        "ColorMode" => match v32 {
            Some(0) => "Natural Color".into(),
            Some(1) => "Black & White".into(),
            Some(2) => "Vivid Color".into(),
            Some(3) => "Solarization".into(),
            Some(4) => "Adobe RGB".into(),
            Some(5) => "Sepia".into(),
            Some(9) => "Natural".into(),
            Some(12) => "Natural+".into(),
            Some(13) => "Natural+ (2)".into(),
            _ => format_ifd_value(entry, be),
        },
        "SceneMode" => match v32 {
            Some(0) => "Standard".into(),
            Some(1) => "Portrait".into(),
            Some(2) => "Text".into(),
            Some(3) => "Night Scene".into(),
            Some(4) => "Sunset".into(),
            Some(5) => "Sports".into(),
            Some(6) => "Landscape".into(),
            Some(7) => "Night Portrait".into(),
            Some(8) => "Macro".into(),
            Some(9) => "Super Macro".into(),
            _ => format_ifd_value(entry, be),
        },
        "Teleconverter" => match v32 {
            Some(0) => "None".into(),
            Some(0x04) => "Minolta/Sony AF 1.4x APO (D) (0x04)".into(),
            Some(0x08) => "Minolta/Sony AF 2x APO (D) (0x08)".into(),
            _ => format_ifd_value(entry, be),
        },
        "ImageStabilization" => match v32 {
            Some(1) => "Off".into(),
            Some(5) => "On".into(),
            _ => format_ifd_value(entry, be),
        },
        "FlashExposureComp" => {
            // Rational value (0/0 means undef)
            use crate::tiff::value::TagValue;
            if let Some(val) = TagValue::from_entry(entry, be) {
                let s = val.display();
                if s == "0/0" { "undef".into() } else { s }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "MakerNoteVersion" => {
            let s = std::str::from_utf8(entry.data).unwrap_or("");
            s.trim_end_matches('\0').trim().to_string()
        }
        _ => format_ifd_value(entry, be),
    }
}

fn format_sigma_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    // Sigma stores many values as ASCII strings with 4-char prefix like "Qual:", "Shar:", etc.
    // Strip the prefix and return just the value.
    let raw = format_ifd_value(entry, be);

    // Handle MeteringMode specially - Sigma may store as ASCII digit or u16
    if name == "MeteringMode" {
        // Try parsing as ASCII first (Sigma often stores numbers as ASCII strings)
        let ascii_val = std::str::from_utf8(entry.data)
            .ok()
            .and_then(|s| s.trim_end_matches('\0').trim().parse::<u16>().ok());
        let v = ascii_val.or_else(|| entry_u16(entry, be));
        return match v {
            Some(0) => "Center-weighted Average".into(),
            Some(1) => "Average".into(),
            Some(2) => "Evaluative".into(),
            Some(3) => "Spot".into(),
            Some(8) => "Multi-segment".into(),
            _ => raw,
        };
    }

    // Strip prefix like "Qual:", "Shar:", "Cont:", "Satu:", "Expo:", "Fill:", "Shad:", "High:", "CC:"
    if let Some(pos) = raw.find(':') {
        let prefix = &raw[..pos];
        // Only strip if prefix is short (2-4 chars) and alphabetic
        if prefix.len() <= 4 && prefix.chars().all(|c| c.is_ascii_alphabetic()) {
            return raw[pos + 1..].to_string();
        }
    }

    raw
}

/// PentaxEv: convert Pentax EV code to floating point value.
/// Handles 1/3 EV fractional step encoding. Works with signed values.
fn pentax_ev(val: u8) -> f64 {
    pentax_ev_signed(val as i32)
}

fn pentax_ev_signed(val: i32) -> f64 {
    let mut v = val as f64;
    if val & 0x01 != 0 {
        let sign: f64 = if val < 0 { -1.0 } else { 1.0 };
        let frac = ((val * sign as i32).unsigned_abs()) & 0x07;
        if frac == 3 {
            v += sign * (8.0 / 3.0 - 3.0);
        } else if frac == 5 {
            v += sign * (16.0 / 3.0 - 5.0);
        }
    }
    v / 8.0
}

/// Decode Pentax CameraSettings (tag 0x0205) - big-endian binary, 14-25 bytes.
fn decode_pentax_camera_settings(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.is_empty() {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Byte 0: PictureMode2
    push(
        tags,
        "PictureMode2",
        match data[0] {
            0 => "Scene Mode".into(),
            1 => "Auto PICT".into(),
            2 => "Program AE".into(),
            3 => "Green Mode".into(),
            4 => "Shutter Speed Priority".into(),
            5 => "Aperture Priority".into(),
            6 => "Program Tv Shift".into(),
            7 => "Program Av Shift".into(),
            8 => "Manual".into(),
            9 => "Bulb".into(),
            10 => "Aperture Priority, Off-Auto-Aperture".into(),
            11 => "Manual, Off-Auto-Aperture".into(),
            13 => "Shutter & Aperture Priority AE".into(),
            15 => "Sensitivity Priority AE".into(),
            16 => "Flash X-Sync Speed AE".into(),
            v => format!("{v}"),
        },
    );

    if data.len() > 1 {
        // Byte 1 bits: ProgramLine(0-1), EVSteps(5), E-DialInProgram(6), ApertureRingUse(7)
        push(
            tags,
            "ProgramLine",
            match data[1] & 0x03 {
                0 => "Normal".into(),
                1 => "Hi Speed".into(),
                2 => "Depth".into(),
                3 => "MTF".into(),
                v => format!("{v}"),
            },
        );
        push(
            tags,
            "EVSteps",
            if data[1] & 0x20 != 0 {
                "1/3 EV Steps".into()
            } else {
                "1/2 EV Steps".into()
            },
        );
        push(
            tags,
            "E-DialInProgram",
            if data[1] & 0x40 != 0 {
                "P Shift".into()
            } else {
                "Tv or Av".into()
            },
        );
        push(
            tags,
            "ApertureRingUse",
            if data[1] & 0x80 != 0 {
                "Permitted".into()
            } else {
                "Prohibited".into()
            },
        );
    }

    if data.len() > 2 {
        // Byte 2: FlashOptions(hi nibble), MeteringMode2(lo nibble)
        push(
            tags,
            "FlashOptions",
            match (data[2] >> 4) & 0x0f {
                0 => "Normal".into(),
                1 => "Red-eye reduction".into(),
                2 => "Auto".into(),
                3 => "Auto, Red-eye reduction".into(),
                5 => "Wireless (Master)".into(),
                6 => "Wireless (Control)".into(),
                8 => "Slow-sync".into(),
                9 => "Slow-sync, Red-eye reduction".into(),
                10 => "Trailing-curtain Sync".into(),
                v => format!("{v}"),
            },
        );
        push(
            tags,
            "MeteringMode2",
            match data[2] & 0x0f {
                0 => "Multi-segment".into(),
                1 => "Center-weighted average".into(),
                2 => "Spot".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 3 {
        // Byte 3: AFPointMode(hi nibble), FocusMode2(lo nibble)
        push(
            tags,
            "AFPointMode",
            match (data[3] >> 4) & 0x0f {
                0 => "Auto".into(),
                1 => "Select".into(),
                2 => "Fixed Center".into(),
                v => format!("{v}"),
            },
        );
        push(
            tags,
            "FocusMode2",
            match data[3] & 0x0f {
                0 => "Manual".into(),
                1 => "AF-S".into(),
                2 => "AF-C".into(),
                3 => "AF-A".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 5 {
        // Bytes 4-5: AFPointSelected2 (int16u big-endian, bitmask)
        let af = u16::from_be_bytes([data[4], data[5]]);
        let af_str = if af == 0 {
            "Auto".to_string()
        } else {
            let names = [
                "Upper-left",
                "Top",
                "Upper-right",
                "Left",
                "Mid-left",
                "Center",
                "Mid-right",
                "Right",
                "Lower-left",
                "Bottom",
                "Lower-right",
            ];
            let mut pts = Vec::new();
            for (i, n) in names.iter().enumerate() {
                if af & (1 << i) != 0 {
                    pts.push(*n);
                }
            }
            if pts.is_empty() {
                format!("{af}")
            } else {
                pts.join(", ")
            }
        };
        push(tags, "AFPointSelected2", af_str);
    }

    if data.len() > 6 {
        // Byte 6: ISOFloor - 100*exp(PentaxEv(val-32)*log(2))
        let pev = pentax_ev(data[6].wrapping_sub(32));
        let iso = 100.0 * (pev * 2.0f64.ln()).exp();
        push(tags, "ISOFloor", format!("{}", (iso + 0.5) as u32));
    }

    if data.len() > 7 {
        // Byte 7: DriveMode2
        push(
            tags,
            "DriveMode2",
            match data[7] {
                0 => "Single-frame".into(),
                v => {
                    let mut modes = Vec::new();
                    if v & 0x01 != 0 {
                        modes.push("Continuous");
                    }
                    if v & 0x02 != 0 {
                        modes.push("Continuous (Lo)");
                    }
                    if v & 0x04 != 0 {
                        modes.push("Self-timer (12 s)");
                    }
                    if v & 0x08 != 0 {
                        modes.push("Self-timer (2 s)");
                    }
                    if v & 0x10 != 0 {
                        modes.push("Remote Control (3 s delay)");
                    }
                    if v & 0x20 != 0 {
                        modes.push("Remote Control");
                    }
                    if v & 0x40 != 0 {
                        modes.push("Exposure Bracket");
                    }
                    if v & 0x80 != 0 {
                        modes.push("Multiple Exposure");
                    }
                    if modes.is_empty() {
                        format!("{v}")
                    } else {
                        modes.join(", ")
                    }
                }
            },
        );
    }

    if data.len() > 8 {
        // Byte 8: ExposureBracketStepSize
        push(
            tags,
            "ExposureBracketStepSize",
            match data[8] {
                3 => "0.3".into(),
                4 => "0.5".into(),
                5 => "0.7".into(),
                8 => "1.0".into(),
                11 => "1.3".into(),
                12 => "1.5".into(),
                13 => "1.7".into(),
                16 => "2.0".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 9 {
        // Byte 9: BracketShotNumber
        push(
            tags,
            "BracketShotNumber",
            match data[9] {
                0 => "n/a".into(),
                0x02 => "1 of 2".into(),
                0x12 => "2 of 2".into(),
                0x03 => "1 of 3".into(),
                0x13 => "2 of 3".into(),
                0x23 => "3 of 3".into(),
                0x05 => "1 of 5".into(),
                0x15 => "2 of 5".into(),
                0x25 => "3 of 5".into(),
                0x35 => "4 of 5".into(),
                0x45 => "5 of 5".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 10 {
        // Byte 10: WhiteBalanceSet(hi nibble), MultipleExposureSet(lo nibble)
        push(
            tags,
            "WhiteBalanceSet",
            match (data[10] >> 4) & 0x0f {
                0 => "Auto".into(),
                1 => "Daylight".into(),
                2 => "Shade".into(),
                3 => "Cloudy".into(),
                4 => "Daylight Fluorescent".into(),
                5 => "Day White Fluorescent".into(),
                6 => "White Fluorescent".into(),
                7 => "Tungsten".into(),
                8 => "Flash".into(),
                9 => "Manual".into(),
                12 => "Set Color Temperature 1".into(),
                13 => "Set Color Temperature 2".into(),
                14 => "Set Color Temperature 3".into(),
                v => format!("{v}"),
            },
        );
        push(
            tags,
            "MultipleExposureSet",
            match data[10] & 0x0f {
                0 => "Off".into(),
                1 => "On".into(),
                v => format!("{v}"),
            },
        );
    }

    // Bytes 11-12: model-specific, skip for now

    if data.len() > 13 {
        // Byte 13: RawAndJpgRecording (K10D only)
        push(
            tags,
            "RawAndJpgRecording",
            match data[13] {
                0x01 => "JPEG (Best)".into(),
                0x04 => "RAW (PEF, Best)".into(),
                0x05 => "RAW+JPEG (PEF, Best)".into(),
                0x08 => "RAW (DNG, Best)".into(),
                0x09 => "RAW+JPEG (DNG, Best)".into(),
                0x21 => "JPEG (Better)".into(),
                0x24 => "RAW (PEF, Better)".into(),
                0x25 => "RAW+JPEG (PEF, Better)".into(),
                0x28 => "RAW (DNG, Better)".into(),
                0x29 => "RAW+JPEG (DNG, Better)".into(),
                0x41 => "JPEG (Good)".into(),
                0x44 => "RAW (PEF, Good)".into(),
                0x45 => "RAW+JPEG (PEF, Good)".into(),
                0x48 => "RAW (DNG, Good)".into(),
                0x49 => "RAW+JPEG (DNG, Good)".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 14 {
        // Byte 14: JpgRecordedPixels(bits 0-1), SensitivitySteps(bit 1 for K-5)
        push(
            tags,
            "JpgRecordedPixels",
            match data[14] & 0x03 {
                0 => "10 MP".into(),
                1 => "6 MP".into(),
                2 => "2 MP".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 16 {
        // Byte 16: FlashOptions2(hi nibble), MeteringMode3(lo nibble)
        push(
            tags,
            "FlashOptions2",
            match (data[16] >> 4) & 0x0f {
                0 => "Normal".into(),
                1 => "Red-eye reduction".into(),
                2 => "Auto".into(),
                3 => "Auto, Red-eye reduction".into(),
                5 => "Wireless (Master)".into(),
                6 => "Wireless (Control)".into(),
                8 => "Slow-sync".into(),
                9 => "Slow-sync, Red-eye reduction".into(),
                10 => "Trailing-curtain Sync".into(),
                v => format!("{v}"),
            },
        );
        push(
            tags,
            "MeteringMode3",
            match data[16] & 0x0f {
                0 => "Multi-segment".into(),
                1 => "Center-weighted average".into(),
                2 => "Spot".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 17 {
        // Byte 17: SRActive(bit 7), Rotation(bits 5-6), ISOSetting(bit 2), SensitivitySteps(bit 1)
        push(
            tags,
            "SRActive",
            if data[17] & 0x80 != 0 {
                "Yes".into()
            } else {
                "No".into()
            },
        );
        push(
            tags,
            "Rotation",
            match (data[17] >> 5) & 0x03 {
                0 => "Horizontal (normal)".into(),
                1 => "Rotate 180".into(),
                2 => "Rotate 90 CW".into(),
                3 => "Rotate 270 CW".into(),
                v => format!("{v}"),
            },
        );
        push(
            tags,
            "ISOSetting",
            match (data[17] >> 2) & 0x01 {
                0 => "Manual".into(),
                1 => "Auto".into(),
                v => format!("{v}"),
            },
        );
        push(
            tags,
            "SensitivitySteps",
            match (data[17] >> 1) & 0x01 {
                0 => "1 EV Steps".into(),
                1 => "As EV Steps".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 18 {
        // Byte 18: TvExposureTimeSetting - exp(-PentaxEv(val-68)*log(2))
        let pev = pentax_ev(data[18].wrapping_sub(68));
        let tv = (-pev * 2.0f64.ln()).exp();
        if let Some(s) = crate::tiff::tags::format_exposure_time(tv) {
            push(tags, "TvExposureTimeSetting", s);
        }
    }

    if data.len() > 19 {
        // Byte 19: AvApertureSetting - exp(PentaxEv(val-68)*log(2)/2)
        let pev = pentax_ev(data[19].wrapping_sub(68));
        let av = (pev * 2.0f64.ln() / 2.0).exp();
        push(tags, "AvApertureSetting", format!("{av:.1}"));
    }

    if data.len() > 20 {
        // Byte 20: SvISOSetting - 100*exp(PentaxEv(val-32)*log(2))
        let pev = pentax_ev(data[20].wrapping_sub(32));
        let iso = 100.0 * (pev * 2.0f64.ln()).exp();
        push(tags, "SvISOSetting", format!("{}", (iso + 0.5) as u32));
    }

    if data.len() > 21 {
        // Byte 21: BaseExposureCompensation - PentaxEv(64-val)
        let ec = pentax_ev(64u8.wrapping_sub(data[21]));
        if ec.abs() < 0.001 {
            push(tags, "BaseExposureCompensation", "0".into());
        } else {
            push(tags, "BaseExposureCompensation", format!("{ec:+.1}"));
        }
    }
}

/// Decode Pentax AEInfo (tag 0x0206) - 14-25 bytes.
fn decode_pentax_ae_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.is_empty() {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Byte 0: AEExposureTime - 24 * 2^((32-val)/8)
    let aet = 24.0 * (2.0f64).powf((32.0 - data[0] as f64) / 8.0);
    if let Some(s) = crate::tiff::tags::format_exposure_time(aet) {
        push(tags, "AEExposureTime", s);
    }

    if data.len() > 1 {
        // Byte 1: AEAperture - 2^((val-68)/16)
        let ap = (2.0f64).powf((data[1] as f64 - 68.0) / 16.0);
        push(tags, "AEAperture", format!("{ap:.1}"));
    }

    if data.len() > 2 {
        // Byte 2: AE_ISO - 100 * 2^((val-32)/8)
        let iso = 100.0 * (2.0f64).powf((data[2] as f64 - 32.0) / 8.0);
        push(tags, "AE_ISO", format!("{}", (iso + 0.5) as u32));
    }

    if data.len() > 3 {
        // Byte 3: AEXv - (val-64)/8
        let xv = (data[3] as f64 - 64.0) / 8.0;
        if xv.abs() < 0.001 {
            push(tags, "AEXv", "0".into());
        } else {
            // Use enough precision to show fractional eighths
            let s = format!("{xv}");
            push(tags, "AEXv", s);
        }
    }

    if data.len() > 4 {
        // Byte 4: AEBXv - int8s / 8
        let bxv = data[4] as i8 as f64 / 8.0;
        if bxv.abs() < 0.001 {
            push(tags, "AEBXv", "0".into());
        } else {
            push(tags, "AEBXv", format!("{bxv:.1}"));
        }
    }

    if data.len() > 5 {
        // Byte 5: AEMinExposureTime
        let met = 24.0 * (2.0f64).powf((32.0 - data[5] as f64) / 8.0);
        if let Some(s) = crate::tiff::tags::format_exposure_time(met) {
            push(tags, "AEMinExposureTime", s);
        }
    }

    if data.len() > 6 {
        // Byte 6: AEProgramMode
        push(
            tags,
            "AEProgramMode",
            match data[6] {
                0 => "M, P or TAv".into(),
                1 => "Av, B or X".into(),
                2 => "Tv".into(),
                3 => "Sv or Green Mode".into(),
                8 => "Hi-speed Program".into(),
                v => format!("{v}"),
            },
        );
    }

    // Byte 7: AEFlags (bitmask)
    // Byte 8: AEApertureSteps
    if data.len() > 8 {
        push(tags, "AEApertureSteps", format!("{}", data[8]));
    }

    if data.len() > 9 {
        // Byte 9: AEMaxAperture - 2^((val-68)/16)
        let ap = (2.0f64).powf((data[9] as f64 - 68.0) / 16.0);
        push(tags, "AEMaxAperture", format!("{ap:.1}"));
    }

    if data.len() > 10 {
        // Byte 10: AEMaxAperture2
        let ap2 = (2.0f64).powf((data[10] as f64 - 68.0) / 16.0);
        push(tags, "AEMaxAperture2", format!("{ap2:.1}"));
    }

    if data.len() > 11 {
        // Byte 11: AEMinAperture - 2^((val-68)/16), PrintConv="%.0f"
        let ap = (2.0f64).powf((data[11] as f64 - 68.0) / 16.0);
        push(tags, "AEMinAperture", format!("{ap:.0}"));
    }

    if data.len() > 12 {
        // Byte 12: AEMeteringMode
        push(
            tags,
            "AEMeteringMode",
            match data[12] {
                0 => "Multi-segment".into(),
                255 => "Multi-segment".into(),
                v => format!("{v}"),
            },
        );
    }

    // FlashExposureCompSet - PentaxEv(int8s val)
    // Base offset 14, shifted to 15 for models with AEInfo size > 20
    let fec_idx = if data.len() > 20 { 15 } else { 14 };
    if data.len() > fec_idx {
        let fec = pentax_ev_signed(data[fec_idx] as i8 as i32);
        if fec.abs() < 0.001 {
            push(tags, "FlashExposureCompSet", "0".into());
        } else {
            push(tags, "FlashExposureCompSet", format!("{fec:+.1}"));
        }
    }
}

/// Decode Pentax FlashInfo (tag 0x0208) - 27 bytes.
fn decode_pentax_flash_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.is_empty() {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Byte 0: FlashStatus
    push(
        tags,
        "FlashStatus",
        match data[0] {
            0x00 => "Off".into(),
            0x01 => "Off (1)".into(),
            0x02 => "External, Did not fire".into(),
            0x06 => "External, Fired".into(),
            0x08 => "Internal, Did not fire (0x08)".into(),
            0x09 => "Internal, Did not fire".into(),
            0x0d => "Internal, Fired".into(),
            v => format!("{v}"),
        },
    );

    if data.len() > 1 {
        // Byte 1: InternalFlashMode
        push(
            tags,
            "InternalFlashMode",
            match data[1] {
                0x00 => "n/a".into(),
                0x86 => "Fired, Wireless (Master)".into(),
                0x95 => "Fired, Slow-sync, Red-eye reduction".into(),
                0xc0 => "Fired".into(),
                0xc1 => "Fired, Red-eye reduction".into(),
                0xc8 => "Fired, Slow-sync".into(),
                0xc9 => "Fired, Slow-sync, Red-eye reduction".into(),
                0xca => "Fired, Trailing-curtain Sync".into(),
                0xf0 => "Did not fire".into(),
                0xf1 => "Did not fire, Red-eye reduction".into(),
                0xf2 => "Did not fire, Auto".into(),
                0xf3 => "Did not fire, Auto, Red-eye reduction".into(),
                0xf4 => "Did not fire (0xf4)".into(),
                0xf5 => "Did not fire, Wireless (Master)".into(),
                0xf6 => "Did not fire, Wireless (Control)".into(),
                0xf8 => "Did not fire, Slow-sync".into(),
                0xf9 => "Did not fire, Slow-sync, Red-eye reduction".into(),
                0xfa => "Did not fire, Trailing-curtain Sync".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 2 {
        // Byte 2: ExternalFlashMode
        push(
            tags,
            "ExternalFlashMode",
            match data[2] {
                0x00 => "n/a".into(),
                0x3f => "Off".into(),
                0x40 => "On, Auto".into(),
                0x80 => "On, Flash Problem".into(),
                0xbf => "On, Manual".into(),
                0xc0 => "On, P-TTL Auto".into(),
                0xc4 => "On, Wireless".into(),
                0xf0 => "Not Connected".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 3 {
        // Byte 3: InternalFlashStrength
        push(tags, "InternalFlashStrength", format!("{}", data[3]));
    }

    // Bytes 4-7: TTL_DA_AUp, TTL_DA_ADown, TTL_DA_BUp, TTL_DA_BDown
    for (i, name) in [
        (4, "TTL_DA_AUp"),
        (5, "TTL_DA_ADown"),
        (6, "TTL_DA_BUp"),
        (7, "TTL_DA_BDown"),
    ] {
        if data.len() > i {
            push(tags, name, format!("{}", data[i]));
        }
    }

    // Byte 24: ExternalFlashGuideNumber (Mask 0x1f)
    if data.len() > 24 {
        let gn_raw = data[24] & 0x1f;
        if gn_raw == 0 {
            push(tags, "ExternalFlashGuideNumber", "n/a".into());
        } else {
            let gn = (2.0f64).powf(gn_raw as f64 / 16.0 + 4.0);
            push(tags, "ExternalFlashGuideNumber", format!("{gn:.0}"));
        }
    }

    // Byte 25: ExternalFlashExposureComp
    if data.len() > 25 {
        push(
            tags,
            "ExternalFlashExposureComp",
            match data[25] {
                0 => "n/a".into(),
                144 => "n/a (Manual)".into(),
                164 => "-3.0".into(),
                167 => "-2.5".into(),
                168 => "-2.0".into(),
                171 => "-1.5".into(),
                172 => "-1.0".into(),
                175 => "-0.5".into(),
                176 => "0.0".into(),
                179 => "0.5".into(),
                180 => "1.0".into(),
                v => format!("{v}"),
            },
        );
    }

    // Byte 26: ExternalFlashBounce
    if data.len() > 26 {
        push(
            tags,
            "ExternalFlashBounce",
            match data[26] {
                0 => "n/a".into(),
                16 => "Direct".into(),
                48 => "Bounce".into(),
                v => format!("{v}"),
            },
        );
    }
}

/// Decode Pentax ShakeReductionInfo (tag 0x005c) - 4 bytes.
fn decode_pentax_sr_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.is_empty() {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Byte 0: SRResult
    push(
        tags,
        "SRResult",
        match data[0] {
            0 => "Not stabilized".into(),
            1 => "Stabilized".into(),
            v => format!("{v}"),
        },
    );

    if data.len() > 1 {
        // Byte 1: ShakeReduction
        push(
            tags,
            "ShakeReduction",
            match data[1] {
                0 => "Off".into(),
                1 => "On".into(),
                4 => "Off (4)".into(),
                5 => "On but Disabled".into(),
                6 => "On (Video)".into(),
                7 => "On (7)".into(),
                15 => "On (15)".into(),
                39 => "On (mode 2)".into(),
                135 => "On (135)".into(),
                v => format!("{v}"),
            },
        );
    }

    if data.len() > 2 {
        // Byte 2: SRHalfPressTime - val/60 seconds
        let t = data[2] as f64 / 60.0;
        push(tags, "SRHalfPressTime", format!("{t:.2} s"));
    }

    if data.len() > 3 {
        // Byte 3: SRFocalLength - if bit 0 set: val*4, else val/2
        let v = data[3];
        let fl = if v & 0x01 != 0 {
            v as u32 * 4
        } else {
            v as u32 / 2
        };
        push(tags, "SRFocalLength", format!("{fl} mm"));
    }
}

/// Decode Pentax CameraInfo (tag 0x0215) - int32u format, 20 bytes.
fn decode_pentax_camera_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 4 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Field 0 (bytes 0-3): PentaxModelID (int32u big-endian)
    let model_id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    push(
        tags,
        "PentaxModelID",
        pentax_model_name(model_id).unwrap_or_else(|| format!("0x{model_id:x}")),
    );

    // Field 1 (bytes 4-7): ManufactureDate (int32u big-endian) - YYYYMMDD
    if data.len() >= 8 {
        let date = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        if date > 0 {
            let y = date / 10000;
            let m = (date / 100) % 100;
            let d = date % 100;
            push(tags, "ManufactureDate", format!("{y:04}:{m:02}:{d:02}"));
        }
    }

    // Field 2 (bytes 8-15): ProductionCode (int32u[2]) - space->dot conversion
    if data.len() >= 16 {
        let pc1 = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        let pc2 = u32::from_be_bytes([data[12], data[13], data[14], data[15]]);
        let code = format!("{pc1} {pc2}").replace(' ', ".");
        push(tags, "ProductionCode", code);
    }

    // Field 4 (bytes 16-19): InternalSerialNumber (int32u big-endian)
    if data.len() >= 20 {
        let sn = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        push(tags, "InternalSerialNumber", format!("{sn}"));
    }
}

/// Decode Pentax BatteryInfo (tag 0x0216) - 6 bytes, big-endian.
fn decode_pentax_battery_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.is_empty() {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Byte 0 lo nibble: PowerSource (Mask 0x0f)
    let ps = data[0] & 0x0f;
    push(
        tags,
        "PowerSource",
        match ps {
            2 => "Body Battery".into(),
            3 => "Grip Battery".into(),
            4 => "External Power Supply".into(),
            v => format!("{v}"),
        },
    );

    if data.len() > 1 {
        // Byte 1 hi nibble: BodyBatteryState (Mask 0xf0)
        let bbs = (data[1] >> 4) & 0x0f;
        // K10D/K20D (6-byte BatteryInfo): 4=Full; other models: 4=Close to Full, 5=Full
        let is_older = data.len() <= 6;
        push(
            tags,
            "BodyBatteryState",
            match (bbs, is_older) {
                (1, _) => "Empty or Missing".into(),
                (2, _) => "Almost Empty".into(),
                (3, _) => "Running Low".into(),
                (4, true) => "Full".into(),
                (4, false) => "Close to Full".into(),
                (5, _) => "Full".into(),
                (v, _) => format!("{v}"),
            },
        );
        // Byte 1 lo nibble: GripBatteryState (Mask 0x0f) - K10D/K20D only
        if is_older {
            let gbs = data[1] & 0x0f;
            push(
                tags,
                "GripBatteryState",
                match gbs {
                    1 => "Empty or Missing".into(),
                    2 => "Almost Empty".into(),
                    3 => "Running Low".into(),
                    4 => "Full".into(),
                    v => format!("{v}"),
                },
            );
        }
    }

    // Bytes 2-5: battery AD readings - K10D/K20D (6-byte BatteryInfo)
    if data.len() == 6 {
        // K10D/K20D format: calibrated for K10D with new Pentax battery
        // DVM readings: 8.18V=186, 8.42-8.40V=192 (full), 6.86V=155 (empty)
        if data.len() > 2 {
            let v = data[2] as f64;
            let voltage = v * 8.18 / 186.0;
            let pct = ((v - 155.0) * 100.0 / 35.0).max(0.0) as u32;
            push(
                tags,
                "BodyBatteryADNoLoad",
                format!("{} ({:.1}V, {}%)", data[2], voltage, pct),
            );
        }
        if data.len() > 3 {
            let v = data[3] as f64;
            let voltage = v * 8.18 / 186.0;
            let pct = ((v - 152.0) * 100.0 / 34.0).max(0.0) as u32;
            push(
                tags,
                "BodyBatteryADLoad",
                format!("{} ({:.1}V, {}%)", data[3], voltage, pct),
            );
        }
        if data.len() > 4 {
            push(tags, "GripBatteryADNoLoad", format!("{}", data[4]));
        }
        if data.len() > 5 {
            push(tags, "GripBatteryADLoad", format!("{}", data[5]));
        }
    }
}

/// Decode Pentax LensInfo (tag 0x0207) - lens type + LensData sub-table.
fn decode_pentax_lens_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 2 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // LensType: first 4 bytes, key is "(byte0 & 0x0f) byte3" for pentaxLensTypes lookup
    if data.len() >= 4 {
        let series = data[0] & 0x0f;
        let model = data[3];
        push(tags, "LensType", pentax_lens_type(series, model));
    }

    // LensData sub-directory starts at offset 4 (17 bytes)
    if data.len() > 4 {
        let ld = &data[4..];

        // LensData byte 0, Mask 0x01: AutoAperture
        if !ld.is_empty() {
            push(
                tags,
                "AutoAperture",
                if ld[0] & 0x01 == 0 {
                    "On".into()
                } else {
                    "Off".into()
                },
            );
        }

        // LensData byte 3, Mask 0xf8: MinFocusDistance
        if ld.len() > 3 {
            let v = (ld[3] >> 3) & 0x1f;
            push(
                tags,
                "MinFocusDistance",
                match v {
                    0 => "0.13-0.19 m".into(),
                    1 => "0.20-0.24 m".into(),
                    2 => "0.25-0.28 m".into(),
                    3 => "0.28-0.30 m".into(),
                    4 => "0.35-0.38 m".into(),
                    5 => "0.40-0.45 m".into(),
                    6 => "0.49-0.50 m".into(),
                    7 => "0.6 m".into(),
                    8 => "0.7 m".into(),
                    9 => "0.8-0.9 m".into(),
                    10 => "1.0 m".into(),
                    11 => "1.1-1.2 m".into(),
                    12 => "1.4-1.5 m".into(),
                    13 => "1.5 m".into(),
                    14 => "2.0 m".into(),
                    15 => "2.0-2.1 m".into(),
                    16 => "2.1 m".into(),
                    17 => "2.2-2.9 m".into(),
                    18 => "3.0 m".into(),
                    19 => "4-5 m".into(),
                    20 => "5.6 m".into(),
                    v => format!("{v}"),
                },
            );
        }

        // LensData byte 3, Mask 0x07: FocusRangeIndex
        if ld.len() > 3 {
            let v = ld[3] & 0x07;
            push(
                tags,
                "FocusRangeIndex",
                match v {
                    7 => "0 (very close)".into(),
                    6 => "1 (close)".into(),
                    4 => "2".into(),
                    5 => "3".into(),
                    1 => "4".into(),
                    0 => "5".into(),
                    2 => "6 (far)".into(),
                    3 => "7 (very far)".into(),
                    v => format!("{v}"),
                },
            );
        }

        // LensData byte 9: LensFocalLength
        // ValueConv: 10*(val>>2) * 4^((val&0x03)-2)
        if ld.len() > 9 && ld[9] > 0 {
            let v = ld[9] as f64;
            let fl =
                10.0 * (v as u32 >> 2) as f64 * (4.0f64).powf(((v as u32 & 0x03) as f64) - 2.0);
            push(tags, "LensFocalLength", format!("{fl:.1} mm"));
        }

        // LensData byte 10, Mask 0xf0: NominalMaxAperture
        // ValueConv: 2^(val/4)
        if ld.len() > 10 {
            let v = (ld[10] >> 4) & 0x0f;
            if v > 0 {
                let ap = (2.0f64).powf(v as f64 / 4.0);
                push(tags, "NominalMaxAperture", format!("{ap:.1}"));
            }
        }

        // LensData byte 10, Mask 0x0f: NominalMinAperture
        // ValueConv: 2^((val+10)/4)
        if ld.len() > 10 {
            let v = ld[10] & 0x0f;
            if v > 0 {
                let ap = (2.0f64).powf((v as f64 + 10.0) / 4.0);
                push(tags, "NominalMinAperture", format!("{ap:.0}"));
            }
        }
    }
}

/// Pentax lens type lookup - key is (series, model).
fn pentax_lens_type(series: u8, model: u8) -> String {
    // For ambiguous lenses, output the generic name
    match (series, model) {
        (3, 44) => "Sigma or Tamron Lens (3 44)".into(),
        (3, 0) => "Sigma 70-200mm F2.8 EX".into(),
        (3, 17) => "smc PENTAX-FA 28-70mm F4 AL".into(),
        (3, 21) => "Tamron AF 28-200mm F3.8-5.6".into(),
        (3, 35) => "Tamron AF 28-300mm F3.5-6.3 AD".into(),
        (3, 41) => "smc PENTAX-F Macro 50mm F2.8 or Sigma Lens".into(),
        (3, 42) => "Sigma 300mm F2.8 EX DG APO IF".into(),
        _ => format!("{series} {model}"),
    }
}

/// Decode Pentax AFInfo (tag 0x021f) - big-endian.
fn decode_pentax_af_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    if data.len() < 8 {
        return;
    }
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Byte 4-5: AFPredictor (int16s big-endian)
    let pred = i16::from_be_bytes([data[4], data[5]]);
    push(tags, "AFPredictor", format!("{pred}"));

    // Byte 6: AFDefocus
    push(tags, "AFDefocus", format!("{}", data[6] as i8));

    // Byte 7: AFIntegrationTime - val * 2 ms
    let ait = data[7] as u32 * 2;
    push(tags, "AFIntegrationTime", format!("{ait} ms"));

    // Byte 0x0b: AFPointsInFocus
    if data.len() > 0x0b {
        push(
            tags,
            "AFPointsInFocus",
            match data[0x0b] {
                0 => "None".into(),
                1 => "Lower-left, Bottom".into(),
                2 => "Bottom".into(),
                3 => "Lower-right, Bottom".into(),
                4 => "Mid-left, Center".into(),
                5 => "Center (horizontal)".into(),
                6 => "Mid-right, Center".into(),
                7 => "Upper-left, Top".into(),
                8 => "Top".into(),
                9 => "Upper-right, Top".into(),
                v => format!("{v}"),
            },
        );
    }
}

/// Decode Pentax ColorInfo (tag 0x0222) - int8s format.
fn decode_pentax_color_info(data: &[u8], tags: &mut Vec<DecodedTag>) {
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    // Byte 16: WBShiftAB (int8s)
    if data.len() > 16 {
        push(tags, "WBShiftAB", format!("{}", data[16] as i8));
    }

    // Byte 17: WBShiftGM (int8s)
    if data.len() > 17 {
        push(tags, "WBShiftGM", format!("{}", data[17] as i8));
    }
}

fn format_pentax_value(entry: &IfdEntry<'_>, name: &str, be: bool) -> String {
    let v = entry_u16(entry, be);
    match name {
        "Quality" => match v {
            Some(0) => "Good".to_string(),
            Some(1) => "Better".to_string(),
            Some(2) => "Best".to_string(),
            Some(3) => "TIFF".to_string(),
            Some(4) => "RAW".to_string(),
            Some(5) => "Premium".to_string(),
            Some(65535) => "n/a".to_string(),
            _ => format_ifd_value(entry, be),
        },
        "FocusMode" => match v {
            Some(0x00) => "Normal".into(),
            Some(0x01) => "Macro".into(),
            Some(0x02) => "Infinity".into(),
            Some(0x03) => "Manual".into(),
            Some(0x04) => "Super Macro".into(),
            Some(0x05) => "Pan Focus".into(),
            Some(0x06) => "Auto-area".into(),
            Some(0x07) => "Zone Select".into(),
            Some(0x08) => "Select".into(),
            Some(0x09) => "Pinpoint".into(),
            Some(0x0a) => "Tracking".into(),
            Some(0x0b) => "Continuous".into(),
            Some(0x0c) => "Snap".into(),
            Some(0x10) => "AF-S (Focus-priority)".into(),
            Some(0x11) => "AF-C (Focus-priority)".into(),
            Some(0x12) => "AF-A (Focus-priority)".into(),
            Some(0x20) => "Contrast-detect (Focus-priority)".into(),
            Some(0x21) => "Tracking Contrast-detect (Focus-priority)".into(),
            Some(0x110) => "AF-S (Release-priority)".into(),
            Some(0x111) => "AF-C (Release-priority)".into(),
            Some(0x112) => "AF-A (Release-priority)".into(),
            Some(0x120) => "Contrast-detect (Release-priority)".into(),
            _ => format_ifd_value(entry, be),
        },
        "FlashMode" => {
            let label = match v {
                Some(0x000) => "Auto, Did not fire",
                Some(0x001) => "Off, Did not fire",
                Some(0x002) => "On, Did not fire",
                Some(0x003) => "Auto, Did not fire, Red-eye reduction",
                Some(0x005) => "On, Did not fire, Wireless (Master)",
                Some(0x100) => "Auto, Fired",
                Some(0x102) => "On, Fired",
                Some(0x103) => "Auto, Fired, Red-eye reduction",
                Some(0x104) => "On, Red-eye reduction",
                Some(0x105) => "On, Wireless (Master)",
                Some(0x106) => "On, Wireless (Control)",
                Some(0x108) => "On, Soft",
                Some(0x109) => "On, Slow-sync",
                Some(0x10a) => "On, Slow-sync, Red-eye reduction",
                Some(0x10b) => "On, Trailing-curtain Sync",
                _ => return format_ifd_value(entry, be),
            };
            // Second u16: flash source
            if entry.data.len() >= 4 {
                let v2 = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                let src = match v2 {
                    0x000 => "n/a - Off-Auto-Aperture",
                    0x03f => "Internal",
                    0x100 => "External, Auto",
                    0x23f => "External, Flash Problem",
                    0x300 => "External, Manual",
                    0x304 => "External, P-TTL Auto",
                    0x305 => "External, Contrast-control Sync",
                    0x306 => "External, High-speed Sync",
                    0x30c => "External, Wireless",
                    0x30d => "External, Wireless, High-speed Sync",
                    _ => return format!("{label}; Unknown (0x{v2:x})"),
                };
                format!("{label}; {src}")
            } else {
                label.into()
            }
        }
        "MeteringMode" | "MeteringMode2" => match v {
            Some(0) => "Multi-segment".to_string(),
            Some(1) => "Center-weighted average".to_string(),
            Some(2) => "Spot".to_string(),
            _ => format_ifd_value(entry, be),
        },
        "WorldTimeLocation" => match v {
            Some(0) => "Hometown".into(),
            Some(1) => "Destination".into(),
            _ => format_ifd_value(entry, be),
        },
        "HometownDST" | "DestinationDST" => match v {
            Some(0) => "No".into(),
            Some(1) => "Yes".into(),
            _ => format_ifd_value(entry, be),
        },
        "ImageTone" => match v {
            Some(0) => "Natural".into(),
            Some(1) => "Bright".into(),
            Some(2) => "Portrait".into(),
            Some(3) => "Landscape".into(),
            Some(4) => "Vibrant".into(),
            Some(5) => "Monochrome".into(),
            Some(6) => "Muted".into(),
            Some(7) => "Reversal Film".into(),
            Some(8) => "Bleach Bypass".into(),
            Some(9) => "Radiant".into(),
            _ => format_ifd_value(entry, be),
        },
        "Saturation" => match v {
            Some(0) => "Low".into(),
            Some(1) => "Normal".into(),
            Some(2) => "High".into(),
            Some(3) => "Med Low".into(),
            Some(4) => "Med High".into(),
            Some(5) => "Very Low".into(),
            Some(6) => "Very High".into(),
            Some(65535) => "-1".into(),
            _ => format_ifd_value(entry, be),
        },
        "Contrast" => match v {
            Some(0) => "Low".into(),
            Some(1) => "Normal".into(),
            Some(2) => "High".into(),
            Some(3) => "Med Low".into(),
            Some(4) => "Med High".into(),
            Some(5) => "Very Low".into(),
            Some(6) => "Very High".into(),
            Some(65535) => "-1".into(),
            _ => format_ifd_value(entry, be),
        },
        "Sharpness" => match v {
            Some(0) => "Soft".into(),
            Some(1) => "Normal".into(),
            Some(2) => "Hard".into(),
            Some(3) => "Med Soft".into(),
            Some(4) => "Med Hard".into(),
            Some(5) => "Very Soft".into(),
            Some(6) => "Very Hard".into(),
            _ => format_ifd_value(entry, be),
        },
        "ISO" => match v {
            Some(3) => "50".into(),
            Some(4) => "64".into(),
            Some(5) => "80".into(),
            Some(6) => "100".into(),
            Some(7) => "125".into(),
            Some(8) => "160".into(),
            Some(9) => "200".into(),
            Some(10) => "250".into(),
            Some(11) => "320".into(),
            Some(12) => "400".into(),
            Some(13) => "500".into(),
            Some(14) => "640".into(),
            Some(15) => "800".into(),
            Some(16) => "1000".into(),
            Some(17) => "1250".into(),
            Some(18) => "1600".into(),
            Some(19) => "2000".into(),
            Some(20) => "2500".into(),
            Some(21) => "3200".into(),
            Some(22) => "4000".into(),
            Some(23) => "5000".into(),
            Some(24) => "6400".into(),
            Some(25) => "8000".into(),
            Some(26) => "10000".into(),
            Some(27) => "12800".into(),
            Some(50) => "50".into(),
            Some(100) => "100".into(),
            Some(200) => "200".into(),
            Some(400) => "400".into(),
            Some(800) => "800".into(),
            Some(1600) => "1600".into(),
            Some(3200) => "3200".into(),
            _ => format_ifd_value(entry, be),
        },
        "NoiseReduction" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            _ => format_ifd_value(entry, be),
        },
        "AELock" => match v {
            Some(0) => "Off".into(),
            Some(1) => "On".into(),
            _ => format_ifd_value(entry, be),
        },
        "WhiteBalance" => {
            // Pentax WhiteBalance is multi-value: first value is the mode
            if entry.data.len() >= 4 {
                let mode = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                match mode {
                    0 => "Auto".into(),
                    1 => "Daylight".into(),
                    2 => "Shade".into(),
                    3 => "Fluorescent".into(),
                    4 => "Tungsten".into(),
                    5 => "Manual".into(),
                    6 => "Daylight Fluorescent".into(),
                    7 => "Day White Fluorescent".into(),
                    8 => "White Fluorescent".into(),
                    9 => "Flash".into(),
                    10 => "Cloudy".into(),
                    15 => "Color Temperature Enhancement".into(),
                    17 => "Kelvin".into(),
                    65534 => "Unknown".into(),
                    65535 => "User-Selected".into(),
                    _ => format!("Unknown ({mode})"),
                }
            } else {
                match v {
                    Some(0) => "Auto".into(),
                    Some(1) => "Daylight".into(),
                    Some(2) => "Shade".into(),
                    Some(3) => "Fluorescent".into(),
                    Some(4) => "Tungsten".into(),
                    Some(5) => "Manual".into(),
                    _ => format_ifd_value(entry, be),
                }
            }
        }
        "AutoBracketing" => {
            // Multi-value: first u16 is bracket step, second is extended bracket
            // ValueConv: val < 10 -> val/3, 10..19 -> val-9.5, 0x1000+ -> (val-0x1000)/2, 0x2000+ -> (val-0x2000)/3
            let step_raw = entry_u16(entry, be).unwrap_or(0);
            let step = if step_raw < 10 {
                step_raw as f64 / 3.0
            } else if step_raw < 20 {
                step_raw as f64 - 9.5
            } else if step_raw & 0x1000 != 0 {
                (step_raw - 0x1000) as f64 / 2.0
            } else if step_raw & 0x2000 != 0 {
                (step_raw - 0x2000) as f64 / 3.0
            } else {
                step_raw as f64
            };
            // Format step: integer if whole, else 1 decimal
            let step_s = if step == 0.0 {
                "0".to_string()
            } else {
                format!("{step:.1}")
            };
            // Second value: extended bracket
            if entry.data.len() >= 4 {
                let ext = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                if ext == 0 {
                    format!("{step_s} EV, No Extended Bracket")
                } else {
                    let t = ext >> 8;
                    let s = ext & 0xff;
                    let name = match t {
                        1 => "WB-BA",
                        2 => "WB-GM",
                        3 => "Saturation",
                        4 => "Sharpness",
                        5 => "Contrast",
                        6 => "Hue",
                        7 => "HighLowKey",
                        _ => return format!("{step_s} EV, Unknown({t})+{s}"),
                    };
                    format!("{step_s} EV, {name}+{s}")
                }
            } else {
                format!("{step_s} EV")
            }
        }
        "WhiteBalanceMode" => match v {
            Some(1) => "Auto (Daylight)".into(),
            Some(2) => "Auto (Shade)".into(),
            Some(3) => "Auto (Flash)".into(),
            Some(4) => "Auto (Tungsten)".into(),
            Some(6) => "Auto (Daylight Fluorescent)".into(),
            Some(7) => "Auto (Day White Fluorescent)".into(),
            Some(8) => "Auto (White Fluorescent)".into(),
            Some(10) => "Auto (Cloudy)".into(),
            Some(0xFFFE) => "Unknown".into(),
            Some(0xFFFF) => "User-Selected".into(),
            _ => format_ifd_value(entry, be),
        },
        "ExposureCompensation" => {
            // Stored as fixed-point value: val / 10.0
            if let Some(v) = v {
                let ev = (v as i16 - 50) as f64 / 10.0;
                if ev == 0.0 {
                    "0".into()
                } else {
                    format!("{ev:+.1}")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PreviewImageSize" => {
            // 2 u16 values: width, height
            if entry.data.len() >= 4 {
                let w = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                let h = if be {
                    u16::from_be_bytes([entry.data[2], entry.data[3]])
                } else {
                    u16::from_le_bytes([entry.data[2], entry.data[3]])
                };
                format!("{w}x{h}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PentaxModelID" => {
            let id = entry_u32(entry, be);
            match id {
                Some(id) => pentax_model_name(id).unwrap_or_else(|| format!("{id}")),
                None => format_ifd_value(entry, be),
            }
        }
        "HometownCity" | "DestinationCity" => match v {
            Some(c) => pentax_city(c as u32),
            None => format_ifd_value(entry, be),
        },
        "EffectiveLV" => {
            // Stored as u16; actual LV = value / 1024
            match v {
                Some(v) if v > 0 => {
                    let lv = v as f64 / 1024.0;
                    format!("{lv:.1}")
                }
                _ => format_ifd_value(entry, be),
            }
        }
        "DataScaling" => {
            // int16u, but may be stored as int32u in some models
            let v32 = entry_u32(entry, be);
            let v16 = v;
            match v16.filter(|&x| x > 0).or(v32.map(|x| x as u16)) {
                Some(v) if v > 0 => format!("{v}"),
                _ => match v32 {
                    Some(v) => format!("{v}"),
                    None => format_ifd_value(entry, be),
                },
            }
        }
        "DriveMode" => {
            // Multi-byte: byte 0 = drive mode, byte 1 = timer, byte 2 = shutter release, byte 3 = exposure count
            if entry.data.len() >= 4 {
                let mode = match entry.data[0] {
                    0 => "Single-frame",
                    1 => "Continuous",
                    2 => "Continuous (Hi)",
                    3 => "Burst",
                    4 => "Continuous (Lo)",
                    255 => "Video",
                    _ => return format_ifd_value(entry, be),
                };
                let timer = match entry.data[1] {
                    0 => "No Timer",
                    1 => "Self-timer (12 s)",
                    2 => "Self-timer (2 s)",
                    15 => "Mirror Lock-up",
                    16 => "Remote Control (0 s)",
                    17 => "Remote Control (3 s)",
                    _ => "Unknown Timer",
                };
                let shutter = match entry.data[2] {
                    0 => "Shutter Button",
                    1 => "Remote Control",
                    _ => "Unknown",
                };
                let exp = match entry.data[3] {
                    0 => "Single Exposure",
                    1 => "Continuous Exposure",
                    _ => "Unknown",
                };
                format!("{mode}; {timer}; {shutter}; {exp}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ShakeReductionInfo" => {
            // Multi-byte: first byte is on/off
            if !entry.data.is_empty() {
                match entry.data[0] {
                    0 => "Off".into(),
                    1 => "On".into(),
                    _ => format_ifd_value(entry, be),
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "ImageEditing" => {
            // Multi-byte: all zeros = None
            if entry.data.iter().all(|&b| b == 0) {
                "None".into()
            } else {
                format_ifd_value(entry, be)
            }
        }
        "AFPointSelected" => match v {
            Some(0xFFFF) => "Auto".into(),
            Some(0) => "Fixed Center".into(),
            Some(1) => "Top-left".into(),
            Some(2) => "Top-center".into(),
            Some(3) => "Top-right".into(),
            Some(4) => "Left".into(),
            Some(5) => "Mid-left".into(),
            Some(6) => "Center".into(),
            Some(7) => "Mid-right".into(),
            Some(8) => "Right".into(),
            Some(9) => "Bottom-left".into(),
            Some(10) => "Bottom-center".into(),
            Some(11) => "Bottom-right".into(),
            Some(16) => "Center".into(),
            _ => format_ifd_value(entry, be),
        },
        "AFPointsInFocus" => match v {
            Some(0) => "None".into(),
            Some(1) => "Upper-left".into(),
            Some(2) => "Top".into(),
            Some(3) => "Upper-right".into(),
            Some(4) => "Left".into(),
            Some(5) => "Mid-left".into(),
            Some(6) => "Center (horizontal)".into(),
            Some(7) => "Mid-right".into(),
            Some(8) => "Right".into(),
            Some(9) => "Lower-left".into(),
            Some(10) => "Bottom".into(),
            Some(11) => "Lower-right".into(),
            Some(0xFFFF) => "None".into(),
            _ => format_ifd_value(entry, be),
        },
        "PictureMode" => {
            // Multi-byte: byte 0 = scene mode, byte 1 = ?, byte 2 = EV step
            if entry.data.len() >= 3 {
                let mode = match entry.data[0] {
                    1 => "Program",
                    2 => "Program AE",
                    3 => "Manual",
                    5 => "Aperture Priority",
                    6 => "Shutter Priority",
                    8 => "Scene Mode",
                    9 => "Auto PICT",
                    11 => "Sensitivity Priority",
                    12 => "Shutter & Aperture Priority",
                    15 => "Auto PICT (2)",
                    16 => "Movie",
                    _ => return format_ifd_value(entry, be),
                };
                let ev = match entry.data[2] {
                    0 => "; 1/2 EV steps",
                    1 => "; 1/3 EV steps",
                    _ => "",
                };
                format!("{mode}{ev}")
            } else {
                format_ifd_value(entry, be)
            }
        }
        "CameraTemperature" => {
            // int8s - single signed byte
            if !entry.data.is_empty() {
                format!("{} C", entry.data[0] as i8)
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PreviewImageBorders" => {
            // 4 u8 values, space-separated as decimal
            if entry.data.len() >= 4 {
                format!(
                    "{} {} {} {}",
                    entry.data[0], entry.data[1], entry.data[2], entry.data[3]
                )
            } else {
                format_ifd_value(entry, be)
            }
        }
        "PentaxVersion" | "InternalSerialNumber" | "FirmwareVersion" => {
            // Version is stored as 4 bytes, printed as "x.x.x.x"
            if entry.data.len() >= 4 && entry.data.iter().all(|&b| b <= 127) {
                format!(
                    "{}.{}.{}.{}",
                    entry.data[0], entry.data[1], entry.data[2], entry.data[3]
                )
            } else {
                let s = std::str::from_utf8(entry.data).unwrap_or("");
                s.trim_end_matches('\0').trim().to_string()
            }
        }
        "CPUFirmwareVersion" | "DSPFirmwareVersion" => {
            // 4 bytes encrypted by toggling all bits (XOR 0xFF), then "d.dd.dd.dd"
            if entry.data.len() >= 4 {
                let a: Vec<u8> = entry.data[..4].iter().map(|&b| b ^ 0xFF).collect();
                format!("{}.{:02}.{:02}.{:02}", a[0], a[1], a[2], a[3])
            } else {
                format_ifd_value(entry, be)
            }
        }
        "Date" => {
            // 4 bytes: year(u16), month(u8), day(u8)
            if entry.data.len() >= 4 {
                let year = if be {
                    u16::from_be_bytes([entry.data[0], entry.data[1]])
                } else {
                    u16::from_le_bytes([entry.data[0], entry.data[1]])
                };
                format!("{year:04}:{:02}:{:02}", entry.data[2], entry.data[3])
            } else {
                format_ifd_value(entry, be)
            }
        }
        "Time" => {
            // 3 bytes: hour, minute, second
            if entry.data.len() >= 3 {
                format!(
                    "{:02}:{:02}:{:02}",
                    entry.data[0], entry.data[1], entry.data[2]
                )
            } else {
                format_ifd_value(entry, be)
            }
        }
        "SensitivityAdjust" => {
            if let Some(raw) = entry_u16(entry, be) {
                let adj = (raw as f64 - 50.0) / 10.0;
                if adj == 0.0 {
                    "0".into()
                } else {
                    format!("{adj:+.1}")
                }
            } else {
                format_ifd_value(entry, be)
            }
        }
        "RawDevelopmentProcess" => {
            let name = match v {
                Some(1) => Some("K10D,K200D,K2000,K-m"),
                Some(3) => Some("K20D"),
                Some(4) => Some("K-7"),
                Some(5) => Some("K-x"),
                Some(6) => Some("645D"),
                Some(7) => Some("K-r"),
                Some(8) => Some("K-5,K-5II,K-5IIs"),
                Some(9) => Some("Q"),
                Some(10) => Some("K-01,K-30,K-50,K-500"),
                Some(11) => Some("Q10"),
                Some(12) => Some("MX-1,Q-S1,Q7"),
                Some(13) => Some("K-3,K-3II"),
                Some(14) => Some("645Z"),
                Some(15) => Some("K-S1,K-S2"),
                Some(16) => Some("K-1"),
                Some(17) => Some("K-70"),
                Some(18) => Some("KP"),
                Some(19) => Some("GR III"),
                Some(20) => Some("K-3III"),
                Some(21) => Some("K-3IIIMonochrome"),
                _ => None,
            };
            match (v, name) {
                (Some(val), Some(n)) => format!("{val} ({n})"),
                _ => format_ifd_value(entry, be),
            }
        }
        "WB_RGGBLevelsDaylight"
        | "WB_RGGBLevelsShade"
        | "WB_RGGBLevelsCloudy"
        | "WB_RGGBLevelsTungsten"
        | "WB_RGGBLevelsFlash"
        | "WB_RGGBLevelsFluorescentD"
        | "WB_RGGBLevelsFluorescentN"
        | "WB_RGGBLevelsFluorescentW" => {
            // 4 x int16u: R, G, G, B levels
            let vals = read_u16_array(entry.data, be);
            vals.iter()
                .map(|v| format!("{v}"))
                .collect::<Vec<_>>()
                .join(" ")
        }
        "AEMeteringSegments" | "FlashMeteringSegments" | "SlaveFlashMeteringSegments" => {
            // int8u array: metering segment values -> LV conversion
            // val=255->"n/a", val=0->"0", else val/8-6 as %.1f
            entry
                .data
                .iter()
                .map(|&v| {
                    if v == 255 {
                        "n/a".to_string()
                    } else if v == 0 {
                        "0".to_string()
                    } else {
                        format!("{:.1}", v as f64 / 8.0 - 6.0)
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => format_ifd_value(entry, be),
    }
}

// -- Pentax city and model lookups --------------------------------------

fn pentax_city(code: u32) -> String {
    static CITIES: &[&str] = &[
        "Pago Pago",
        "Honolulu",
        "Anchorage",
        "Vancouver",
        "San Francisco",
        "Los Angeles",
        "Calgary",
        "Denver",
        "Mexico City",
        "Chicago",
        "Miami",
        "Toronto",
        "New York",
        "Santiago",
        "Caracus",
        "Halifax",
        "Buenos Aires",
        "Sao Paulo",
        "Rio de Janeiro",
        "Madrid",
        "London",
        "Paris",
        "Milan",
        "Rome",
        "Berlin",
        "Johannesburg",
        "Istanbul",
        "Cairo",
        "Jerusalem",
        "Moscow",
        "Jeddah",
        "Tehran",
        "Dubai",
        "Karachi",
        "Kabul",
        "Male",
        "Delhi",
        "Colombo",
        "Kathmandu",
        "Dacca",
        "Yangon",
        "Bangkok",
        "Kuala Lumpur",
        "Vientiane",
        "Singapore",
        "Phnom Penh",
        "Ho Chi Minh",
        "Jakarta",
        "Hong Kong",
        "Perth",
        "Beijing",
        "Shanghai",
        "Manila",
        "Taipei",
        "Seoul",
        "Adelaide",
        "Tokyo",
        "Guam",
        "Sydney",
        "Noumea",
        "Wellington",
        "Auckland",
        "Lima",
        "Dakar",
        "Algiers",
        "Helsinki",
        "Athens",
        "Nairobi",
        "Amsterdam",
        "Stockholm",
        "Lisbon",
        "Copenhagen",
        "Warsaw",
        "Prague",
        "Budapest",
    ];
    if (code as usize) < CITIES.len() {
        CITIES[code as usize].to_string()
    } else {
        format!("{code}")
    }
}

fn pentax_model_name(id: u32) -> Option<String> {
    static MODELS: &[(u32, &str)] = &[
        (0x0000d, "Optio 330/430"),
        (0x12926, "Optio 230"),
        (0x12958, "Optio 330GS"),
        (0x12962, "Optio 450/550"),
        (0x1296c, "Optio S"),
        (0x12971, "Optio S V1.01"),
        (0x12994, "*ist D"),
        (0x129b2, "Optio 33L"),
        (0x129bc, "Optio 33LF"),
        (0x129c6, "Optio 33WR/43WR/555"),
        (0x129d5, "Optio S4"),
        (0x12a02, "Optio MX"),
        (0x12a0c, "Optio S40"),
        (0x12a16, "Optio S4i"),
        (0x12a34, "Optio 30"),
        (0x12a52, "Optio S30"),
        (0x12a66, "Optio 750Z"),
        (0x12a70, "Optio SV"),
        (0x12a75, "Optio SVi"),
        (0x12a7a, "Optio X"),
        (0x12a8e, "Optio S5i"),
        (0x12a98, "Optio S50"),
        (0x12aa2, "*ist DS"),
        (0x12ab6, "Optio MX4"),
        (0x12ac0, "Optio S5n"),
        (0x12aca, "Optio WP"),
        (0x12afc, "Optio S55"),
        (0x12b10, "Optio S5z"),
        (0x12b1a, "*ist DL"),
        (0x12b24, "Optio S60"),
        (0x12b2e, "Optio S45"),
        (0x12b38, "Optio S6"),
        (0x12b4c, "Optio WPi"),
        (0x12b56, "BenQ DC X600"),
        (0x12b60, "*ist DS2"),
        (0x12b62, "Samsung GX-1S"),
        (0x12b6a, "Optio A10"),
        (0x12b7e, "*ist DL2"),
        (0x12b80, "Samsung GX-1L"),
        (0x12b9c, "K100D"),
        (0x12b9d, "K110D"),
        (0x12ba2, "K100D Super"),
        (0x12bb0, "Optio T10/T20"),
        (0x12be2, "Optio W10"),
        (0x12bf6, "Optio M10"),
        (0x12c1e, "K10D"),
        (0x12c20, "Samsung GX10"),
        (0x12c28, "Optio S7"),
        (0x12c2d, "Optio L20"),
        (0x12c32, "Optio M20"),
        (0x12c3c, "Optio W20"),
        (0x12c46, "Optio A20"),
        (0x12c78, "Optio E30"),
        (0x12c7d, "Optio E35"),
        (0x12c82, "Optio T30"),
        (0x12c8c, "Optio M30"),
        (0x12c91, "Optio L30"),
        (0x12c96, "Optio W30"),
        (0x12ca0, "Optio A30"),
        (0x12cb4, "Optio E40"),
        (0x12cbe, "Optio M40"),
        (0x12cc3, "Optio L40"),
        (0x12cc5, "Optio L36"),
        (0x12cc8, "Optio Z10"),
        (0x12cd2, "K20D"),
        (0x12cd4, "Samsung GX20"),
        (0x12cdc, "Optio S10"),
        (0x12ce6, "Optio A40"),
        (0x12cf0, "Optio V10"),
        (0x12cfa, "K200D"),
        (0x12d04, "Optio S12"),
        (0x12d0e, "Optio E50"),
        (0x12d18, "Optio M50"),
        (0x12d22, "Optio L50"),
        (0x12d2c, "Optio V20"),
        (0x12d40, "Optio W60"),
        (0x12d4a, "Optio M60"),
        (0x12d68, "Optio E60/M90"),
        (0x12d72, "K2000"),
        (0x12d73, "K-m"),
        (0x12d86, "Optio P70"),
        (0x12d90, "Optio L70"),
        (0x12d9a, "Optio E70"),
        (0x12dae, "X70"),
        (0x12db8, "K-7"),
        (0x12dcc, "Optio W80"),
        (0x12dea, "Optio P80"),
        (0x12df4, "Optio WS80"),
        (0x12dfe, "K-x"),
        (0x12e08, "645D"),
        (0x12e12, "Optio E80"),
        (0x12e30, "Optio W90"),
        (0x12e3a, "Optio I-10"),
        (0x12e44, "Optio H90"),
        (0x12e4e, "Optio E90"),
        (0x12e58, "X90"),
        (0x12e6c, "K-r"),
        (0x12e76, "K-5"),
        (0x12e8a, "Optio RS1000/RS1500"),
        (0x12e94, "Optio RZ10"),
        (0x12e9e, "Optio LS1000"),
        (0x12ebc, "Optio WG-1 GPS"),
        (0x12ed0, "Optio S1"),
        (0x12ee4, "Q"),
        (0x12ef8, "K-01"),
        (0x12f0c, "Optio RZ18"),
        (0x12f16, "Optio VS20"),
        (0x12f2a, "Optio WG-2 GPS"),
        (0x12f48, "Optio LS465"),
        (0x12f52, "K-30"),
        (0x12f5c, "X-5"),
        (0x12f66, "Q10"),
        (0x12f70, "K-5 II"),
        (0x12f71, "K-5 II s"),
        (0x12f7a, "Q7"),
        (0x12f84, "MX-1"),
        (0x12f8e, "WG-3 GPS"),
        (0x12f98, "WG-3"),
        (0x12fa2, "WG-10"),
        (0x12fb6, "K-50"),
        (0x12fc0, "K-3"),
        (0x12fca, "K-500"),
        (0x12fe8, "WG-4"),
        (0x12fde, "WG-4 GPS"),
        (0x13006, "WG-20"),
        (0x13010, "645Z"),
        (0x1301a, "K-S1"),
        (0x13024, "K-S2"),
        (0x1302e, "Q-S1"),
        (0x13056, "WG-30"),
        (0x1307e, "WG-30W"),
        (0x13088, "WG-5 GPS"),
        (0x13092, "K-1"),
        (0x1309c, "K-3 II"),
        (0x131f0, "WG-M2"),
        (0x1320e, "GR III"),
        (0x13222, "K-70"),
        (0x1322c, "KP"),
        (0x13240, "K-1 Mark II"),
        (0x13254, "K-3 Mark III"),
        (0x13290, "WG-70"),
        (0x1329a, "GR IIIx"),
        (0x132b8, "KF"),
        (0x132d6, "K-3 Mark III Monochrome"),
        (0x132e0, "GR IV"),
    ];
    MODELS
        .iter()
        .find(|&&(mid, _)| mid == id)
        .map(|&(_, name)| name.to_string())
}

// -- MN10: Lens identification database ----------------------------------

/// Look up a Canon model name by model ID.
fn canon_model_name(id: u32) -> Option<&'static str> {
    CANON_MODELS
        .iter()
        .find(|&&(mid, _)| mid == id)
        .map(|&(_, name)| name)
}

/// Look up a lens name by Canon lens ID.
fn canon_picture_style(v: u16) -> String {
    match v {
        0x00 => "None".into(),
        0x01 => "Standard".into(),
        0x02 => "Portrait".into(),
        0x03 => "High Saturation".into(),
        0x04 => "Adobe RGB".into(),
        0x05 => "Low Saturation".into(),
        0x06 => "CM Set 1".into(),
        0x07 => "CM Set 2".into(),
        0x21 => "User Def. 1".into(),
        0x22 => "User Def. 2".into(),
        0x23 => "User Def. 3".into(),
        0x41 => "PC 1".into(),
        0x42 => "PC 2".into(),
        0x43 => "PC 3".into(),
        0x81 => "Standard".into(),
        0x82 => "Portrait".into(),
        0x83 => "Landscape".into(),
        0x84 => "Neutral".into(),
        0x85 => "Faithful".into(),
        0x86 => "Monochrome".into(),
        0x87 => "Auto".into(),
        0x88 => "Fine Detail".into(),
        0xff | 0xffff => "n/a".into(),
        _ => format!("{v}"),
    }
}

pub fn canon_lens_name(lens_type: u16) -> Option<&'static str> {
    CANON_LENSES
        .iter()
        .find(|&&(id, _)| id == lens_type)
        .map(|&(_, name)| name)
}

/// Look up a lens name by Nikon lens ID string.
pub fn nikon_lens_name(lens_id: &str) -> Option<&'static str> {
    NIKON_LENSES
        .iter()
        .find(|&&(id, _)| id == lens_id)
        .map(|&(_, name)| name)
}

// Canon lens type -> name (most common lenses)
static CANON_LENSES: &[(u16, &str)] = &[
    (1, "Canon EF 50mm f/1.8"),
    (2, "Canon EF 28mm f/2.8"),
    (3, "Canon EF 135mm f/2.8 Soft"),
    (4, "Canon EF 35-105mm f/3.5-4.5"),
    (6, "Canon EF 35-70mm f/3.5-4.5"),
    (9, "Canon EF 50mm f/2.5 Macro"),
    (10, "Canon EF 100mm f/2.8 Macro"),
    (22, "Canon EF 100-300mm f/5.6L"),
    (26, "Canon EF 35-80mm f/4-5.6"),
    (29, "Canon EF 50mm f/1.8 II"),
    (32, "Canon EF 24mm f/2.8 IS USM"),
    (37, "Canon EF 35-80mm f/4-5.6 III"),
    (39, "Canon EF 75-300mm f/4-5.6"),
    (40, "Canon EF 28-80mm f/3.5-5.6"),
    (43, "Canon EF 28-105mm f/4-5.6"),
    (49, "Canon EF 50mm f/1.8 STM"),
    (124, "Canon MP-E 65mm f/2.8 1-5x Macro"),
    (125, "Canon TS-E 24mm f/3.5L"),
    (131, "Canon EF 28-300mm f/3.5-5.6L IS USM"),
    (136, "Canon EF 28-80mm f/3.5-5.6 USM IV"),
    (150, "Canon EF 14mm f/2.8L II USM"),
    (152, "Canon EF 24-105mm f/4L IS USM"),
    (153, "Canon EF 85mm f/1.2L II USM"),
    (154, "Canon EF 70-200mm f/4L IS USM"),
    (155, "Canon EF 85mm f/1.8 USM"),
    (156, "Canon EF 24-70mm f/2.8L USM"),
    (160, "Canon EF 70-200mm f/2.8L IS II USM"),
    (161, "Canon EF 100mm f/2.8L Macro IS USM"),
    (162, "Canon EF 16-35mm f/2.8L II USM"),
    (173, "Canon EF 35mm f/1.4L II USM"),
    (234, "Canon EF-S 17-85mm f/4-5.6 IS USM"),
    (235, "Canon EF-S 10-22mm f/3.5-4.5 USM"),
    (236, "Canon EF-S 60mm f/2.8 Macro USM"),
    (237, "Canon EF 24-105mm f/4L IS USM"),
    (238, "Canon EF 70-300mm f/4-5.6 IS USM"),
    (239, "Canon EF 85mm f/1.2L II USM"),
    (240, "Canon EF-S 17-55mm f/2.8 IS USM"),
    (241, "Canon EF 50mm f/1.2L USM"),
    (242, "Canon EF 70-200mm f/4L IS USM"),
    (243, "Canon EF 70-200mm f/4L IS USM + 1.4x"),
    (244, "Canon EF 70-200mm f/4L IS USM + 2x"),
    (245, "Canon EF 70-200mm f/4L IS USM + 2.8x"),
    (246, "Canon EF 16-35mm f/2.8L II USM"),
    (247, "Canon EF 14mm f/2.8L II USM"),
    (248, "Canon EF 200mm f/2L IS USM"),
    (249, "Canon EF 800mm f/5.6L IS USM"),
    (250, "Canon EF 24mm f/1.4L II USM"),
    (251, "Canon EF 70-200mm f/2.8L IS II USM"),
    (252, "Canon EF 70-200mm f/2.8L IS II USM + 1.4x"),
    (253, "Canon EF 70-200mm f/2.8L IS II USM + 2x"),
    (254, "Canon EF 100mm f/2.8L Macro IS USM"),
    (488, "Canon EF 24-105mm f/4L IS II USM"),
    (747, "Canon EF 16-35mm f/2.8L III USM"),
    (4142, "Canon RF 24-105mm F4 L IS USM"),
    (4143, "Canon RF 28-70mm F2 L USM"),
    (4144, "Canon RF 50mm F1.2 L USM"),
    (4146, "Canon EF-S 18-55mm f/3.5-5.6 IS STM"),
    (4148, "Canon RF 85mm F1.2 L USM"),
    (4150, "Canon RF 24-240mm F4-6.3 IS USM"),
    (4152, "Canon RF 70-200mm F2.8 L IS USM"),
    (4153, "Canon RF 15-35mm F2.8 L IS USM"),
    (4154, "Canon RF 24-70mm F2.8 L IS USM"),
    (4156, "Canon RF 85mm F2 Macro IS STM"),
    (4159, "Canon RF 100-500mm F4.5-7.1 L IS USM"),
    (4160, "Canon RF 600mm F11 IS STM"),
    (4161, "Canon RF 800mm F11 IS STM"),
    (4162, "Canon RF 50mm F1.8 STM"),
];

// Nikon lens ID string -> name (most common)
static NIKON_LENSES: &[(&str, &str)] = &[
    ("00 00", "Manual Lens / No Lens"),
    ("01 58", "Nikon AF 50mm f/1.8"),
    ("02 42", "Nikon AF 35mm f/2D"),
    ("06 36", "Nikon AF 75-300mm f/4.5-5.6"),
    ("07 46", "Nikon AF 80-200mm f/2.8D ED"),
    ("09 48", "Nikon AF 35-135mm f/3.5-4.5"),
    ("24 44", "Nikon AF 85mm f/1.8D"),
    ("25 48", "Nikon AF-S 300mm f/4D IF-ED"),
    ("2C 48", "Nikon AF-S 70-200mm f/2.8G ED VR"),
    ("38 4C", "Nikon AF-S 70-300mm f/4.5-5.6G IF-ED VR"),
    ("3C 5C", "Nikon AF-S 50mm f/1.4G"),
    ("3D 54", "Nikon AF-S 35mm f/1.8G DX"),
    ("48 48", "Nikon AF-S 120-300mm f/2.8E FL ED SR VR"),
    ("4C 40", "Nikon AF-S 18-200mm f/3.5-5.6G DX VR"),
    ("8B 40", "Nikon AF-S 16-80mm f/2.8-4E DX VR"),
    ("92 48", "Nikon AF-S 24-70mm f/2.8E ED VR"),
    ("93 48", "Nikon AF-S 70-200mm f/2.8E FL ED VR"),
    ("A3 4C", "Nikon AF-S 70-300mm f/4.5-5.6E ED VR"),
    ("A4 54", "Nikon AF-S 14-24mm f/2.8G ED"),
    ("A5 40", "Nikon AF-S 24-120mm f/4G ED VR"),
];

// Canon model ID -> model name (common models)
static CANON_MODELS: &[(u32, &str)] = &[
    (0x01010000, "PowerShot S30"),
    (0x01100000, "PowerShot G2"),
    (0x01110000, "PowerShot S40"),
    (0x01200000, "PowerShot S45"),
    (0x01210000, "PowerShot S50"),
    (0x01230000, "PowerShot S70"),
    (0x01240000, "PowerShot S60"),
    (
        0x01250000,
        "PowerShot S500 / Digital IXUS 500 / IXY Digital 500",
    ),
    (
        0x01270000,
        "PowerShot S400 / Digital IXUS 400 / IXY Digital 400",
    ),
    (0x01280000, "PowerShot A510"),
    (
        0x01540000,
        "PowerShot SD300 / Digital IXUS 40 / IXY Digital 50",
    ),
    (0x01600000, "PowerShot SX100 IS"),
    (0x01810000, "PowerShot SX40 HS"),
    (0x01880000, "PowerShot SX50 HS"),
    (0x03190000, "PowerShot ELPH 110 HS / IXUS 125 HS / IXY 220F"),
    (0x03750000, "PowerShot SX60 HS"),
    (0x04040000, "PowerShot Pro90 IS"),
    (0x06040000, "PowerShot S100 / Digital IXUS / IXY Digital"),
    (0x06050000, "PowerShot A5"),
    (0x06060000, "PowerShot A5 Zoom / IXY Digital 320"),
    (0x06080000, "PowerShot A50"),
    (
        0x06140000,
        "PowerShot S110 / Digital IXUS v / IXY Digital 200",
    ),
    (0x80000001, "EOS-1D"),
    (0x80000167, "EOS-1DS"),
    (0x80000168, "EOS 10D"),
    (0x80000169, "EOS-1D Mark III"),
    (0x80000170, "EOS Digital Rebel / 300D / Kiss Digital"),
    (0x80000174, "EOS-1D Mark II"),
    (0x80000175, "EOS 20D"),
    (0x80000176, "EOS Digital Rebel XSi / 450D / Kiss X2"),
    (0x80000188, "EOS-1Ds Mark II"),
    (0x80000189, "EOS Digital Rebel XT / 350D / Kiss Digital N"),
    (0x80000190, "EOS 40D"),
    (0x80000213, "EOS 5D"),
    (0x80000215, "EOS-1Ds Mark III"),
    (0x80000218, "EOS 5D Mark II"),
    (0x80000232, "EOS-1D Mark II N"),
    (0x80000234, "EOS 30D"),
    (0x80000236, "EOS Digital Rebel XTi / 400D / Kiss Digital X"),
    (0x80000250, "EOS 7D"),
    (0x80000252, "EOS Rebel T1i / 500D / Kiss X3"),
    (0x80000254, "EOS Rebel XS / 1000D / Kiss F"),
    (0x80000261, "EOS 50D"),
    (0x80000269, "EOS-1D X"),
    (0x80000270, "EOS Rebel T2i / 550D / Kiss X4"),
    (0x80000281, "EOS-1D Mark IV"),
    (0x80000285, "EOS 5D Mark III"),
    (0x80000286, "EOS Rebel T3i / 600D / Kiss X5"),
    (0x80000287, "EOS 60D"),
    (0x80000288, "EOS Rebel T3 / 1100D / Kiss X50"),
    (0x80000289, "EOS 7D Mark II"),
    (0x80000301, "EOS Rebel T4i / 650D / Kiss X6i"),
    (0x80000302, "EOS 6D"),
    (0x80000324, "EOS-1D C"),
    (0x80000325, "EOS 70D"),
    (0x80000326, "EOS Rebel T5i / 700D / Kiss X7i"),
    (0x80000327, "EOS Rebel T5 / 1200D / Kiss X70 / Hi"),
    (0x80000328, "EOS-1D X Mark II"),
    (0x80000331, "EOS M"),
    (0x80000346, "EOS Rebel SL1 / 100D / Kiss X7"),
    (0x80000347, "EOS Rebel T6s / 760D / 8000D"),
    (0x80000349, "EOS 5D Mark IV"),
    (0x80000350, "EOS 80D"),
    (0x80000355, "EOS M2"),
    (0x80000382, "EOS 5DS"),
    (0x80000393, "EOS Rebel T6i / 750D / Kiss X8i"),
    (0x80000401, "EOS 5DS R"),
    (0x80000404, "EOS Rebel T6 / 1300D / Kiss X80"),
    (0x80000405, "EOS Rebel T7i / 800D / Kiss X9i"),
    (0x80000406, "EOS 6D Mark II"),
    (0x80000408, "EOS 77D / 9000D"),
    (0x80000417, "EOS Rebel SL2 / 200D / Kiss X9"),
    (0x80000421, "EOS R5"),
    (0x80000422, "EOS Rebel T100 / 4000D / 3000D"),
    (0x80000424, "EOS R"),
    (0x80000428, "EOS-1D X Mark III"),
    (0x80000432, "EOS Rebel T7 / 2000D / 1500D / Kiss X90"),
    (0x80000433, "EOS RP"),
    (0x80000435, "EOS Rebel T8i / 850D / X10i"),
    (0x80000436, "EOS SL3 / 250D / Kiss X10"),
    (0x80000437, "EOS 90D"),
    (0x80000450, "EOS R3"),
    (0x80000453, "EOS R6"),
    (0x80000464, "EOS R7"),
    (0x80000465, "EOS R10"),
    (0x80000468, "EOS M50 Mark II / Kiss M2"),
    (0x80000480, "EOS R50"),
    (0x80000481, "EOS R6 Mark II"),
    (0x80000487, "EOS R8"),
    (0x80000491, "EOS R5 Mark II"),
    (0x80000495, "EOS R1"),
    (0x80000498, "EOS R100"),
];

// --- Google HDR+ MakerNotes (XMP protobuf) parser --------------------------

/// Decode Google HDR+ MakerNotes from base64-encoded XMP data.
/// Data flow: base64 -> HDRP\x03 -> XOR decrypt -> gzip decompress -> protobuf parse.
pub fn decode_google_hdrp(base64_data: &str) -> Vec<DecodedTag> {
    let Some(raw) = hdrp_base64_decode(base64_data) else {
        return Vec::new();
    };
    if raw.len() < 5 || &raw[..4] != b"HDRP" || raw[4] != 0x03 {
        return Vec::new();
    }
    let decrypted = hdrp_xor_decrypt(&raw[5..]);
    let Some(decompressed) = hdrp_gzip_decompress(&decrypted) else {
        return Vec::new();
    };
    hdrp_parse_protobuf(&decompressed)
}

fn hdrp_base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in input.as_bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn hdrp_xor_decrypt(data: &[u8]) -> Vec<u8> {
    let padded_len = (data.len() + 7) & !7;
    let mut buf = Vec::with_capacity(padded_len);
    buf.extend_from_slice(data);
    buf.resize(padded_len, 0);
    let mut key: u64 = 0x2515606b_4a7791cd;
    let mut i = 0;
    while i + 7 < buf.len() {
        key ^= key >> 12;
        key ^= key.wrapping_shl(25);
        key ^= key >> 27;
        key = key.wrapping_mul(0x2545f491_4f6cdd1d);
        let lo = key as u32;
        let hi = (key >> 32) as u32;
        let w0 = u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]) ^ lo;
        let w1 = u32::from_le_bytes([buf[i + 4], buf[i + 5], buf[i + 6], buf[i + 7]]) ^ hi;
        buf[i..i + 4].copy_from_slice(&w0.to_le_bytes());
        buf[i + 4..i + 8].copy_from_slice(&w1.to_le_bytes());
        i += 8;
    }
    buf.truncate(data.len());
    buf
}

fn hdrp_gzip_decompress(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 10 || data[0] != 0x1f || data[1] != 0x8b {
        return miniz_oxide::inflate::decompress_to_vec(data).ok();
    }
    let mut pos = 10usize;
    let flg = data[3];
    if flg & 0x04 != 0 {
        // FEXTRA
        if pos + 2 > data.len() {
            return None;
        }
        let xlen = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2 + xlen;
    }
    if flg & 0x08 != 0 {
        // FNAME
        while pos < data.len() && data[pos] != 0 {
            pos += 1;
        }
        pos += 1;
    }
    if flg & 0x10 != 0 {
        // FCOMMENT
        while pos < data.len() && data[pos] != 0 {
            pos += 1;
        }
        pos += 1;
    }
    if flg & 0x02 != 0 {
        pos += 2;
    } // FHCRC
    if pos >= data.len() {
        return None;
    }
    let deflate_end = if data.len() >= pos + 8 {
        data.len() - 8
    } else {
        data.len()
    };
    miniz_oxide::inflate::decompress_to_vec(&data[pos..deflate_end]).ok()
}

fn hdrp_read_varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        if *pos >= data.len() {
            return None;
        }
        let b = data[*pos];
        *pos += 1;
        result |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn hdrp_parse_fields(data: &[u8]) -> Vec<(u32, u8, &[u8])> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let Some(tag) = hdrp_read_varint(data, &mut pos) else {
            break;
        };
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;
        match wire_type {
            0 => {
                // varint
                let start = pos;
                if hdrp_read_varint(data, &mut pos).is_some() {
                    fields.push((field_num, 0, &data[start..pos]));
                }
            }
            1 => {
                // fixed64
                if pos + 8 <= data.len() {
                    fields.push((field_num, 1, &data[pos..pos + 8]));
                    pos += 8;
                } else {
                    break;
                }
            }
            2 => {
                // length-delimited
                let len = match hdrp_read_varint(data, &mut pos) {
                    Some(v) => v as usize,
                    None => break,
                };
                if pos + len <= data.len() {
                    fields.push((field_num, 2, &data[pos..pos + len]));
                    pos += len;
                } else {
                    break;
                }
            }
            5 => {
                // fixed32
                if pos + 4 <= data.len() {
                    fields.push((field_num, 5, &data[pos..pos + 4]));
                    pos += 4;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    fields
}

fn hdrp_varint_value(data: &[u8]) -> u64 {
    let mut pos = 0;
    hdrp_read_varint(data, &mut pos).unwrap_or(0)
}

fn hdrp_f32_value(data: &[u8]) -> f32 {
    if data.len() >= 4 {
        f32::from_le_bytes([data[0], data[1], data[2], data[3]])
    } else {
        0.0
    }
}

fn hdrp_parse_protobuf(data: &[u8]) -> Vec<DecodedTag> {
    let mut tags = Vec::new();
    let fields = hdrp_parse_fields(data);
    for &(fnum, wtype, fdata) in &fields {
        match fnum {
            1 if wtype == 2 => {
                // submessage: field 1 = ImageName
                for &(sf, sw, sd) in &hdrp_parse_fields(fdata) {
                    if sf == 1 && sw == 2 {
                        if let Ok(s) = std::str::from_utf8(sd) {
                            tags.push(DecodedTag {
                                name: "ImageName".into(),
                                value: s.into(),
                            });
                        }
                    }
                }
            }
            9 if wtype == 2 => {
                // submessage: field 3 = FrameCount
                for &(sf, sw, sd) in &hdrp_parse_fields(fdata) {
                    if sf == 3 && sw == 0 {
                        tags.push(DecodedTag {
                            name: "FrameCount".into(),
                            value: hdrp_varint_value(sd).to_string(),
                        });
                    }
                }
            }
            12 if wtype == 2 => {
                // device info submessage
                for &(sf, sw, sd) in &hdrp_parse_fields(fdata) {
                    match (sf, sw) {
                        (1, 2) => {
                            if let Ok(s) = std::str::from_utf8(sd) {
                                tags.push(DecodedTag {
                                    name: "DeviceMake".into(),
                                    value: s.into(),
                                });
                            }
                        }
                        (2, 2) => {
                            if let Ok(s) = std::str::from_utf8(sd) {
                                tags.push(DecodedTag {
                                    name: "DeviceModel".into(),
                                    value: s.into(),
                                });
                            }
                        }
                        (3, 2) => {
                            if let Ok(s) = std::str::from_utf8(sd) {
                                tags.push(DecodedTag {
                                    name: "DeviceCodename".into(),
                                    value: s.into(),
                                });
                            }
                        }
                        (4, 2) => {
                            if let Ok(s) = std::str::from_utf8(sd) {
                                tags.push(DecodedTag {
                                    name: "DeviceHardwareRevision".into(),
                                    value: s.into(),
                                });
                            }
                        }
                        (6, 2) => {
                            if let Ok(s) = std::str::from_utf8(sd) {
                                tags.push(DecodedTag {
                                    name: "HDRPSoftware".into(),
                                    value: s.into(),
                                });
                            }
                        }
                        (7, 2) => {
                            if let Ok(s) = std::str::from_utf8(sd) {
                                tags.push(DecodedTag {
                                    name: "AndroidRelease".into(),
                                    value: s.into(),
                                });
                            }
                        }
                        (8, 0) => {
                            let ms = hdrp_varint_value(sd);
                            tags.push(DecodedTag {
                                name: "SoftwareDate".into(),
                                value: hdrp_format_millis(ms),
                            });
                        }
                        (9, 2) => {
                            if let Ok(s) = std::str::from_utf8(sd) {
                                tags.push(DecodedTag {
                                    name: "Application".into(),
                                    value: s.into(),
                                });
                            }
                        }
                        (10, 2) => {
                            if let Ok(s) = std::str::from_utf8(sd) {
                                tags.push(DecodedTag {
                                    name: "AppVersion".into(),
                                    value: s.into(),
                                });
                            }
                        }
                        (12, 2) => {
                            // ExposureTime range
                            for &(ssf, ssw, ssd) in &hdrp_parse_fields(sd) {
                                if ssw == 5 {
                                    let v = hdrp_f32_value(ssd) as f64 / 1000.0;
                                    let name = if ssf == 1 {
                                        "ExposureTimeMin"
                                    } else if ssf == 2 {
                                        "ExposureTimeMax"
                                    } else {
                                        continue;
                                    };
                                    tags.push(DecodedTag {
                                        name: name.into(),
                                        value: hdrp_format_f64(v),
                                    });
                                }
                            }
                        }
                        (13, 2) => {
                            // ISO range
                            for &(ssf, ssw, ssd) in &hdrp_parse_fields(sd) {
                                if ssw == 5 {
                                    let v = hdrp_f32_value(ssd) as f64;
                                    let name = if ssf == 1 {
                                        "ISOMin"
                                    } else if ssf == 2 {
                                        "ISOMax"
                                    } else {
                                        continue;
                                    };
                                    tags.push(DecodedTag {
                                        name: name.into(),
                                        value: hdrp_format_f64(v),
                                    });
                                }
                            }
                        }
                        (14, 5) => {
                            let v = hdrp_f32_value(sd) as f64;
                            tags.push(DecodedTag {
                                name: "MaxAnalogISO".into(),
                                value: hdrp_format_f64(v),
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    tags
}

fn hdrp_format_f64(v: f64) -> String {
    // Small magnitudes print in scientific notation, matching how these
    // values are conventionally displayed.
    if v != 0.0 && v.abs() < 0.001 {
        format!("{v:e}")
    } else {
        format!("{v}")
    }
}

fn hdrp_format_millis(ms: u64) -> String {
    let total_secs = (ms / 1000) as i64;
    let millis = (ms % 1000) as u32;
    let days = total_secs / 86400;
    let day_secs = (total_secs % 86400) as u32;
    let h = day_secs / 3600;
    let m = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    let (year, month, day) = unix_days_to_date(days);
    format!("{year:04}:{month:02}:{day:02} {h:02}:{m:02}:{s:02}.{millis:03}+00:00")
}

// --- Qualcomm Camera Attributes (APP7) parser ------------------------------

/// Decode Qualcomm Camera Attributes from a JPEG APP7 segment.
/// The data should start after the `\x1aQualcomm Camera Attributes` header.
pub fn decode_qualcomm(data: &[u8]) -> Vec<DecodedTag> {
    let mut tags = Vec::new();
    let mut pos = 0;

    while pos + 3 < data.len() {
        let val_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        let tag_len = data[pos + 2] as usize;
        if pos + 8 + tag_len + val_len > data.len() {
            break;
        }
        let tag_name_raw = std::str::from_utf8(&data[pos + 3..pos + 3 + tag_len])
            .unwrap_or("")
            .trim_end_matches('\0');
        pos += 3 + tag_len; // point to format byte
        let fmt = data[pos];
        // Skip cnt1 (u16) and cnt2 (u16)
        pos += 5; // format + cnt1 + cnt2

        // Read value based on format
        let value = if val_len > 0 && pos + val_len <= data.len() {
            match fmt {
                0 => {
                    // int8u
                    format!("{}", data[pos])
                }
                1 => {
                    // int8s
                    format!("{}", data[pos] as i8)
                }
                2 => {
                    // int16u
                    if val_len >= 2 {
                        format!("{}", u16::from_le_bytes([data[pos], data[pos + 1]]))
                    } else {
                        String::new()
                    }
                }
                3 => {
                    // int16s
                    if val_len >= 2 {
                        format!("{}", i16::from_le_bytes([data[pos], data[pos + 1]]))
                    } else {
                        String::new()
                    }
                }
                4 => {
                    // int32u
                    if val_len >= 4 {
                        format!(
                            "{}",
                            u32::from_le_bytes([
                                data[pos],
                                data[pos + 1],
                                data[pos + 2],
                                data[pos + 3]
                            ])
                        )
                    } else {
                        String::new()
                    }
                }
                5 => {
                    // int32s
                    if val_len >= 4 {
                        format!(
                            "{}",
                            i32::from_le_bytes([
                                data[pos],
                                data[pos + 1],
                                data[pos + 2],
                                data[pos + 3]
                            ])
                        )
                    } else {
                        String::new()
                    }
                }
                6 => {
                    // float
                    if val_len >= 4 {
                        let v = f32::from_le_bytes([
                            data[pos],
                            data[pos + 1],
                            data[pos + 2],
                            data[pos + 3],
                        ]);
                        ciff_format_float(v)
                    } else {
                        String::new()
                    }
                }
                7 => {
                    // double
                    if val_len >= 8 {
                        let v = f64::from_le_bytes([
                            data[pos],
                            data[pos + 1],
                            data[pos + 2],
                            data[pos + 3],
                            data[pos + 4],
                            data[pos + 5],
                            data[pos + 6],
                            data[pos + 7],
                        ]);
                        format!("{v}")
                    } else {
                        String::new()
                    }
                }
                _ => String::new(),
            }
        } else {
            String::new()
        };

        pos += val_len;

        if !tag_name_raw.is_empty() && !value.is_empty() {
            // Convert snake_case to CamelCase
            let name = qualcomm_to_camel(tag_name_raw);
            tags.push(DecodedTag { name, value });
        }
    }
    tags
}

/// Convert a Qualcomm snake_case tag name to CamelCase.
/// "aec_current_sensor_luma" -> "AECCurrentSensorLuma"
fn qualcomm_to_camel(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if c == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            // Check for known all-caps abbreviations
            result.push(c.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }
    // Fix known abbreviation patterns
    result = result.replace("Aec", "AEC");
    result = result.replace("Asf", "ASF");
    result = result.replace("Awb", "AWB");
    result = result.replace("Af", "AF");
    result
}

// --- Canon CIFF (Camera Image File Format) parser --------------------------
//
// CIFF is used in Canon CRW raw files and in JPEG APP0 segments from older
// Canon PowerShot cameras. The format is a hierarchical heap with a directory
// at the end of each block.

/// Decode Canon CIFF data from a JPEG APP0 segment (HEAPJPGM).
/// Returns decoded tags prefixed for the MakerNotes group.
pub fn decode_ciff(data: &[u8]) -> Vec<DecodedTag> {
    let mut tags = Vec::new();
    if data.len() < 26 {
        return tags;
    }
    let le = data[0] == b'I' && data[1] == b'I';
    if !(le || data[0] == b'M' && data[1] == b'M') {
        return tags;
    }
    // Header length
    let header_len = if le {
        u32::from_le_bytes([data[2], data[3], data[4], data[5]])
    } else {
        u32::from_be_bytes([data[2], data[3], data[4], data[5]])
    } as usize;
    if header_len < 14 || header_len > data.len() {
        return tags;
    }
    // Verify HEAPJPGM signature
    if &data[6..14] != b"HEAPJPGM" {
        return tags;
    }
    // Parse the root directory starting from header_len to end of data
    decode_ciff_dir(data, header_len, data.len() - header_len, le, &mut tags, 0);
    tags
}

/// Recursively decode a CIFF directory block.
/// `block_start` is the offset of the block in `data`, `block_size` is its length.
fn decode_ciff_dir(
    data: &[u8],
    block_start: usize,
    block_size: usize,
    le: bool,
    tags: &mut Vec<DecodedTag>,
    depth: u32,
) {
    if depth > 10 || block_size < 6 {
        return;
    }
    let block_end = block_start + block_size;
    if block_end > data.len() || block_end < 4 {
        return;
    }
    // Last 4 bytes of block = directory offset (relative to block_start)
    let dir_rel = ciff_u32(data, block_end - 4, le) as usize;
    let dir_abs = block_start + dir_rel;
    if dir_abs + 2 > block_end - 4 {
        return;
    }
    let entry_count = ciff_u16(data, dir_abs, le) as usize;
    if dir_abs + 2 + entry_count * 10 > block_end - 4 {
        return;
    }
    for i in 0..entry_count {
        let pt = dir_abs + 2 + i * 10;
        let tag_raw = ciff_u16(data, pt, le);
        let size = ciff_u32(data, pt + 2, le) as usize;
        let value_ptr = ciff_u32(data, pt + 6, le) as usize;

        if tag_raw & 0x8000 != 0 {
            continue; // bad entry
        }
        let tag_id = tag_raw & 0x3FFF;
        let tag_type = (tag_raw >> 8) & 0x38;
        let value_in_dir = tag_raw & 0x4000 != 0;

        // Subdirectory types (0x28 = subdirectory, 0x30 = subdirectory)
        if (tag_type == 0x28 || tag_type == 0x30) && !value_in_dir {
            let sub_start = value_ptr + block_start;
            if sub_start < block_end && size > 0 {
                let sub_size = size.min(block_end - sub_start);
                decode_ciff_dir(data, sub_start, sub_size, le, tags, depth + 1);
            }
            continue;
        }

        // Get value data
        let (val_data, val_offset) = if value_in_dir {
            // Value stored in the size+ptr fields (8 bytes at pt+2)
            if pt + 10 <= data.len() {
                (&data[pt + 2..pt + 10], pt + 2)
            } else {
                continue;
            }
        } else {
            let abs_ptr = value_ptr + block_start;
            if abs_ptr + size <= data.len() {
                (&data[abs_ptr..abs_ptr + size], abs_ptr)
            } else {
                continue;
            }
        };

        // Decode the tag
        decode_ciff_tag(tag_id, val_data, val_offset, le, tags);
    }
}

fn decode_ciff_tag(tag_id: u16, data: &[u8], _offset: usize, le: bool, tags: &mut Vec<DecodedTag>) {
    let push = |tags: &mut Vec<DecodedTag>, name: &str, val: String| {
        tags.push(DecodedTag {
            name: name.to_string(),
            value: val,
        });
    };

    match tag_id {
        // Strings
        0x0805 => push(tags, "CanonFileDescription", ciff_string(data)),
        0x080B => push(tags, "CanonFirmwareVersion", ciff_string(data)),
        0x080D => push(tags, "ROMOperationMode", ciff_string(data)),
        0x0810 => push(tags, "OwnerName", ciff_string(data)),
        0x0815 => push(tags, "CanonImageType", ciff_string(data)),
        0x0816 => push(tags, "OriginalFileName", ciff_string(data)),
        0x0817 => push(tags, "ThumbnailFileName", ciff_string(data)),

        // MakeModel (binary subdirectory: Make at 0, Model at 6)
        0x080A => {
            if data.len() >= 6 {
                let make = ciff_string(&data[..6]);
                push(tags, "Make", make);
                if data.len() > 6 {
                    let model = ciff_string(&data[6..]);
                    push(tags, "Model", model);
                }
            }
        }

        // TargetImageType
        0x100A => {
            let v = ciff_u16(data, 0, le);
            push(
                tags,
                "TargetImageType",
                match v {
                    0 => "Real-world Subject".into(),
                    1 => "Written Document".into(),
                    _ => format!("{v}"),
                },
            );
        }

        // ShutterReleaseMethod
        0x1010 => {
            let v = ciff_u16(data, 0, le);
            push(
                tags,
                "ShutterReleaseMethod",
                match v {
                    0 => "Single Shot".into(),
                    2 => "Continuous Shooting".into(),
                    4 => "Self-Timer".into(),
                    _ => format!("{v}"),
                },
            );
        }

        // ShutterReleaseTiming
        0x1011 => {
            let v = ciff_u16(data, 0, le);
            push(
                tags,
                "ShutterReleaseTiming",
                match v {
                    0 => "Priority on shutter".into(),
                    1 => "Priority on focus".into(),
                    _ => format!("{v}"),
                },
            );
        }

        // BaseISO
        0x101C => {
            let v = ciff_u16(data, 0, le);
            push(tags, "BaseISO", format!("{v}"));
        }

        // ImageFormat subdirectory (binary: 2 x int32u -> FileFormat + TargetCompressionRatio)
        0x1803 => {
            if data.len() >= 8 {
                let file_fmt = ciff_u32(data, 0, le);
                let name = match file_fmt {
                    0x00010000 => "JPEG (lossy)",
                    0x00010002 => "JPEG (non-quantization)",
                    0x00010003 => "JPEG (lossy/non-quantization toggled)",
                    0x00020001 => "CRW",
                    _ => "",
                };
                if !name.is_empty() {
                    push(tags, "FileFormat", name.into());
                } else {
                    push(tags, "FileFormat", format!("0x{file_fmt:08x}"));
                }
                let ratio = f32::from_bits(ciff_u32(data, 4, le));
                push(tags, "TargetCompressionRatio", format!("{}", ratio as i32));
            }
        }

        // RecordID
        0x1804 => {
            let v = ciff_u32(data, 0, le);
            push(tags, "RecordID", format!("{v}"));
        }

        // TargetDistanceSetting (float, mm)
        0x1807 => {
            if data.len() >= 4 {
                let v = f32::from_bits(ciff_u32(data, 0, le));
                push(tags, "TargetDistanceSetting", format!("{} mm", v as i32));
            }
        }

        // TimeStamp subdirectory (int32u[3]: unix_time, timezone_offset, timezone_info)
        0x180E => {
            if data.len() >= 12 {
                let ts = ciff_u32(data, 0, le);
                let tz_code = ciff_u32(data, 4, le) as i32;
                let tz_info = ciff_u32(data, 8, le);
                // Convert unix timestamp
                let secs = ts as i64;
                let days = secs / 86400;
                let time_of_day = secs % 86400;
                let (year, month, day) = unix_days_to_date(days);
                let hour = time_of_day / 3600;
                let minute = (time_of_day % 3600) / 60;
                let second = time_of_day % 60;
                push(
                    tags,
                    "DateTimeOriginal",
                    format!("{year:04}:{month:02}:{day:02} {hour:02}:{minute:02}:{second:02}"),
                );
                let tz_hours = tz_code / 3600;
                push(tags, "TimeZoneCode", format!("{tz_hours}"));
                push(tags, "TimeZoneInfo", format!("{tz_info}"));
            }
        }

        // ImageInfo subdirectory (int32u[7]: width, height, pixel_aspect, rotation, comp_depth, color_depth, colorBW)
        0x1810 => {
            if data.len() >= 28 {
                let pixel_aspect = f32::from_bits(ciff_u32(data, 8, le));
                let rotation = ciff_u32(data, 12, le) as i32;
                let comp_depth = ciff_u32(data, 16, le);
                let color_depth = ciff_u32(data, 20, le);
                let color_bw = ciff_u32(data, 24, le);
                push(tags, "PixelAspectRatio", format!("{}", pixel_aspect as i32));
                push(tags, "Rotation", format!("{rotation}"));
                push(tags, "ComponentBitDepth", format!("{comp_depth}"));
                push(tags, "ColorBitDepth", format!("{color_depth}"));
                push(tags, "ColorBW", format!("{color_bw}"));
            }
        }

        // FlashInfo subdirectory (float[2]: guide_number, threshold)
        0x1813 => {
            if data.len() >= 8 {
                let guide = f32::from_bits(ciff_u32(data, 0, le));
                let thresh = f32::from_bits(ciff_u32(data, 4, le));
                push(tags, "FlashGuideNumber", ciff_format_float(guide));
                push(tags, "FlashThreshold", ciff_format_float(thresh));
            }
        }

        // MeasuredEV (float + 5.0 offset)
        0x1814 => {
            if data.len() >= 4 {
                let v = f32::from_bits(ciff_u32(data, 0, le));
                let ev = v + 5.0;
                push(tags, "MeasuredEV", ciff_format_float(ev));
            }
        }

        // FileNumber
        0x1817 => {
            if data.len() >= 4 {
                let v = ciff_u32(data, 0, le);
                push(tags, "FileNumber", format!("{v}"));
            }
        }

        // ExposureInfo subdirectory (float[3]: compensation, shutter, aperture)
        0x1818 => {
            if data.len() >= 12 {
                let comp = f32::from_bits(ciff_u32(data, 0, le));
                let shutter_apex = f32::from_bits(ciff_u32(data, 4, le));
                let aperture_apex = f32::from_bits(ciff_u32(data, 8, le));

                push(tags, "ExposureCompensation", format!("{}", comp as i32));

                // ShutterSpeedValue: APEX to exposure time, then format like ExifTool
                if shutter_apex.abs() < 100.0 {
                    let exposure_time = 1.0 / 2.0_f64.powf(shutter_apex as f64);
                    if let Some(s) = crate::tiff::tags::format_exposure_time(exposure_time) {
                        push(tags, "ShutterSpeedValue", s);
                    }
                }

                // ApertureValue: APEX to f-number
                let f_number = 2.0_f64.powf((aperture_apex as f64) / 2.0);
                push(tags, "ApertureValue", format!("{f_number:.1}"));
            }
        }

        // FocalLength info (tag 0x1029 = CanonFocalLength subdirectory, int16u array)
        0x1029 => {
            if data.len() >= 2 {
                let focal_type = ciff_u16(data, 0, le);
                if focal_type != 0 {
                    push(
                        tags,
                        "FocalType",
                        match focal_type {
                            1 => "Fixed".into(),
                            2 => "Zoom".into(),
                            _ => format!("{focal_type}"),
                        },
                    );
                }
            }
            // Index 2 = FocalPlaneXSize, index 3 = FocalPlaneYSize (in 1/1000 inch)
            if data.len() >= 8 {
                let x_raw = ciff_u16(data, 4, le);
                let y_raw = ciff_u16(data, 6, le);
                if x_raw >= 40 {
                    let x_mm = x_raw as f64 * 25.4 / 1000.0;
                    push(tags, "FocalPlaneXSize", format!("{x_mm:.2} mm"));
                }
                if y_raw >= 40 {
                    let y_mm = y_raw as f64 * 25.4 / 1000.0;
                    push(tags, "FocalPlaneYSize", format!("{y_mm:.2} mm"));
                }
            }
        }

        // FreeBytes
        0x0001 => {
            // Binary data - ExifTool shows "(Binary data N bytes)"
            // We skip binary blobs, they're filtered out in the test
        }

        _ => {} // Skip unknown tags
    }
}

/// Read a float from the CIFF data and check for FocalPlane sizes.
/// CIFF stores some float values at specific offsets that need special handling.
fn ciff_u16(data: &[u8], offset: usize, le: bool) -> u16 {
    if offset + 2 > data.len() {
        return 0;
    }
    if le {
        u16::from_le_bytes([data[offset], data[offset + 1]])
    } else {
        u16::from_be_bytes([data[offset], data[offset + 1]])
    }
}

fn ciff_u32(data: &[u8], offset: usize, le: bool) -> u32 {
    if offset + 4 > data.len() {
        return 0;
    }
    if le {
        u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    } else {
        u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    }
}

/// Format a CIFF float value: use enough decimal places, strip trailing zeros.
fn ciff_format_float(v: f32) -> String {
    // Use 6 decimal places then strip trailing zeros
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn ciff_string(data: &[u8]) -> String {
    let s = std::str::from_utf8(data).unwrap_or("");
    s.trim_end_matches('\0').trim().to_string()
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
fn unix_days_to_date(days: i64) -> (i64, i64, i64) {
    // Algorithm from Howard Hinnant's chrono-compatible date algorithms
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiff::exif::{MakerNoteFormat, MakerNoteRef};

    fn build_ifd_data(entries: &[(u16, u16, u32, Vec<u8>)], big_endian: bool) -> Vec<u8> {
        let mut data = Vec::new();
        let count = entries.len() as u16;
        if big_endian {
            data.extend_from_slice(&count.to_be_bytes());
        } else {
            data.extend_from_slice(&count.to_le_bytes());
        }

        let entries_end = 2 + entries.len() * 12 + 4;
        let mut ext_offset = entries_end;
        let mut ext_data = Vec::new();

        for &(tag, dtype, count, ref value) in entries {
            if big_endian {
                data.extend_from_slice(&tag.to_be_bytes());
                data.extend_from_slice(&dtype.to_be_bytes());
                data.extend_from_slice(&count.to_be_bytes());
            } else {
                data.extend_from_slice(&tag.to_le_bytes());
                data.extend_from_slice(&dtype.to_le_bytes());
                data.extend_from_slice(&count.to_le_bytes());
            }
            let type_size = crate::tiff::DataType::from_u16(dtype).map_or(1, |t| t.size());
            let total = count as usize * type_size;
            if total <= 4 {
                let mut padded = [0u8; 4];
                let copy_len = value.len().min(4);
                padded[..copy_len].copy_from_slice(&value[..copy_len]);
                data.extend_from_slice(&padded);
            } else {
                if big_endian {
                    data.extend_from_slice(&(ext_offset as u32).to_be_bytes());
                } else {
                    data.extend_from_slice(&(ext_offset as u32).to_le_bytes());
                }
                ext_data.extend_from_slice(value);
                ext_offset += value.len();
            }
        }
        // Next IFD offset = 0
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&ext_data);
        data
    }

    #[test]
    fn mn1_detect_vendor() {
        assert_eq!(detect_vendor(b"Nikon\0\x02\x10\0\0"), Vendor::Nikon);
        assert_eq!(detect_vendor(b"FUJIFILM\x0C\0\0\0"), Vendor::Fujifilm);
        assert_eq!(detect_vendor(b"OLYMP\0\x01\0"), Vendor::Olympus);
        assert_eq!(detect_vendor(b"OLYMPUS\0\x01\0"), Vendor::Olympus);
        assert_eq!(detect_vendor(b"Panasonic\0\0\0"), Vendor::Panasonic);
        assert_eq!(detect_vendor(b"Apple iOS\0\x01\x01"), Vendor::Apple);
        assert_eq!(detect_vendor(b"STMN\0\0\0\0"), Vendor::Samsung);
        assert_eq!(detect_vendor(b"SONY DSC \0\0\0"), Vendor::Sony);
        assert_eq!(detect_vendor(b"\x05\0"), Vendor::Unknown);
    }

    #[test]
    fn mn1_detect_vendor_ignores_make_predicates() {
        // Rules that need `Make` cannot match without it, so a headerless
        // Canon payload stays unknown until the Make tag is supplied.
        let payload = b"\x0c\0\x01\0\x03\0\x01\0\0\0";
        assert_eq!(detect_vendor(payload), Vendor::Unknown);
        assert_eq!(resolve_vendor(payload, Some("Canon"), None), Vendor::Canon);
        assert_eq!(
            resolve_vendor(payload, None, Some("EOS R5")),
            Vendor::Unknown
        );
    }

    #[test]
    fn mn1_resolve_vendor_sony_signatures() {
        // exiftool accepts every one of these with a 12-byte header.
        for payload in [
            &b"SONY DSC \0\0\0"[..],
            &b"SONY CAM \0\0\0"[..],
            &b"SONY MOBILE\0"[..],
            &b"\0\0SONY PIC\0"[..],
            &b"VHAB     \0"[..],
        ] {
            assert_eq!(resolve_vendor(payload, None, None), Vendor::Sony);
        }
        // Headerless SR2/ARW payloads resolve through the Make tag.
        let arw = b"\x01\x00\x00\x00IFD\0\0";
        assert_eq!(resolve_vendor(arw, Some("SONY"), None), Vendor::Sony);
    }

    #[test]
    fn mn1_resolve_vendor_hasselblad_rebadge_is_sony() {
        // Sony-built Hasselblad bodies carry Sony maker notes.
        let payload = b"\x0a\0\x01\0\x02\0\x01\0\0\0";
        assert_eq!(
            resolve_vendor(payload, Some("HASSELBLAD"), Some("Lusso")),
            Vendor::Sony
        );
        // A Hasselblad body outside the Sony-built range is not claimed.
        assert_eq!(
            resolve_vendor(payload, Some("HASSELBLAD"), Some("X2D 100C")),
            Vendor::Unknown
        );
    }

    #[test]
    fn mn1_resolve_vendor_payload_beats_make() {
        // exiftool keeps `AOC\0` payloads for Pentax even when Make says Kodak.
        let aoc = b"AOC\0II\x2a\0\x08\0\0\0";
        assert_eq!(
            resolve_vendor(aoc, Some("EASTMAN KODAK"), None),
            Vendor::Pentax
        );
        // ... except for the Optio 330RS/430RS, whose body is Casio-style.
        assert_eq!(
            resolve_vendor(aoc, None, Some("PENTAX Optio 430RS")),
            Vendor::Unknown
        );
    }

    #[test]
    fn mn1_resolve_vendor_mixed_predicates() {
        // JVC/Victor text maker notes need both the Make and the payload.
        let ver = b"VER:1.00\0\0";
        assert_eq!(resolve_vendor(ver, None, None), Vendor::Unknown);
        assert_eq!(resolve_vendor(ver, Some("JVC"), None), Vendor::Jvc);
        assert_eq!(
            resolve_vendor(ver, Some("Victor Company"), None),
            Vendor::Jvc
        );
        // Leica shares the Panasonic tag table.
        let payload = b"\x0c\0\x01\0\x03\0\x01\0\0\0";
        assert_eq!(
            resolve_vendor(payload, Some("LEICA"), Some("Q3")),
            Vendor::Panasonic
        );
    }

    #[test]
    fn mn1_vendor_from_make() {
        assert_eq!(vendor_from_make("Canon"), Vendor::Canon);
        assert_eq!(vendor_from_make("NIKON CORPORATION"), Vendor::Nikon);
        assert_eq!(vendor_from_make("SONY"), Vendor::Sony);
        assert_eq!(vendor_from_make("FUJIFILM"), Vendor::Fujifilm);
        assert_eq!(vendor_from_make("Panasonic"), Vendor::Panasonic);
        assert_eq!(vendor_from_make("OLYMPUS CORPORATION"), Vendor::Olympus);
        assert_eq!(vendor_from_make("OM Digital Solutions"), Vendor::Olympus);
        assert_eq!(vendor_from_make("SAMSUNG"), Vendor::Samsung);
        assert_eq!(vendor_from_make("Apple"), Vendor::Apple);
        assert_eq!(vendor_from_make("Unknown Brand"), Vendor::Unknown);
    }

    #[test]
    fn mn2_canon_tag_names() {
        assert_eq!(maker_tag_name(0x0006, Vendor::Canon), "CanonImageType");
        assert_eq!(maker_tag_name(0x0095, Vendor::Canon), "LensModel");
        assert_eq!(maker_tag_name(0x0009, Vendor::Canon), "OwnerName");
        assert_eq!(maker_tag_name(0xFFFF, Vendor::Canon), "Unknown");
    }

    #[test]
    fn mn3_nikon_tag_names() {
        assert_eq!(maker_tag_name(0x0007, Vendor::Nikon), "FocusMode");
        assert_eq!(maker_tag_name(0x001D, Vendor::Nikon), "SerialNumber");
    }

    #[test]
    fn mn3_nikon_tiff_parsing() {
        // Build a Nikon Type 3 maker note: "Nikon\0\x02\x10\0\0" + TIFF
        let mut mn_data = b"Nikon\0\x02\x10\0\0".to_vec();
        // Embedded TIFF (LE)
        mn_data.extend_from_slice(b"II");
        mn_data.extend_from_slice(&42u16.to_le_bytes());
        mn_data.extend_from_slice(&8u32.to_le_bytes());

        // IFD with 1 entry at offset 8 (relative to TIFF start at byte 10 of MN)
        let ifd = build_ifd_data(
            &[
                (0x0001, 7, 4, b"0211".to_vec()), // MakerNoteVersion
            ],
            false,
        );
        mn_data.extend_from_slice(&ifd);

        let mnr = MakerNoteRef {
            data: &mn_data,
            offset: 0,
            format: MakerNoteFormat::NikonTiff { tiff_offset: 10 },
        };

        let result = parse_maker_note(&mnr, &mn_data, false).unwrap();
        assert_eq!(result.vendor, Vendor::Nikon);
        assert!(result.ifd.is_some());
        let ifd = result.ifd.unwrap();
        assert_eq!(ifd.entries[0].tag, 0x0001);
    }

    #[test]
    fn mn4_sony_tag_names() {
        assert_eq!(maker_tag_name(0x0102, Vendor::Sony), "Quality");
        assert_eq!(maker_tag_name(0x0114, Vendor::Sony), "CameraSettings");
    }

    #[test]
    fn mn4_sony_burst_tag_names() {
        assert_eq!(maker_tag_name(0xB049, Vendor::Sony), "ReleaseMode");
        assert_eq!(maker_tag_name(0xB04A, Vendor::Sony), "SequenceNumber");
    }

    #[test]
    fn mn4_sony_decodes_burst_sequence() {
        // Sony maker notes start with a 12-byte header and keep absolute
        // value offsets, so the IFD is parsed against the parent TIFF data.
        let mut mn_data = b"SONY DSC \0\0\0".to_vec();
        mn_data.extend_from_slice(&build_ifd_data(
            &[
                (0xB049, 3, 1, 2u16.to_le_bytes().to_vec()), // ReleaseMode = Continuous
                (0xB04A, 3, 1, 4u16.to_le_bytes().to_vec()), // SequenceNumber = 4th frame
            ],
            false,
        ));

        let mnr = MakerNoteRef {
            data: &mn_data,
            offset: 0,
            format: MakerNoteFormat::HeaderIfd {
                header_size: 12,
                relative_offsets: false,
            },
        };
        let mn = parse_maker_note(&mnr, &mn_data, false).unwrap();
        assert_eq!(mn.vendor, Vendor::Sony);

        let tags = decode_maker_tags(&mn);
        let value = |name: &str| {
            tags.iter()
                .find(|tag| tag.name == name)
                .map(|tag| tag.value.as_str())
        };
        assert_eq!(value("ReleaseMode"), Some("2"));
        assert_eq!(value("SequenceNumber"), Some("4"));
    }

    #[test]
    fn mn5_fuji_tag_names() {
        assert_eq!(maker_tag_name(0x1000, Vendor::Fujifilm), "Quality");
        assert_eq!(maker_tag_name(0x1021, Vendor::Fujifilm), "FocusMode");
    }

    #[test]
    fn mn5_fuji_always_le() {
        // Fujifilm header with relative offsets
        let mut mn_data = b"FUJIFILM".to_vec();
        mn_data.extend_from_slice(&12u32.to_le_bytes()); // offset to IFD
        let ifd = build_ifd_data(
            &[
                (0x1000, 2, 6, b"FINE\0\0".to_vec()), // Quality
            ],
            false,
        );
        mn_data.extend_from_slice(&ifd);

        let mnr = MakerNoteRef {
            data: &mn_data,
            offset: 0,
            format: MakerNoteFormat::HeaderIfd {
                header_size: 12,
                relative_offsets: true,
            },
        };

        // Parse with parent BE - Fuji should still use LE
        let result = parse_maker_note(&mnr, &mn_data, true).unwrap();
        assert_eq!(result.vendor, Vendor::Fujifilm);
        assert!(!result.big_endian); // Always LE
        assert!(result.ifd.is_some());
    }

    #[test]
    fn mn6_panasonic_tag_names() {
        assert_eq!(maker_tag_name(0x0001, Vendor::Panasonic), "ImageQuality");
        assert_eq!(maker_tag_name(0x0007, Vendor::Panasonic), "FocusMode");
    }

    #[test]
    fn mn7_olympus_tag_names() {
        assert_eq!(maker_tag_name(0x0201, Vendor::Olympus), "Quality");
        assert_eq!(maker_tag_name(0x0404, Vendor::Olympus), "SerialNumber");
    }

    #[test]
    fn mn8_samsung_tag_names() {
        assert_eq!(maker_tag_name(0x0001, Vendor::Samsung), "MakerNoteVersion");
        assert_eq!(maker_tag_name(0x0A02, Vendor::Samsung), "SerialNumber");
    }

    #[test]
    fn mn9_apple_tag_names() {
        assert_eq!(maker_tag_name(0x0003, Vendor::Apple), "RunTime");
        assert_eq!(maker_tag_name(0x000A, Vendor::Apple), "HDRImageType");
        assert_eq!(maker_tag_name(0x002D, Vendor::Apple), "ColorTemperature");
    }

    #[test]
    fn mn10_canon_lens_lookup() {
        assert_eq!(canon_lens_name(29), Some("Canon EF 50mm f/1.8 II"));
        assert_eq!(canon_lens_name(49), Some("Canon EF 50mm f/1.8 STM"));
        assert_eq!(canon_lens_name(152), Some("Canon EF 24-105mm f/4L IS USM"));
        assert_eq!(canon_lens_name(4142), Some("Canon RF 24-105mm F4 L IS USM"));
        assert_eq!(canon_lens_name(9999), None);
    }

    #[test]
    fn panasonic_face_records_respect_endianness_count_and_truncation() {
        for be in [false, true] {
            let data: Vec<u8> = [2u16, 213, 76, 13, 13, 100, 50, 20, 10]
                .into_iter()
                .flat_map(|v| if be { v.to_be_bytes() } else { v.to_le_bytes() })
                .collect();
            let mut tags = Vec::new();
            decode_panasonic_faces(&data, be, &mut tags);
            assert_eq!(tags.len(), 3);
            assert_eq!(tags[0].value, "2");
            assert_eq!(tags[1].name, "Face1Position");
            assert_eq!(tags[1].value, "213 76 13 13");
            assert_eq!(tags[2].value, "100 50 20 10");
            tags.clear();
            decode_panasonic_faces(&data[..17], be, &mut tags);
            assert_eq!(tags.len(), 2);
            assert_eq!(tags[0].value, "1");
            tags.clear();
            decode_panasonic_faces(&data[..1], be, &mut tags);
            assert!(tags.is_empty());
            decode_panasonic_faces(&[0, 0], be, &mut tags);
            assert_eq!(tags.len(), 1);
            assert_eq!(tags[0].value, "0");
        }
    }

    #[test]
    fn panasonic_af_position_uses_the_rational_tag_and_keeps_precision() {
        assert_eq!(maker_tag_name(0x0048, Vendor::Panasonic), "FlashCurtain");
        assert_eq!(maker_tag_name(0x004d, Vendor::Panasonic), "AFPointPosition");
        for be in [false, true] {
            for (values, expected) in [
                ([320_u32, 1024, 640, 1024], "0.3125 0.625"),
                ([1, 0, 1, 2], "n/a"),
                ([u32::MAX, 1024, u32::MAX, 1024], "n/a"),
            ] {
                let bytes: Vec<u8> = values
                    .into_iter()
                    .flat_map(|value| {
                        if be {
                            value.to_be_bytes()
                        } else {
                            value.to_le_bytes()
                        }
                    })
                    .collect();
                let entry = IfdEntry {
                    tag: 0x004d,
                    data_type: tiff::DataType::Rational,
                    raw_type: 5,
                    count: 2,
                    data: &bytes,
                    inline: false,
                };
                assert_eq!(
                    format_panasonic_value(&entry, "AFPointPosition", be),
                    expected
                );
            }
        }
    }

    #[test]
    fn mn10_nikon_lens_lookup() {
        assert_eq!(nikon_lens_name("3D 54"), Some("Nikon AF-S 35mm f/1.8G DX"));
        assert_eq!(
            nikon_lens_name("92 48"),
            Some("Nikon AF-S 24-70mm f/2.8E ED VR")
        );
        assert_eq!(nikon_lens_name("FF FF"), None);
    }

    #[test]
    fn mn2_canon_standard_ifd() {
        // Canon: no header, standard IFD, parse from tiff_data at offset
        let mut tiff_data = vec![0u8; 100]; // padding before MN
        let mn_offset = 100;

        // Build IFD at offset 100
        let ifd = build_ifd_data(
            &[
                (0x0006, 2, 14, b"Canon EOS 5D\0\0".to_vec()),
                (0x0009, 2, 9, b"John Doe\0".to_vec()),
            ],
            false,
        );
        tiff_data.extend_from_slice(&ifd);

        let mn_data = &tiff_data[mn_offset..];
        let mnr = MakerNoteRef {
            data: mn_data,
            offset: mn_offset,
            format: MakerNoteFormat::StandardIfd,
        };

        let result = parse_maker_note(&mnr, &tiff_data, false).unwrap();
        assert!(result.ifd.is_some());
        let parsed_ifd = result.ifd.unwrap();
        assert_eq!(parsed_ifd.entries.len(), 2);
        assert_eq!(parsed_ifd.entries[0].tag, 0x0006);
    }
}
