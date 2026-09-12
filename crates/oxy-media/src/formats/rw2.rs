//! Panasonic RAW preview ranges in IFD0. 0x002e is JpgFromRaw; newer
//! cameras store the large JpgFromRaw2 in 0x0127. These are byte-array tags,
//! not a JPEG-offset/length tag pair. No RAW entropy scanning is performed.
//! Reference: https://exiftool.org/TagNames/PanasonicRaw.html
use std::io::{self, Read, Seek, SeekFrom};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct JpegRange {
    pub offset: u64,
    pub length: u32,
}

/// JpgFromRaw2's TIFF byte count includes alignment padding (00/FF), which is
/// not part of the JPEG. Preserve all JPEG bytes through EOI and keep strict
/// artifact validation; do not relax it to accept arbitrary trailing data.
pub(crate) fn payload_length(
    reader: &mut (impl Read + Seek),
    range: &JpegRange,
) -> io::Result<Option<u32>> {
    let count = range.length.min(4096);
    reader.seek(SeekFrom::Start(
        range.offset + u64::from(range.length - count),
    ))?;
    let mut tail = vec![0; count as usize];
    reader.read_exact(&mut tail)?;
    let end = tail
        .iter()
        .rposition(|byte| !matches!(byte, 0 | 0xff))
        .map_or(0, |last| last + 1);
    Ok((end >= 2 && tail[end - 2..end] == [0xff, 0xd9])
        .then_some(range.length - count + end as u32))
}

pub(crate) fn jpeg_ranges(reader: &mut (impl Read + Seek)) -> io::Result<Vec<JpegRange>> {
    let size = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    let mut header = [0; 8];
    reader.read_exact(&mut header)?;
    let little = match &header[..4] {
        b"II\x55\0" => true,
        b"MM\0\x55" => false,
        _ => return Ok(Vec::new()),
    };
    let word = |bytes: [u8; 2]| {
        if little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }
    };
    let dword = |bytes: [u8; 4]| {
        if little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        }
    };
    let directory = u64::from(dword(header[4..8].try_into().unwrap()));
    if directory < 8 || directory.checked_add(2).is_none_or(|end| end > size) {
        return Ok(Vec::new());
    }
    reader.seek(SeekFrom::Start(directory))?;
    let mut count = [0; 2];
    reader.read_exact(&mut count)?;
    let count = usize::from(word(count));
    if count > 1024 || directory + 2 + count as u64 * 12 + 4 > size {
        return Ok(Vec::new());
    }
    let mut entries = vec![0; count * 12];
    reader.read_exact(&mut entries)?;
    let mut ranges = Vec::new();
    for entry in entries.chunks_exact(12) {
        let tag = word(entry[..2].try_into().unwrap());
        let format = word(entry[2..4].try_into().unwrap());
        let length = dword(entry[4..8].try_into().unwrap());
        let offset = u64::from(dword(entry[8..12].try_into().unwrap()));
        if matches!(tag, 0x002e | 0x0127)
            && format == 7
            && (5..=128 * 1024 * 1024).contains(&length)
            && offset >= 8
            && offset + u64::from(length) <= size
            && !ranges
                .iter()
                .any(|range: &JpegRange| range.offset == offset)
        {
            ranges.push(JpegRange { offset, length });
        }
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn rw2(tag: u16, format: u16, length: u32, offset: u32) -> Vec<u8> {
        let mut data = b"II\x55\0\x08\0\0\0\x01\0".to_vec();
        data.extend(tag.to_le_bytes());
        data.extend(format.to_le_bytes());
        data.extend(length.to_le_bytes());
        data.extend(offset.to_le_bytes());
        data.resize(128, 0);
        data
    }

    #[test]
    fn large_preview_uses_byte_count_and_offset_from_jpgfromraw2() {
        for tag in [0x002e, 0x0127] {
            assert_eq!(
                jpeg_ranges(&mut Cursor::new(rw2(tag, 7, 64, 32))).unwrap(),
                vec![JpegRange {
                    offset: 32,
                    length: 64
                }]
            );
        }
    }

    #[test]
    fn unrelated_sensor_data_invalid_types_and_ranges_are_not_previews() {
        for (tag, format, length, offset) in [
            (0x0118, 7, 64, 32),
            (0x0127, 4, 64, 32),
            (0x0127, 7, 4, 32),
            (0x0127, 7, 64, 100),
            (0x0127, 7, u32::MAX, 32),
            (0x0127, 7, 64, u32::MAX),
        ] {
            assert!(
                jpeg_ranges(&mut Cursor::new(rw2(tag, format, length, offset)))
                    .unwrap()
                    .is_empty()
            );
        }
        let mut wrong_magic = rw2(0x0127, 7, 64, 32);
        wrong_magic[2] = 42;
        assert!(
            jpeg_ranges(&mut Cursor::new(wrong_magic))
                .unwrap()
                .is_empty()
        );
        let mut unbounded = rw2(0x0127, 7, 64, 32);
        unbounded[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(jpeg_ranges(&mut Cursor::new(unbounded)).unwrap().is_empty());
    }

    #[test]
    fn only_terminal_alignment_padding_is_removed() {
        let jpeg = [0xff, 0xd8, 1, 0, 0xff, 0, 3, 0xff, 0xd9];
        for suffix in [&[][..], &[0xff, 0xff, 0, 0][..]] {
            let bytes = [jpeg.as_slice(), suffix].concat();
            let range = JpegRange {
                offset: 0,
                length: bytes.len() as u32,
            };
            assert_eq!(
                payload_length(&mut Cursor::new(bytes), &range).unwrap(),
                Some(jpeg.len() as u32)
            );
        }
        for bytes in [vec![0xff, 0xd8, 0, 0], [jpeg.as_slice(), &[7]].concat()] {
            let range = JpegRange {
                offset: 0,
                length: bytes.len() as u32,
            };
            assert_eq!(
                payload_length(&mut Cursor::new(bytes), &range).unwrap(),
                None
            );
        }
    }
}
