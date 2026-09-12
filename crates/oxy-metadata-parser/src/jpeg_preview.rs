//! Pure parsers for JPEG preview planning. Offsets in the returned ranges are
//! absolute source offsets; callers own bounded reads and source-version fences.

use crate::{
    core::{Error, Result},
    tiff::{self, DataType, Ifd},
};

pub const MAX_MPF_ENTRIES: usize = 64;

/// Rating/label-only packets do not describe pixel edits. Unknown properties,
/// extended packets and malformed XML are conservatively treated as edits.
#[cfg(feature = "xmp")]
pub fn xmp_preserves_preview(payload: &[u8]) -> bool {
    use crate::xmp;
    let Some(xml) = payload
        .strip_prefix(xmp::JPEG_XMP_HEADER)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    else {
        return false;
    };
    let Ok(data) = xmp::parse_xmp(xml) else {
        return false;
    };
    !data.properties.is_empty()
        && data.properties.iter().all(|property| {
            (property.namespace == xmp::ns::XMP
                && matches!(property.name.as_str(), "Rating" | "Label"))
                || (property.namespace == "adobe:ns:meta/"
                    && property.name == "XMPToolkit"
                    && property.value.as_str() == Some(""))
                || (property.namespace == xmp::ns::RDF && property.name == "About")
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JpegRange {
    pub offset: u64,
    pub length: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExifPresentation {
    pub orientation: Option<u16>,
    pub srgb: bool,
    pub software: Option<String>,
    pub thumbnail: Option<JpegRange>,
}

fn invalid(message: &str) -> Error {
    Error::Format(message.into())
}

fn short(ifd: &Ifd<'_>, tag: u16, be: bool) -> Option<u16> {
    let entry = ifd.entry(tag)?;
    (entry.data_type == DataType::Short)
        .then(|| entry.as_u16(be))
        .flatten()
}

fn long(ifd: &Ifd<'_>, tag: u16, be: bool) -> Option<u32> {
    let entry = ifd.entry(tag)?;
    (entry.data_type == DataType::Long)
        .then(|| entry.as_u32(be))
        .flatten()
}

/// Parse TIFF bytes after `Exif\0\0`, without visiting MakerNotes or pixels.
pub fn exif_presentation(data: &[u8], tiff_offset: u64) -> Result<ExifPresentation> {
    let header = tiff::parse_header(data)?;
    if header.bigtiff {
        return Err(invalid("BigTIFF is not supported in JPEG EXIF"));
    }
    let be = header.big_endian;
    let ifd = tiff::parse_ifd(data, header.ifd0_offset, be, false)?;
    let orientation = short(&ifd, 0x0112, be).filter(|value| (1..=8).contains(value));
    let software = ifd
        .entry(0x0131)
        .and_then(tiff::IfdEntry::as_ascii)
        .map(str::to_owned);
    let srgb = ifd
        .sub_ifd_offset(0x8769, be)
        .and_then(|offset| tiff::parse_ifd(data, offset, be, false).ok())
        .is_some_and(|exif| short(&exif, 0xa001, be) == Some(1));
    let thumbnail = (|| {
        if ifd.next_ifd_offset == 0 {
            return None;
        }
        let thumb = tiff::parse_ifd(data, ifd.next_ifd_offset, be, false).ok()?;
        if short(&thumb, 0x0103, be) != Some(6) {
            return None;
        }
        let offset = u64::from(long(&thumb, 0x0201, be)?);
        let length = u64::from(long(&thumb, 0x0202, be)?);
        if length < 4 || offset.checked_add(length)? > data.len() as u64 {
            return None;
        }
        Some(JpegRange {
            offset: tiff_offset.checked_add(offset)?,
            length,
        })
    })();
    Ok(ExifPresentation {
        orientation,
        srgb,
        software,
        thumbnail,
    })
}

#[derive(Clone, Copy)]
struct Entry {
    attribute: u32,
    range: JpegRange,
    dependents: [u16; 2],
}

/// Parse TIFF bytes after `MPF\0`. Only JPEG large-thumbnail roles explicitly
/// linked to the representative primary are returned. Stereo, gain maps and
/// unrelated pictures are never treated as previews, regardless of their size.
pub fn mpf_thumbnails(data: &[u8], tiff_offset: u64, source_length: u64) -> Result<Vec<JpegRange>> {
    let header = tiff::parse_header(data)?;
    if header.bigtiff {
        return Err(invalid("BigTIFF MPF is unsupported"));
    }
    let be = header.big_endian;
    let ifd = tiff::parse_ifd(data, header.ifd0_offset, be, false)?;
    let count = long(&ifd, 0xb001, be).ok_or_else(|| invalid("missing MP image count"))? as usize;
    if !(2..=MAX_MPF_ENTRIES).contains(&count) {
        return Err(invalid("MP image count exceeds budget"));
    }
    let table = ifd
        .entry(0xb002)
        .ok_or_else(|| invalid("missing MP entries"))?;
    if table.data_type != DataType::Undefined || table.data.len() != count * 16 {
        return Err(invalid("invalid MP entry table length"));
    }
    let u32_at = |bytes: &[u8]| {
        let bytes: [u8; 4] = bytes.try_into().expect("MP field has four bytes");
        if be {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    };
    let u16_at = |bytes: &[u8]| {
        let bytes: [u8; 2] = bytes.try_into().expect("MP dependency has two bytes");
        if be {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        }
    };
    let mut entries = Vec::with_capacity(count);
    for bytes in table.data.chunks_exact(16) {
        let relative = u64::from(u32_at(&bytes[8..12]));
        let offset = if relative == 0 {
            0
        } else {
            tiff_offset
                .checked_add(relative)
                .ok_or_else(|| invalid("MP offset overflow"))?
        };
        let length = u64::from(u32_at(&bytes[4..8]));
        if length < 4
            || offset
                .checked_add(length)
                .is_none_or(|end| end > source_length)
        {
            return Err(invalid("MP range outside source"));
        }
        let dependents = [u16_at(&bytes[12..14]), u16_at(&bytes[14..16])];
        if dependents.iter().any(|&index| usize::from(index) > count) {
            return Err(invalid("MP dependency outside index"));
        }
        entries.push(Entry {
            attribute: u32_at(&bytes[..4]),
            range: JpegRange { offset, length },
            dependents,
        });
    }
    let primary = entries[0];
    if primary.range.offset != 0 || primary.attribute & 0x07ff_ffff != 0x0003_0000 {
        return Err(invalid("MP index has no JPEG primary at source start"));
    }
    for (index, entry) in entries.iter().enumerate() {
        if entry.dependents.contains(&((index + 1) as u16)) {
            return Err(invalid("self-referential MP entry"));
        }
        for other in &entries[..index] {
            if entry.range.offset < other.range.offset + other.range.length
                && other.range.offset < entry.range.offset + entry.range.length
            {
                return Err(invalid("overlapping MP ranges"));
            }
        }
    }
    Ok(entries
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(index, entry)| {
            let role = entry.attribute & 0x07ff_ffff;
            let linked =
                primary.dependents.contains(&((index + 1) as u16)) || entry.dependents.contains(&1);
            (linked && (0x0001_0001..=0x0001_0005).contains(&role)).then_some(entry.range)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "xmp")]
    fn rating_only_xmp_does_not_invalidate_camera_preview() {
        let xml = r#"<x:xmpmeta xmlns:x='adobe:ns:meta/' x:xmptk=''><rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'><rdf:Description rdf:about='' xmlns:xmp='http://ns.adobe.com/xap/1.0/'><xmp:Rating>0</xmp:Rating></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let payload = [crate::xmp::JPEG_XMP_HEADER, xml.as_bytes()].concat();
        assert!(
            xmp_preserves_preview(&payload),
            "{:?}",
            crate::xmp::parse_xmp(xml)
        );
        let edited = xml.replace(
            "<xmp:Rating>0</xmp:Rating>",
            "<xmp:CreatorTool>Editor</xmp:CreatorTool>",
        );
        assert!(!xmp_preserves_preview(
            &[crate::xmp::JPEG_XMP_HEADER, edited.as_bytes()].concat()
        ));
    }

    fn fixture(be: bool, role: u32, relative: u32, dependency: u16) -> Vec<u8> {
        let mut data = Vec::new();
        let s = |v: u16| if be { v.to_be_bytes() } else { v.to_le_bytes() };
        let l = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
        data.extend_from_slice(if be { b"MM" } else { b"II" });
        data.extend(s(42));
        data.extend(l(8));
        data.extend(s(2));
        for (tag, kind, count, value) in [(0xb001, 4, 1, 2), (0xb002, 7, 32, 38)] {
            data.extend(s(tag));
            data.extend(s(kind));
            data.extend(l(count));
            data.extend(l(value));
        }
        data.extend(l(0));
        for (attr, len, offset, dep) in
            [(0xa0030000, 1000, 0, dependency), (role, 200, relative, 0)]
        {
            data.extend(l(attr));
            data.extend(l(len));
            data.extend(l(offset));
            data.extend(s(dep));
            data.extend(s(0));
        }
        data
    }

    #[test]
    fn mpf_offsets_are_relative_to_tiff_in_both_byte_orders() {
        for be in [false, true] {
            assert_eq!(
                mpf_thumbnails(&fixture(be, 0x40010001, 900, 2), 100, 1200).unwrap(),
                vec![JpegRange {
                    offset: 1000,
                    length: 200
                }]
            );
        }
    }

    #[test]
    fn rejects_unrelated_roles_ranges_and_corruption() {
        for role in [0x40050000, 0x40020002, 0x01010001] {
            assert!(
                mpf_thumbnails(&fixture(false, role, 900, 2), 100, 1200)
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(
            mpf_thumbnails(&fixture(false, 0x40010001, 900, 0), 100, 1200)
                .unwrap()
                .is_empty()
        );
        for (offset, dep) in [
            (899, 2),
            (901, 2),
            (0, 2),
            (900, 1),
            (900, 3),
            (u32::MAX, 2),
        ] {
            assert!(mpf_thumbnails(&fixture(false, 0x40010001, offset, dep), 100, 1200).is_err());
        }
        let data = fixture(false, 0x40010001, 900, 2);
        for end in 0..data.len() {
            assert!(mpf_thumbnails(&data[..end], 100, 1200).is_err());
        }
        assert!(mpf_thumbnails(&data, u64::MAX, u64::MAX).is_err());
    }
}
