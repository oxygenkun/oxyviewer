//! Format-neutral metadata reads and normalization, without application state.
use crate::types::{CaptureMetadata, FocusInfo, FocusRegion};
use std::path::Path;
use thiserror::Error;

mod capture;
mod engine;
pub mod xmp;
#[cfg(test)]
pub(crate) use crate::sony_hif_fixture;
pub use engine::{
    MetadataDocument, MetadataReader, NativeMetadataReader, RawMetadataTag, heif_metadata_digest,
};
use xmp::{
    is_rejected_xmp_rating, normalize_color_label, parse_pick_label, parse_xmp_rating, xmp_value,
};
use xmp::{xmp_array_values, xmp_prefixed_value};

#[derive(Debug, Error)]
pub enum MetadataReadError {
    #[error("metadata read failed: {0}")]
    Read(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl MetadataDocument {
    /// Overlay the explicitly supplied sidecar while retaining embedded capture
    /// fields that its XMP does not contain. The caller owns sidecar discovery.
    pub fn apply_sidecar(&mut self, xml: &str) -> Result<(), MetadataReadError> {
        let tags = oxy_metadata_parser::xmp_tags(xml)
            .map_err(|error| MetadataReadError::Read(error.to_string()))?;
        self.editable = xmp::editable_from_sidecar_xml(xml);
        capture::overlay_xmp(&mut self.capture, &tags);
        Ok(())
    }
}

/// Lightweight capture facts used for chronological grouping. This deliberately
/// avoids the full native reader's HEIF item and chroma inspection.
pub fn read_capture_summary(path: &Path) -> Result<CaptureMetadata, MetadataReadError> {
    let tags = oxy_metadata_parser::tags(path)
        .map_err(|error| MetadataReadError::Read(error.to_string()))?;
    Ok(capture::from_tags(&tags, path))
}

pub(crate) fn format_number(value: f64) -> String {
    if (value - value.round()).abs() < 0.005 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}").trim_end_matches('0').to_owned()
    }
}

pub(crate) fn orient_focus_info(
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    frame_size: Option<(u32, u32)>,
    orientation: u16,
) -> FocusInfo {
    let (coordinate_width, coordinate_height, center_x, center_y, swap_frame_axes) =
        match orientation {
            2 => (width, height, width.saturating_sub(x), y, false),
            3 => (
                width,
                height,
                width.saturating_sub(x),
                height.saturating_sub(y),
                false,
            ),
            4 => (width, height, x, height.saturating_sub(y), false),
            5 => (height, width, y, x, true),
            6 => (height, width, height.saturating_sub(y), x, true),
            7 => (
                height,
                width,
                height.saturating_sub(y),
                width.saturating_sub(x),
                true,
            ),
            8 => (height, width, y, width.saturating_sub(x), true),
            _ => (width, height, x, y, false),
        };
    let (frame_width, frame_height) = frame_size
        .map(|(frame_width, frame_height)| {
            if swap_frame_axes {
                (frame_height, frame_width)
            } else {
                (frame_width, frame_height)
            }
        })
        .unzip();
    FocusInfo {
        coordinate_width,
        coordinate_height,
        regions: vec![FocusRegion {
            center_x,
            center_y,
            width: frame_width,
            height: frame_height,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotates_focus_location_and_exact_frame_size() {
        let info = orient_focus_info(7_008, 4_672, 1_204, 2_277, Some((219, 217)), 6);
        assert_eq!(info.coordinate_width, 4_672);
        assert_eq!(info.coordinate_height, 7_008);
        assert_eq!(info.regions[0].center_x, 2_395);
        assert_eq!(info.regions[0].center_y, 1_204);
        assert_eq!(info.regions[0].width, Some(217));
        assert_eq!(info.regions[0].height, Some(219));
    }
}
