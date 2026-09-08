use crate::{
    MediaError,
    backends::libheif,
    decode_control::{DecodePriority, acquire_decode, try_acquire_heif_session_cache_write},
    pipeline::heif::backend::{
        BackendExecutionError, HeifBackend as PlannedHeifBackend, HeifBackendPlan, HeifOperation,
        backend_plan, capabilities as backend_capabilities, execute_backend_plan,
        format_attempt_diagnostics,
    },
};
use image::{DynamicImage, RgbaImage};
use oxy_domain::{
    HeifBackendKind, HeifCapabilities, HeifDecodeSession, HeifDecodeStatus, HeifDiagnostics,
    HeifTilePayload,
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
#[derive(Debug, Clone)]
pub struct HeifTile {
    pub width: u32,
    pub height: u32,
    pub payload: HeifTileData,
}

#[derive(Debug, Clone)]
pub enum HeifTileData {
    Rgba { stride: u32, bytes: Arc<[u8]> },
    Jpeg(Arc<[u8]>),
}

#[derive(Debug, Clone)]
pub struct HeifTilePublication {
    pub session_id: String,
    pub generation: u64,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub payload: HeifTilePayload,
}

#[derive(Default)]
struct ServiceState {
    active: Option<ActiveSession>,
    tiles: HashMap<(String, u64, u32, u32), HeifTile>,
}

struct ActiveSession {
    id: String,
    generation: u64,
    path: PathBuf,
    decode_path: PathBuf,
    source_revision: crate::cache::SourceRevision,
    display_sharpening: bool,
    cancelled: Arc<AtomicBool>,
    backend_plan: HeifBackendPlan,
}

enum SessionDecode {
    Image {
        image: DynamicImage,
        codec: &'static str,
        presentation: crate::cache::ArtifactPresentation,
    },
    #[cfg(target_os = "windows")]
    EncodedTiles(Vec<crate::backends::ffmpeg_heif::EncodedTile>),
}

#[derive(Default)]
pub struct HeifDecodeService {
    next_session: AtomicU64,
    state: Mutex<ServiceState>,
    publication: Mutex<()>,
}

impl HeifDecodeService {
    pub fn capabilities(&self) -> Vec<HeifCapabilities> {
        backend_capabilities()
    }

    pub fn begin(
        &self,
        path: &Path,
        cache_dir: &Path,
        generation: u64,
        display_sharpening: bool,
    ) -> Result<HeifDecodeSession, MediaError> {
        // Capture the identity before any source probing or decode. Session
        // pixels may be persisted after a dwell, but always retain this fence.
        let source_revision = crate::cache::SourceRevision::observe(path)?;
        let size = libheif::dimensions(path)?;
        let cached = display_sharpening
            .then(|| crate::pipeline::heif::artifact::cached_heif_full(path, cache_dir))
            .transpose()?
            .flatten();
        let (decode_path, backend_plan) = cached.map_or_else(
            || (path.to_owned(), backend_plan(path, HeifOperation::Session)),
            |result| (result.path, HeifBackendPlan::cached_artifact()),
        );
        let selected_backend = backend_plan.first();
        let id = format!(
            "heif-{}",
            self.next_session.fetch_add(1, Ordering::Relaxed) + 1
        );
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(active) = state.active.take() {
            active.cancelled.store(true, Ordering::Release);
        }
        state.tiles.clear();
        state.active = Some(ActiveSession {
            id: id.clone(),
            generation,
            path: path.to_owned(),
            decode_path,
            source_revision,
            display_sharpening,
            cancelled: Arc::new(AtomicBool::new(false)),
            backend_plan,
        });
        let backend = selected_backend.kind();
        #[cfg(target_os = "windows")]
        let expected_tiles = if backend == HeifBackendKind::FfmpegSoftware {
            crate::backends::ffmpeg_heif::tile_count(path).unwrap_or_else(|_| {
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
            acceleration: selected_backend.acceleration(),
            status: HeifDecodeStatus::Decoding,
        })
    }

    pub fn decode<F, G>(
        &self,
        session: &HeifDecodeSession,
        cache_dir: &Path,
        mut publish: F,
        mut complete: G,
    ) -> Result<HeifDiagnostics, MediaError>
    where
        F: FnMut(HeifTilePublication),
        G: FnMut(&HeifDiagnostics),
    {
        let started = Instant::now();
        let (path, decode_path, source_revision, cancelled, display_sharpening, backend_plan) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(active) = &state.active else {
                return Err(MediaError::Cancelled);
            };
            if active.id != session.id || active.generation != session.generation {
                return Err(MediaError::Cancelled);
            }
            (
                active.path.clone(),
                active.decode_path.clone(),
                active.source_revision.clone(),
                Arc::clone(&active.cancelled),
                active.display_sharpening,
                active.backend_plan.clone(),
            )
        };
        let decode_permit = acquire_decode(DecodePriority::Foreground, &|| {
            cancelled.load(Ordering::Acquire)
        })?;
        let queue_wait_ms = elapsed_ms(started);
        if cancelled.load(Ordering::Acquire) {
            return Err(MediaError::Cancelled);
        }
        if crate::cache::SourceRevision::observe(&path)? != source_revision {
            return Err(MediaError::StaleSourceRevision);
        }
        let decode_started = Instant::now();
        let decoded = execute_backend_plan(
            &backend_plan,
            || cancelled.load(Ordering::Acquire),
            |backend| decode_session_backend(backend, session, &decode_path, display_sharpening),
        )
        .map_err(BackendExecutionError::into_media_error)?;
        let decode_ms = elapsed_ms(decode_started);
        let backend = decoded.backend.kind();
        let acceleration = decoded.backend.acceleration();
        let fallback_reason = format_attempt_diagnostics(&decoded.diagnostics);
        let tile_started = Instant::now();
        let (codec, canonical_image) = match decoded.value {
            #[cfg(target_os = "windows")]
            SessionDecode::EncodedTiles(tiles) => {
                let mut canonical = RgbaImage::new(session.width, session.height);
                for tile in &tiles {
                    let decoded_tile = image::load_from_memory(&tile.jpeg)?.into_rgba8();
                    copy_rgba_tile(&mut canonical, &decoded_tile, tile.x, tile.y)?;
                }
                for tile in tiles {
                    let stored = if display_sharpening {
                        crop_rgba_rect(&canonical, tile.x, tile.y, tile.width, tile.height, true)
                    } else {
                        HeifTile {
                            width: tile.width,
                            height: tile.height,
                            payload: HeifTileData::Jpeg(Arc::from(tile.jpeg)),
                        }
                    };
                    let encoded = !display_sharpening;
                    let event =
                        tile_event(session, tile.x, tile.y, tile.width, tile.height, encoded);
                    if !self.publish_tile_if_current(
                        session,
                        &cancelled,
                        stored,
                        event,
                        &mut publish,
                    ) {
                        return Err(MediaError::Cancelled);
                    }
                }
                (
                    "FFmpeg HEVC tile-grid",
                    Some((
                        DynamicImage::ImageRgba8(canonical),
                        crate::pipeline::heif::artifact::backend_presentation(
                            PlannedHeifBackend::Ffmpeg,
                        ),
                    )),
                )
            }
            SessionDecode::Image {
                image,
                codec,
                presentation,
            } => {
                // Keep canonical pixels unsharpened for cache persistence and
                // apply display sharpening only while cropping. Presentation
                // facts come from the selected backend; fallback RGB is not
                // assumed to have undergone an sRGB transform.
                let image = image.into_rgba8();
                for (x, y) in tile_coordinates(image.width(), image.height(), session.tile_size) {
                    let tile = crop_rgba(&image, x, y, session.tile_size, display_sharpening);
                    let event = tile_event(session, x, y, tile.width, tile.height, false);
                    if !self.publish_tile_if_current(session, &cancelled, tile, event, &mut publish)
                    {
                        return Err(MediaError::Cancelled);
                    }
                }
                (codec, Some((DynamicImage::ImageRgba8(image), presentation)))
            }
        };
        // Do not let a large JPEG encode or fsync serialize the next selected
        // image behind this session's foreground decode permit.
        drop(decode_permit);
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
        if !self.complete_if_current(session, &cancelled, diagnostics.clone(), &mut complete) {
            return Err(MediaError::Cancelled);
        }
        if backend != HeifBackendKind::CachedArtifact
            && let Some((image, presentation)) = canonical_image.as_ref()
        {
            cache_session_image_if_stable(
                &cancelled,
                &source_revision,
                cache_dir,
                image,
                *presentation,
            );
        }
        Ok(diagnostics)
    }

    #[cfg(test)]
    fn insert_tile_if_current(
        &self,
        session: &HeifDecodeSession,
        cancelled: &AtomicBool,
        x: u32,
        y: u32,
        tile: HeifTile,
    ) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancelled.load(Ordering::Acquire) || !active_matches(&state, session) {
            return false;
        }
        state
            .tiles
            .insert((session.id.clone(), session.generation, x, y), tile);
        true
    }

    fn publish_tile_if_current<F>(
        &self,
        session: &HeifDecodeSession,
        cancelled: &AtomicBool,
        tile: HeifTile,
        event: HeifTilePublication,
        publish: &mut F,
    ) -> bool
    where
        F: FnMut(HeifTilePublication),
    {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cancelled.load(Ordering::Acquire) || !active_matches(&state, session) {
                return false;
            }
            state.tiles.insert(
                (session.id.clone(), session.generation, event.x, event.y),
                tile,
            );
        }
        // Session replacement uses the same publication lock, so a newer
        // generation cannot be installed before this callback returns.
        publish(event);
        true
    }

    fn complete_if_current<G>(
        &self,
        session: &HeifDecodeSession,
        cancelled: &AtomicBool,
        diagnostics: HeifDiagnostics,
        complete: &mut G,
    ) -> bool
    where
        G: FnMut(&HeifDiagnostics),
    {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cancelled.load(Ordering::Acquire) || !active_matches(&state, session) {
                return false;
            }
        }
        complete(&diagnostics);
        true
    }

    pub fn cancel(&self, session_id: &str) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tiles
            .get(&(session.to_owned(), generation, x, y))
            .cloned()
    }
}

