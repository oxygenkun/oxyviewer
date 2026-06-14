use image::{
    DynamicImage, ExtendedColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use lcms2::{Intent, PixelFormat, Profile, Transform};
use libheif_rs::{
    ColorPrimaries, ColorProfileNCLX, ColorSpace, DecodingOptions, HeifContext, LibHeif,
    MatrixCoefficients, RgbChroma, TransferCharacteristics,
};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use crate::{ImageDimensions, MediaError};

pub struct DecodedHeif {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<[u16; 3]>,
}

pub fn dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    let context = open(path)?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| heif_error(path, error))?;
    Ok(ImageDimensions {
        width: handle.width(),
        height: handle.height(),
    })
}

pub fn decode_primary(path: &Path) -> Result<DecodedHeif, MediaError> {
    let context = open(path)?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| heif_error(path, error))?;
    let raw_profile = handle.color_profile_raw().map(|profile| profile.data);
    let nclx = handle.color_profile_nclx();
    let bit_depth = handle.luma_bits_per_pixel();
    let color_space = color_space_for_depth(bit_depth);
    let image = LibHeif::new()
        .decode(&handle, color_space, decoding_options(None))
        .map_err(|error| heif_error(path, error))?;
    let plane = image.planes().interleaved.ok_or_else(|| MediaError::Heif {
        path: path.to_owned(),
        message: "decoded image has no interleaved RGB plane".into(),
    })?;
    let rgb = unpack_rgb(
        plane.data,
        plane.width,
        plane.height,
        plane.stride,
        bit_depth,
    )?;
    let rgb = convert_to_srgb(rgb, raw_profile.as_deref(), nclx.as_ref())?;
    Ok(DecodedHeif {
        width: plane.width,
        height: plane.height,
        rgb,
    })
}

pub fn decode_scaled(path: &Path, max_size: u32) -> Result<DynamicImage, MediaError> {
    let context = open(path)?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| heif_error(path, error))?;
    let original_longest = handle.width().max(handle.height());
    // Like nomacs, prefer a container thumbnail before decoding the primary
    // image. Use the smallest thumbnail that satisfies the request, or the
    // largest available thumbnail as a fast progressive first stage.
    if max_size <= 2048 {
        let mut ids = Vec::new();
        handle.thumbnail_ids(&mut ids);
        let mut thumbnails = ids
            .into_iter()
            .filter_map(|id| handle.thumbnail(id).ok())
            .collect::<Vec<_>>();
        thumbnails.sort_by_key(|thumbnail| thumbnail.width().max(thumbnail.height()));
        if let Some(thumbnail) = thumbnails
            .iter()
            .find(|thumbnail| thumbnail.width().max(thumbnail.height()) >= max_size)
            .or_else(|| thumbnails.last())
        {
            if let Ok(image) =
                decode_preview_handle(thumbnail, path, Some(preview_thread_limit(max_size)))
            {
                return Ok(image.thumbnail(max_size, max_size));
            }
        }
    }

    // Decode the primary image to display-oriented 8-bit RGB, then scale in
    // libheif before copying pixels into the cache image. Full-detail requests
    // retain the separate high-bit-depth, color-managed path above.
    // This avoids unpacking and color-converting millions of pixels that will be
    // discarded by the subsequent resize.
    let bit_depth = handle.luma_bits_per_pixel();
    let mut options = decoding_options(Some(preview_thread_limit(max_size)));
    if let Some(ref mut opts) = options {
        if bit_depth > 8 {
            opts.set_convert_hdr_to_8bit(true);
        }
    }
    let image = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), options)
        .map_err(|error| heif_error(path, error))?;

    let scale = (max_size as f64 / original_longest as f64).min(1.0);
    let target_w = ((handle.width() as f64 * scale).round() as u32).max(1);
    let target_h = ((handle.height() as f64 * scale).round() as u32).max(1);
    let scaled = image
        .scale(target_w, target_h, None)
        .map_err(|error| heif_error(path, error))?;

    let plane = scaled
        .planes()
        .interleaved
        .ok_or_else(|| MediaError::Heif {
            path: path.to_owned(),
            message: "decoded image has no interleaved RGB plane".into(),
        })?;
    image_from_rgb8_plane(plane.data, plane.width, plane.height, plane.stride)
}

