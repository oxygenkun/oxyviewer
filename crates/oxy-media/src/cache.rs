//! Rebuildable preview artifact storage.

mod encode;
mod model;
mod store;
mod v2;

pub(crate) use encode::{ensure_srgb_icc, write_jpeg_atomically};

pub(crate) use model::candidate_rank;
pub use model::{
    ArtifactLocation, ArtifactPresentation, ArtifactRepresentation, CacheRequest, ColorRequirement,
    ColorState as CacheColorState, DetailRequirement, DisplayDimensions,
    MEDIA_CACHE_POLICY_REVISION, MediaArtifact, OrientationRequirement, OrientationState,
    PresentationRequirement, RepresentationRequirement, Satisfaction, SharpeningState,
    SourceRevision, VariantIdentity, satisfies,
};
pub use store::{CacheUsage, clear_preview_cache, preview_cache_usage, prune_preview_cache};
pub use v2::{
    ArtifactLease, CacheHit, CacheLookup, CachePublication, DiskMediaCache, MediaCache,
    MemoryMediaCache, PendingArtifact, PendingStagedArtifact, V2CacheUsage,
};
