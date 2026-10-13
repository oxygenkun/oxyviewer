use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RawSystemAvailability {
    Available,
    Missing,
    Unavailable,
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RawSystemCodec {
    pub name: String,
    pub decoder_id: String,
    pub version: String,
    pub extensions: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RawSystemAttemptState {
    Ready,
    UnsupportedFile,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RawSystemAttempt {
    pub state: RawSystemAttemptState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RawDecoderStatus {
    pub install_available: bool,
    pub availability: RawSystemAvailability,
    pub codecs: Vec<RawSystemCodec>,
    pub detail: Option<String>,
    pub attempt: Option<RawSystemAttempt>,
}