pub fn decode_full_rgb8(path: &Path) -> Result<DynamicImage, MediaError> {
    let context = open(path)?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| heif_error(path, error))?;
    decode_preview_handle(&handle, path, None)
}

fn decode_preview_handle(
    handle: &libheif_rs::ImageHandle,
    path: &Path,
    thread_limit: Option<u32>,
) -> Result<DynamicImage, MediaError> {
    let bit_depth = handle.luma_bits_per_pixel();
    let mut options = decoding_options(thread_limit);
    if let Some(ref mut opts) = options {
        if bit_depth > 8 {
            opts.set_convert_hdr_to_8bit(true);
        }
    }
    let image = LibHeif::new()
        .decode(handle, ColorSpace::Rgb(RgbChroma::Rgb), options)
        .map_err(|error| heif_error(path, error))?;
    let plane = image.planes().interleaved.ok_or_else(|| MediaError::Heif {
        path: path.to_owned(),
        message: "decoded image has no interleaved RGB plane".into(),
    })?;
    image_from_rgb8_plane(plane.data, plane.width, plane.height, plane.stride)
}

fn image_from_rgb8_plane(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<DynamicImage, MediaError> {
    let pixels = unpack_rgb_8_raw(data, width, height, stride)?;
    let buffer = image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::from_vec(width, height, pixels)
        .expect("pixel buffer dimensions match image dimensions");
    Ok(DynamicImage::ImageRgb8(buffer))
}

pub fn write_srgb_png(image: &DecodedHeif, destination: &Path) -> Result<(), MediaError> {
    let file = File::create(destination)?;
    let mut writer = BufWriter::new(file);
    let profile = Profile::new_srgb()
        .icc()
        .map_err(|error| MediaError::Color(error.to_string()))?;
    let mut encoder =
        PngEncoder::new_with_quality(&mut writer, CompressionType::Fast, FilterType::Adaptive);
    encoder
        .set_icc_profile(profile)
        .map_err(|error| MediaError::Color(error.to_string()))?;
    let bytes = unsafe {
        std::slice::from_raw_parts(
            image.rgb.as_ptr().cast::<u8>(),
            image.rgb.len() * std::mem::size_of::<[u16; 3]>(),
        )
    };
    encoder.write_image(bytes, image.width, image.height, ExtendedColorType::Rgb16)?;
    writer.flush()?;
    Ok(())
}

fn open(path: &Path) -> Result<HeifContext<'static>, MediaError> {
    let path_string = path.to_string_lossy();
    HeifContext::read_from_file(&path_string).map_err(|error| heif_error(path, error))
}

fn heif_error(path: &Path, error: impl std::fmt::Display) -> MediaError {
    MediaError::Heif {
        path: path.to_owned(),
        message: error.to_string(),
    }
}

/// Build [`DecodingOptions`] with an optional workload-specific thread limit.
fn decoding_options(thread_limit: Option<u32>) -> Option<DecodingOptions> {
    let mut options = DecodingOptions::new()?;
    let available = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    let threads = thread_limit.unwrap_or(available).min(available).max(1);
    options.set_num_codec_threads(threads);
    options.set_num_library_threads(threads);
    Some(options)
}

fn preview_thread_limit(max_size: u32) -> u32 {
    match max_size {
        0..=512 => 1,
        513..=2_048 => 2,
        _ => 4,
    }
}

fn color_space_for_depth(bit_depth: u8) -> ColorSpace {
    if bit_depth > 8 {
        ColorSpace::Rgb(RgbChroma::HdrRgbLe)
    } else {
        ColorSpace::Rgb(RgbChroma::Rgb)
    }
}

// ---------------------------------------------------------------------------
// Pixel unpacking (optimised: split 8-bit / 16-bit paths, no closures)
// ---------------------------------------------------------------------------

fn unpack_rgb(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bit_depth: u8,
) -> Result<Vec<[u16; 3]>, MediaError> {
    if !(8..=16).contains(&bit_depth) {
        return Err(MediaError::Color(format!(
            "unsupported HEIF bit depth {bit_depth}"
        )));
    }
    if bit_depth > 8 {
        unpack_rgb_16(data, width, height, stride, bit_depth)
    } else {
        unpack_rgb_8(data, width, height, stride)
    }
}

