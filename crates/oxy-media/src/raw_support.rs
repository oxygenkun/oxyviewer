//! System codec discovery and recent file-specific failures. All native work is
//! outside the state mutex; callers run discovery and decoding on media workers.

use oxy_domain::RawDecoderStatus;
use std::path::Path;

/// Does not decode the selected file; reports only a previously attempted decode.
pub fn raw_decoder_status(path: Option<&Path>, refresh: bool) -> RawDecoderStatus {
    #[cfg(target_os = "windows")]
    return windows::status(path, refresh);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (path, refresh);
        RawDecoderStatus {
            install_available: false,
            availability: oxy_domain::RawSystemAvailability::UnsupportedPlatform,
            codecs: Vec::new(),
            detail: None,
            attempt: None,
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use windows::decode;

pub fn raw_retry_revision(source_revision: &str) -> Option<u64> {
    #[cfg(target_os = "windows")]
    return windows::retry_revision(source_revision);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = source_revision;
        None
    }
}

pub fn request_raw_retry(path: &Path) -> Result<(), crate::MediaError> {
    #[cfg(target_os = "windows")]
    return windows::request_retry(path);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use crate::{
        MediaError, SourceRevision, backends::windows_wic::raw, media_source::DecodedImage,
    };
    use oxy_domain::{RawSystemAttempt, RawSystemAttemptState, RawSystemAvailability};
    use oxy_runtime::CancellationToken;
    use std::{
        collections::VecDeque,
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };

    struct Attempt {
        revision: String,
        value: RawSystemAttempt,
        at: Instant,
    }
    #[derive(Default)]
    struct State {
        catalog: Option<RawDecoderStatus>,
        epoch: u64,
        attempts: VecDeque<Attempt>,
        retries: VecDeque<(String, u64)>,
    }
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    fn state() -> &'static Mutex<State> {
        STATE.get_or_init(Mutex::default)
    }

    pub(super) fn retry_revision(revision: &str) -> Option<u64> {
        state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retries
            .iter()
            .find(|(key, _)| key == revision)
            .map(|(_, nonce)| *nonce)
    }

    pub(super) fn request_retry(path: &Path) -> Result<(), MediaError> {
        let revision = oxy_fs::observe_source_revision(path)?.revision_id;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| MediaError::CacheArtifact(error.to_string()))?
            .as_nanos() as u64;
        let mut state = state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = state
            .retries
            .iter()
            .find(|(key, _)| key == &revision)
            .map_or(0, |(_, value)| *value);
        state
            .attempts
            .retain(|attempt| attempt.revision != revision);
        state.retries.retain(|(key, _)| key != &revision);
        state.epoch = state.epoch.wrapping_add(1);
        state
            .retries
            .push_back((revision, nonce.max(previous.saturating_add(1))));
        while state.retries.len() > 256 {
            state.retries.pop_front();
        }
        Ok(())
    }

    pub(super) fn status(path: Option<&Path>, refresh: bool) -> RawDecoderStatus {
        let revision = path.and_then(|path| oxy_fs::observe_source_revision(path).ok());
        let (catalog, epoch) = {
            let mut state = state()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if refresh {
                state.epoch = state.epoch.wrapping_add(1);
                state.catalog = None;
                state.attempts.clear();
            }
            (state.catalog.clone(), state.epoch)
        };
        let mut catalog = catalog.unwrap_or_else(|| {
            let (availability, codecs, detail) = match raw::codecs() {
                Ok(codecs) if codecs.is_empty() => (RawSystemAvailability::Missing, codecs, None),
                Ok(codecs) => (RawSystemAvailability::Available, codecs, None),
                Err(error) => (
                    RawSystemAvailability::Unavailable,
                    Vec::new(),
                    Some(error.to_string()),
                ),
            };
            let catalog = RawDecoderStatus {
                install_available: false,
                availability,
                codecs,
                detail,
                attempt: None,
            };
            let mut state = state()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.epoch == epoch {
                state.catalog = Some(catalog.clone());
            }
            catalog
        });
        catalog.install_available = !catalog.codecs.iter().any(|codec| {
            codec
                .decoder_id
                .eq_ignore_ascii_case("41945702-8302-44A6-9445-AC98E8AFA086")
        });
        if catalog.availability == RawSystemAvailability::Available
            && let Some(extension) = path
                .and_then(Path::extension)
                .and_then(|value| value.to_str())
            && !catalog.codecs.iter().any(|codec| {
                codec.extensions.split(',').any(|candidate| {
                    candidate
                        .trim()
                        .trim_start_matches('.')
                        .eq_ignore_ascii_case(extension)
                })
            })
        {
            catalog.availability = RawSystemAvailability::Missing;
            catalog.detail = Some(format!(
                "No system RAW codec advertises .{extension} support"
            ));
        }
        if let Some(revision) = revision {
            let state = state()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            catalog.attempt = state
                .attempts
                .iter()
                .find(|attempt| attempt.revision == revision.revision_id)
                .map(|attempt| attempt.value.clone());
        }
        catalog
    }

    pub(crate) fn decode(
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<DecodedImage, MediaError> {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let revision = oxy_fs::observe_source_revision(path)?;
        let epoch = {
            let state = state()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(previous) = state.attempts.iter().find(|attempt| {
                attempt.revision == revision.revision_id
                    && attempt.value.state != RawSystemAttemptState::Ready
                    && attempt.at.elapsed() < Duration::from_secs(30)
            }) {
                return Err(MediaError::NativeDecode {
                    backend: "Windows WIC RAW",
                    message: previous.value.detail.clone().unwrap_or_default(),
                });
            }
            state.epoch
        };
        let outcome = raw::decode(path, cancellation);
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let value = match &outcome {
            Ok(_) => RawSystemAttempt {
                state: RawSystemAttemptState::Ready,
                detail: None,
            },
            Err(
                MediaError::Cancelled
                | MediaError::StaleSourceRevision
                | MediaError::StaleCacheGeneration,
            ) => return outcome,
            Err(error) => RawSystemAttempt {
                state: if matches!(
                    error,
                    MediaError::NativeDecode { .. } | MediaError::Color(_)
                ) {
                    RawSystemAttemptState::UnsupportedFile
                } else {
                    RawSystemAttemptState::Failed
                },
                detail: Some(error.to_string()),
            },
        };
        let mut state = state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.epoch == epoch {
            state
                .attempts
                .retain(|attempt| attempt.revision != revision.revision_id);
            state.attempts.push_back(Attempt {
                revision: revision.revision_id,
                value,
                at: Instant::now(),
            });
            while state.attempts.len() > 64 {
                state.attempts.pop_front();
            }
        }
        outcome
    }
}
