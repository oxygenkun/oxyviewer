//! Bounded header reads. Never scan the primary entropy stream for previews.
use crate::{MediaError, media_source::PixelDimensions};
use oxy_metadata_parser::jpeg_preview::{self, ExifPresentation, JpegRange};
use std::io::{Read, Seek, SeekFrom};

pub(crate) const HEADER_BUFFER: usize = 64 * 1024;
const METADATA_BUDGET: usize = 1024 * 1024;
const MARKER_BUDGET: usize = 4096;

pub(crate) struct TrackedReader<T> {
    pub inner: T,
    pub bytes: u64,
    pub calls: u64,
}

impl<T> TrackedReader<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            bytes: 0,
            calls: 0,
        }
    }
}

impl<T: Read> Read for TrackedReader<T> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.bytes += count as u64;
        self.calls += 1;
        Ok(count)
    }
}

impl<T: Seek> Seek for TrackedReader<T> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}

#[derive(Default, Debug)]
pub(crate) struct Header {
    pub encoded_dimensions: Option<oxy_domain::EncodedDimensions>,
    pub exif: ExifPresentation,
    pub previews: Vec<JpegRange>,
    pub has_icc: bool,
    pub has_edit_metadata: bool,
    pub complete: bool,
}

/// A reader may be limited to an MPF range. `base` is its absolute start;
/// the reader's current stream position is used for relative TIFF locations.
pub(crate) fn probe(
    reader: &mut (impl Read + Seek),
    source_length: u64,
    cancelled: impl Fn() -> bool,
) -> Result<Header, MediaError> {
    let mut header = Header::default();
    let mut magic = [0; 2];
    reader.read_exact(&mut magic)?;
    if magic != [0xff, 0xd8] {
        return Ok(header);
    }
    let mut metadata_bytes = 0;
    for _ in 0..MARKER_BUDGET {
        if cancelled() {
            return Err(MediaError::Cancelled);
        }
        reader.read_exact(&mut magic)?;
        if magic[0] != 0xff {
            return Ok(header);
        }
        while magic[1] == 0xff {
            metadata_bytes += 1;
            if metadata_bytes > METADATA_BUDGET {
                return Ok(header);
            }
            reader.read_exact(&mut magic[1..])?;
        }
        let marker = magic[1];
        if marker == 0xda {
            header.complete = true;
            return Ok(header);
        }
        if marker == 0xd9 || marker == 0 || marker == 0xd8 {
            return Ok(header);
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        reader.read_exact(&mut magic)?;
        let length = usize::from(u16::from_be_bytes(magic));
        if length < 2 {
            return Ok(header);
        }
        let length = length - 2;
        let offset = reader.stream_position()?;
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > source_length)
        {
            return Ok(header);
        }
        let sof = matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf);
        if marker == 0xed {
            header.has_edit_metadata = true;
        }
        if !sof && (marker == 0xe1 || marker == 0xe2) {
            metadata_bytes += length;
            if metadata_bytes > METADATA_BUDGET {
                // Still seek to SOF so primary decoding remains possible.
                // Uninspected metadata must never authorize an embedded image.
                header.has_edit_metadata = true;
                reader.seek(SeekFrom::Current(length as i64))?;
                continue;
            }
        }
        if marker == 0xe1 || marker == 0xe2 || sof {
            let mut payload = vec![0; length];
            reader.read_exact(&mut payload)?;
            if sof && payload.len() >= 6 {
                header.encoded_dimensions = Some(oxy_domain::EncodedDimensions(PixelDimensions {
                    width: u32::from(u16::from_be_bytes([payload[3], payload[4]])),
                    height: u32::from(u16::from_be_bytes([payload[1], payload[2]])),
                }));
            } else if marker == 0xe1 {
                if let Some(tiff) = payload.strip_prefix(b"Exif\0\0") {
                    match jpeg_preview::exif_presentation(tiff, offset + 6) {
                        Ok(exif) => header.exif = exif,
                        Err(_) => header.has_edit_metadata = true,
                    }
                } else {
                    // XMP may retain crop/development history and stale previews.
                    header.has_edit_metadata |= !jpeg_preview::xmp_preserves_preview(&payload);
                }
            } else if let Some(tiff) = payload.strip_prefix(b"MPF\0") {
                header.previews = jpeg_preview::mpf_thumbnails(tiff, offset + 4, source_length)
                    .unwrap_or_default();
            } else if payload.starts_with(b"ICC_PROFILE\0") {
                header.has_icc = true;
            }
        } else {
            reader.seek(SeekFrom::Current(length as i64))?;
        }
    }
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn metadata_budget_keeps_primary_dimensions_without_reading_entropy() {
        let mut bytes = vec![0xff, 0xd8];
        for _ in 0..20 {
            bytes.extend_from_slice(&[0xff, 0xe1, 0xff, 0xff]);
            bytes.resize(bytes.len() + 65_533, 0);
        }
        bytes.extend_from_slice(&[0xff, 0xc0, 0, 8, 8, 3, 32, 4, 176, 3, 0xff, 0xda]);
        let entropy_offset = bytes.len();
        bytes.resize(bytes.len() + 8 * 1024 * 1024, 1);
        let length = bytes.len() as u64;
        let mut reader =
            BufReader::with_capacity(HEADER_BUFFER, TrackedReader::new(Cursor::new(bytes)));
        let header = probe(&mut reader, length, || false).unwrap();
        assert!(header.complete && header.has_edit_metadata);
        assert_eq!(
            header.encoded_dimensions,
            Some(oxy_domain::EncodedDimensions(PixelDimensions {
                width: 1200,
                height: 800
            }))
        );
        // At most one bounded read-ahead enters entropy; no pixel stream scan.
        assert!(reader.get_ref().bytes <= (entropy_offset + HEADER_BUFFER) as u64);
        assert!(reader.stream_position().unwrap() <= entropy_offset as u64);
    }

    #[test]
    fn malformed_lengths_and_cancellation_do_not_become_complete_headers() {
        for bytes in [
            vec![0xff, 0xd8, 0xff, 0xe1, 0, 1],
            vec![0xff, 0xd8, 0xff, 0xe1, 1, 0],
        ] {
            let length = bytes.len() as u64;
            assert!(
                !probe(&mut Cursor::new(bytes), length, || false)
                    .unwrap()
                    .complete
            );
        }
        assert!(matches!(
            probe(&mut Cursor::new([0xff, 0xd8]), 2, || true),
            Err(MediaError::Cancelled)
        ));
    }
}
