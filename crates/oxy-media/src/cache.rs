//! Rebuildable preview artifact storage.

mod encode;
mod model;
mod store;

pub(crate) use encode::{ensure_srgb_icc, write_jpeg_atomically};

pub(crate) use model::candidate_rank;
pub use model::{
    ArtifactLocation, ArtifactPresentation, ArtifactRequirement, CacheRequest, ColorRequirement,
    ColorState as CacheColorState, DetailRequirement, ImageOrigin, MEDIA_CACHE_POLICY_REVISION,
    MediaArtifact, OrientationRequirement, OrientationState, PixelDimensions,
    PresentationRequirement, Satisfaction, SharpeningState, SourceRevision, VariantIdentity,
    satisfies,
};
pub use store::{
    ArtifactLease, CacheHit, CacheLookup, CachePublication, CacheUsage, DiskMediaCache, MediaCache,
    MemoryMediaCache, PendingArtifact, PendingStagedArtifact,
};
