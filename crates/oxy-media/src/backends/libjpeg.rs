//! IDCT reduction using the same pinned libjpeg backend as coefficient stitching.
use crate::{MediaError, media_source::PixelDimensions};
use image::{DynamicImage, RgbImage};
use oxy_runtime::CancellationToken;
use std::ffi::{c_char, c_int, c_ulong, c_void};

unsafe extern "C" {
    fn oxy_libjpeg_decode_scaled(
        input: *const u8,
        length: c_ulong,
        output: *mut u8,
        capacity: usize,
        expected_width: u32,
        expected_height: u32,
        denominator: u32,
        width: *mut u32,
        height: *mut u32,
        context: *mut c_void,
        cancelled: extern "C" fn(*mut c_void) -> c_int,
        message: *mut c_char,
        message_size: usize,
    ) -> c_int;
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DecodePlan {
    source: oxy_domain::EncodedDimensions,
    denominator: u32,
    pub output: oxy_domain::EncodedDimensions,
}

pub(crate) fn plan_decode(
    encoded_dimensions: oxy_domain::EncodedDimensions,
    target_edge: u32,
) -> DecodePlan {
    let dimensions = encoded_dimensions.0;
    let mut denominator = 1;
    while denominator < 8
        && dimensions.width.max(dimensions.height) / (denominator * 2) >= target_edge.max(1)
    {
        denominator *= 2;
    }
    DecodePlan {
        source: encoded_dimensions,
        denominator,
        output: oxy_domain::EncodedDimensions(PixelDimensions {
            width: dimensions.width.div_ceil(denominator),
            height: dimensions.height.div_ceil(denominator),
        }),
    }
}

extern "C" fn cancelled(context: *mut c_void) -> c_int {
    // SAFETY: decode_scaled holds this token borrowed throughout the synchronous C call.
    i32::from(unsafe { &*context.cast::<CancellationToken>() }.is_cancelled())
}

pub(crate) fn decode_scaled(
    bytes: &[u8],
    plan: DecodePlan,
    cancellation: &CancellationToken,
) -> Result<DynamicImage, MediaError> {
    let DecodePlan {
        source: dimensions,
        denominator,
        output: scaled,
    } = plan;
    let dimensions = dimensions.0;
    let scaled = scaled.0;
    let capacity = u64::from(scaled.width)
        .checked_mul(u64::from(scaled.height))
        .and_then(|pixels| pixels.checked_mul(3))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| MediaError::CacheArtifact("scaled JPEG size overflow".into()))?;
    let length = c_ulong::try_from(bytes.len())
        .map_err(|_| MediaError::CacheArtifact("JPEG input too large".into()))?;
    let mut output = vec![0; capacity];
    let mut message = [0_u8; 256];
    let (mut width, mut height) = (0, 0);
    // SAFETY: buffers and token outlive the call; C bounds-checks output before
    // writing. Callbacks cannot unwind; libjpeg longjmps remain inside C.
    let status = unsafe {
        oxy_libjpeg_decode_scaled(
            bytes.as_ptr(),
            length,
            output.as_mut_ptr(),
            capacity,
            dimensions.width,
            dimensions.height,
            denominator,
            &mut width,
            &mut height,
            std::ptr::from_ref(cancellation).cast_mut().cast(),
            cancelled,
            message.as_mut_ptr().cast(),
            message.len(),
        )
    };
    match status {
        0 => RgbImage::from_raw(width, height, output)
            .map(DynamicImage::ImageRgb8)
            .ok_or_else(|| MediaError::CacheArtifact("invalid scaled JPEG output".into())),
        2 => Err(MediaError::Cancelled),
        _ => Err(MediaError::CacheArtifact(format!(
            "JPEG reduction: {}",
            String::from_utf8_lossy(&message).trim_end_matches('\0')
        ))),
    }
}

pub(crate) mod stitch;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_target_controls_idct_plan_and_matches_decoded_dimensions() {
        let source = PixelDimensions {
            width: 1601,
            height: 1001,
        };
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode_image(&DynamicImage::new_rgb8(source.width, source.height))
            .unwrap();
        for (target, denominator) in [(1600, 1), (800, 2), (512, 2), (200, 8)] {
            let plan = plan_decode(oxy_domain::EncodedDimensions(source), target);
            assert_eq!(plan.denominator, denominator);
            let decoded = decode_scaled(&bytes, plan, &CancellationToken::default()).unwrap();
            assert_eq!(
                (decoded.width(), decoded.height()),
                (plan.output.0.width, plan.output.0.height)
            );
        }
    }

    #[test]
    fn source_mismatch_rejects_a_stale_decode_plan() {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode_image(&DynamicImage::new_rgb8(32, 24))
            .unwrap();
        let plan = plan_decode(
            oxy_domain::EncodedDimensions(PixelDimensions {
                width: 64,
                height: 48,
            }),
            16,
        );
        assert!(decode_scaled(&bytes, plan, &CancellationToken::default()).is_err());
    }
}
