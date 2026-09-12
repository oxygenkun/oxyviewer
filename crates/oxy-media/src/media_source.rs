use crate::{MediaError, backends::libheif, pipeline::raw};
use image::ImageReader;
use oxy_domain::{AssetKind, PreviewKind, PreviewResult, RenderLevel};
use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub use oxy_domain::{DisplayDimensions, EncodedDimensions, PixelDimensions};

// Public source probes report pixel extents; callers must interpret format orientation.
pub type ImageDimensions = PixelDimensions;

pub(crate) fn source_facts(
    origin: oxy_domain::ImageOrigin,
    candidate_id: String,
    encoded_dimensions: EncodedDimensions,
    exif_orientation: u8,
    reference_dimensions: DisplayDimensions,
) -> oxy_domain::ArtifactFacts {
    use oxy_domain::{
        ArtifactFacts, ByteIntegrity, DetailCoverage, PreviewContentRect, Sampling, SourceImage,
    };
    let display_dimensions = encoded_dimensions.to_display(exif_orientation);
    ArtifactFacts {
        exif_orientation,
        source: SourceImage {
            exif_orientation,
            revision_id: String::new(),
            candidate_id,
            origin,
            encoded_dimensions: Some(encoded_dimensions),
            display_dimensions,
        },
        encoded_dimensions,
        display_dimensions,
        detail: DetailCoverage {
            reference_dimensions,
            region: PreviewContentRect {
                x: 0,
                y: 0,
                width: reference_dimensions.0.width,
                height: reference_dimensions.0.height,
            },
            sampled_dimensions: display_dimensions,
            sampling: Sampling::Native,
        },
        processing: Vec::new(),
        byte_integrity: ByteIntegrity::Unverified,
    }
}

/// Bind newly produced facts once; derivations must retain the same source revision.
pub(crate) fn bind_facts(
    facts: &mut oxy_domain::ArtifactFacts,
    source: &crate::cache::SourceRevision,
) -> Result<(), MediaError> {
    if !facts.source.revision_id.is_empty() && facts.source.revision_id != source.revision_id {
        return Err(MediaError::StaleSourceRevision);
    }
    if !facts.is_consistent() {
        return Err(MediaError::CacheArtifact("inconsistent image facts".into()));
    }
    facts.source.revision_id.clone_from(&source.revision_id);
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_facts(
    origin: oxy_domain::ImageOrigin,
    dimensions: PixelDimensions,
    native: bool,
) -> oxy_domain::ArtifactFacts {
    let mut facts = source_facts(
        origin,
        "test-source-image".into(),
        EncodedDimensions(dimensions),
        1,
        DisplayDimensions(dimensions),
    );
    if !native {
        facts.detail.sampling = oxy_domain::Sampling::Reduced;
    }
    facts
}

#[derive(Debug)]
pub(crate) struct DecodedImage {
    pub image: image::DynamicImage,
    pub facts: oxy_domain::ArtifactFacts,
}

pub(crate) fn decoded_facts(
    origin: oxy_domain::ImageOrigin,
    candidate_id: String,
    source: DisplayDimensions,
    reference: DisplayDimensions,
    output: DisplayDimensions,
    backend: &str,
) -> oxy_domain::ArtifactFacts {
    let mut facts = source_facts(
        origin,
        candidate_id,
        EncodedDimensions(source.0),
        1,
        reference,
    );
    // Container decoders report oriented pixels, not the pre-transform storage extent.
    facts.source.encoded_dimensions = None;
    facts.processing.push(oxy_domain::ImageOperation::Decode {
        backend: backend.into(),
    });
    facts.resize(output);
    facts.encoded_dimensions = EncodedDimensions(output.0);
    facts
}

fn raster_dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    let raster = || -> Result<ImageDimensions, MediaError> {
        let reader = ImageReader::new(BufReader::new(File::open(path)?)).with_guessed_format()?;
        let (width, height) = reader.into_dimensions()?;
        Ok(ImageDimensions { width, height })
    };

    raster()
}

