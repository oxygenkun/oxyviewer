//! Face-crop resources for the people panel.
//!
//! A review queue that asks "is this Alice?" must show the face. Crops are
//! produced on demand from the source pixels, cached in memory as encoded JPEG
//! bytes, and handed to the WebView as `oxy-media://` resources so image bytes
//! never travel through JSON IPC.
//!
//! The encoded-byte cache is the expensive part: decoding a source and
//! resampling it costs far more than re-registering an existing buffer. A
//! resource handle is short-lived and leased by the frontend, so it is created
//! per request while the bytes behind it are reused.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use oxy_domain::{AssetKind, MediaResourceDescriptor, PreviewPriority, RenderLevel};
use oxy_library::Library;
use oxy_runtime::CancellationToken;

use crate::state::cache::CacheManager;

/// Crop edge in device pixels. Large enough to recognize a face, small enough
/// that a 50-member cluster stays cheap.
pub(crate) const DEFAULT_CROP_SIZE: u32 = 128;
/// How far beyond the detected box the crop reaches, to include hair and chin.
pub(crate) const CROP_MARGIN: f32 = 1.5;
pub(crate) const CROP_QUALITY: u8 = 85;
/// Encoded crops retained before the oldest are dropped. At ~6 KB each this is
/// a few megabytes, independent of library size.
const MAX_CACHED_CROPS: usize = 1024;

struct CachedCrop {
    bytes: Arc<[u8]>,
    size: u32,
}

pub(crate) struct FaceCropService {
    library: Arc<Library>,
    cache: Arc<CacheManager>,
    resources: oxy_media::ResourceRegistry,
    render_gate: Mutex<()>,
    crops: Mutex<HashMap<(String, u32), Arc<CachedCrop>>>,
    order: Mutex<VecDeque<(String, u32)>>,
}

impl FaceCropService {
    pub(crate) fn new(
        library: Arc<Library>,
        cache: Arc<CacheManager>,
        resources: oxy_media::ResourceRegistry,
    ) -> Self {
        Self {
            library,
            cache,
            resources,
            render_gate: Mutex::new(()),
            crops: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
        }
    }

    /// Produces one resource per observation that still exists.
    ///
    /// Observations are skipped rather than failing the batch: a stale review
    /// item whose asset was re-analyzed away should leave a gap, not break the
    /// whole panel.
    pub(crate) fn crops(
        &self,
        observation_ids: &[String],
        size: u32,
    ) -> Result<Vec<FaceCropResource>, String> {
        let size = size.clamp(32, 512);
        let mut crops = Vec::with_capacity(observation_ids.len());
        for observation_id in observation_ids {
            if let Some(crop) = self.cached(observation_id, size) {
                crops.push(self.register(observation_id, crop)?);
                continue;
            }
            let crop = match self.load_or_render(observation_id, size) {
                Ok(crop) => crop,
                Err(error) => {
                    if observation_ids.len() == 1 {
                        return Err(error);
                    }
                    eprintln!("face crop skipped for {observation_id}: {error}");
                    continue;
                }
            };
            crops.push(self.register(observation_id, crop)?);
        }
        Ok(crops)
    }

