use crate::MediaError;
pub(crate) mod raw;
use image::{DynamicImage, ImageBuffer, Rgba};
use std::{os::windows::ffi::OsStrExt, path::Path, ptr};
use windows::{
    Win32::{
        Foundation::GENERIC_READ,
        Graphics::Imaging::{
            CLSID_WICImagingFactory2, GUID_ContainerFormatHeif, GUID_WICPixelFormat32bppRGBA,
            IWICBitmapCodecInfo, IWICBitmapSource, IWICImagingFactory, WICBitmapDitherTypeNone,
            WICBitmapPaletteTypeCustom, WICComponentEnumerateDefault,
            WICDecodeMetadataCacheOnDemand, WICDecoder,
        },
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
    },
    core::{Interface, PCWSTR},
};

struct ComApartment(bool);

impl ComApartment {
    fn enter() -> Self {
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };
        Self(initialized)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

pub fn capability() -> Result<(), MediaError> {
    let _apartment = ComApartment::enter();
    let factory = factory()?;
    let enumerator = unsafe {
        factory
            .CreateComponentEnumerator(WICDecoder.0 as u32, WICComponentEnumerateDefault.0 as u32)
    }
    .map_err(wic_error)?;
    loop {
        let mut items = [None];
        let mut fetched = 0;
        let result = unsafe { enumerator.Next(&mut items, Some(&mut fetched)) };
        if fetched == 0 {
            break;
        }
        if result.is_err() {
            return Err(wic_error(result.into()));
        }
        let Some(item) = items[0].take() else {
            continue;
        };
        let Ok(codec) = item.cast::<IWICBitmapCodecInfo>() else {
            continue;
        };
        if unsafe { codec.GetContainerFormat() }.map_err(wic_error)? == GUID_ContainerFormatHeif {
            return Ok(());
        }
    }
    Err(MediaError::NativeDecoderUnavailable)
}

pub fn decode_full_rgba8(path: &Path) -> Result<DynamicImage, MediaError> {
    let _apartment = ComApartment::enter();
    let factory = factory()?;
    let (_, converter) = open_converted_frame(&factory, path)?;
    copy_rgba8(&converter)
}

fn copy_rgba8(
    converter: &windows::Win32::Graphics::Imaging::IWICFormatConverter,
) -> Result<DynamicImage, MediaError> {
    let source: IWICBitmapSource = converter
        .cast()
        .map_err(|error| wic_stage_error("cast converted frame", error))?;
    let mut width = 0;
    let mut height = 0;
    unsafe { source.GetSize(&mut width, &mut height) }
        .map_err(|error| wic_stage_error("read decoded dimensions", error))?;
    let stride = width
        .checked_mul(4)
        .ok_or_else(|| native_error("decoded row stride overflow"))?;
    let buffer_len = usize::try_from(stride)
        .ok()
        .and_then(|stride| {
            usize::try_from(height)
                .ok()
                .and_then(|height| stride.checked_mul(height))
        })
        .ok_or_else(|| native_error("decoded image buffer size overflow"))?;
    let mut pixels = vec![0; buffer_len];
    unsafe { source.CopyPixels(ptr::null(), stride, &mut pixels) }
        .map_err(|error| wic_stage_error("copy decoded pixels", error))?;
    let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_vec(width, height, pixels)
        .ok_or_else(|| native_error("decoded image buffer dimensions do not match"))?;
    Ok(DynamicImage::ImageRgba8(image))
}

#[cfg(feature = "bench-tools")]
pub(crate) fn benchmark_raw(
    path: &Path,
    require_develop: bool,
) -> Result<(DynamicImage, serde_json::Value), MediaError> {
    use windows::Win32::Graphics::Imaging::{
        IWICDevelopRaw, WICAsShotParameterSet, WICRawRenderModeBestQuality,
    };
    let _apartment = ComApartment::enter();
    let factory = factory()?;
    let wide = wide_path(path);
    let decoder = unsafe {
        factory.CreateDecoderFromFilename(
            PCWSTR(wide.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )
    }
    .map_err(|error| wic_stage_error("open RAW decoder", error))?;
    let info = unsafe { decoder.GetDecoderInfo() }.map_err(wic_error)?;
    let mut name = vec![0u16; 256];
    let mut length = 0;
    unsafe { info.GetFriendlyName(&mut name, &mut length) }.map_err(wic_error)?;
    let name = String::from_utf16_lossy(&name[..length.saturating_sub(1) as usize]);
    let clsid = unsafe { info.GetCLSID() }.map_err(wic_error)?;
    let frame = unsafe { decoder.GetFrame(0) }.map_err(wic_error)?;
    if !require_develop {
        let orientation = benchmark_metadata_u32(&frame, "/ifd/{ushort=274}");
        let color_space = benchmark_metadata_u32(&frame, "/ifd/exif/{ushort=40961}");
        let mut contexts = [Some(
            unsafe { factory.CreateColorContext() }.map_err(wic_error)?,
        )];
        let mut count = 0;
        let context_result = unsafe { frame.GetColorContexts(&mut contexts, &mut count) };
        let context = contexts[0].as_ref().unwrap();
        let context_exif = if context_result.is_ok() && count > 0 {
            unsafe { context.GetExifColorSpace() }.ok()
        } else {
            None
        };
        let converter = unsafe { factory.CreateFormatConverter() }.map_err(wic_error)?;
        unsafe {
            converter.Initialize(
                &frame,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
        }
        .map_err(wic_error)?;
        let format = unsafe { frame.GetPixelFormat() }.map_err(wic_error)?;
        let mut image = if format == windows::Win32::Graphics::Imaging::GUID_WICPixelFormat24bppRGB
        {
            let mut width = 0;
            let mut height = 0;
            unsafe { frame.GetSize(&mut width, &mut height) }.map_err(wic_error)?;
            let stride = width
                .checked_mul(3)
                .ok_or_else(|| native_error("RGB stride overflow"))?;
            let size = (stride as usize)
                .checked_mul(height as usize)
                .ok_or_else(|| native_error("RGB size overflow"))?;
            let mut pixels = vec![0; size];
            unsafe { frame.CopyPixels(ptr::null(), stride, &mut pixels) }.map_err(wic_error)?;
            DynamicImage::ImageRgb8(
                image::RgbImage::from_raw(width, height, pixels)
                    .ok_or_else(|| native_error("invalid RGB size"))?,
            )
        } else {
            copy_rgba8(&converter)?
        };
        if let Some(orientation) =
            orientation.and_then(|value| image::metadata::Orientation::from_exif(value as u8))
        {
            image.apply_orientation(orientation);
        }
        return Ok((
            image,
            serde_json::json!({
                "decoder": name, "clsid": format!("{clsid:?}"),
                "developRaw": frame.cast::<IWICDevelopRaw>().is_ok(),
                "nativeFull": "unverified", "color": "unverified", "orientation": orientation,
                "exifColorSpace": color_space, "contextExifColorSpace": context_exif,
                "colorContextCount": count, "colorContextError": context_result.err().map(|error| error.to_string()),
                "nativePixelFormat": format!("{format:?}"),
            }),
        ));
    }
    let raw: IWICDevelopRaw = frame.cast().map_err(|error| {
        native_error(format!(
            "{name} ({clsid:?}) does not expose IWICDevelopRaw: {error}"
        ))
    })?;
    unsafe { raw.LoadParameterSet(WICAsShotParameterSet) }
        .map_err(|error| wic_stage_error("load as-shot RAW settings", error))?;
    unsafe { raw.SetRenderMode(WICRawRenderModeBestQuality) }
        .map_err(|error| wic_stage_error("request best-quality RAW rendering", error))?;
    let srgb = unsafe { factory.CreateColorContext() }.map_err(wic_error)?;
    unsafe { srgb.InitializeFromExifColorSpace(1) }.map_err(wic_error)?;
    unsafe { raw.SetDestinationColorContext(&srgb) }
        .map_err(|error| wic_stage_error("request sRGB RAW output", error))?;
    let converter = unsafe { factory.CreateFormatConverter() }.map_err(wic_error)?;
    unsafe {
        converter.Initialize(
            &raw,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
    }
    .map_err(wic_error)?;
    let image = copy_rgba8(&converter)?;
    Ok((
        image,
        serde_json::json!({
            "decoder": name, "clsid": format!("{clsid:?}"),
            "renderMode": "BestQuality", "color": "sRGB", "parameters": "AsShot",
        }),
    ))
}

#[cfg(feature = "bench-tools")]
fn benchmark_metadata_u32(
    frame: &windows::Win32::Graphics::Imaging::IWICBitmapFrameDecode,
    query: &str,
) -> Option<u32> {
    use windows::Win32::System::Com::StructuredStorage::{
        PROPVARIANT, PropVariantClear, PropVariantToUInt32,
    };
    let reader = unsafe { frame.GetMetadataQueryReader() }.ok()?;
    let query = query.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut value = PROPVARIANT::default();
    let result = unsafe { reader.GetMetadataByName(PCWSTR(query.as_ptr()), &mut value) };
    let number = result
        .and_then(|()| unsafe { PropVariantToUInt32(&value) })
        .ok();
    let _ = unsafe { PropVariantClear(&mut value) };
    number
}

pub fn can_decode(path: &Path) -> Result<(), MediaError> {
    let _apartment = ComApartment::enter();
    let factory = factory()?;
    open_converted_frame(&factory, path).map(|_| ())
}

fn open_converted_frame(
    factory: &IWICImagingFactory,
    path: &Path,
) -> Result<
    (
        windows::Win32::Graphics::Imaging::IWICBitmapFrameDecode,
        windows::Win32::Graphics::Imaging::IWICFormatConverter,
    ),
    MediaError,
> {
    let wide_path = wide_path(path);
    let decoder = unsafe {
        factory.CreateDecoderFromFilename(
            PCWSTR(wide_path.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )
    }
    .map_err(|error| wic_stage_error("open file decoder", error))?;
    let frame = unsafe { decoder.GetFrame(0) }
        .map_err(|error| wic_stage_error("open primary frame", error))?;
    let converter = unsafe { factory.CreateFormatConverter() }
        .map_err(|error| wic_stage_error("create RGBA8 converter", error))?;
    unsafe {
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
    }
    .map_err(|error| wic_stage_error("initialize RGBA8 converter", error))?;
    Ok((frame, converter))
}

fn factory() -> Result<IWICImagingFactory, MediaError> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory2, None, CLSCTX_INPROC_SERVER) }
        .map_err(wic_error)
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn wic_error(error: windows::core::Error) -> MediaError {
    native_error(error.to_string())
}

fn wic_stage_error(stage: &str, error: windows::core::Error) -> MediaError {
    native_error(format!("{stage}: {error}"))
}

fn native_error(message: impl Into<String>) -> MediaError {
    MediaError::NativeDecode {
        backend: "Windows WIC",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_fixture_when_heif_codec_is_installed() {
        if let Err(error) = capability() {
            eprintln!("skipping WIC HEIF decode test: {error}");
            return;
        }
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        if let Err(error) = can_decode(&fixture) {
            eprintln!("skipping unsupported WIC HEIF fixture: {error}");
            return;
        }
        let image = decode_full_rgba8(&fixture).expect("WIC probe accepted fixture");
        assert!(image.width() > 4_000);
        assert!(image.height() > 4_000);
    }
}
