use image::{
    DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage, RgbaImage, imageops::FilterType,
};
mod embedded;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::ffi::CString;
use std::{
    ffi::{CStr, c_char, c_int, c_uint, c_void},
    io::Cursor,
    path::Path,
    slice,
};

const LIBRAW_OPTIONS_NO_DATAERR_CALLBACK: c_uint = 1 << 1;
const LIBRAW_SUCCESS: c_int = 0;
const LIBRAW_IMAGE_JPEG: c_int = 1;
const LIBRAW_IMAGE_BITMAP: c_int = 2;

pub enum Preview {
    EmbeddedJpeg(Vec<u8>),
    EmbeddedImage(crate::media_source::DecodedImage),
}

#[repr(C)]
struct LibRawData {
    _private: [u8; 0],
}

#[repr(C)]
struct LibRawProcessedImage {
    image_type: c_int,
    height: u16,
    width: u16,
    colors: u16,
    bits: u16,
    data_size: u32,
    data: [u8; 1],
}

unsafe extern "C" {
    fn libraw_init(flags: c_uint) -> *mut LibRawData;
    fn libraw_close(raw: *mut LibRawData);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn libraw_open_file(raw: *mut LibRawData, path: *const c_char) -> c_int;
    #[cfg(target_os = "windows")]
    fn libraw_open_wfile(raw: *mut LibRawData, path: *const u16) -> c_int;
    fn libraw_unpack(raw: *mut LibRawData) -> c_int;
    fn libraw_adjust_sizes_info_only(raw: *mut LibRawData) -> c_int;
    fn libraw_dcraw_process(raw: *mut LibRawData) -> c_int;
    fn libraw_set_progress_handler(
        raw: *mut LibRawData,
        callback: extern "C" fn(*mut c_void, c_int, c_int, c_int) -> c_int,
        context: *mut c_void,
    );
    fn libraw_dcraw_make_mem_image(
        raw: *mut LibRawData,
        error: *mut c_int,
    ) -> *mut LibRawProcessedImage;
    fn libraw_dcraw_make_mem_thumb(
        raw: *mut LibRawData,
        error: *mut c_int,
    ) -> *mut LibRawProcessedImage;
    fn libraw_dcraw_clear_mem(image: *mut LibRawProcessedImage);
    fn libraw_get_iheight(raw: *mut LibRawData) -> c_int;
    fn libraw_get_iwidth(raw: *mut LibRawData) -> c_int;
    fn libraw_strerror(error: c_int) -> *const c_char;
    fn oxy_libraw_configure_full(raw: *mut LibRawData);
    fn oxy_libraw_configure_preview(raw: *mut LibRawData);
    fn oxy_libraw_unpack_sized_thumb(raw: *mut LibRawData, target_size: c_uint) -> c_int;
}

pub fn dimensions(path: &Path) -> Result<crate::ImageDimensions, String> {
    Processor::open(path)?.dimensions()
}

pub fn embedded(
    path: &Path,
    max_size: u32,
) -> Result<(Preview, oxy_domain::DisplayDimensions), String> {
    let (image, reference) = embedded_preview(path, max_size)?;
    if image.image_type() == LIBRAW_IMAGE_JPEG {
        Ok((Preview::EmbeddedJpeg(image.data().to_vec()), reference))
    } else {
        if max_size == 0 {
            return Err("largest embedded preview is not a JPEG".into());
        }
        use sha2::{Digest, Sha256};
        let candidate_id = format!("libraw-bitmap:{:x}", Sha256::digest(image.data()));
        let image = image.decode()?;
        let source = oxy_domain::DisplayDimensions((image.width(), image.height()).into());
        let mut facts = crate::media_source::source_facts(
            oxy_domain::ImageOrigin::EmbeddedPreview,
            candidate_id,
            oxy_domain::EncodedDimensions(source.0),
            1,
            source,
        );
        facts.source.encoded_dimensions = None;
        let image = fit(image, max_size);
        facts.resize(oxy_domain::DisplayDimensions(
            (image.width(), image.height()).into(),
        ));
        facts.encoded_dimensions = oxy_domain::EncodedDimensions(facts.display_dimensions.0);
        Ok((
            Preview::EmbeddedImage(crate::media_source::DecodedImage { image, facts }),
            reference,
        ))
    }
}

