use crate::MediaError;
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
