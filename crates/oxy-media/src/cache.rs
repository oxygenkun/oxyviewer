//! Rebuildable preview artifact storage.

mod encode;
mod model;
mod v2;

pub(crate) use encode::{ensure_srgb_icc, write_jpeg_atomically};

pub use model::{
    ArtifactLocation, ArtifactPresentation, ArtifactRepresentation, CacheRequest, ColorRequirement,
    ColorState as CacheColorState, DetailRequirement, DisplayDimensions,
    MEDIA_CACHE_POLICY_REVISION, MediaArtifact, OrientationRequirement, OrientationState,
    PresentationRequirement, RepresentationRequirement, Satisfaction, SharpeningState,
    SourceRevision, VariantIdentity, satisfies,
};
pub(crate) use model::{LARGEST_RAW_JPEG_TARGET, candidate_rank};
pub use v2::{
    ArtifactLease, CacheHit, CacheLookup, CachePublication, CacheUsage, DiskMediaCache, MediaCache,
    MemoryMediaCache, PendingArtifact, PendingStagedArtifact,
};