    fn load_or_render(&self, observation_id: &str, size: u32) -> Result<Arc<CachedCrop>, String> {
        if let Some(crop) = self.cached(observation_id, size) {
            return Ok(crop);
        }
        if let Some((cached_size, bytes)) = self
            .library
            .face_crop(observation_id, size)
            .map_err(|error| error.to_string())?
        {
            let crop = Arc::new(CachedCrop {
                bytes: Arc::from(bytes),
                size: cached_size,
            });
            self.remember(observation_id, size, Arc::clone(&crop));
            return Ok(crop);
        }
        let observation = self
            .library
            .face_observation(observation_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown observation {observation_id}"))?;
        let kind = AssetKind::from_path(&observation.asset_path)
            .ok_or_else(|| format!("unsupported asset {}", observation.asset_path.display()))?;
        // Full RAW/HEIF/TIFF decodes can allocate large buffers. Serialize
        // those while allowing cheap source rasters to finish independently.
        let _gate = (!oxy_domain::full_resolution_is_the_source(kind)).then(|| {
            self.render_gate
                .lock()
                .expect("face crop render gate poisoned")
        });
        if let Some(crop) = self.cached(observation_id, size) {
            return Ok(crop);
        }
        let crop = self.render(observation_id, size)?;
        // Older scans have no prepared crop. Fill the same rebuildable cache
        // after the one-time Full decode so reopening is fast too.
        if let Err(error) = self
            .library
            .store_face_crops(&[oxy_library::StoredFaceCrop {
                observation_id: observation_id.to_string(),
                size: crop.size,
                jpeg: crop.bytes.to_vec(),
            }])
        {
            eprintln!("face crop cache write skipped for {observation_id}: {error}");
        }
        self.remember(observation_id, size, Arc::clone(&crop));
        Ok(crop)
    }

    fn cached(&self, observation_id: &str, size: u32) -> Option<Arc<CachedCrop>> {
        self.crops
            .lock()
            .expect("face crop cache poisoned")
            .get(&(observation_id.to_string(), size))
            .cloned()
    }

    fn remember(&self, observation_id: &str, size: u32, crop: Arc<CachedCrop>) {
        let key = (observation_id.to_string(), size);
        let mut crops = self.crops.lock().expect("face crop cache poisoned");
        let mut order = self.order.lock().expect("face crop order poisoned");
        crops.insert(key.clone(), crop);
        order.push_back(key);
        while order.len() > MAX_CACHED_CROPS {
            if let Some(evicted) = order.pop_front() {
                crops.remove(&evicted);
            }
        }
    }

    fn register(
        &self,
        observation_id: &str,
        crop: Arc<CachedCrop>,
    ) -> Result<FaceCropResource, String> {
        let dimensions = oxy_media::PixelDimensions {
            width: crop.size,
            height: crop.size,
        };
        let handle = self
            .resources
            .register_encoded(
                crop.bytes.clone(),
                "image/jpeg",
                dimensions,
                oxy_media::ImageOrigin::PrimaryImage,
            )
            .map_err(|error| error.to_string())?;
        Ok(FaceCropResource {
            observation_id: observation_id.to_string(),
            descriptor: MediaResourceDescriptor {
                resource_id: handle.descriptor.resource_id,
                url: handle.descriptor.url,
                media_type: "image/jpeg".to_string(),
            },
        })
    }

    /// Decodes the source, crops the face, and encodes it once.
    fn render(&self, observation_id: &str, size: u32) -> Result<Arc<CachedCrop>, String> {
        let observation = self
            .library
            .face_observation(observation_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown observation {observation_id}"))?;
        let kind = AssetKind::from_path(&observation.asset_path)
            .ok_or_else(|| format!("unsupported asset {}", observation.asset_path.display()))?;
        // Background priority: filling the review panel must never displace the
        // grid or the loupe.
        // A Full request with interim delivery could still return a low-detail
        // preview, producing a blurry crop for the review panel.
        let preview = oxy_media::preview_for_app_upgrade(
            &observation.asset_path,
            &self.cache.preview_dir(),
            RenderLevel::Full,
            PreviewPriority::Preload,
            kind,
            &CancellationToken::default(),
        )
        .map_err(|error| error.to_string())?
        .result;
        let mut pixels =
            oxy_media::decode_rgb_pixels(&preview.path).map_err(|error| error.to_string())?;
        // Source rasters retain encoded orientation; developed full artifacts
        // from the other formats are already display-corrected.
        if oxy_domain::full_resolution_is_the_source(kind) {
            let orientation = preview
                .image_facts
                .as_ref()
                .map_or(1, |facts| facts.exif_orientation);
            pixels = oxy_media::apply_exif_orientation(pixels, orientation)
                .map_err(|error| error.to_string())?;
        }
        let (bytes, width, height) =
            oxy_media::face_crop_jpeg(&pixels, observation.bbox, size, CROP_MARGIN, CROP_QUALITY)
                .map_err(|error| error.to_string())?;
        debug_assert_eq!((width, height), (size, size));
        Ok(Arc::new(CachedCrop {
            bytes: Arc::from(bytes),
            size,
        }))
    }
}

/// One crop, ready to be turned into an `<img src>`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FaceCropResource {
    pub observation_id: String,
    pub descriptor: MediaResourceDescriptor,
}

/// The asset kinds a crop can be produced from.
#[cfg(test)]
fn supports_crops(path: &std::path::Path) -> bool {
    AssetKind::from_path(path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_domain::{FaceObservation, NormalizedPoint, NormalizedRect};
    use std::path::Path;

    fn service() -> (Arc<Library>, FaceCropService, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let library = Arc::new(Library::in_memory().unwrap());
        let cache = Arc::new(
            CacheManager::load(
                directory.path().join("previews"),
                directory.path().join("cache-settings.json"),
            )
            .unwrap(),
        );
        let resources = oxy_media::ResourceRegistry::new(oxy_media::ResourceRegistryLimits::new(
            64,
            8 * 1024 * 1024,
        ));
        let service = FaceCropService::new(library.clone(), cache, resources);
        (library, service, directory)
    }

    #[test]
    fn prepared_crop_loads_without_decoding_the_source() {
        let (library, service, _directory) = service();
        let path = Path::new("/missing/photo.jpg");
        library
            .replace_asset_faces(
                path,
                "asset-1",
                "revision",
                "detector",
                "embedder",
                &[oxy_library::StoredFace {
                    observation: FaceObservation {
                        observation_id: "obs-1".into(),
                        asset_id: "asset-1".into(),
                        asset_path: path.to_path_buf(),
                        source_revision: "revision".into(),
                        local_index: 0,
                        bbox: NormalizedRect::new(0.25, 0.25, 0.5, 0.5),
                        landmarks: vec![NormalizedPoint::new(0.4, 0.4); 5],
                        detection_score: 0.99,
                        detector_fingerprint: "detector".into(),
                    },
                    embedding: vec![0.1, 0.2],
                }],
            )
            .unwrap();
        assert!(
            service.crops(&["obs-1".into()], 96).is_err(),
            "a failed single crop must be retryable, not cached as an empty success"
        );
        library
            .store_face_crops(&[oxy_library::StoredFaceCrop {
                observation_id: "obs-1".into(),
                size: 128,
                jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
            }])
            .unwrap();

        let crops = service.crops(&["obs-1".into()], 96).unwrap();
        assert_eq!(crops.len(), 1);
        let resource = service
            .resources
            .resolve(&crops[0].descriptor.resource_id)
            .unwrap();
        assert_eq!(resource.dimensions.width, 128);
        assert_eq!(
            service.resources.materialize(&resource).unwrap(),
            [0xff, 0xd8, 0xff, 0xd9]
        );
    }

    #[test]
    #[ignore = "requires the Sony HIF fixture; exercises full decode rather than the cache"]
    fn heif_full_crop_can_be_served() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/DSC00449.HIF");
        let (library, service, _directory) = service();
        let revision = oxy_domain::face_source_revision_for_path(&path).unwrap();
        library
            .replace_asset_faces(
                &path,
                "hif-asset",
                &revision,
                "detector",
                "embedder",
                &[oxy_library::StoredFace {
                    observation: FaceObservation {
                        observation_id: "hif-face".into(),
                        asset_id: "hif-asset".into(),
                        asset_path: path.clone(),
                        source_revision: revision.clone(),
                        local_index: 0,
                        bbox: NormalizedRect::new(0.35, 0.35, 0.2, 0.2),
                        landmarks: vec![NormalizedPoint::new(0.4, 0.4); 5],
                        detection_score: 0.99,
                        detector_fingerprint: "detector".into(),
                    },
                    embedding: vec![0.1, 0.2],
                }],
            )
            .unwrap();

        let crops = service.crops(&["hif-face".into()], 128).unwrap();
        assert_eq!(crops.len(), 1);
        let resource = service
            .resources
            .resolve(&crops[0].descriptor.resource_id)
            .unwrap();
        assert!(service.resources.materialize(&resource).unwrap().len() > 100);
    }

    #[test]
    fn a_detected_face_becomes_a_servable_crop() {
        let (library, service, directory) = service();
        // Encode the fixture with the crate's own JPEG encoder: the crop path
        // needs a real decodable file, and this keeps the test free of codec
        // and fixture dependencies.
        let pixels = oxy_media::RgbPixels::new(
            96,
            96,
            (0..96 * 96)
                .flat_map(|index| [(index % 251) as u8, 64, 200])
                .collect(),
        )
        .unwrap();
        let (encoded, _, _) = oxy_media::face_crop_jpeg(
            &pixels,
            NormalizedRect::new(0.0, 0.0, 1.0, 1.0),
            96,
            1.0,
            90,
        )
        .unwrap();
        let path = directory.path().join("photo.jpg");
        std::fs::write(&path, encoded).unwrap();

        let revision = oxy_domain::face_source_revision_for_path(&path).unwrap();
        library
            .replace_asset_faces(
                &path,
                "asset-1",
                &revision,
                "detector",
                "embedder",
                &[oxy_library::StoredFace {
                    observation: FaceObservation {
                        observation_id: "obs-1".into(),
                        asset_id: "asset-1".into(),
                        asset_path: path.clone(),
                        source_revision: revision.clone(),
                        local_index: 0,
                        bbox: NormalizedRect::new(0.25, 0.25, 0.5, 0.5),
                        landmarks: vec![NormalizedPoint::new(0.4, 0.4); 5],
                        detection_score: 0.99,
                        detector_fingerprint: "detector".into(),
                    },
                    embedding: vec![0.1, 0.2, 0.3],
                }],
            )
            .unwrap();

        // A failure anywhere in preview/decode/encode is swallowed into an empty
        // batch, which the panel renders as a blank square with no other clue.
        let crops = service.crops(&["obs-1".to_string()], 64).unwrap();
        assert_eq!(crops.len(), 1, "a stored observation must produce a crop");
        assert_eq!(crops[0].observation_id, "obs-1");
        assert!(crops[0].descriptor.url.starts_with("oxy-media://"));

        let resource = service
            .resources
            .resolve(&crops[0].descriptor.resource_id)
            .expect("the crop must be resolvable by the protocol handler");
        let bytes = service.resources.materialize(&resource).unwrap();
        assert!(!bytes.is_empty(), "the crop must carry encoded JPEG bytes");
    }

    /// Highest-contrast pattern a JPEG still carries: 4 px cells become one
    /// cycle per 8x8 block, which survives quantization but not a 3x downscale.
    fn checkerboard(edge: u32) -> oxy_media::RgbPixels {
        let mut data = Vec::with_capacity((edge * edge * 3) as usize);
        for y in 0..edge {
            for x in 0..edge {
                let value = if ((x / 4) + (y / 4)) % 2 == 0 { 250 } else { 5 };
                data.extend_from_slice(&[value, value, value]);
            }
        }
        oxy_media::RgbPixels::new(edge, edge, data).unwrap()
    }

    fn crop_luma_range(
        service: &FaceCropService,
        directory: &tempfile::TempDir,
        observation_id: &str,
        size: u32,
    ) -> u8 {
        let crops = service.crops(&[observation_id.to_string()], size).unwrap();
        assert_eq!(
            crops.len(),
            1,
            "a stored observation must produce a crop, not a blank placeholder"
        );
        let resource = service
            .resources
            .resolve(&crops[0].descriptor.resource_id)
            .expect("the crop must be resolvable");
        let bytes = service.resources.materialize(&resource).unwrap();
        let path = directory.path().join("crop.jpg");
        std::fs::write(&path, bytes).unwrap();
        let pixels = oxy_media::decode_rgb_pixels(&path).unwrap();
        let (min, max) = pixels.data.iter().fold((255u8, 0u8), |(min, max), &value| {
            (min.min(value), max.max(value))
        });
        max - min
    }

    #[test]
    fn a_crop_is_cut_from_the_full_resolution_source() {
        let (library, service, directory) = service();
        // 1600 px on the long edge is well past the 512 px bounded preview, so a
        // crop taken from the preview would have to invent this detail.
        let (encoded, _, _) = oxy_media::face_crop_jpeg(
            &checkerboard(1600),
            NormalizedRect::new(0.0, 0.0, 1.0, 1.0),
            1600,
            1.0,
            95,
        )
        .unwrap();
        let path = directory.path().join("group.jpg");
        std::fs::write(&path, encoded).unwrap();

        let revision = oxy_domain::face_source_revision_for_path(&path).unwrap();
        // A face covering a twentieth of the frame: 80 px of source pixels for a
        // 128 px crop.
        let bbox = NormalizedRect::new(0.4, 0.4, 0.05, 0.05);
        library
            .replace_asset_faces(
                &path,
                "asset-1",
                &revision,
                "detector",
                "embedder",
                &[oxy_library::StoredFace {
                    observation: FaceObservation {
                        observation_id: "obs-1".into(),
                        asset_id: "asset-1".into(),
                        asset_path: path.clone(),
                        source_revision: revision.clone(),
                        local_index: 0,
                        bbox,
                        landmarks: vec![NormalizedPoint::new(0.42, 0.42); 5],
                        detection_score: 0.99,
                        detector_fingerprint: "detector".into(),
                    },
                    embedding: vec![0.1, 0.2, 0.3],
                }],
            )
            .unwrap();

        let range = crop_luma_range(&service, &directory, "obs-1", 128);
        assert!(
            range > 150,
            "a crop of a small face must carry real pixels, not a stretched 512 px preview (range {range})"
        );
    }

    #[test]
    fn an_unknown_observation_yields_no_resource_instead_of_an_error() {
        let (_library, service, _directory) = service();
        let crops = service
            .crops(&["missing".to_string(), "also-missing".to_string()], 64)
            .expect("a stale id must not break the panel");
        assert!(crops.is_empty());
        assert!(supports_crops(Path::new("/photos/a.jpg")));
        assert!(!supports_crops(Path::new("/photos/a.txt")));
    }

    #[test]
    fn the_crop_cache_is_bounded() {
        let (_library, service, _directory) = service();
        for index in 0..(MAX_CACHED_CROPS + 16) {
            service.remember(
                &format!("obs-{index}"),
                64,
                Arc::new(CachedCrop {
                    bytes: Arc::from(vec![0u8; 4]),
                    size: 64,
                }),
            );
        }
        assert_eq!(
            service.crops.lock().unwrap().len(),
            MAX_CACHED_CROPS,
            "the cache must not grow with the library"
        );
        assert!(
            service.cached("obs-0", 64).is_none(),
            "the oldest entry is evicted first"
        );
        assert!(
            service
                .cached(&format!("obs-{}", MAX_CACHED_CROPS + 15), 64)
                .is_some()
        );
    }

    #[test]
    fn a_cached_crop_is_reused_and_registered_per_request() {
        let (_library, service, _directory) = service();
        let crop = Arc::new(CachedCrop {
            bytes: Arc::from(vec![1u8, 2, 3]),
            size: 64,
        });
        service.remember("obs-1", 64, crop);
        let first = service
            .register("obs-1", service.cached("obs-1", 64).unwrap())
            .unwrap();
        let second = service
            .register("obs-1", service.cached("obs-1", 64).unwrap())
            .unwrap();
        assert_eq!(first.observation_id, "obs-1");
        assert_ne!(
            first.descriptor.resource_id, second.descriptor.resource_id,
            "each request gets its own leaseable handle"
        );
        assert!(first.descriptor.url.starts_with("oxy-media://"));
        assert!(
            service.cached("obs-1", 128).is_none(),
            "size is part of the key"
        );
    }
}
