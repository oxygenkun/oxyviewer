//! Resolve RAW JPEG candidates whose dimensions LibRaw has not parsed (CR3
//! track JPEGs, and any other backend exposing an unresolved JPEG range).
use super::{LibRawData, Processor};
use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

unsafe extern "C" {
    fn oxy_libraw_add_jpeg(raw: *mut LibRawData, offset: u64, length: u32, width: u32, height: u32);
    fn oxy_libraw_unknown_jpeg(
        raw: *mut LibRawData,
        index: u32,
        offset: *mut u64,
        length: *mut u32,
    ) -> i32;
    fn oxy_libraw_resolve_jpeg(raw: *mut LibRawData, index: u32, width: u32, height: u32);
}

pub(super) fn resolve_unknown_dimensions(path: &Path, raw: &Processor<'_>) -> Result<(), String> {
    let mut file = None;
    if path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("rw2") || extension.eq_ignore_ascii_case("rwl")
    }) {
        let mut reader = File::open(path).map_err(|error| error.to_string())?;
        for range in crate::formats::rw2::jpeg_ranges(&mut reader).unwrap_or_default() {
            let Ok(Some(length)) = crate::formats::rw2::payload_length(&mut reader, &range) else {
                continue;
            };
            if let Some((width, height)) = probe_range(&mut reader, range.offset, u64::from(length))
            {
                // SAFETY: the adapter bounds the list and stores the validated range;
                // the normal LibRaw extraction path retains orientation/EXIF handling.
                unsafe { oxy_libraw_add_jpeg(raw.inner, range.offset, length, width, height) };
            }
        }
        file = Some(reader);
    }
    // LibRaw has at most LIBRAW_THUMBNAIL_MAXCOUNT (8) entries. Retain a bound
    // even if a future backend changes its representation.
    for index in 0..8 {
        let (mut offset, mut length) = (0, 0);
        // SAFETY: raw owns a live LibRaw instance; output pointers outlive the call.
        let status = unsafe { oxy_libraw_unknown_jpeg(raw.inner, index, &mut offset, &mut length) };
        if status < 0 {
            break;
        }
        if status == 0 {
            continue;
        }
        if file.is_none() {
            file = Some(File::open(path).map_err(|error| error.to_string())?);
        }
        let file = file.as_mut().unwrap();
        let Some((width, height)) = probe_range(file, offset, u64::from(length)) else {
            // An invalid or over-budget candidate cannot displace a known preview.
            continue;
        };
        // SAFETY: the wrapper checks index, format, and JPEG dimension bounds.
        unsafe { oxy_libraw_resolve_jpeg(raw.inner, index, width, height) };
    }
    Ok(())
}

fn probe_range(file: &mut File, offset: u64, length: u64) -> Option<(u32, u32)> {
    if length < 4 || offset.checked_add(length)? > file.metadata().ok()?.len() {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    // Bound both range and header work, including malicious lengths/fill bytes.
    // A small read buffer keeps NAS reads cheap; no entropy bytes are decoded.
    dimensions(&mut BufReader::with_capacity(
        4096,
        file.take(length.min(1024 * 1024)),
    ))
}

fn dimensions(reader: &mut impl Read) -> Option<(u32, u32)> {
    let mut pair = [0; 2];
    reader.read_exact(&mut pair).ok()?;
    if pair != [0xff, 0xd8] {
        return None;
    }
    for _ in 0..4096 {
        reader.read_exact(&mut pair).ok()?;
        if pair[0] != 0xff {
            return None;
        }
        while pair[1] == 0xff {
            reader.read_exact(&mut pair[1..]).ok()?;
        }
        let marker = pair[1];
        if matches!(marker, 0 | 0xd8..=0xda) {
            return None;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        reader.read_exact(&mut pair).ok()?;
        let length = u16::from_be_bytes(pair).checked_sub(2)?;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if length < 6 {
                return None;
            }
            let mut frame = [0; 6];
            reader.read_exact(&mut frame).ok()?;
            let height = u32::from(u16::from_be_bytes([frame[1], frame[2]]));
            let width = u32::from(u16::from_be_bytes([frame[3], frame[4]]));
            return (width > 0 && height > 0 && length == 6 + 3 * u16::from(frame[5]))
                .then_some((width, height));
        }
        let skipped =
            std::io::copy(&mut reader.take(u64::from(length)), &mut std::io::sink()).ok()?;
        if skipped != u64::from(length) {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn jpeg() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode_image(&image::DynamicImage::new_rgb8(60, 40))
            .unwrap();
        bytes
    }

    #[test]
    fn resolves_dimensions_without_reading_entropy_and_rejects_truncation() {
        let bytes = jpeg();
        let mut reader = Cursor::new(&bytes);
        assert_eq!(dimensions(&mut reader), Some((60, 40)));
        assert!(reader.position() < bytes.len() as u64);
        let header_end = reader.position() as usize;
        for end in 0..header_end {
            assert_eq!(dimensions(&mut &bytes[..end]), None);
        }
    }

    #[test]
    fn range_cannot_read_the_next_candidate_or_overflow_the_file() {
        let bytes = jpeg();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&[0; 16]).unwrap();
        file.write_all(&bytes).unwrap();
        assert_eq!(
            probe_range(&mut file, 16, bytes.len() as u64),
            Some((60, 40))
        );
        assert_eq!(probe_range(&mut file, 0, 16), None);
        assert_eq!(probe_range(&mut file, 16, 8), None);
        assert_eq!(probe_range(&mut file, u64::MAX - 1, 8), None);
        assert_eq!(probe_range(&mut file, 16, bytes.len() as u64 + 1), None);
    }

    #[test]
    fn header_budget_rejects_unbounded_fill_and_metadata() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&[0xff, 0xd8]).unwrap();
        file.write_all(&vec![0xff; 1024 * 1024]).unwrap();
        assert_eq!(probe_range(&mut file, 0, 1024 * 1024 + 2), None);
    }

    #[test]
    fn real_cr3_and_rw2_choose_the_largest_encoded_jpeg() {
        for (variable, expected) in [
            ("OXY_CR3_FIXTURE", (6000, 4000)),
            ("OXY_RW2_FIXTURE", (6000, 4000)),
        ] {
            let Some(path) = std::env::var_os(variable) else {
                eprintln!("skipping {variable}: fixture not configured");
                continue;
            };
            for target in [0, 4096] {
                let (super::super::Preview::EmbeddedJpeg(bytes), _) =
                    super::super::embedded(Path::new(&path), target).unwrap()
                else {
                    panic!("expected camera JPEG for {variable}");
                };
                assert_eq!(dimensions(&mut bytes.as_slice()), Some(expected));
            }
            let cache = tempfile::tempdir().unwrap();
            let full = crate::pipeline::raw::full(
                Path::new(&path),
                cache.path(),
                &oxy_runtime::CancellationToken::default(),
            )
            .unwrap();
            assert_eq!(full.kind, oxy_domain::PreviewKind::Embedded);
            assert_eq!(
                full.satisfaction,
                Some(oxy_domain::MediaSatisfaction::Satisfied)
            );
            assert_eq!((full.width, full.height), (4000, 6000));
            assert!(full.image_facts.unwrap().processing.is_empty());
        }
    }
}