fn active_matches(state: &ServiceState, session: &HeifDecodeSession) -> bool {
    state
        .active
        .as_ref()
        .is_some_and(|active| active.id == session.id && active.generation == session.generation)
}

fn tile_event(
    session: &HeifDecodeSession,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    encoded: bool,
) -> HeifTilePublication {
    HeifTilePublication {
        session_id: session.id.clone(),
        generation: session.generation,
        x,
        y,
        width,
        height,
        payload: if encoded {
            HeifTilePayload::Jpeg
        } else {
            HeifTilePayload::Rgba
        },
    }
}

fn decode_session_backend(
    backend: PlannedHeifBackend,
    session: &HeifDecodeSession,
    path: &Path,
    _display_sharpening: bool,
) -> Result<SessionDecode, MediaError> {
    let presentation = crate::pipeline::heif::artifact::backend_presentation(backend);
    match backend {
        PlannedHeifBackend::CachedArtifact => Ok(SessionDecode::Image {
            image: image::ImageReader::open(path)?.decode()?,
            codec: "cached full JPEG",
            presentation,
        }),
        #[cfg(target_os = "windows")]
        PlannedHeifBackend::Platform(_) => Ok(SessionDecode::Image {
            image: crate::backends::windows_wic::decode_full_rgba8(path)?,
            codec: "Windows WIC HEIF decoder",
            presentation,
        }),
        #[cfg(target_os = "macos")]
        PlannedHeifBackend::Platform(_) => Ok(SessionDecode::Image {
            image: crate::backends::apple_image_io::decode_rgba8(
                path,
                session.width.max(session.height),
            )?,
            codec: "Apple ImageIO HEIF decoder",
            presentation,
        }),
        #[cfg(target_os = "linux")]
        PlannedHeifBackend::Platform(_) => Err(MediaError::NativeDecoderUnavailable),
        PlannedHeifBackend::Ffmpeg => {
            crate::backends::ffmpeg_heif::can_decode(path)?;
            #[cfg(target_os = "windows")]
            {
                return Ok(SessionDecode::EncodedTiles(
                    crate::backends::ffmpeg_heif::decode_full_jpeg_tiles(
                        path,
                        crate::ImageDimensions {
                            width: session.width,
                            height: session.height,
                        },
                        false,
                    )?,
                ));
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                Ok(SessionDecode::Image {
                    image: crate::backends::ffmpeg_heif::decode_full_rgba8(
                        path,
                        crate::ImageDimensions {
                            width: session.width,
                            height: session.height,
                        },
                        false,
                    )?,
                    codec: "FFmpeg HEVC tile-grid",
                    presentation,
                })
            }
        }
        PlannedHeifBackend::FfmpegRgbaFallback => Ok(SessionDecode::Image {
            image: crate::backends::ffmpeg_heif::decode_full_rgba8(
                path,
                crate::ImageDimensions {
                    width: session.width,
                    height: session.height,
                },
                false,
            )?,
            codec: "FFmpeg HEVC tile-grid",
            presentation,
        }),
        PlannedHeifBackend::Libheif => Ok(SessionDecode::Image {
            image: libheif::decode_full_rgb8(path)?,
            codec: "libheif/libde265",
            presentation,
        }),
    }
}