pub fn dimensions(path: &Path, kind: AssetKind) -> Result<ImageDimensions, MediaError> {
    match kind {
        AssetKind::Jpeg | AssetKind::Png | AssetKind::Webp | AssetKind::Tiff => {
            raster_dimensions(path)
        }
        AssetKind::Heif => libheif::dimensions(path),
        AssetKind::Raw => raw::dimensions(path),
    }
}

pub(crate) fn preview_result(
    path: PathBuf,
    kind: PreviewKind,
    level: RenderLevel,
) -> Result<PreviewResult, MediaError> {
    use image::ImageDecoder;
    let mut decoder = ImageReader::new(BufReader::new(File::open(&path)?))
        .with_guessed_format()?
        .into_decoder()?;
    let encoded = EncodedDimensions(decoder.dimensions().into());
    let orientation = decoder.orientation()?.to_exif();
    let display = encoded.to_display(orientation);
    let mut facts = source_facts(
        oxy_domain::ImageOrigin::PrimaryImage,
        "primary".into(),
        encoded,
        orientation,
        display,
    );
    facts.byte_integrity = oxy_domain::ByteIntegrity::SourceFile;
    facts.source.revision_id = crate::cache::SourceRevision::observe(&path)?.revision_id;
    let size = display.0;
    Ok(PreviewResult {
        image_facts: Some(facts),
        geometry: None,
        path,
        width: size.width,
        height: size.height,
        kind,
        render_level: level,
        resource: None,
        satisfaction: None,
        persistence: None,
        diagnostics: None,
    })
}

pub(crate) fn has_complete_jpeg_markers(path: &Path) -> Result<bool, std::io::Error> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() < 4 {
        return Ok(false);
    }
    let mut start = [0_u8; 2];
    file.read_exact(&mut start)?;
    file.seek(SeekFrom::End(-2))?;
    let mut end = [0_u8; 2];
    file.read_exact(&mut end)?;
    Ok(start == [0xff, 0xd8] && end == [0xff, 0xd9])
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_domain::{ImageOperation, ImageOrigin};

    #[test]
    fn coordinate_frames_validate_all_exif_orientations() {
        for orientation in 1..=8 {
            let encoded = EncodedDimensions((600, 400).into());
            let display = encoded.to_display(orientation);
            let mut facts = source_facts(
                ImageOrigin::PrimaryImage,
                "primary".into(),
                encoded,
                orientation,
                display,
            );
            assert!(facts.is_consistent());
            assert_eq!(
                display.0,
                if orientation >= 5 {
                    (400, 600).into()
                } else {
                    (600, 400).into()
                }
            );
            facts.exif_orientation = if orientation >= 5 { 1 } else { 6 };
            assert!(!facts.is_consistent());
        }
    }

    #[test]
    fn resampling_and_partial_coverage_cannot_recover_native_detail() {
        let mut facts = test_facts(ImageOrigin::RawSensor, (6000, 4000).into(), true);
        assert!(facts.native_detail());
        let source = facts.source.clone();
        facts.resize(DisplayDimensions((600, 400).into()));
        facts.resize(DisplayDimensions((6000, 4000).into()));
        assert_eq!(facts.source, source);
        assert_eq!(
            facts.detail.sampled_dimensions,
            DisplayDimensions((600, 400).into())
        );
        assert!(!facts.native_detail());
        assert!(matches!(
            facts.processing.last(),
            Some(ImageOperation::Resize { .. })
        ));
        let mut cropped = test_facts(ImageOrigin::PrimaryImage, (6000, 4000).into(), true);
        cropped.detail.region.width = 3000;
        assert!(cropped.is_consistent());
        assert!(!cropped.native_detail());
        cropped.detail.region.x = u32::MAX;
        assert!(!cropped.is_consistent());
    }
}
