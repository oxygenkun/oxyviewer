//! Pixel delivery through the format pipelines and the shared artifact lifecycle.

use crate::{
    ArtifactLocation, ArtifactRequirement, CacheColorState, CacheRequest, DiskMediaCache,
    MEDIA_CACHE_POLICY_REVISION, MediaCache, MediaError, SourceRevision,
};
use image::DynamicImage;
use oxy_domain::{ArtifactFacts, AssetKind, DisplayDimensions, EncodedDimensions};
use oxy_runtime::CancellationToken;
use std::path::Path;

#[derive(Debug)]
pub struct MediaPixels {
    pub source_revision: SourceRevision,
    /// Display-oriented pixels, retaining alpha and the decoder pixel type.
    pub image: DynamicImage,
    /// Actual sampling, reference canvas, source and processing history.
    pub facts: ArtifactFacts,
    /// Encoded RGB is retained unless the chosen decoder establishes sRGB.
    /// This is deliberately not an unconditional color-management promise.
    pub color: CacheColorState,
    pub icc_profile: Option<Vec<u8>>,
    pub cache_hit: bool,
}

/// Reuses qualified media artifacts with their leases, without publishing a
/// second analysis cache or entering the interactive preview queues.
pub struct MediaService {
    cache: DiskMediaCache,
}

impl MediaService {
    pub fn new(cache_dir: &Path) -> Result<Self, MediaError> {
        Ok(Self {
            cache: DiskMediaCache::new(cache_dir, 256)?,
        })
    }

    /// Blocking pixel preparation. Run on a background worker, never the UI executor.
    pub fn prepare_pixels(
        &self,
        path: &Path,
        kind: AssetKind,
        expected: &SourceRevision,
        request: crate::MediaRequest,
        cancellation: &CancellationToken,
    ) -> Result<MediaPixels, MediaError> {
        prepare(
            path,
            kind,
            expected,
            request,
            cancellation,
            Some(&self.cache),
        )
    }
}

/// Standalone entry point for consumers without a media cache.
pub(crate) fn prepare_uncached(
    path: &Path,
    kind: AssetKind,
    expected: &SourceRevision,
    request: crate::MediaRequest,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    prepare(path, kind, expected, request, cancellation, None)
}

fn prepare(
    path: &Path,
    kind: AssetKind,
    expected: &SourceRevision,
    request: crate::MediaRequest,
    cancellation: &CancellationToken,
    cache: Option<&DiskMediaCache>,
) -> Result<MediaPixels, MediaError> {
    check_cancelled(cancellation)?;
    if request.max_size == 0 {
        return Err(MediaError::InvalidMediaRequest);
    }
    if SourceRevision::observe(path)? != *expected {
        return Err(MediaError::StaleSourceRevision);
    }
    let dimensions = crate::dimensions(path, kind)?;
    let target = request
        .max_size
        .min(dimensions.width.max(dimensions.height));
    let mut input = None;
    // JPEG/raster originals already have a cheap, bounded path. Prefer their
    // original samples over lossy display-cache encodings.
    if super::dispatcher::prefer_cached_pixels(kind)
        && let Some(cache) = cache
    {
        input = cached(cache, expected, request, cancellation)?;
        if input.is_none() {
            input = running(cache, expected, request, cancellation)?;
        }
    }
    let mut input = match input {
        Some(input) => input,
        None => super::dispatcher::prepare_pixels(
            path,
            kind,
            expected,
            dimensions,
            crate::MediaRequest {
                max_size: target,
                ..request
            },
            cancellation,
        )?,
    };
    check_cancelled(cancellation)?;
    if SourceRevision::observe(path)? != *expected {
        return Err(MediaError::StaleSourceRevision);
    }
    crate::media_source::bind_facts(&mut input.facts, expected)?;
    if !request.accepts(&input.facts)
        || !request.presentation.accepts(crate::ArtifactPresentation {
            geometry: None,
            orientation: crate::OrientationState::Applied,
            color: input.color,
            sharpening: crate::SharpeningState::None,
        })
    {
        return Err(MediaError::UnqualifiedFrameRepresentation);
    }
    Ok(input)
}

