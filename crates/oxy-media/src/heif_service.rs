use crate::{MediaError, heif};
use image::DynamicImage;
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
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct HeifDecodeService {
    next_session: AtomicU64,
    state: Mutex<ServiceState>,
}

impl HeifDecodeService {
    pub fn capabilities(&self) -> Vec<HeifCapabilities> {
        vec![platform_capability(), software_capability()]
    }

    pub fn begin(
        &self,
        path: &Path,
        generation: u64,
        hardware_acceleration: bool,
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
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        let platform = platform_capability();
        let fallback = hardware_acceleration && !platform.available;
        Ok(HeifDecodeSession {
            id,
            generation,
            width: size.width,
            height: size.height,
            tile_size: DEFAULT_TILE_SIZE,
            backend: HeifBackendKind::LibheifSoftware,
            acceleration: AccelerationKind::Software,
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
        let cancelled = {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let Some(active) = &state.active else {
                return Err(MediaError::Cancelled);
            };
            if active.id != session.id || active.generation != session.generation {
                return Err(MediaError::Cancelled);
            }
            active.cancelled.clone()
        };
        let image = heif::decode_full_rgb8(&path)?;
        let decode_ms = elapsed_ms(started);
        let tile_started = Instant::now();
        for (x, y) in tile_coordinates(image.width(), image.height(), session.tile_size) {
            if cancelled.load(Ordering::Acquire) {
                return Err(MediaError::Cancelled);
            }
            let tile = crop_rgba(&image, x, y, session.tile_size);
            let event = HeifTileReady {
                session_id: session.id.clone(),
                generation: session.generation,
                x,
                y,
                width: tile.width,
                height: tile.height,
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
        let platform = platform_capability();
        let diagnostics = HeifDiagnostics {
            backend: HeifBackendKind::LibheifSoftware,
            acceleration: AccelerationKind::Software,
            codec: Some("libheif/libde265".into()),
            decode_ms,
            tile_publish_ms: elapsed_ms(tile_started),
            total_ms: elapsed_ms(started),
            fallback_reason: platform
                .detail
                .filter(|_| session.status == HeifDecodeStatus::CompatibilityFallback),
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

fn crop_rgba(image: &DynamicImage, x: u32, y: u32, tile_size: u32) -> HeifTile {
    let width = tile_size.min(image.width() - x);
    let height = tile_size.min(image.height() - y);
    let rgba = image.crop_imm(x, y, width, height).into_rgba8();
    HeifTile {
        width,
        height,
        stride: width * 4,
        rgba: Arc::from(rgba.into_raw()),
    }
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

fn software_capability() -> HeifCapabilities {
    HeifCapabilities {
        backend: HeifBackendKind::LibheifSoftware,
        acceleration: AccelerationKind::Software,
        available: true,
        detail: Some("portable compatibility backend".into()),
    }
}

fn platform_capability() -> HeifCapabilities {
    #[cfg(target_os = "windows")]
    let backend = HeifBackendKind::WindowsWic;
    #[cfg(target_os = "macos")]
    let backend = HeifBackendKind::AppleImageIo;
    #[cfg(target_os = "linux")]
    let backend = HeifBackendKind::LinuxVaapi;
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    let backend = HeifBackendKind::LibheifSoftware;
    HeifCapabilities {
        backend,
        acceleration: AccelerationKind::Hardware,
        available: false,
        detail: Some("native hardware adapter is not available in this build".into()),
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
    fn crop_produces_tightly_packed_edge_tile() {
        let image = DynamicImage::new_rgb8(600, 550);
        let tile = crop_rgba(&image, 512, 512, 512);
        assert_eq!((tile.width, tile.height, tile.stride), (88, 38, 352));
        assert_eq!(tile.rgba.len(), 88 * 38 * 4);
    }
}
