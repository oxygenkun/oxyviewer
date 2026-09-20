//! Serves scan-time face crops as leased `oxy-media://` resources.
//!
//! Detection writes the 128px JPEG beside each observation. UI reads never
//! decode a Full image; a missing crop is a rebuildable cache miss repaired by
//! re-analysis, not hidden foreground media work.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use oxy_domain::MediaResourceDescriptor;
use oxy_library::Library;

pub(crate) const DEFAULT_CROP_SIZE: u32 = 128;
pub(crate) const CROP_MARGIN: f32 = 1.5;
pub(crate) const CROP_QUALITY: u8 = 85;
const MAX_CACHED_CROPS: usize = 1024;

struct CachedCrop {
    bytes: Arc<[u8]>,
    size: u32,
}

pub(crate) struct FaceCropService {
    library: Arc<Library>,
    resources: oxy_media::ResourceRegistry,
    crops: Mutex<HashMap<(String, u32), Arc<CachedCrop>>>,
    order: Mutex<VecDeque<(String, u32)>>,
}

impl FaceCropService {
    pub(crate) fn new(library: Arc<Library>, resources: oxy_media::ResourceRegistry) -> Self {
        Self {
            library,
            resources,
            crops: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
        }
    }

    pub(crate) fn clear(&self) {
        self.crops.lock().expect("face crop cache poisoned").clear();
        self.order.lock().expect("face crop order poisoned").clear();
    }

    pub(crate) fn crops(
        &self,
        observation_ids: &[String],
        size: u32,
    ) -> Result<Vec<FaceCropResource>, String> {
        let size = size.clamp(32, 512);
        let mut result = Vec::with_capacity(observation_ids.len());
        for observation_id in observation_ids {
            let crop = match self.load(observation_id, size) {
                Ok(crop) => crop,
                Err(error) if observation_ids.len() > 1 => {
                    eprintln!("face crop skipped for {observation_id}: {error}");
                    continue;
                }
                Err(error) => return Err(error),
            };
            result.push(self.register(observation_id, crop)?);
        }
        Ok(result)
    }

    fn load(&self, observation_id: &str, size: u32) -> Result<Arc<CachedCrop>, String> {
        if let Some(crop) = self.cached(observation_id, size) {
            return Ok(crop);
        }
        let (cached_size, bytes) = self
            .library
            .face_crop(observation_id, size)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("prepared crop is missing for {observation_id}"))?;
        let crop = Arc::new(CachedCrop {
            bytes: Arc::from(bytes),
            size: cached_size,
        });
        self.remember(observation_id, size, Arc::clone(&crop));
        Ok(crop)
    }

    fn cached(&self, observation_id: &str, size: u32) -> Option<Arc<CachedCrop>> {
        self.crops
            .lock()
            .expect("face crop cache poisoned")
            .get(&(observation_id.to_owned(), size))
            .cloned()
    }

    fn remember(&self, observation_id: &str, size: u32, crop: Arc<CachedCrop>) {
        let key = (observation_id.to_owned(), size);
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
        let handle = self
            .resources
            .register_encoded(
                crop.bytes.clone(),
                "image/jpeg",
                oxy_media::PixelDimensions {
                    width: crop.size,
                    height: crop.size,
                },
                oxy_media::ImageOrigin::PrimaryImage,
            )
            .map_err(|error| error.to_string())?;
        Ok(FaceCropResource {
            observation_id: observation_id.to_owned(),
            descriptor: MediaResourceDescriptor {
                resource_id: handle.descriptor.resource_id,
                url: handle.descriptor.url,
                media_type: "image/jpeg".into(),
            },
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FaceCropResource {
    pub observation_id: String,
    pub descriptor: MediaResourceDescriptor,
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_domain::{FaceObservation, NormalizedPoint, NormalizedRect};
    use std::path::Path;

    fn service() -> (Arc<Library>, FaceCropService) {
        let library = Arc::new(Library::in_memory().unwrap());
        let resources = oxy_media::ResourceRegistry::new(oxy_media::ResourceRegistryLimits::new(
            64,
            8 * 1024 * 1024,
        ));
        let service = FaceCropService::new(Arc::clone(&library), resources);
        (library, service)
    }

    fn store_observation(library: &Library) {
        library
            .replace_asset_faces(
                Path::new("/photo.jpg"),
                "asset",
                "revision",
                "detector",
                "embedder",
                &[oxy_library::StoredFace {
                    observation: FaceObservation {
                        observation_id: "obs".into(),
                        asset_id: "asset".into(),
                        asset_path: "/photo.jpg".into(),
                        source_revision: "revision".into(),
                        local_index: 0,
                        bbox: NormalizedRect::new(0.1, 0.1, 0.5, 0.5),
                        landmarks: vec![NormalizedPoint::new(0.2, 0.2); 5],
                        detection_score: 0.9,
                        detector_fingerprint: "detector".into(),
                    },
                    embedding: vec![0.1],
                    face_pixels: 64,
                    clarity: 0.5,
                }],
            )
            .unwrap();
    }

    #[test]
    fn serves_only_prepared_crops() {
        let (library, service) = service();
        store_observation(&library);
        assert!(service.crops(&["obs".into()], 64).is_err());
        library
            .store_face_crops(&[oxy_library::StoredFaceCrop {
                observation_id: "obs".into(),
                size: 128,
                jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
            }])
            .unwrap();
        let crop = service.crops(&["obs".into()], 64).unwrap();
        assert_eq!(crop.len(), 1);
        assert!(crop[0].descriptor.url.starts_with("oxy-media://"));
    }

    #[test]
    fn cache_is_bounded() {
        let (_library, service) = service();
        for index in 0..MAX_CACHED_CROPS + 1 {
            service.remember(
                &format!("obs-{index}"),
                64,
                Arc::new(CachedCrop {
                    bytes: Arc::from([1, 2, 3]),
                    size: 64,
                }),
            );
        }
        assert_eq!(service.crops.lock().unwrap().len(), MAX_CACHED_CROPS);
        assert!(service.cached("obs-0", 64).is_none());
    }
}
