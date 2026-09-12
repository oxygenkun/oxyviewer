//! RAW full frames, independently qualified from the HEIF WIC adapter.
use super::{ComApartment, factory, native_error, wic_error, wic_stage_error, wide_path};
use crate::{MediaError, media_source::DecodedImage};
use image::{DynamicImage, RgbImage, metadata::Orientation};
use oxy_domain::{
    DisplayDimensions, EncodedDimensions, ImageOperation, ImageOrigin, RawSystemCodec,
};
use oxy_runtime::CancellationToken;
use std::{path::Path, ptr};
use windows::{
    Win32::{
        Foundation::GENERIC_READ,
        Graphics::Imaging::{
            GUID_WICPixelFormat24bppRGB, IWICBitmapCodecInfo, IWICBitmapDecoderInfo,
            IWICBitmapFrameDecode, IWICDevelopRaw, WICAsShotParameterSet,
            WICComponentEnumerateRefresh, WICDecodeMetadataCacheOnDemand, WICDecoder,
            WICRawRenderModeBestQuality,
        },
        System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear, PropVariantToUInt32},
    },
    core::{GUID, Interface, PCWSTR},
};

const MICROSOFT_RAW: GUID = GUID::from_u128(0x41945702_8302_44a6_9445_ac98e8afa086);

fn codec_string(
    read: impl Fn(&mut [u16], *mut u32) -> windows::core::Result<()>,
) -> Result<String, MediaError> {
    // Some installed codecs reject a non-null empty buffer during the usual
    // size query. A bounded writable buffer works across those implementations.
    let mut length = 0;
    let mut value = vec![0; 65_536];
    read(&mut value, &mut length).map_err(wic_error)?;
    if length as usize > value.len() {
        return Err(native_error("codec string exceeds limit"));
    }
    value.truncate(length as usize);
    Ok(String::from_utf16_lossy(&value)
        .trim_end_matches('\0')
        .to_owned())
}

pub(crate) fn codecs() -> Result<Vec<RawSystemCodec>, MediaError> {
    let _apartment = ComApartment::enter();
    let factory = factory()?;
    // COM objects stay on this worker and are released before the apartment.
    let enumerator = unsafe {
        factory
            .CreateComponentEnumerator(WICDecoder.0 as u32, WICComponentEnumerateRefresh.0 as u32)
    }
    .map_err(wic_error)?;
    let mut codecs = Vec::new();
    loop {
        let mut items = [None];
        let mut fetched = 0;
        unsafe { enumerator.Next(&mut items, Some(&mut fetched)) }
            .ok()
            .map_err(wic_error)?;
        if fetched == 0 {
            break;
        }
        let Some(codec) = items[0]
            .as_ref()
            .and_then(|item| item.cast::<IWICBitmapCodecInfo>().ok())
        else {
            continue;
        };
        let extensions =
            codec_string(|buffer, count| unsafe { codec.GetFileExtensions(buffer, count) })?;
        if !extensions.split(',').any(|extension| {
            matches!(
                extension.trim().to_ascii_lowercase().as_str(),
                ".arw"
                    | ".cr2"
                    | ".cr3"
                    | ".nef"
                    | ".nrw"
                    | ".dng"
                    | ".raf"
                    | ".rw2"
                    | ".orf"
                    | ".pef"
                    | ".srw"
            )
        }) {
            continue;
        }
        // Registration can outlive an installation or be visible from a context
        // that cannot activate the package. Probe activation without decoding a file.
        let Ok(decoder_info) = codec.cast::<IWICBitmapDecoderInfo>() else {
            continue;
        };
        if unsafe { decoder_info.CreateInstance() }.is_err() {
            continue;
        }
        codecs.push(RawSystemCodec {
            name: codec_string(|buffer, count| unsafe { codec.GetFriendlyName(buffer, count) })?,
            decoder_id: format!("{:?}", unsafe { codec.GetCLSID() }.map_err(wic_error)?),
            version: codec_string(|buffer, count| unsafe { codec.GetVersion(buffer, count) })?,
            extensions,
        });
    }
    Ok(codecs)
}

fn metadata(frame: &IWICBitmapFrameDecode, query: &str) -> Option<u32> {
    let reader = unsafe { frame.GetMetadataQueryReader() }.ok()?;
    let query = query.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut value = PROPVARIANT::default();
    let result = unsafe { reader.GetMetadataByName(PCWSTR(query.as_ptr()), &mut value) }
        .and_then(|()| unsafe { PropVariantToUInt32(&value) })
        .ok();
    let _ = unsafe { PropVariantClear(&mut value) };
    result
}

