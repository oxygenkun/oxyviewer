use image::{
    ExtendedColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use lcms2::{Intent, PixelFormat, Profile, Transform};
use libheif_rs::{
    ColorPrimaries, ColorProfileNCLX, ColorSpace, HeifContext, LibHeif, MatrixCoefficients,
    RgbChroma, TransferCharacteristics,
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
    let color_space = if bit_depth > 8 {
        ColorSpace::Rgb(RgbChroma::HdrRgbLe)
    } else {
        ColorSpace::Rgb(RgbChroma::Rgb)
    };
    let image = LibHeif::new()
        .decode(&handle, color_space, None)
        .map_err(|error| heif_error(path, error))?;
    let plane = image.planes().interleaved.ok_or_else(|| MediaError::Heif {
        path: path.to_owned(),
        message: "decoded image has no interleaved RGB plane".into(),
    })?;
    let width = plane.width;
    let height = plane.height;
    let rgb = unpack_rgb(plane.data, width, height, plane.stride, bit_depth)?;
    let rgb = convert_to_srgb(rgb, raw_profile.as_deref(), nclx.as_ref())?;
    Ok(DecodedHeif { width, height, rgb })
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
    let bytes_per_sample = if bit_depth > 8 { 2 } else { 1 };
    let row_bytes = width as usize * 3 * bytes_per_sample;
    if stride < row_bytes || data.len() < stride * height as usize {
        return Err(MediaError::Color("invalid decoded HEIF RGB stride".into()));
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for row in data.chunks_exact(stride).take(height as usize) {
        for pixel in row[..row_bytes].chunks_exact(3 * bytes_per_sample) {
            let read = |offset| {
                if bytes_per_sample == 1 {
                    u16::from(pixel[offset]) * 257
                } else {
                    let value = u16::from_le_bytes([pixel[offset * 2], pixel[offset * 2 + 1]]);
                    normalize_to_u16(value, bit_depth)
                }
            };
            pixels.push([read(0), read(1), read(2)]);
        }
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
    let transfer = match nclx.transfer_characteristics() {
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
    for pixel in pixels {
        let source = pixel.map(|sample| transfer(f32::from(sample) / 65_535.0));
        let mut linear = multiply(matrix, source);
        if hdr {
            linear = linear.map(|sample| sample.max(0.0) / (1.0 + sample.max(0.0)));
        }
        for (output, sample) in pixel.iter_mut().zip(linear) {
            *output = (linear_to_srgb(sample.clamp(0.0, 1.0)) * 65_535.0).round() as u16;
        }
    }
    Ok(())
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
}
