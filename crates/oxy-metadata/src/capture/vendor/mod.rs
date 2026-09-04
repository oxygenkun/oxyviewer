use super::{TagLookup, format::ImageType};
use oxy_domain::CaptureMetadata;

mod sony;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CameraVendor {
    Sony,
    Canon,
    Nikon,
    Fujifilm,
    Panasonic,
    Olympus,
    Apple,
    Other,
}

impl CameraVendor {
    pub(super) fn from_make(make: Option<&str>) -> Self {
        let make = make.unwrap_or_default().trim().to_ascii_lowercase();
        if make.contains("sony") {
            Self::Sony
        } else if make.contains("canon") {
            Self::Canon
        } else if make.contains("nikon") {
            Self::Nikon
        } else if make.contains("fujifilm") || make.contains("fuji") {
            Self::Fujifilm
        } else if make.contains("panasonic") {
            Self::Panasonic
        } else if make.contains("olympus") || make.contains("om digital") {
            Self::Olympus
        } else if make.contains("apple") {
            Self::Apple
        } else {
            Self::Other
        }
    }
}

pub(super) fn apply(
    vendor: CameraVendor,
    image_type: ImageType,
    values: &TagLookup<'_>,
    capture: &mut CaptureMetadata,
) {
    match vendor {
        CameraVendor::Sony => sony::apply(image_type, values, capture),
        CameraVendor::Canon
        | CameraVendor::Nikon
        | CameraVendor::Fujifilm
        | CameraVendor::Panasonic
        | CameraVendor::Olympus
        | CameraVendor::Apple
        | CameraVendor::Other => {}
    }
}