fn open(path: &Path) -> Result<IWICBitmapFrameDecode, MediaError> {
    let factory = factory()?;
    let path = wide_path(path);
    let decoder = unsafe {
        factory.CreateDecoderFromFilename(
            PCWSTR(path.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )
    }
    .map_err(|error| wic_stage_error("open RAW full decoder", error))?;
    let info = unsafe { decoder.GetDecoderInfo() }.map_err(wic_error)?;
    let id = unsafe { info.GetCLSID() }.map_err(wic_error)?;
    let frame = unsafe { decoder.GetFrame(0) }.map_err(wic_error)?;
    if id != MICROSOFT_RAW {
        // Unknown generic frame decoders may return embedded JPEGs. Require the
        // RAW development interface instead of inferring native detail from size.
        let raw = frame
            .cast::<IWICDevelopRaw>()
            .map_err(|error| wic_stage_error("qualify RAW development interface", error))?;
        unsafe { raw.LoadParameterSet(WICAsShotParameterSet) }.map_err(wic_error)?;
        unsafe { raw.SetRenderMode(WICRawRenderModeBestQuality) }.map_err(wic_error)?;
        let srgb = unsafe { factory.CreateColorContext() }.map_err(wic_error)?;
        unsafe { srgb.InitializeFromExifColorSpace(1) }.map_err(wic_error)?;
        unsafe { raw.SetDestinationColorContext(&srgb) }.map_err(wic_error)?;
    } else if metadata(&frame, "/ifd/exif/{ushort=40961}") != Some(1) {
        return Err(MediaError::Color(
            "system RAW frame has no verified sRGB output; using built-in development".into(),
        ));
    }
    Ok(frame)
}

fn frame_dimensions(frame: &IWICBitmapFrameDecode) -> Result<(u32, u32, Orientation), MediaError> {
    let mut width = 0;
    let mut height = 0;
    unsafe { frame.GetSize(&mut width, &mut height) }.map_err(wic_error)?;
    let orientation = metadata(frame, "/ifd/{ushort=274}")
        .and_then(|value| u8::try_from(value).ok())
        .and_then(Orientation::from_exif)
        .ok_or_else(|| native_error("RAW display orientation is unavailable"))?;
    if width == 0 || height == 0 {
        return Err(native_error("RAW frame is empty"));
    }
    Ok((width, height, orientation))
}

pub(crate) fn dimensions(path: &Path) -> Result<crate::ImageDimensions, MediaError> {
    let _apartment = ComApartment::enter();
    let frame = open(path)?;
    let (width, height, orientation) = frame_dimensions(&frame)?;
    Ok(EncodedDimensions((width, height).into())
        .to_display(orientation.to_exif())
        .0)
}

pub(crate) fn decode(
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<DecodedImage, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let _apartment = ComApartment::enter();
    let frame = open(path)?;
    let (width, height, orientation) = frame_dimensions(&frame)?;
    let display = EncodedDimensions((width, height).into()).to_display(orientation.to_exif());
    if let Ok(reference) = crate::backends::libraw::dimensions(path)
        && !native_extent_matches(display.0, reference)
    {
        return Err(native_error(
            "system RAW frame does not cover native sensor detail",
        ));
    }
    if unsafe { frame.GetPixelFormat() }.map_err(wic_error)? != GUID_WICPixelFormat24bppRGB {
        return Err(native_error(
            "system RAW full requires a qualified RGB24 frame",
        ));
    }
    let stride = width
        .checked_mul(3)
        .ok_or_else(|| native_error("RAW stride overflow"))?;
    let bytes = (stride as usize)
        .checked_mul(height as usize)
        .filter(|size| *size <= 1024 * 1024 * 1024)
        .ok_or_else(|| native_error("RAW frame exceeds pixel memory budget"))?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(bytes)
        .map_err(|error| native_error(error.to_string()))?;
    pixels.resize(bytes, 0);
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    // WIC has no reliable cancellation API. Recheck immediately after the native
    // call and never publish its result after the request has been cancelled.
    unsafe { frame.CopyPixels(ptr::null(), stride, &mut pixels) }.map_err(wic_error)?;
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let mut image = DynamicImage::ImageRgb8(
        RgbImage::from_raw(width, height, pixels)
            .ok_or_else(|| native_error("invalid RAW pixel extent"))?,
    );
    image.apply_orientation(orientation);
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let actual = DisplayDimensions((image.width(), image.height()).into());
    let mut facts = crate::media_source::source_facts(
        ImageOrigin::RawSensor,
        "raw-sensor".into(),
        EncodedDimensions(actual.0),
        1,
        actual,
    );
    facts.source.encoded_dimensions = None;
    facts.processing.push(ImageOperation::Develop {
        backend: "Windows WIC RAW".into(),
    });
    facts.processing.push(ImageOperation::ColorConvert {
        target: "sRGB".into(),
    });
    if orientation.to_exif() != 1 {
        facts.processing.push(ImageOperation::Orient {
            exif: orientation.to_exif(),
        });
    }
    Ok(DecodedImage { image, facts })
}

fn native_extent_matches(
    actual: crate::ImageDimensions,
    reference: crate::ImageDimensions,
) -> bool {
    let actual = (
        actual.width.max(actual.height),
        actual.width.min(actual.height),
    );
    let reference = (
        reference.width.max(reference.height),
        reference.width.min(reference.height),
    );
    // Allow small active-area differences, never a bounded preview or upscaled frame.
    [(actual.0, reference.0), (actual.1, reference.1)]
        .into_iter()
        .all(|(a, b)| {
            u64::from(a) * 100 >= u64::from(b) * 98 && u64::from(a) * 98 <= u64::from(b) * 100
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_preview_or_upscaled_extents_but_accepts_rotation_and_active_area() {
        let reference = (4688, 7028).into();
        assert!(native_extent_matches((7028, 4688).into(), reference));
        assert!(native_extent_matches((7008, 4672).into(), reference));
        assert!(!native_extent_matches((4096, 2731).into(), reference));
        assert!(!native_extent_matches((14056, 9376).into(), reference));
    }
    #[test]
    fn cancellation_precedes_file_and_codec_access() {
        let token = CancellationToken::default();
        token.cancel();
        assert!(matches!(
            decode(Path::new("missing.arw"), &token),
            Err(MediaError::Cancelled)
        ));
    }
}
