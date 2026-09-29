use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("media request dimensions are zero or its allocation estimate overflows")]
    InvalidMediaRequest,
    #[error("analysis requires a nonzero minimum and a target no smaller than that minimum")]
    InvalidAnalysisRequirement,
    #[error("frame representation lacks required sampling detail or complete frame coverage")]
    UnqualifiedFrameRepresentation,
    #[error("analysis input for this media format is not implemented")]
    AnalysisFormatUnavailable,
    #[error("analysis source detail is insufficient: {available} < {required} pixels")]
    InsufficientAnalysisDetail { available: u32, required: u32 },
    #[error("format requires a native decoder that is not available")]
    NativeDecoderUnavailable,
    #[error("{backend} native decode failed: {message}")]
    NativeDecode {
        backend: &'static str,
        message: String,
    },
    #[error("LibRaw failed for {path}: {message}")]
    LibRaw { path: PathBuf, message: String },
    #[error("libheif failed for {path}: {message}")]
    Heif { path: PathBuf, message: String },
    #[error("color conversion failed: {0}")]
    Color(String),
    #[error("decode session was cancelled")]
    Cancelled,
    #[error("cache was cleared while the artifact was being produced")]
    StaleCacheGeneration,
    #[error("source changed while the artifact was being produced")]
    StaleSourceRevision,
    #[error("invalid media cache manifest: {0}")]
    CacheManifest(String),
    #[error("invalid media cache artifact: {0}")]
    CacheArtifact(String),
    #[error(
        "media resource {budget} budget is exhausted: current={current}, limit={limit}, requested={requested}"
    )]
    ResourceBudgetExhausted {
        budget: &'static str,
        current: usize,
        limit: usize,
        requested: usize,
    },
    #[error("all backend attempts failed ({attempts}): {source}")]
    BackendAttempts {
        attempts: String,
        #[source]
        source: Box<MediaError>,
    },
    #[error("system preview generation failed for {path}: {message}")]
    PreviewGenerationFailed { path: PathBuf, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
}
