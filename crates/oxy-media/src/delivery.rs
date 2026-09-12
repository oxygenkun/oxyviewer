//! Encoded-image delivery limits shared by producers and cache matching.
use crate::media_source::PixelDimensions;

pub(crate) const THUMBNAIL_EDGE: u32 = 512;
pub(crate) const THUMBNAIL_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const THUMBNAIL_LIMITS: DeliveryLimits = DeliveryLimits {
    max_edge: THUMBNAIL_EDGE,
    max_bytes: THUMBNAIL_BYTES,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct DeliveryLimits {
    pub max_edge: u32,
    pub max_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    Direct,
    NeedsConversion,
}

impl DeliveryLimits {
    pub(crate) fn classify(self, dimensions: PixelDimensions, bytes: u64) -> Delivery {
        if dimensions.width <= self.max_edge
            && dimensions.height <= self.max_edge
            && bytes <= self.max_bytes
        {
            Delivery::Direct
        } else {
            Delivery::NeedsConversion
        }
    }

    pub(crate) fn accepts(self, dimensions: PixelDimensions, bytes: u64) -> bool {
        self.classify(dimensions, bytes) == Delivery::Direct
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_checks_dimensions_and_encoded_bytes_independently() {
        let dimensions = PixelDimensions {
            width: 512,
            height: 340,
        };
        assert!(THUMBNAIL_LIMITS.accepts(dimensions, THUMBNAIL_BYTES));
        assert_eq!(
            THUMBNAIL_LIMITS.classify(dimensions, THUMBNAIL_BYTES + 1),
            Delivery::NeedsConversion
        );
        assert!(!THUMBNAIL_LIMITS.accepts(
            PixelDimensions {
                width: 513,
                height: 340
            },
            100
        ));
    }
}