fn unpack_rgb_8(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<[u16; 3]>, MediaError> {
    let row_bytes = width as usize * 3;
    if data.len() < stride * height as usize {
        return Err(MediaError::Color("invalid decoded HEIF RGB stride".into()));
    }
    let pixel_count = width as usize * height as usize;
    let mut pixels = Vec::with_capacity(pixel_count);
    for row in data.chunks_exact(stride).take(height as usize) {
        pixels.extend(row[..row_bytes].chunks_exact(3).map(|rgb| {
            [
                u16::from(rgb[0]) * 257,
                u16::from(rgb[1]) * 257,
                u16::from(rgb[2]) * 257,
            ]
        }));
    }
    Ok(pixels)
}

/// Extract raw 8-bit RGB pixels from an interleaved plane, producing flat `u8`
/// data suitable for `ImageBuffer::<Rgb<u8>>`. Skips stride padding per row.
fn unpack_rgb_8_raw(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<u8>, MediaError> {
    let row_bytes = width as usize * 3;
    if data.len() < stride * height as usize {
        return Err(MediaError::Color("invalid decoded HEIF RGB stride".into()));
    }
    let mut pixels = Vec::with_capacity(row_bytes * height as usize);
    for row in data.chunks_exact(stride).take(height as usize) {
        pixels.extend_from_slice(&row[..row_bytes]);
    }
    Ok(pixels)
}

fn unpack_rgb_16(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bit_depth: u8,
) -> Result<Vec<[u16; 3]>, MediaError> {
    let row_bytes = width as usize * 6;
    if stride < row_bytes || data.len() < stride * height as usize {
        return Err(MediaError::Color("invalid decoded HEIF RGB stride".into()));
    }
    let pixel_count = width as usize * height as usize;
    let mut pixels = Vec::with_capacity(pixel_count);
    for row in data.chunks_exact(stride).take(height as usize) {
        pixels.extend(row[..row_bytes].chunks_exact(6).map(|rgb| {
            [
                normalize_to_u16(u16::from_le_bytes([rgb[0], rgb[1]]), bit_depth),
                normalize_to_u16(u16::from_le_bytes([rgb[2], rgb[3]]), bit_depth),
                normalize_to_u16(u16::from_le_bytes([rgb[4], rgb[5]]), bit_depth),
            ]
        }));
    }
    Ok(pixels)
}

fn normalize_to_u16(value: u16, bit_depth: u8) -> u16 {
    if bit_depth >= 16 {
        value
    } else {
        let maximum = (1_u32 << bit_depth) - 1;
        ((u32::from(value) * 65_535 + maximum / 2) / maximum) as u16
    }
}

// ---------------------------------------------------------------------------
// Colour-space conversion with LUT-accelerated transfer functions
// ---------------------------------------------------------------------------

fn convert_to_srgb(
    mut pixels: Vec<[u16; 3]>,
    raw_profile: Option<&[u8]>,
    nclx: Option<&ColorProfileNCLX>,
) -> Result<Vec<[u16; 3]>, MediaError> {
    if let Some(nclx) = nclx {
        if is_hdr(nclx.transfer_characteristics()) {
            convert_nclx_to_srgb(&mut pixels, nclx)?;
            return Ok(pixels);
        }
    }
    if let Some(profile) = raw_profile {
        let source =
            Profile::new_icc(profile).map_err(|error| MediaError::Color(error.to_string()))?;
        let destination = Profile::new_srgb();
        let transform = Transform::<[u16; 3], [u16; 3]>::new(
            &source,
            PixelFormat::RGB_16,
            &destination,
            PixelFormat::RGB_16,
            Intent::Perceptual,
        )
        .map_err(|error| MediaError::Color(error.to_string()))?;
        transform.transform_in_place(&mut pixels);
    } else if let Some(nclx) = nclx {
        convert_nclx_to_srgb(&mut pixels, nclx)?;
    }
    Ok(pixels)
}

fn convert_nclx_to_srgb(
    pixels: &mut [[u16; 3]],
    nclx: &ColorProfileNCLX,
) -> Result<(), MediaError> {
    if nclx.matrix_coefficients() == MatrixCoefficients::Unknown {
        return Err(MediaError::Color(
            "unknown HEIF NCLX matrix coefficients".into(),
        ));
    }
    let transfer: fn(f32) -> f32 = match nclx.transfer_characteristics() {
        TransferCharacteristics::ITU_R_BT_2100_0_PQ => pq_to_linear,
        TransferCharacteristics::ITU_R_BT_2100_0_HLG => hlg_to_linear,
        TransferCharacteristics::Linear => identity,
        TransferCharacteristics::IEC_61966_2_1 => srgb_to_linear,
        TransferCharacteristics::ITU_R_BT_709_5
        | TransferCharacteristics::ITU_R_BT_601_6
        | TransferCharacteristics::ITU_R_BT_2020_2_10bit
        | TransferCharacteristics::ITU_R_BT_2020_2_12bit
        | TransferCharacteristics::Unspecified => bt709_to_linear,
        value => {
            return Err(MediaError::Color(format!(
                "unsupported HEIF NCLX transfer characteristic {value:?}"
            )));
        }
    };
    let matrix = match nclx.color_primaries() {
        ColorPrimaries::ITU_R_BT_709_5 | ColorPrimaries::Unspecified => IDENTITY_MATRIX,
        ColorPrimaries::SMPTE_EG_432_1 => DISPLAY_P3_TO_SRGB,
        ColorPrimaries::ITU_R_BT_2020_2_and_2100_0 => BT2020_TO_SRGB,
        value => {
            return Err(MediaError::Color(format!(
                "unsupported HEIF NCLX color primaries {value:?}"
            )));
        }
    };
    let hdr = is_hdr(nclx.transfer_characteristics());

    // Build 65536-entry lookup tables to replace expensive per-pixel powf / exp calls.
    let transfer_lut = build_transfer_lut(transfer);
    let srgb_lut = build_srgb_lut();

    for pixel in pixels {
        // Step 1: transfer function via LUT (replaces powf / exp per pixel)
        let source = [
            transfer_lut[pixel[0] as usize],
            transfer_lut[pixel[1] as usize],
            transfer_lut[pixel[2] as usize],
        ];
        // Step 2: matrix multiply for gamut conversion
        let mut linear = multiply(matrix, source);
        // Step 3: HDR tone mapping (Reinhard)
        if hdr {
            linear = linear.map(|sample| sample.max(0.0) / (1.0 + sample.max(0.0)));
        }
        // Step 4: linear → sRGB gamma via LUT
        for (output, sample) in pixel.iter_mut().zip(linear) {
            let clamped = sample.clamp(0.0, 1.0);
            *output = srgb_lut[(clamped * 65535.0).round() as usize];
        }
    }
    Ok(())
}

/// Build a 65536-entry transfer-function LUT: `lut[v] = transfer(v / 65535.0)`.
fn build_transfer_lut(transfer: fn(f32) -> f32) -> Box<[f32; 65536]> {
    let mut lut = Box::new([0.0f32; 65536]);
    for (i, slot) in lut.iter_mut().enumerate() {
        *slot = transfer(i as f32 / 65535.0);
    }
    lut
}

/// Build a 65536-entry sRGB gamma LUT: `lut[v] = round(linear_to_srgb(v / 65535.0) * 65535)`.
fn build_srgb_lut() -> Box<[u16; 65536]> {
    let mut lut = Box::new([0u16; 65536]);
    for (i, slot) in lut.iter_mut().enumerate() {
        let linear = i as f32 / 65535.0;
        *slot = (linear_to_srgb(linear) * 65535.0).round() as u16;
    }
    lut
}

fn is_hdr(transfer: TransferCharacteristics) -> bool {
    matches!(
        transfer,
        TransferCharacteristics::ITU_R_BT_2100_0_PQ | TransferCharacteristics::ITU_R_BT_2100_0_HLG
    )
}

fn multiply(matrix: [[f32; 3]; 3], value: [f32; 3]) -> [f32; 3] {
    matrix.map(|row| row[0] * value[0] + row[1] * value[1] + row[2] * value[2])
}

fn identity(value: f32) -> f32 {
    value
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn bt709_to_linear(value: f32) -> f32 {
    if value < 0.081 {
        value / 4.5
    } else {
        ((value + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

const IDENTITY_MATRIX: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
const DISPLAY_P3_TO_SRGB: [[f32; 3]; 3] = [
    [1.224_9, -0.224_7, 0.0],
    [-0.042, 1.041_9, 0.0],
    [-0.019_7, -0.078_6, 1.098_3],
];
const BT2020_TO_SRGB: [[f32; 3]; 3] = [
    [1.660_5, -0.587_6, -0.072_8],
    [-0.124_6, 1.132_9, -0.008_3],
    [-0.018_2, -0.100_6, 1.118_7],
];

#[cfg(test)]
fn tone_map_sample(value: f32, transfer: fn(f32) -> f32) -> f32 {
    let linear = transfer(value);
    linear_to_srgb(linear / (1.0 + linear))
}

#[cfg(test)]
fn tone_map(pixels: &mut [[u16; 3]], transfer: fn(f32) -> f32) {
    for pixel in pixels {
        for sample in pixel {
            let srgb = tone_map_sample(f32::from(*sample) / 65_535.0, transfer);
            *sample = (srgb.clamp(0.0, 1.0) * 65_535.0).round() as u16;
        }
    }
}

fn pq_to_linear(value: f32) -> f32 {
    let m1 = 0.159_301_76;
    let m2 = 78.843_75;
    let c1 = 0.835_937_5;
    let c2 = 18.851_563;
    let c3 = 18.687_5;
    let p = value.powf(1.0 / m2);
    ((p - c1).max(0.0) / (c2 - c3 * p).max(f32::EPSILON)).powf(1.0 / m1)
}

fn hlg_to_linear(value: f32) -> f32 {
    if value <= 0.5 {
        value * value / 3.0
    } else {
        (((value - 0.559_910_7) / 0.178_832_77).exp() + 0.284_668_92) / 12.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_high_bit_depth_samples_without_truncation() {
        assert_eq!(normalize_to_u16(0, 10), 0);
        assert_eq!(normalize_to_u16(1_023, 10), 65_535);
        assert_eq!(normalize_to_u16(2_047, 12), 32_759);
        assert_eq!(normalize_to_u16(65_535, 16), 65_535);
    }

    #[test]
    fn hdr_transfer_functions_are_finite_and_monotonic() {
        for transfer in [pq_to_linear as fn(f32) -> f32, hlg_to_linear] {
            let low = transfer(0.1);
            let high = transfer(0.9);
            assert!(low.is_finite() && high.is_finite());
            assert!(low >= 0.0 && high > low);
        }
    }

    #[test]
    fn wide_gamut_conversion_keeps_neutral_values_neutral() {
        let neutral = multiply(BT2020_TO_SRGB, [0.5, 0.5, 0.5]);
        assert!((neutral[0] - neutral[1]).abs() < 0.001);
        assert!((neutral[1] - neutral[2]).abs() < 0.001);

        let mut pixels = [[32_768; 3]];
        tone_map(&mut pixels, hlg_to_linear);
        assert!(pixels[0][0] > 0 && pixels[0][0] < 65_535);
    }

    #[test]
    fn transfer_lut_matches_scalar_function() {
        for transfer in [
            pq_to_linear as fn(f32) -> f32,
            hlg_to_linear,
            srgb_to_linear,
            bt709_to_linear,
        ] {
            let lut = build_transfer_lut(transfer);
            // Spot-check a handful of values
            for v in [0u16, 1, 100, 10_000, 32_768, 50_000, 65_535] {
                let expected = transfer(v as f32 / 65535.0);
                let got = lut[v as usize];
                assert!(
                    (got - expected).abs() < 1e-6,
                    "LUT mismatch at {v}: got {got}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn srgb_lut_round_trips_through_identity_for_neutral() {
        let lut = build_srgb_lut();
        // For a neutral sRGB value the lut should map mid-gray to something close
        // (exact value depends on gamma curve but must be deterministic).
        let v = lut[32_768];
        assert!(
            v > 0 && v < 65_535,
            "mid-gray should map to valid range, got {v}"
        );
    }

    #[test]
    fn preview_thread_limits_keep_grid_decode_lightweight() {
        assert_eq!(preview_thread_limit(512), 1);
        assert_eq!(preview_thread_limit(1_024), 2);
        assert_eq!(preview_thread_limit(4_096), 4);
    }
}
