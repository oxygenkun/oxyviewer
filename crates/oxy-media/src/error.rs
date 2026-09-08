use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
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
    #[error("media resource budget is exhausted")]
    ResourceBudgetExhausted,
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
