//! Single-threaded SIMD triangle convolution in encoded RGB space.
//! No gamma conversion, sharpening, crop, or extra decode workers.
use crate::MediaError;
use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer, images, pixels::U16x3};
use image::{DynamicImage, RgbImage};

pub(super) fn rgb_frame(image: DynamicImage, target: u32) -> Result<DynamicImage, MediaError> {
    if target == 0 {
        return Err(MediaError::InvalidMediaRequest);
    }
    if image.width().max(image.height()) <= target {
        return Ok(image);
    }
    let DynamicImage::ImageRgb8(rgb) = image else {
        return Ok(image.resize(target, target, image::imageops::FilterType::Triangle));
    };
    let ratio = f64::from(target) / f64::from(rgb.width().max(rgb.height()));
    let width = (f64::from(rgb.width()) * ratio).round().max(1.) as u32;
    let height = (f64::from(rgb.height()) * ratio).round().max(1.) as u32;
    // Retain eight fractional bits in U16 intermediates. U8 rounds after each
    // pass; that small pixel error can move detector landmarks and face crops.
    let input = images::TypedImage::from_pixels(
        rgb.width(),
        rgb.height(),
        rgb.pixels()
            .map(|p| U16x3::new(p.0.map(|v| u16::from(v) << 8)))
            .collect(),
    )
    .map_err(|e| MediaError::CacheArtifact(e.to_string()))?;
    drop(rgb);
    let mut output = images::TypedImage::<U16x3>::new(width, height);
    Resizer::new()
        .resize_typed(
            &input,
            &mut output,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear)),
        )
        .map_err(|e| MediaError::CacheArtifact(e.to_string()))?;
    drop(input);
    let bytes = output
        .pixels()
        .iter()
        .flat_map(|p| p.0.map(|v| ((u32::from(v) + 128) >> 8).min(255) as u8))
        .collect();
    RgbImage::from_raw(width, height, bytes)
        .map(DynamicImage::ImageRgb8)
        .ok_or_else(|| MediaError::CacheArtifact("invalid resized RGB buffer".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "real JPEG fixtures; writes paired old/new inputs for model parity"]
    fn real_jpeg_resize_parity() {
        use crate::{backends::libjpeg, formats::jpeg};
        let source = std::path::PathBuf::from(std::env::var_os("OXY_BENCH_IMAGE_DIR").unwrap());
        let output = std::path::PathBuf::from(std::env::var_os("OXY_RESIZE_PARITY_DIR").unwrap());
        std::fs::create_dir_all(&output).unwrap();
        let mut assets = oxy_fs::scan_assets_with_progress(&source, |_| {}).unwrap();
        assets.retain(|a| {
            a.path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("jpg"))
        });
        assets.sort_by(|a, b| a.path.cmp(&b.path));
        assert!(assets.len() >= 16);
        let mut rows = Vec::new();
        for i in 0..16 {
            let path = &assets[i * (assets.len() - 1) / 15].path;
            let bytes = std::fs::read(path).unwrap();
            let header = jpeg::probe(
                &mut std::io::Cursor::new(&bytes),
                bytes.len() as u64,
                || false,
            )
            .unwrap();
            let plan = libjpeg::plan_decode(header.encoded_dimensions.unwrap(), 1600);
            let decoded =
                libjpeg::decode_scaled(&bytes, plan, &oxy_runtime::CancellationToken::default())
                    .unwrap();
            let mut old = decoded.resize(1600, 1600, image::imageops::FilterType::Triangle);
            let started = std::time::Instant::now();
            let mut new = rgb_frame(decoded, 1600).unwrap();
            let resize_ms = started.elapsed().as_secs_f64() * 1000.;
            let orientation =
                image::metadata::Orientation::from_exif(header.exif.orientation.unwrap_or(1) as u8)
                    .unwrap();
            old.apply_orientation(orientation);
            new.apply_orientation(orientation);
            let old = old.to_rgb8();
            let new = new.to_rgb8();
            assert_eq!(old.dimensions(), new.dimensions());
            let diffs: Vec<_> = old
                .as_raw()
                .iter()
                .zip(new.as_raw())
                .map(|(a, b)| a.abs_diff(*b))
                .collect();
            let max = *diffs.iter().max().unwrap();
            let mean = diffs.iter().map(|&v| u64::from(v)).sum::<u64>() as f64 / diffs.len() as f64;
            assert!(max <= 2 && mean < 0.5, "max={max}, mean={mean}");
            old.save(output.join(format!("{i:02}-old.png"))).unwrap();
            new.save(output.join(format!("{i:02}-new.png"))).unwrap();
            rows.push(serde_json::json!({"file":path.file_name().unwrap().to_string_lossy(),"resize_ms":resize_ms,"max_channel_error":max,"mean_channel_error":mean}));
        }
        std::fs::write(
            output.join("pixels.json"),
            serde_json::to_vec_pretty(&rows).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn triangle_preserves_geometry_and_bounds_rounding_error() {
        for (width, height, target) in [
            (2376, 1584, 1600),
            (99, 171, 63),
            (1601, 1001, 1600),
            (1, 190, 31),
        ] {
            let image = DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
                image::Rgb([
                    (x * 17 + y * 31) as u8,
                    (x * 3 + y * 7) as u8,
                    (x + y * 11) as u8,
                ])
            }));
            let reference = image
                .resize(target, target, image::imageops::FilterType::Triangle)
                .to_rgb8();
            let actual = rgb_frame(image, target).unwrap().to_rgb8();
            assert_eq!(actual.dimensions(), reference.dimensions());
            let max = actual
                .as_raw()
                .iter()
                .zip(reference.as_raw())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(max <= 2, "rounding error={max}");
            let mean = actual
                .as_raw()
                .iter()
                .zip(reference.as_raw())
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum::<u64>() as f64
                / actual.as_raw().len() as f64;
            assert!(
                mean < 0.02,
                "intermediate precision lost: mean error={mean}"
            );
        }
        let image = DynamicImage::new_rgb8(17, 23);
        assert_eq!(
            rgb_frame(image.clone(), 1600).unwrap().to_rgb8(),
            image.to_rgb8()
        );
    }
}
