use crate::pipeline::{
    input::{MediaPixels, check_cancelled},
    pixels::{estimate, finish, fit, orient, pixel_bytes},
};
use crate::{
    CacheColorState, MediaError, MediaRequest, SourceRevision,
    backends::{libjpeg, libraw},
    decode_control,
};
use image::ImageDecoder;
use oxy_domain::{DisplayDimensions, EncodedDimensions, ImageOperation, PixelDimensions};
use oxy_runtime::CancellationToken;
use std::{io::Cursor, path::Path};
pub(crate) fn decode_pixels(
    path: &Path,
    revision: &SourceRevision,
    dimensions: PixelDimensions,
    request: MediaRequest,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    let target = request.max_size;
    // Extraction only: reserve compressed source/candidate space before LibRaw
    // opens the container. Release before acquiring the pixel decode permit.
    let embedded = {
        let cost = revision
            .size_bytes
            .checked_mul(2)
            .and_then(|n| n.checked_add(16 * 1024 * 1024))
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(MediaError::InvalidMediaRequest)?;
        let _permit = decode_control::acquire_pixel_input(cost, &|| cancellation.is_cancelled())?;
        super::extract_embedded(path, 0)
    };
    check_cancelled(cancellation)?;
    match embedded {
        Ok((libraw::Preview::EmbeddedJpeg(bytes), reference)) => {
            match camera_jpeg(&bytes, revision, reference, request, cancellation) {
                Ok(input) if request.accepts(&input.facts) => return Ok(input),
                Err(
                    error @ (MediaError::Cancelled | MediaError::ResourceBudgetExhausted { .. }),
                ) => return Err(error),
                _ => {}
            }
        }
        Ok((libraw::Preview::EmbeddedImage(decoded), _)) if request.accepts(&decoded.facts) => {
            return finish(
                decoded.image,
                decoded.facts,
                revision,
                CacheColorState::EmbeddedOrUnknown,
                None,
                target,
                cancellation,
            );
        }
        _ => {}
    }
    check_cancelled(cancellation)?;
    let cost = estimate(dimensions, 12, revision.size_bytes)?;
    let _permit = decode_control::acquire_pixel_input(cost, &|| cancellation.is_cancelled())?;
    // Share the existing large RAW working-set exclusion with foreground full
    // development. Never hold a preview queue slot while waiting for it.
    let _raw = decode_control::acquire_raw_full_decode(&|| cancellation.is_cancelled())?;
    let decoded = super::decode_developed_pixels(
        path,
        if request.detail == crate::DetailRequirement::NativeDetail {
            None
        } else {
            Some(target)
        },
        cancellation,
    )?;
    finish(
        decoded.image,
        decoded.facts,
        revision,
        CacheColorState::Srgb,
        None,
        target,
        cancellation,
    )
}

fn camera_jpeg(
    bytes: &[u8],
    revision: &SourceRevision,
    reference: DisplayDimensions,
    request: MediaRequest,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    let target = request.max_size;
    let mut decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(bytes))?;
    let dimensions = EncodedDimensions(decoder.dimensions().into());
    let orientation = decoder.orientation()?;
    let icc_profile = decoder.icc_profile()?;
    let mut facts = super::camera_jpeg_facts(bytes, reference)?;
    if !request.accepts(&facts) {
        return Err(MediaError::UnqualifiedFrameRepresentation);
    }
    let plan = libjpeg::plan_decode(dimensions, target);
    let cost = estimate(
        dimensions.0,
        6,
        (bytes.len() as u64)
            .checked_add(pixel_bytes(plan.output.0, 6)?)
            .ok_or(MediaError::InvalidMediaRequest)?,
    )?;
    let _permit = decode_control::acquire_pixel_input(cost, &|| cancellation.is_cancelled())?;
    let mut image = fit(libjpeg::decode_scaled(bytes, plan, cancellation)?, target);
    facts.processing.push(ImageOperation::Decode {
        backend: "LibRaw embedded JPEG / libjpeg-turbo".into(),
    });
    orient(&mut image, &mut facts, orientation);
    finish(
        image,
        facts,
        revision,
        CacheColorState::EmbeddedOrUnknown,
        icc_profile,
        target,
        cancellation,
    )
}