#[cfg(test)]
pub(crate) fn qualified(
    facts: &ArtifactFacts,
    source: oxy_domain::PixelDimensions,
    target: u32,
) -> bool {
    crate::request::MediaRequest::full_frame(source, target).accepts(facts)
}

pub(crate) fn cached(
    cache: &dyn MediaCache,
    source: &SourceRevision,
    frame: crate::MediaRequest,
    cancellation: &CancellationToken,
) -> Result<Option<MediaPixels>, MediaError> {
    let generation = cache.generation()?;
    let target = frame.max_size;
    let request = CacheRequest {
        source_revision: source.clone(),
        detail: frame.detail,
        artifact: ArtifactRequirement::AnyDisplay,
        presentation: frame.presentation,
        policy_revision: MEDIA_CACHE_POLICY_REVISION,
        allow_interim: false,
    };
    let Some(hit) = cache.lookup(&request)? else {
        return Ok(None);
    };
    // Padded camera JPEGs need their content transform, not a naive resize.
    // Leave these to the format planner rather than guessing that transform.
    if hit.artifact.variant.presentation.geometry.is_some()
        || !matches!(hit.artifact.media_type.as_str(), "image/jpeg" | "image/png")
    {
        return Ok(None);
    }
    let path = match &hit.artifact.location {
        ArtifactLocation::Managed(path) | ArtifactLocation::Original(path) => path,
    };
    let mut input = super::raster::decode_pixels(path, source, target, cancellation)?;
    let mut facts = hit.artifact.facts.clone();
    facts.resize(DisplayDimensions(
        (input.image.width(), input.image.height()).into(),
    ));
    facts.encoded_dimensions = EncodedDimensions(facts.display_dimensions.0);
    facts.exif_orientation = 1;
    input.facts = facts;
    input.color = hit.artifact.variant.presentation.color;
    input.cache_hit = true;
    // The hit owns its lease throughout all file reads and decoding.
    hit.lease.renew()?;
    if cache.generation()? != generation {
        return Err(MediaError::StaleCacheGeneration);
    }
    Ok(Some(input))
}

pub(crate) fn check_cancelled(cancellation: &CancellationToken) -> Result<(), MediaError> {
    if cancellation.is_cancelled() {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

fn running(
    cache: &DiskMediaCache,
    source: &SourceRevision,
    frame: crate::MediaRequest,
    cancellation: &CancellationToken,
) -> Result<Option<MediaPixels>, MediaError> {
    let generation = cache.generation()?;
    let request = CacheRequest {
        source_revision: source.clone(),
        detail: frame.detail,
        presentation: frame.presentation,
        artifact: ArtifactRequirement::AnyDisplay,
        policy_revision: MEDIA_CACHE_POLICY_REVISION,
        allow_interim: false,
    };
    let Some(super::artifact::PixelHandoff {
        result,
        lease,
        color,
    }) = super::artifact::join_pixel_output(cache.root(), &request, generation, cancellation)?
    else {
        return Ok(None);
    };
    let bytes = crate::shared_resource_registry().materialize(&lease)?;
    let mut input = super::raster::decode_bytes(&bytes, source, frame.max_size, cancellation)?;
    let mut facts = result
        .image_facts
        .ok_or(MediaError::UnqualifiedFrameRepresentation)?;
    facts.resize(DisplayDimensions(
        (input.image.width(), input.image.height()).into(),
    ));
    facts.encoded_dimensions = EncodedDimensions(facts.display_dimensions.0);
    facts.exif_orientation = 1;
    input.facts = facts;
    input.color = color;
    if cache.generation()? != generation {
        return Err(MediaError::StaleCacheGeneration);
    }
    // The resource lease remains alive across materialization and decoding.
    drop(lease);
    Ok(Some(input))
}
