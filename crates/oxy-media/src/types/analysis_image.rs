//! Consumer requirements for a complete, display-oriented analysis frame.
//! This contract describes detail, not a viewing level or a decoder.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisRequirement {
    /// Reject an original smaller than this; never report it as an empty image.
    pub minimum_source_long_edge: u32,
    /// Return this much detail when the original has it, without upscaling.
    /// An embedded preview must meet this edge AND the corresponding short edge
    /// (1% short-edge tolerance for camera crop versus sensor active-area borders).
    pub target_long_edge: u32,
}
