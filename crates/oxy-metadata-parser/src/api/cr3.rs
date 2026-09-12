//! Canon CR3 metadata in moov / Canon UUID / CMTn TIFF boxes.
//! https://exiftool.org/TagNames/Canon.html#uuid
use super::Tag;
use crate::{
    core::TagValue,
    heif::parse_boxes,
    tiff::{self, maker_notes, tags::TagGroup},
};

const CANON_UUID: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];

pub(super) fn collect(data: &[u8], tags: &mut Vec<Tag>) {
    let Ok(boxes) = parse_boxes(data) else { return };
    for moov in boxes.iter().filter(|b| &b.box_type == b"moov") {
        if moov
            .offset
            .checked_add(moov.size)
            .is_none_or(|end| end > data.len() as u64)
        {
            continue;
        }
        let Ok(children) = parse_boxes(moov.data) else {
            continue;
        };
        for uuid in children
            .iter()
            .filter(|b| &b.box_type == b"uuid" && b.data.starts_with(&CANON_UUID))
        {
            if uuid
                .offset
                .checked_add(uuid.size)
                .is_none_or(|end| end > moov.data.len() as u64)
            {
                continue;
            }
            let Ok(records) = parse_boxes(&uuid.data[16..]) else {
                continue;
            };
            for record in records {
                if record
                    .offset
                    .checked_add(record.size)
                    .is_none_or(|end| end > (uuid.data.len() - 16) as u64)
                {
                    continue;
                }
                if record.data.len() > 16 * 1024 * 1024 {
                    continue;
                }
                let group = match &record.box_type {
                    b"CMT1" => TagGroup::Ifd0,
                    b"CMT2" => TagGroup::ExifIfd,
                    b"CMT4" => TagGroup::GpsIfd,
                    b"CMT3" => {
                        maker(record.data, data, tags);
                        continue;
                    }
                    _ => continue,
                };
                let Ok(header) = tiff::parse_header(record.data) else {
                    continue;
                };
                let Ok(ifd) = tiff::parse_ifd(
                    record.data,
                    header.ifd0_offset,
                    header.big_endian,
                    header.bigtiff,
                ) else {
                    continue;
                };
                for entry in &ifd.entries {
                    let Some(def) = tiff::tags::find_tag(entry.tag, group) else {
                        continue;
                    };
                    if let Some(value) = TagValue::from_entry(entry, header.big_endian) {
                        let text = tiff::tags::print_value(def, &value);
                        tags.push(Tag::with_typed("EXIF", def.name, text, value));
                    }
                }
            }
        }
    }
}

fn maker(data: &[u8], file: &[u8], tags: &mut Vec<Tag>) {
    let Ok(header) = tiff::parse_header(data) else {
        return;
    };
    let Ok(ifd) = tiff::parse_ifd(data, header.ifd0_offset, header.big_endian, header.bigtiff)
    else {
        return;
    };
    let base = data.as_ptr() as usize - file.as_ptr() as usize;
    let maker = maker_notes::MakerNote {
        vendor: maker_notes::Vendor::Canon,
        ifd: Some(ifd),
        big_endian: header.big_endian,
        nikon_tiff_offset: 0,
    };
    for tag in maker_notes::decode_maker_tags_with_tiff(
        &maker,
        data,
        base,
        base + header.ifd0_offset as usize,
        data,
    ) {
        tags.push(Tag::new("MakerNotes", &tag.name, tag.value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn atom(name: &[u8; 4], data: &[u8]) -> Vec<u8> {
        [(data.len() as u32 + 8).to_be_bytes().as_slice(), name, data].concat()
    }
    #[test]
    fn reads_only_the_canon_metadata_container_and_bounds_truncated_records() {
        let tiff = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x08\0\0\0\0\0\0\0";
        let cmt = atom(b"CMT1", tiff);
        let uuid = atom(b"uuid", &[CANON_UUID.as_slice(), &cmt].concat());
        let file = atom(b"moov", &uuid);
        let mut tags = Vec::new();
        collect(&file, &mut tags);
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "Orientation");
        assert_eq!(
            tags[0].typed_value.as_ref().and_then(TagValue::to_u32),
            Some(8)
        );
        for end in 0..file.len() {
            let mut tags = Vec::new();
            collect(&file[..end], &mut tags);
            assert!(tags.is_empty());
        }
        for file in [
            atom(b"mdat", &uuid),
            atom(
                b"moov",
                &atom(b"uuid", &[[0; 16].as_slice(), &cmt].concat()),
            ),
        ] {
            let mut tags = Vec::new();
            collect(&file, &mut tags);
            assert!(tags.is_empty());
        }
    }
}
