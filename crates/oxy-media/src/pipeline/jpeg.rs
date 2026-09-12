mod selection;

use super::{
    artifact::{ArtifactCache, register_original_resource},
    jpeg_transform::{EncodedJpegThumbnail, normalize, plan_conversion},
};
use crate::{
    MediaError,
    cache::{
        ArtifactPresentation, ArtifactRepresentation, ColorRequirement, DetailRequirement,
        DisplayDimensions, OrientationRequirement, PresentationRequirement,
        RepresentationRequirement, SharpeningState, SourceRevision,
    },
    decode_control::{self, DecodePriority},
    delivery::{Delivery, THUMBNAIL_EDGE, THUMBNAIL_LIMITS},
    formats::jpeg::{self, Header},
    policy::JPEG_THUMBNAIL,
};
use oxy_domain::{PreviewKind, PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::{
    fs::File,
    io::{BufReader, Cursor, Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};

pub(crate) fn thumbnail(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: DecodePriority,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let source = SourceRevision::observe(path)?;
    let artifacts = ArtifactCache::for_source_revision(source.clone(), cache_dir)?;
    let request = artifacts.request(
        DetailRequirement::Display {
            min_long_edge: THUMBNAIL_EDGE,
        },
        RepresentationRequirement::BoundedThumbnail {
            target: JPEG_THUMBNAIL,
        },
        PresentationRequirement {
            orientation: OrientationRequirement::DisplayCorrect,
            color: ColorRequirement::Any,
            sharpening: SharpeningState::None,
        },
        false,
    );
    if let Some(result) = artifacts.lookup(&request, level)? {
        return Ok(result);
    }
    artifacts.coordinate_work(
        &request,
        level,
        JPEG_THUMBNAIL,
        || cancellation.is_cancelled(),
        |generation| {
            check_source(path, &source, cancellation)?;
            let started = Instant::now();
            let mut reader = BufReader::with_capacity(
                jpeg::HEADER_BUFFER,
                jpeg::TrackedReader::new(File::open(path)?),
            );
            let length = reader.get_ref().inner.metadata()?.len();
            let primary = jpeg::probe(&mut reader, length, || cancellation.is_cancelled())?;
            let probe_ms = started.elapsed().as_millis() as u64;
            let dimensions = primary.dimensions.ok_or_else(|| {
                MediaError::CacheArtifact("JPEG has no bounded SOF dimensions".into())
            })?;
            if primary.complete && THUMBNAIL_LIMITS.classify(dimensions, length) == Delivery::Direct
            {
                check_source(path, &source, cancellation)?;
                return register_original_resource(
                    path,
                    crate::media_source::preview_result(
                        path.to_owned(),
                        PreviewKind::Original,
                        level,
                    )?,
                );
            }
            let mut ranges = primary.previews.clone();
            ranges.sort_by_key(|range| range.length);
            let mut probe_budget = (1024_u64 * 1024).saturating_sub(reader.get_ref().bytes);
            let mut candidates = Vec::new();
            for range in ranges {
                check_source(path, &source, cancellation)?;
                reader.seek(SeekFrom::Start(range.offset))?;
                // Probe a prefix without materializing a potentially malicious MP entry.
                let prefix_length = range.length.min(jpeg::HEADER_BUFFER as u64) as usize;
                if prefix_length as u64 > probe_budget {
                    break;
                }
                probe_budget = probe_budget.saturating_sub(jpeg::HEADER_BUFFER as u64);
                let mut prefix = vec![0; prefix_length];
                reader.read_exact(&mut prefix)?;
                let candidate = match jpeg::probe(&mut Cursor::new(prefix), range.length, || {
                    cancellation.is_cancelled()
                }) {
                    Ok(candidate) => candidate,
                    Err(error) if candidate_failure(&error) => continue,
                    Err(error) => return Err(error),
                };
                if !selection::embedded_is_eligible(&primary, &candidate) {
                    continue;
                }
                candidates.push((range, candidate));
            }
            candidates.sort_by_key(|(range, candidate)| {
                let dimensions = candidate
                    .dimensions
                    .expect("eligible candidate has dimensions");
                (
                    u64::from(dimensions.width) * u64::from(dimensions.height),
                    range.length,
                )
            });
            // Corrupt payloads cannot turn a many-picture MP index into an
            // unbounded sequence of full materializations and decode attempts.
            for (range, candidate) in candidates.into_iter().take(3) {
                let conversion = plan_conversion(
                    candidate
                        .dimensions
                        .expect("eligible candidate has dimensions"),
                    range.length,
                )?;
                let _permit =
                    match decode_control::acquire_conversion(priority, conversion.cost, &|| {
                        cancellation.is_cancelled()
                    }) {
                        Ok(permit) => permit,
                        Err(MediaError::ResourceBudgetExhausted { .. }) => continue,
                        Err(error) => return Err(error),
                    };
                reader.seek(SeekFrom::Start(range.offset))?;
                let bytes_length = usize::try_from(range.length).map_err(|_| {
                    MediaError::CacheArtifact("JPEG preview length overflow".into())
                })?;
                let mut bytes = vec![0; bytes_length];
                reader.read_exact(&mut bytes)?;
                check_source(path, &source, cancellation)?;
                match normalize(
                    bytes,
                    &primary,
                    Some(&candidate),
                    conversion.decode,
                    cancellation,
                ) {
                    Ok(EncodedJpegThumbnail {
                        bytes,
                        dimensions,
                        presentation,
                    }) => {
                        let mut result = artifacts.publish(
                            bytes,
                            dimensions,
                            ArtifactRepresentation::Embedded,
                            thumbnail_presentation(&primary, dimensions, presentation),
                            false,
                            JPEG_THUMBNAIL.into(),
                            level,
                            generation,
                            &request,
                        )?;
                        result.diagnostics = Some(oxy_domain::PreviewDiagnostics {
                            backend: Some("jpeg-mpf-thumbnail".into()),
                            source_read_bytes: Some(reader.get_ref().bytes),
                            source_read_calls: Some(reader.get_ref().calls),
                            probe_ms: Some(probe_ms),
                            total_ms: Some(started.elapsed().as_millis() as u64),
                            ..Default::default()
                        });
                        return Ok(result);
                    }
                    Err(error) if candidate_failure(&error) => continue,
                    Err(error) => return Err(error),
                }
            }
            let conversion = plan_conversion(dimensions, length)?;
            let _permit = decode_control::acquire_conversion(priority, conversion.cost, &|| {
                cancellation.is_cancelled()
            })?;
            check_source(path, &source, cancellation)?;
            reader.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            (&mut reader).take(length).read_to_end(&mut bytes)?;
            check_source(path, &source, cancellation)?;
            let EncodedJpegThumbnail {
                bytes,
                dimensions: output_dimensions,
                presentation,
            } = normalize(bytes, &primary, None, conversion.decode, cancellation)?;
            let mut result = artifacts.publish(
                bytes,
                output_dimensions,
                ArtifactRepresentation::Decoded,
                thumbnail_presentation(&primary, output_dimensions, presentation),
                dimensions.width.max(dimensions.height) <= THUMBNAIL_EDGE,
                JPEG_THUMBNAIL.into(),
                level,
                generation,
                &request,
            )?;
            result.diagnostics = Some(oxy_domain::PreviewDiagnostics {
                backend: Some("jpeg-primary-thumbnail".into()),
                source_read_bytes: Some(reader.get_ref().bytes),
                source_read_calls: Some(reader.get_ref().calls),
                probe_ms: Some(probe_ms),
                total_ms: Some(started.elapsed().as_millis() as u64),
                ..Default::default()
            });
            Ok(result)
        },
    )
}

fn check_source(
    path: &Path,
    source: &SourceRevision,
    cancellation: &CancellationToken,
) -> Result<(), MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    if SourceRevision::observe(path)? != *source {
        return Err(MediaError::StaleSourceRevision);
    }
    Ok(())
}

fn candidate_failure(error: &MediaError) -> bool {
    matches!(
        error,
        MediaError::Image(_) | MediaError::Color(_) | MediaError::CacheArtifact(_)
    ) || matches!(error, MediaError::Io(error) if error.kind() == std::io::ErrorKind::UnexpectedEof)
}

fn thumbnail_presentation(
    primary: &Header,
    encoded: DisplayDimensions,
    mut result: ArtifactPresentation,
) -> ArtifactPresentation {
    if let Some(mut display) = primary.dimensions {
        if primary
            .exif
            .orientation
            .is_some_and(|orientation| orientation >= 5)
        {
            std::mem::swap(&mut display.width, &mut display.height);
        }
        result.geometry = Some(oxy_domain::PreviewGeometry {
            display_size: oxy_domain::PreviewDisplaySize {
                width: display.width,
                height: display.height,
            },
            content_rect: oxy_domain::PreviewContentRect {
                x: 0,
                y: 0,
                width: encoded.width,
                height: encoded.height,
            },
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::DynamicImage;

    #[test]
    fn thumbnail_keeps_primary_display_geometry_separate_from_rounded_mpf_pixels() {
        let header = Header {
            dimensions: Some(DisplayDimensions {
                width: 7008,
                height: 4672,
            }),
            exif: oxy_metadata_parser::jpeg_preview::ExifPresentation {
                orientation: Some(8),
                ..Default::default()
            },
            ..Default::default()
        };
        let geometry = thumbnail_presentation(
            &header,
            DisplayDimensions {
                width: 342,
                height: 512,
            },
            super::super::jpeg_transform::presentation(true),
        )
        .geometry
        .unwrap();
        assert_eq!(
            (geometry.display_size.width, geometry.display_size.height),
            (4672, 7008)
        );
        assert_eq!(
            (geometry.content_rect.width, geometry.content_rect.height),
            (342, 512)
        );
    }

    #[test]
    fn primary_fallback_and_full_are_separate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("primary.jpg");
        DynamicImage::new_rgb8(1200, 800).save(&path).unwrap();
        let cancellation = CancellationToken::default();
        let first = thumbnail(
            &path,
            directory.path(),
            RenderLevel::Thumbnail,
            DecodePriority::Visible,
            &cancellation,
        )
        .unwrap();
        assert_eq!((first.width, first.height), (512, 341));
        assert_ne!(first.path, path);
        let second = thumbnail(
            &path,
            directory.path(),
            RenderLevel::Thumbnail,
            DecodePriority::Visible,
            &cancellation,
        )
        .unwrap();
        assert_eq!(first.path, second.path);
        let full = super::super::dispatcher::preview(
            &path,
            directory.path(),
            RenderLevel::Full,
            oxy_domain::PreviewPriority::Loupe,
            oxy_domain::AssetKind::Jpeg,
            &cancellation,
        )
        .unwrap();
        assert_eq!(full.path, path);
        assert_eq!((full.width, full.height), (1200, 800));
    }

    /// External photographs are opt-in and never checked into the repository.
    #[test]
    fn external_jpeg_fixture_uses_mpf_and_bounds() {
        let Some(path) = std::env::var_os("OXY_JPEG_FIXTURE").map(std::path::PathBuf::from) else {
            return;
        };
        let mut reader = BufReader::with_capacity(jpeg::HEADER_BUFFER, File::open(&path).unwrap());
        let length = reader.get_ref().metadata().unwrap().len();
        let header = jpeg::probe(&mut reader, length, || false).unwrap();
        assert!(header.complete);
        assert!(!header.previews.is_empty(), "{header:?}");
        println!("primary header: {header:?}");
        for range in &header.previews {
            reader.seek(SeekFrom::Start(range.offset)).unwrap();
            let mut bytes = vec![0; range.length.min(jpeg::HEADER_BUFFER as u64) as usize];
            reader.read_exact(&mut bytes).unwrap();
            let child = jpeg::probe(&mut Cursor::new(bytes), range.length, || false);
            println!("candidate header: {child:?}");
        }
        let directory = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let result = thumbnail(
            &path,
            directory.path(),
            RenderLevel::Thumbnail,
            DecodePriority::Visible,
            &CancellationToken::default(),
        )
        .unwrap();
        assert!(result.width <= 512 && result.height <= 512);
        assert_eq!(result.width.max(result.height), 512);
        assert_eq!(result.kind, PreviewKind::Embedded);
        let diagnostics = result.diagnostics.as_ref().unwrap();
        assert!(
            diagnostics.source_read_bytes.unwrap() < length / 4,
            "{diagnostics:?}"
        );
        println!("read diagnostics: {diagnostics:?}");
        let output = std::fs::read(&result.path).unwrap();
        assert!(output.len() <= crate::delivery::THUMBNAIL_BYTES as usize);
        println!(
            "MPF thumbnail: {}x{}, {} bytes, {:?}",
            result.width,
            result.height,
            output.len(),
            started.elapsed()
        );
        if let Some(output_path) = std::env::var_os("OXY_JPEG_OUTPUT") {
            std::fs::write(output_path, output).unwrap();
        }
    }
}