pub fn developed(
    path: &Path,
    max_size: Option<u32>,
    cancellation: &oxy_runtime::CancellationToken,
) -> Result<crate::media_source::DecodedImage, String> {
    let (image, source_dimensions) = developed_preview(path, max_size.is_none(), cancellation)?;
    let developed = oxy_domain::DisplayDimensions((image.width(), image.height()).into());
    // Full development establishes the actual active-area canvas. Header estimates
    // may differ slightly and must not be reported as a resize that never happened.
    let reference = if max_size.is_none() {
        developed
    } else {
        source_dimensions
    };
    let mut facts = crate::media_source::source_facts(
        oxy_domain::ImageOrigin::RawSensor,
        "raw-sensor".into(),
        oxy_domain::EncodedDimensions(reference.0),
        1,
        reference,
    );
    facts.source.encoded_dimensions = None;
    facts.processing.push(oxy_domain::ImageOperation::Develop {
        backend: "LibRaw".into(),
    });
    facts
        .processing
        .push(oxy_domain::ImageOperation::ColorConvert {
            target: "sRGB".into(),
        });
    facts.display_dimensions = developed;
    facts.detail.sampled_dimensions = developed;
    if max_size.is_some() {
        facts.detail.sampling = oxy_domain::Sampling::Reduced;
    }
    let image = match max_size {
        Some(size) => fit(image, size),
        None => image,
    };
    facts.resize(oxy_domain::DisplayDimensions(
        (image.width(), image.height()).into(),
    ));
    facts.encoded_dimensions = oxy_domain::EncodedDimensions(facts.display_dimensions.0);
    Ok(crate::media_source::DecodedImage { image, facts })
}

fn embedded_preview(
    path: &Path,
    max_size: u32,
) -> Result<(ProcessedImage, oxy_domain::DisplayDimensions), String> {
    let raw = Processor::open(path)?;
    let reference = oxy_domain::DisplayDimensions(raw.dimensions()?);
    embedded::resolve_unknown_dimensions(path, &raw)?;
    check(unsafe { oxy_libraw_unpack_sized_thumb(raw.inner, max_size) })?;
    Ok((ProcessedImage::thumbnail(&raw)?, reference))
}

extern "C" fn development_cancelled(
    context: *mut c_void,
    _stage: c_int,
    _iteration: c_int,
    _expected: c_int,
) -> c_int {
    // LibRaw invokes this synchronously; the borrowed token outlives Processor.
    let cancellation = unsafe { &*context.cast::<oxy_runtime::CancellationToken>() };
    c_int::from(cancellation.is_cancelled())
}

fn developed_preview(
    path: &Path,
    full: bool,
    cancellation: &oxy_runtime::CancellationToken,
) -> Result<(DynamicImage, oxy_domain::DisplayDimensions), String> {
    if cancellation.is_cancelled() {
        return Err("RAW development cancelled".into());
    }
    let raw = Processor::open_cancellable(path, Some(cancellation))?;
    unsafe {
        if full {
            oxy_libraw_configure_full(raw.inner);
        } else {
            oxy_libraw_configure_preview(raw.inner);
        }
    };
    check(unsafe { libraw_unpack(raw.inner) })?;
    check(unsafe { libraw_dcraw_process(raw.inner) })?;
    let image = ProcessedImage::developed(&raw)?;
    let image = image.decode()?;
    // adjust_sizes_info_only advances LibRaw's processing state and cannot run
    // on the instance that will subsequently unpack/develop sensor data.
    let reference = if full {
        oxy_domain::DisplayDimensions((image.width(), image.height()).into())
    } else {
        oxy_domain::DisplayDimensions(dimensions(path)?)
    };
    Ok((image, reference))
}

fn fit(image: DynamicImage, max_size: u32) -> DynamicImage {
    if image.width() <= max_size && image.height() <= max_size {
        image
    } else {
        image.resize(max_size, max_size, FilterType::Triangle)
    }
}

struct Processor<'c> {
    inner: *mut LibRawData,
    _cancellation: std::marker::PhantomData<&'c oxy_runtime::CancellationToken>,
}

impl<'c> Processor<'c> {
    fn dimensions(&self) -> Result<crate::ImageDimensions, String> {
        check(unsafe { libraw_adjust_sizes_info_only(self.inner) })?;
        let width = unsafe { libraw_get_iwidth(self.inner) };
        let height = unsafe { libraw_get_iheight(self.inner) };
        if width <= 0 || height <= 0 {
            return Err(format!("invalid dimensions {width}x{height}"));
        }
        Ok(crate::ImageDimensions {
            width: width as u32,
            height: height as u32,
        })
    }

    fn open(path: &Path) -> Result<Self, String> {
        Self::open_cancellable(path, None)
    }