fn cache_session_image_if_stable(
    cancelled: &AtomicBool,
    source_revision: &crate::cache::SourceRevision,
    cache_dir: &Path,
    image: &DynamicImage,
    presentation: crate::cache::ArtifactPresentation,
) -> bool {
    // A short dwell period prevents a fast arrow-key sweep from launching a
    // full-resolution JPEG encode for every transient selection.
    for _ in 0..25 {
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    let _cache_write_permit = loop {
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        if let Some(permit) = try_acquire_heif_session_cache_write() {
            break permit;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    if let Err(error) = crate::pipeline::heif::artifact::cache_full_image(
        source_revision,
        cache_dir,
        image,
        presentation,
    ) {
        eprintln!(
            "failed to cache HEIF loupe image {}: {error}",
            source_revision.canonical_path.display()
        );
        return false;
    }
    true
}

#[cfg(target_os = "windows")]
fn copy_rgba_tile(
    destination: &mut RgbaImage,
    tile: &RgbaImage,
    x: u32,
    y: u32,
) -> Result<(), MediaError> {
    if x.saturating_add(tile.width()) > destination.width()
        || y.saturating_add(tile.height()) > destination.height()
    {
        return Err(MediaError::CacheArtifact(
            "decoded HEIF tile exceeds the display canvas".into(),
        ));
    }
    let destination_stride = destination.width() as usize * 4;
    let tile_stride = tile.width() as usize * 4;
    for row in 0..tile.height() as usize {
        let source_start = row * tile_stride;
        let destination_start = (y as usize + row) * destination_stride + x as usize * 4;
        destination.as_mut()[destination_start..destination_start + tile_stride]
            .copy_from_slice(&tile.as_raw()[source_start..source_start + tile_stride]);
    }
    Ok(())
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
    crop_rgba_rect(image, x, y, width, height, display_sharpening)
}

fn crop_rgba_rect(
    image: &RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    display_sharpening: bool,
) -> HeifTile {
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
        payload: HeifTileData::Rgba {
            stride: width * 4,
            bytes: Arc::from(rgba),
        },
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

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    use oxy_domain::AccelerationKind;

    fn rgba_payload(tile: &HeifTile) -> (u32, &[u8]) {
        match &tile.payload {
            HeifTileData::Rgba { stride, bytes } => (*stride, bytes),
            HeifTileData::Jpeg(_) => panic!("expected RGBA tile"),
        }
    }

    fn begin(
        service: &HeifDecodeService,
        path: &Path,
        generation: u64,
        display_sharpening: bool,
    ) -> Result<HeifDecodeSession, MediaError> {
        service.begin(
            path,
            Path::new("target/oxy-media-test-empty-cache"),
            generation,
            display_sharpening,
        )
    }

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
        let (stride, bytes) = rgba_payload(&tile);
        assert_eq!((tile.width, tile.height, stride), (88, 38, 352));
        assert_eq!(bytes.len(), 88 * 38 * 4);
    }

    #[test]
    fn display_sharpening_preserves_flat_pixels_and_alpha() {
        let image = RgbaImage::from_pixel(3, 3, image::Rgba([80, 120, 160, 77]));
        let tile = crop_rgba(&image, 0, 0, 3, true);
        assert_eq!(rgba_payload(&tile).1, image.as_raw());
    }

    #[test]
    fn display_sharpening_increases_edge_definition() {
        let mut image = RgbaImage::from_pixel(3, 3, image::Rgba([64, 64, 64, 255]));
        image.put_pixel(1, 1, image::Rgba([128, 128, 128, 255]));
        let tile = crop_rgba(&image, 0, 0, 3, true);
        let bytes = rgba_payload(&tile).1;
        let center = (3 + 1) * 4;
        assert!(bytes[center] > 128);
        assert_eq!(bytes[center + 3], 255);
    }

    #[test]
    fn successful_begin_supersedes_old_session_and_rejects_late_tiles() {
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let service = HeifDecodeService::default();
        let first = begin(&service, &fixture, 1, false).unwrap();
        let first_cancelled = {
            let state = service.state.lock().unwrap();
            Arc::clone(&state.active.as_ref().unwrap().cancelled)
        };
        assert!(service.insert_tile_if_current(
            &first,
            &first_cancelled,
            0,
            0,
            HeifTile {
                width: 1,
                height: 1,
                payload: HeifTileData::Rgba {
                    stride: 4,
                    bytes: Arc::from([0, 0, 0, 255]),
                },
            },
        ));

        let second = begin(&service, &fixture, 2, false).unwrap();
        assert!(first_cancelled.load(Ordering::Acquire));
        assert!(service.tile(&first.id, first.generation, 0, 0).is_none());
        assert!(!service.insert_tile_if_current(
            &first,
            &first_cancelled,
            0,
            0,
            HeifTile {
                width: 1,
                height: 1,
                payload: HeifTileData::Rgba {
                    stride: 4,
                    bytes: Arc::from([0, 0, 0, 255]),
                },
            },
        ));
        assert!(!service.cancel(&first.id));
        assert!(service.cancel(&second.id));
    }

    #[test]
    fn failed_begin_does_not_replace_the_active_session() {
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let service = HeifDecodeService::default();
        let first = begin(&service, &fixture, 1, false).unwrap();

        let missing = fixture.with_file_name("missing-phase-c-fixture.hif");
        assert!(begin(&service, &missing, 2, false).is_err());
        assert!(service.cancel(&first.id));
    }

    #[test]
    fn cancelled_session_does_not_publish_completion() {
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let service = HeifDecodeService::default();
        let session = begin(&service, &fixture, 1, false).unwrap();
        let cancelled = {
            let state = service.state.lock().unwrap();
            Arc::clone(&state.active.as_ref().unwrap().cancelled)
        };
        assert!(service.cancel(&session.id));
        let mut completed = false;
        let diagnostics = HeifDiagnostics {
            backend: session.backend,
            acceleration: session.acceleration,
            codec: None,
            queue_wait_ms: 0,
            decode_ms: 0,
            tile_publish_ms: 0,
            total_ms: 0,
            fallback_reason: None,
        };
        assert!(
            !service
                .complete_if_current(&session, &cancelled, diagnostics, &mut |_| completed = true,)
        );
        assert!(!completed);
    }

    #[test]
    fn cancelled_transient_session_skips_full_jpeg_cache() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("transient.hif");
        let cache = directory.path().join("previews");
        std::fs::write(&source, b"cache identity fixture").unwrap();
        let cancelled = AtomicBool::new(true);
        let source_revision = crate::cache::SourceRevision::observe(&source).unwrap();

        assert!(!cache_session_image_if_stable(
            &cancelled,
            &source_revision,
            &cache,
            &DynamicImage::new_rgb8(8, 4),
            crate::pipeline::heif::artifact::backend_presentation(PlannedHeifBackend::Libheif,),
        ));
        assert!(
            crate::pipeline::heif::artifact::cached_heif_full(&source, &cache)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn source_replacement_during_session_dwell_cannot_publish_old_pixels() {
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("replacement.HIF");
        std::fs::copy(&fixture, &source).unwrap();
        let revision = crate::cache::SourceRevision::observe(&source).unwrap();
        let dimensions = libheif::dimensions(&source).unwrap();
        let image = DynamicImage::new_rgb8(dimensions.width, dimensions.height);
        let cache = directory.path().join("previews");
        let cancelled = AtomicBool::new(false);
        let replacement = std::fs::read(&fixture).unwrap();
        let replacer = {
            let source = source.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                std::fs::write(source, replacement).unwrap();
            })
        };

        assert!(!cache_session_image_if_stable(
            &cancelled,
            &revision,
            &cache,
            &image,
            crate::pipeline::heif::artifact::backend_presentation(PlannedHeifBackend::Libheif,),
        ));
        replacer.join().unwrap();
        assert!(
            crate::pipeline::heif::artifact::cached_heif_full(&source, &cache)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn completed_session_cache_work_does_not_block_next_selection_decode() {
        if crate::backends::ffmpeg_heif::capability().is_err() {
            return;
        }
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let service = Arc::new(HeifDecodeService::default());
        let first_session = begin(&service, &fixture, 1, true).unwrap();
        let first_cache = tempfile::tempdir().unwrap();
        let first_cache_path = first_cache.path().to_owned();
        let first_service = service.clone();
        let (completed_tx, completed_rx) = std::sync::mpsc::channel();
        let first_worker = std::thread::spawn(move || {
            first_service.decode(
                &first_session,
                &first_cache_path,
                |_| {},
                |_| {
                    let _ = completed_tx.send(());
                },
            )
        });

        completed_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("first selection should publish completion before cache encoding");

        let second_session = begin(&service, &fixture, 2, true).unwrap();
        let second_cache = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let mut first_tile_elapsed = None;
        let second_result = service.decode(
            &second_session,
            second_cache.path(),
            |_| {
                if first_tile_elapsed.is_none() {
                    first_tile_elapsed = Some(started.elapsed());
                    service.cancel(&second_session.id);
                }
            },
            |_| {},
        );

        assert!(matches!(second_result, Err(MediaError::Cancelled)));
        assert!(
            first_tile_elapsed.expect("second selection should publish a tile")
                < std::time::Duration::from_secs(5),
            "cache work from the previous selection blocked the foreground decode gate"
        );
        first_worker.join().unwrap().unwrap();
    }

    #[test]
    fn selects_and_decodes_full_tile_fixture() {
        if crate::backends::ffmpeg_heif::capability().is_err() {
            return;
        }
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let service = HeifDecodeService::default();
        let session = begin(&service, &fixture, 1, false).unwrap();
        let mut events = Vec::new();
        let cache = tempfile::tempdir().unwrap();
        let diagnostics = service
            .decode(&session, cache.path(), |event| events.push(event), |_| {})
            .unwrap();
        assert_eq!(diagnostics.backend, session.backend);
        assert_eq!(events.len(), session.expected_tiles as usize);
        let mut covered_pixels = 0_u64;
        for event in events {
            #[cfg(target_os = "windows")]
            assert_eq!(event.payload, HeifTilePayload::Jpeg);
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            assert_eq!(event.payload, HeifTilePayload::Rgba);
            assert!(event.x + event.width <= session.width);
            assert!(event.y + event.height <= session.height);
            let tile = service
                .tile(&session.id, session.generation, event.x, event.y)
                .unwrap();
            #[cfg(target_os = "windows")]
            {
                let HeifTileData::Jpeg(jpeg) = tile.payload else {
                    panic!("expected JPEG tile");
                };
                let decoded = image::load_from_memory(&jpeg).unwrap();
                assert_eq!(decoded.width(), event.width);
                assert_eq!(decoded.height(), event.height);
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                assert_eq!(
                    rgba_payload(&tile).1.len(),
                    (event.width * event.height * 4) as usize
                );
            }
            covered_pixels += u64::from(event.width) * u64::from(event.height);
        }
        assert_eq!(
            covered_pixels,
            u64::from(session.width) * u64::from(session.height)
        );
        assert!(
            crate::pipeline::heif::artifact::cached_heif_full(&fixture, cache.path())
                .unwrap()
                .is_some()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn prefers_qualified_ffmpeg_tile_grid_over_slower_wic() {
        if crate::backends::ffmpeg_heif::capability().is_err() {
            return;
        }
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let service = HeifDecodeService::default();
        let session = begin(&service, &fixture, 1, true).unwrap();
        assert_eq!(session.backend, HeifBackendKind::FfmpegSoftware);
        assert_eq!(session.acceleration, AccelerationKind::Software);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn selects_and_decodes_apple_image_io_fixture() {
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        if crate::backends::apple_image_io::can_decode(&fixture).is_err() {
            return;
        }
        let service = HeifDecodeService::default();
        let session = begin(&service, &fixture, 1, true).unwrap();
        assert_eq!(session.backend, HeifBackendKind::AppleImageIo);
        assert_eq!(session.acceleration, AccelerationKind::Unknown);
        let mut tiles = 0;
        let cache = tempfile::tempdir().unwrap();
        let diagnostics = service
            .decode(&session, cache.path(), |_| tiles += 1, |_| {})
            .unwrap();
        assert_eq!(diagnostics.backend, HeifBackendKind::AppleImageIo);
        assert_eq!(tiles, session.expected_tiles);
    }
}
