use crate::{HeifDecodePriority, MediaError, heif};
use image::{DynamicImage, RgbaImage};
use oxy_domain::{
    AccelerationKind, HeifBackendKind, HeifCapabilities, HeifDecodeRequest, HeifDecodeSession,
    HeifDecodeStatus, HeifDiagnostics, HeifStatusEvent, HeifTileReady,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

#[cfg(target_os = "windows")]
pub const DEFAULT_TILE_SIZE: u32 = 1_024;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub const DEFAULT_TILE_SIZE: u32 = 512;
pub type TileSink = Box<dyn FnMut(HeifTileReady) + Send>;

pub trait HeifBackend: Send + Sync {
    fn capabilities(&self) -> HeifCapabilities;
    fn start_decode(
        &self,
        request: HeifDecodeRequest,
        sink: TileSink,
    ) -> Result<HeifDecodeSession, MediaError>;
}

#[derive(Debug, Clone)]
pub struct HeifTile {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub rgba: Arc<[u8]>,
    pub encoded_jpeg: Option<Arc<[u8]>>,
}

#[derive(Default)]
struct ServiceState {
    active: Option<ActiveSession>,
    tiles: HashMap<(String, u64, u32, u32), HeifTile>,
    diagnostics: Option<HeifDiagnostics>,
}

struct ActiveSession {
    id: String,
    generation: u64,
    display_sharpening: bool,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct HeifDecodeService {
    next_session: AtomicU64,
    state: Mutex<ServiceState>,
}

impl HeifDecodeService {
    pub fn capabilities(&self) -> Vec<HeifCapabilities> {
        vec![
            platform_capability(),
            ffmpeg_capability(),
            libheif_capability(),
        ]
    }

    pub fn begin(
        &self,
        path: &Path,
        generation: u64,
        hardware_acceleration: bool,
        display_sharpening: bool,
    ) -> Result<HeifDecodeSession, MediaError> {
        let size = heif::dimensions(path)?;
        let id = format!(
            "heif-{}",
            self.next_session.fetch_add(1, Ordering::Relaxed) + 1
        );
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(active) = state.active.take() {
            active.cancelled.store(true, Ordering::Release);
        }
        state.tiles.clear();
        state.active = Some(ActiveSession {
            id: id.clone(),
            generation,
            display_sharpening,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        let platform = platform_capability();
        #[cfg(target_os = "windows")]
        let use_ffmpeg = crate::ffmpeg_heif::can_decode(path).is_ok();
        #[cfg(target_os = "windows")]
        let use_platform = !use_ffmpeg
            && hardware_acceleration
            && platform.available
            && crate::windows_wic::can_decode(path).is_ok();
        #[cfg(target_os = "macos")]
        let use_platform = hardware_acceleration
            && platform.available
            && crate::apple_image_io::can_decode(path).is_ok();
        #[cfg(target_os = "linux")]
        let use_platform = hardware_acceleration && platform.available;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let use_ffmpeg = !use_platform && crate::ffmpeg_heif::can_decode(path).is_ok();
        let fallback = hardware_acceleration && !use_platform;
        let backend = if use_platform {
            platform.backend
        } else if use_ffmpeg {
            HeifBackendKind::FfmpegSoftware
        } else {
            HeifBackendKind::LibheifSoftware
        };
        #[cfg(target_os = "windows")]
        let expected_tiles = if backend == HeifBackendKind::FfmpegSoftware {
            crate::ffmpeg_heif::tile_count(path).unwrap_or_else(|_| {
                tile_coordinates(size.width, size.height, DEFAULT_TILE_SIZE).len()
            })
        } else {
            tile_coordinates(size.width, size.height, DEFAULT_TILE_SIZE).len()
        } as u32;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let expected_tiles =
            tile_coordinates(size.width, size.height, DEFAULT_TILE_SIZE).len() as u32;
        Ok(HeifDecodeSession {
            id,
            generation,
            width: size.width,
            height: size.height,
            tile_size: DEFAULT_TILE_SIZE,
            expected_tiles,
            backend,
            acceleration: if use_platform {
                platform.acceleration
            } else {
                AccelerationKind::Software
            },
            status: if fallback {
                HeifDecodeStatus::CompatibilityFallback
            } else {
                HeifDecodeStatus::Decoding
            },
        })
    }

    pub fn decode<F>(
        &self,
        session: &HeifDecodeSession,
        path: PathBuf,
        mut publish: F,
    ) -> Result<HeifDiagnostics, MediaError>
    where
        F: FnMut(HeifTileReady),
    {
        let started = Instant::now();
        let (cancelled, display_sharpening) = {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let Some(active) = &state.active else {
                return Err(MediaError::Cancelled);
            };
            if active.id != session.id || active.generation != session.generation {
                return Err(MediaError::Cancelled);
            }
            (active.cancelled.clone(), active.display_sharpening)
        };
        let _decode_permit = crate::acquire_heif_decode(HeifDecodePriority::Foreground);
        let queue_wait_ms = elapsed_ms(started);
        if cancelled.load(Ordering::Acquire) {
            return Err(MediaError::Cancelled);
        }
        let decode_started = Instant::now();
        #[cfg(target_os = "windows")]
        if session.backend == HeifBackendKind::FfmpegSoftware
            && let Ok(tiles) = crate::ffmpeg_heif::decode_full_jpeg_tiles(
                &path,
                crate::ImageDimensions {
                    width: session.width,
                    height: session.height,
                },
                display_sharpening,
            )
        {
            let decode_ms = elapsed_ms(decode_started);
            let tile_started = Instant::now();
            for tile in tiles {
                if cancelled.load(Ordering::Acquire) {
                    return Err(MediaError::Cancelled);
                }
                let event = HeifTileReady {
                    session_id: session.id.clone(),
                    generation: session.generation,
                    x: tile.x,
                    y: tile.y,
                    width: tile.width,
                    height: tile.height,
                    encoding: Some("jpeg".into()),
                    url: format!(
                        "oxy-media://localhost/tile/{}/{}/{}/{}",
                        session.id, session.generation, tile.x, tile.y
                    ),
                };
                self.state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .tiles
                    .insert(
                        (session.id.clone(), session.generation, tile.x, tile.y),
                        HeifTile {
                            width: tile.width,
                            height: tile.height,
                            stride: tile.width * 4,
                            rgba: Arc::from([]),
                            encoded_jpeg: Some(Arc::from(tile.jpeg)),
                        },
                    );
                publish(event);
            }
            let diagnostics = HeifDiagnostics {
                backend: HeifBackendKind::FfmpegSoftware,
                acceleration: AccelerationKind::Software,
                codec: Some("FFmpeg HEVC tile-grid".into()),
                queue_wait_ms,
                decode_ms,
                tile_publish_ms: elapsed_ms(tile_started),
                total_ms: elapsed_ms(started),
                fallback_reason: (session.status == HeifDecodeStatus::CompatibilityFallback)
                    .then(|| fallback_reason(&path)),
            };
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .diagnostics = Some(diagnostics.clone());
            return Ok(diagnostics);
        }
        let (image, backend, acceleration, codec, fallback_reason) =
            decode_for_session(session, &path, display_sharpening)?;
        // Native adapters already return RGBA8. Compatibility adapters may
        // return RGB8, so normalize once here instead of asking every tile
        // crop to repeat dynamic-image conversion work.
        let image = image.into_rgba8();
        #[cfg(target_os = "macos")]
        let mut image = image;
        let decode_ms = elapsed_ms(decode_started);
        let tile_started = Instant::now();
        #[cfg(target_os = "macos")]
        let per_tile_sharpening = if display_sharpening {
            crate::apple_image_io::sharpen_rgba8(&mut image)?;
            false
        } else {
            false
        };
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let per_tile_sharpening = display_sharpening && backend != HeifBackendKind::FfmpegSoftware;
        for (x, y) in tile_coordinates(image.width(), image.height(), session.tile_size) {
            if cancelled.load(Ordering::Acquire) {
                return Err(MediaError::Cancelled);
            }
            let tile = crop_rgba(&image, x, y, session.tile_size, per_tile_sharpening);
            let event = HeifTileReady {
                session_id: session.id.clone(),
                generation: session.generation,
                x,
                y,
                width: tile.width,
                height: tile.height,
                encoding: None,
                url: format!(
                    "oxy-media://localhost/tile/{}/{}/{}/{}",
                    session.id, session.generation, x, y
                ),
            };
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .tiles
                .insert((session.id.clone(), session.generation, x, y), tile);
            publish(event);
        }
        let diagnostics = HeifDiagnostics {
            backend,
            acceleration,
            codec: Some(codec.into()),
            queue_wait_ms,
            decode_ms,
            tile_publish_ms: elapsed_ms(tile_started),
            total_ms: elapsed_ms(started),
            fallback_reason,
        };
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .diagnostics = Some(diagnostics.clone());
        Ok(diagnostics)
    }

    pub fn cancel(&self, session_id: &str) -> bool {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let Some(active) = &state.active else {
            return false;
        };
        if active.id != session_id {
            return false;
        }
        active.cancelled.store(true, Ordering::Release);
        true
    }

    pub fn tile(&self, session: &str, generation: u64, x: u32, y: u32) -> Option<HeifTile> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .tiles
            .get(&(session.to_owned(), generation, x, y))
            .cloned()
    }

    pub fn diagnostics(&self) -> Option<HeifDiagnostics> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .diagnostics
            .clone()
    }

    pub fn status_event(
        session: &HeifDecodeSession,
        status: HeifDecodeStatus,
        diagnostics: Option<HeifDiagnostics>,
        message: Option<String>,
    ) -> HeifStatusEvent {
        HeifStatusEvent {
            session_id: session.id.clone(),
            generation: session.generation,
            status,
            diagnostics,
            message,
        }
    }
}

fn crop_rgba(
    image: &RgbaImage,
    x: u32,
    y: u32,
    tile_size: u32,
    display_sharpening: bool,
) -> HeifTile {
    let width = tile_size.min(image.width() - x);
    let height = tile_size.min(image.height() - y);
    let row_bytes = width as usize * 4;
    let source_stride = image.width() as usize * 4;
    let source = image.as_raw();
    let mut rgba = Vec::with_capacity(row_bytes * height as usize);
    if !display_sharpening {
        for row in y..(y + height) {
            let start = row as usize * source_stride + x as usize * 4;
            rgba.extend_from_slice(&source[start..start + row_bytes]);
        }
    } else {
        for source_y in y..(y + height) {
            let up = source_y.saturating_sub(1);
            let down = (source_y + 1).min(image.height() - 1);
            for source_x in x..(x + width) {
                let left = source_x.saturating_sub(1);
                let right = (source_x + 1).min(image.width() - 1);
                let center_index = source_y as usize * source_stride + source_x as usize * 4;
                let laplacian = 4 * pixel_luma(source, center_index)
                    - pixel_luma(
                        source,
                        source_y as usize * source_stride + left as usize * 4,
                    )
                    - pixel_luma(
                        source,
                        source_y as usize * source_stride + right as usize * 4,
                    )
                    - pixel_luma(source, up as usize * source_stride + source_x as usize * 4)
                    - pixel_luma(
                        source,
                        down as usize * source_stride + source_x as usize * 4,
                    );
                // A one-fifth luma unsharp mask gives Sony-like edge definition
                // without color halos. This affects only transient display tiles.
                let delta = laplacian / 5;
                rgba.extend_from_slice(&[
                    (source[center_index] as i32 + delta).clamp(0, 255) as u8,
                    (source[center_index + 1] as i32 + delta).clamp(0, 255) as u8,
                    (source[center_index + 2] as i32 + delta).clamp(0, 255) as u8,
                    source[center_index + 3],
                ]);
            }
        }
    }
    HeifTile {
        width,
        height,
        stride: width * 4,
        rgba: Arc::from(rgba),
        encoded_jpeg: None,
    }
}

#[inline]
fn pixel_luma(source: &[u8], index: usize) -> i32 {
    (2 * source[index] as i32 + 5 * source[index + 1] as i32 + source[index + 2] as i32) / 8
}

fn tile_coordinates(width: u32, height: u32, tile_size: u32) -> Vec<(u32, u32)> {
    let mut result = Vec::new();
    for y in (0..height).step_by(tile_size as usize) {
        for x in (0..width).step_by(tile_size as usize) {
            result.push((x, y));
        }
    }
    let center = (width as i64 / 2, height as i64 / 2);
    result.sort_by_key(|(x, y)| {
        let tile_center = (
            i64::from(*x + tile_size.min(width - *x) / 2),
            i64::from(*y + tile_size.min(height - *y) / 2),
        );
        (tile_center.0 - center.0).pow(2) + (tile_center.1 - center.1).pow(2)
    });
    result
}

fn ffmpeg_capability() -> HeifCapabilities {
    match crate::ffmpeg_heif::capability() {
        Ok(()) => HeifCapabilities {
            backend: HeifBackendKind::FfmpegSoftware,
            acceleration: AccelerationKind::Software,
            available: true,
            detail: Some("FFmpeg HEIF tile-grid decoder".into()),
        },
        Err(error) => HeifCapabilities {
            backend: HeifBackendKind::FfmpegSoftware,
            acceleration: AccelerationKind::Software,
            available: false,
            detail: Some(error.to_string()),
        },
    }
}

fn libheif_capability() -> HeifCapabilities {
    HeifCapabilities {
        backend: HeifBackendKind::LibheifSoftware,
        acceleration: AccelerationKind::Software,
        available: true,
        detail: Some("portable compatibility backend".into()),
    }
}

fn platform_capability() -> HeifCapabilities {
    #[cfg(target_os = "windows")]
    return match crate::windows_wic::capability() {
        Ok(()) => HeifCapabilities {
            backend: HeifBackendKind::WindowsWic,
            acceleration: AccelerationKind::Unknown,
            available: true,
            detail: Some(
                "Windows WIC HEIF decoder installed; GPU acceleration is not verified".into(),
            ),
        },
        Err(error) => HeifCapabilities {
            backend: HeifBackendKind::WindowsWic,
            acceleration: AccelerationKind::Unknown,
            available: false,
            detail: Some(error.to_string()),
        },
    };
    #[cfg(target_os = "macos")]
    return match crate::apple_image_io::capability() {
        Ok(()) => HeifCapabilities {
            backend: HeifBackendKind::AppleImageIo,
            acceleration: AccelerationKind::Unknown,
            available: true,
            detail: Some(
                "Apple ImageIO native HEIF decoder; hardware use is selected internally and cannot be verified".into(),
            ),
        },
        Err(error) => HeifCapabilities {
            backend: HeifBackendKind::AppleImageIo,
            acceleration: AccelerationKind::Unknown,
            available: false,
            detail: Some(error.to_string()),
        },
    };
    #[cfg(target_os = "linux")]
    {
        HeifCapabilities {
            backend: HeifBackendKind::LinuxVaapi,
            acceleration: AccelerationKind::Hardware,
            available: false,
            detail: Some("native hardware adapter is not available in this build".into()),
        }
    }
}

fn decode_for_session(
    session: &HeifDecodeSession,
    path: &Path,
    display_sharpening: bool,
) -> Result<
    (
        DynamicImage,
        HeifBackendKind,
        AccelerationKind,
        &'static str,
        Option<String>,
    ),
    MediaError,
> {
    #[cfg(target_os = "windows")]
    if session.backend == HeifBackendKind::WindowsWic {
        match crate::windows_wic::decode_full_rgba8(path) {
            Ok(image) => {
                return Ok((
                    image,
                    HeifBackendKind::WindowsWic,
                    AccelerationKind::Unknown,
                    "Windows WIC HEIF decoder",
                    None,
                ));
            }
            Err(error) => {
                return decode_ffmpeg_or_libheif(
                    path,
                    session,
                    Some(error.to_string()),
                    display_sharpening,
                );
            }
        }
    }
    #[cfg(target_os = "macos")]
    if session.backend == HeifBackendKind::AppleImageIo {
        match crate::apple_image_io::decode_rgba8(path, session.width.max(session.height)) {
            Ok(image) => {
                return Ok((
                    image,
                    HeifBackendKind::AppleImageIo,
                    AccelerationKind::Unknown,
                    "Apple ImageIO HEIF decoder",
                    None,
                ));
            }
            Err(error) => {
                return decode_ffmpeg_or_libheif(
                    path,
                    session,
                    Some(error.to_string()),
                    display_sharpening,
                );
            }
        }
    }
    decode_ffmpeg_or_libheif(
        path,
        session,
        (session.status == HeifDecodeStatus::CompatibilityFallback).then(|| fallback_reason(path)),
        display_sharpening,
    )
}

fn decode_ffmpeg_or_libheif(
    path: &Path,
    session: &HeifDecodeSession,
    fallback_reason: Option<String>,
    display_sharpening: bool,
) -> Result<
    (
        DynamicImage,
        HeifBackendKind,
        AccelerationKind,
        &'static str,
        Option<String>,
    ),
    MediaError,
> {
    if session.backend == HeifBackendKind::FfmpegSoftware
        || crate::ffmpeg_heif::can_decode(path).is_ok()
    {
        match crate::ffmpeg_heif::decode_full_rgba8(
            path,
            crate::ImageDimensions {
                width: session.width,
                height: session.height,
            },
            display_sharpening,
        ) {
            Ok(image) => {
                return Ok((
                    image,
                    HeifBackendKind::FfmpegSoftware,
                    AccelerationKind::Software,
                    "FFmpeg HEVC tile-grid",
                    fallback_reason,
                ));
            }
            Err(error) => {
                let reason = append_fallback_reason(fallback_reason, error.to_string());
                return Ok((
                    heif::decode_full_rgb8(path)?,
                    HeifBackendKind::LibheifSoftware,
                    AccelerationKind::Software,
                    "libheif/libde265",
                    Some(reason),
                ));
            }
        }
    }
    Ok((
        heif::decode_full_rgb8(path)?,
        HeifBackendKind::LibheifSoftware,
        AccelerationKind::Software,
        "libheif/libde265",
        fallback_reason,
    ))
}

fn append_fallback_reason(existing: Option<String>, next: String) -> String {
    existing
        .map(|existing| format!("{existing}; {next}"))
        .unwrap_or(next)
}

fn fallback_reason(_path: &Path) -> String {
    #[cfg(target_os = "windows")]
    {
        let platform = platform_capability();
        if platform.available {
            return crate::windows_wic::can_decode(_path)
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "Windows WIC was not selected".into());
        }
        platform
            .detail
            .unwrap_or_else(|| "Windows WIC is unavailable".into())
    }
    #[cfg(target_os = "macos")]
    {
        let platform = platform_capability();
        if platform.available {
            return crate::apple_image_io::can_decode(_path)
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "Apple ImageIO was not selected".into());
        }
        platform
            .detail
            .unwrap_or_else(|| "Apple ImageIO is unavailable".into())
    }
    #[cfg(target_os = "linux")]
    {
        platform_capability()
            .detail
            .unwrap_or_else(|| "native HEIF decoder is unavailable".into())
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_order_starts_near_center_and_covers_edges() {
        let coordinates = tile_coordinates(1_200, 800, 512);
        assert_eq!(coordinates.len(), 6);
        assert!(coordinates[0].0 > 0);
        assert!(coordinates.contains(&(1_024, 512)));
    }

    #[test]
    fn default_tiles_preserve_the_platform_progressive_policy() {
        let coordinates = tile_coordinates(7_008, 4_672, DEFAULT_TILE_SIZE);
        #[cfg(target_os = "windows")]
        {
            assert_eq!(DEFAULT_TILE_SIZE, 1_024);
            assert_eq!(coordinates.len(), 35);
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            assert_eq!(DEFAULT_TILE_SIZE, 512);
            assert_eq!(coordinates.len(), 140);
        }
    }

    #[test]
    fn crop_produces_tightly_packed_edge_tile() {
        let image = DynamicImage::new_rgb8(600, 550).into_rgba8();
        let tile = crop_rgba(&image, 512, 512, 512, false);
        assert_eq!((tile.width, tile.height, tile.stride), (88, 38, 352));
        assert_eq!(tile.rgba.len(), 88 * 38 * 4);
    }

    #[test]
    fn display_sharpening_preserves_flat_pixels_and_alpha() {
        let image = RgbaImage::from_pixel(3, 3, image::Rgba([80, 120, 160, 77]));
        let tile = crop_rgba(&image, 0, 0, 3, true);
        assert_eq!(tile.rgba.as_ref(), image.as_raw());
    }

    #[test]
    fn display_sharpening_increases_edge_definition() {
        let mut image = RgbaImage::from_pixel(3, 3, image::Rgba([64, 64, 64, 255]));
        image.put_pixel(1, 1, image::Rgba([128, 128, 128, 255]));
        let tile = crop_rgba(&image, 0, 0, 3, true);
        let center = (3 + 1) * 4;
        assert!(tile.rgba[center] > 128);
        assert_eq!(tile.rgba[center + 3], 255);
    }

    #[test]
    fn selects_and_decodes_ffmpeg_tile_grid_fixture() {
        if crate::ffmpeg_heif::capability().is_err() {
            return;
        }
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let service = HeifDecodeService::default();
        let session = service.begin(&fixture, 1, false, false).unwrap();
        assert_eq!(session.backend, HeifBackendKind::FfmpegSoftware);
        let mut events = Vec::new();
        let diagnostics = service
            .decode(&session, fixture, |event| events.push(event))
            .unwrap();
        assert_eq!(diagnostics.backend, HeifBackendKind::FfmpegSoftware);
        assert_eq!(events.len(), session.expected_tiles as usize);
        let mut covered_pixels = 0_u64;
        for event in events {
            #[cfg(target_os = "windows")]
            assert_eq!(event.encoding.as_deref(), Some("jpeg"));
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            assert_eq!(event.encoding, None);
            assert!(event.x + event.width <= session.width);
            assert!(event.y + event.height <= session.height);
            let tile = service
                .tile(&session.id, session.generation, event.x, event.y)
                .unwrap();
            #[cfg(target_os = "windows")]
            {
                let decoded =
                    image::load_from_memory(tile.encoded_jpeg.as_deref().unwrap()).unwrap();
                assert_eq!(decoded.width(), event.width);
                assert_eq!(decoded.height(), event.height);
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                assert!(tile.encoded_jpeg.is_none());
                assert_eq!(tile.rgba.len(), (event.width * event.height * 4) as usize);
            }
            covered_pixels += u64::from(event.width) * u64::from(event.height);
        }
        assert_eq!(
            covered_pixels,
            u64::from(session.width) * u64::from(session.height)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn prefers_qualified_ffmpeg_tile_grid_over_slower_wic() {
        if crate::ffmpeg_heif::capability().is_err() {
            return;
        }
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let service = HeifDecodeService::default();
        let session = service.begin(&fixture, 1, true, true).unwrap();
        assert_eq!(session.backend, HeifBackendKind::FfmpegSoftware);
        assert_eq!(session.acceleration, AccelerationKind::Software);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn selects_and_decodes_apple_image_io_fixture() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        if crate::apple_image_io::can_decode(&fixture).is_err() {
            return;
        }
        let service = HeifDecodeService::default();
        let session = service.begin(&fixture, 1, true, true).unwrap();
        assert_eq!(session.backend, HeifBackendKind::AppleImageIo);
        assert_eq!(session.acceleration, AccelerationKind::Unknown);
        let mut tiles = 0;
        let diagnostics = service.decode(&session, fixture, |_| tiles += 1).unwrap();
        assert_eq!(diagnostics.backend, HeifBackendKind::AppleImageIo);
        assert_eq!(tiles, session.expected_tiles);
    }
}