    fn open_cancellable(
        path: &Path,
        cancellation: Option<&'c oxy_runtime::CancellationToken>,
    ) -> Result<Self, String> {
        let inner = unsafe { libraw_init(LIBRAW_OPTIONS_NO_DATAERR_CALLBACK) };
        if inner.is_null() {
            return Err("initialization failed".into());
        }
        let processor = Self {
            inner,
            _cancellation: std::marker::PhantomData,
        };
        if let Some(cancellation) = cancellation {
            // LibRaw only invokes this callback synchronously during operations.
            // Callers keep the borrowed token alive through Processor::drop.
            unsafe {
                libraw_set_progress_handler(
                    inner,
                    development_cancelled,
                    std::ptr::from_ref(cancellation).cast_mut().cast(),
                );
            }
        }
        processor.open_path(path)?;
        Ok(processor)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn open_path(&self, path: &Path) -> Result<(), String> {
        use std::os::unix::ffi::OsStrExt;

        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "path contains a null byte".to_owned())?;
        check(unsafe { libraw_open_file(self.inner, path.as_ptr()) })
    }

    #[cfg(target_os = "windows")]
    fn open_path(&self, path: &Path) -> Result<(), String> {
        use std::os::windows::ffi::OsStrExt;

        let path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        check(unsafe { libraw_open_wfile(self.inner, path.as_ptr()) })
    }
}

impl Drop for Processor<'_> {
    fn drop(&mut self) {
        unsafe { libraw_close(self.inner) };
    }
}

struct ProcessedImage {
    inner: *mut LibRawProcessedImage,
}

impl ProcessedImage {
    fn thumbnail(raw: &Processor<'_>) -> Result<Self, String> {
        Self::make(|error| unsafe { libraw_dcraw_make_mem_thumb(raw.inner, error) })
    }

    fn developed(raw: &Processor<'_>) -> Result<Self, String> {
        Self::make(|error| unsafe { libraw_dcraw_make_mem_image(raw.inner, error) })
    }

    fn make(make: impl FnOnce(*mut c_int) -> *mut LibRawProcessedImage) -> Result<Self, String> {
        let mut error = LIBRAW_SUCCESS;
        let inner = make(&mut error);
        check(error)?;
        if inner.is_null() {
            return Err("LibRaw returned an empty image".into());
        }
        Ok(Self { inner })
    }

    fn decode(&self) -> Result<DynamicImage, String> {
        let image = unsafe { &*self.inner };
        let data = self.data();
        match self.image_type() {
            LIBRAW_IMAGE_JPEG => decode_jpeg(data),
            LIBRAW_IMAGE_BITMAP => decode_bitmap(image, data),
            image_type => Err(format!("unsupported LibRaw image type {image_type}")),
        }
    }

    fn image_type(&self) -> c_int {
        unsafe { (*self.inner).image_type }
    }

    fn data(&self) -> &[u8] {
        let image = unsafe { &*self.inner };
        unsafe { slice::from_raw_parts(image.data.as_ptr(), image.data_size as usize) }
    }
}

impl Drop for ProcessedImage {
    fn drop(&mut self) {
        unsafe { libraw_dcraw_clear_mem(self.inner) };
    }
}

fn decode_jpeg(data: &[u8]) -> Result<DynamicImage, String> {
    let reader = ImageReader::with_format(Cursor::new(data), ImageFormat::Jpeg);
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn decode_bitmap(image: &LibRawProcessedImage, data: &[u8]) -> Result<DynamicImage, String> {
    if image.bits != 8 {
        return Err(format!("unsupported bitmap bit depth {}", image.bits));
    }
    match image.colors {
        3 => RgbImage::from_raw(image.width.into(), image.height.into(), data.to_vec())
            .map(DynamicImage::ImageRgb8)
            .ok_or_else(|| "invalid RGB bitmap length".into()),
        4 => RgbaImage::from_raw(image.width.into(), image.height.into(), data.to_vec())
            .map(DynamicImage::ImageRgba8)
            .ok_or_else(|| "invalid RGBA bitmap length".into()),
        colors => Err(format!("unsupported bitmap channel count {colors}")),
    }
}

fn check(code: c_int) -> Result<(), String> {
    if code == LIBRAW_SUCCESS {
        return Ok(());
    }
    let message = unsafe {
        let pointer = libraw_strerror(code);
        (!pointer.is_null()).then(|| CStr::from_ptr(pointer).to_string_lossy().into_owned())
    }
    .unwrap_or_else(|| "unknown error".into());
    Err(format!("{message} ({code})"))
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[test]
    fn progress_callback_observes_cancellation_and_prevents_new_development() {
        let token = oxy_runtime::CancellationToken::default();
        let context = std::ptr::from_ref(&token).cast_mut().cast();
        assert_eq!(development_cancelled(context, 0, 0, 1), 0);
        token.cancel();
        assert_eq!(development_cancelled(context, 0, 0, 1), 1);
        assert_eq!(
            developed(Path::new("does-not-exist.arw"), None, &token).unwrap_err(),
            "RAW development cancelled"
        );
    }
}
