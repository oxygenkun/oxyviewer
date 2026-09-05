//! Bounded, request-time source probing.
//!
//! Probing is invoked only after a media representation is requested. It must
//! never be added to directory discovery or first-page enumeration.

use crate::formats::heif::quirks::sony;
use oxy_domain::AssetKind;
use std::path::Path;

use crate::pipeline::planner::{Presence, SourceFacts, Vendor};

#[derive(Debug)]
pub(crate) struct ProbedSource {
    pub(crate) facts: SourceFacts,
    pub(crate) heif_fast_jpeg: Option<sony::EmbeddedJpeg>,
}

impl ProbedSource {
    pub(crate) const fn unprobed(kind: AssetKind) -> Self {
        Self {
            facts: SourceFacts::unprobed(kind),
            heif_fast_jpeg: None,
        }
    }
}

/// Identify Sony's bounded sidebar JPEG and retain it for execution. Probe
/// failures deliberately leave the fact unknown: normal HEIF decoding remains
/// available and reports the authoritative source/backend error.
pub(crate) fn heif(path: &Path) -> ProbedSource {
    let Ok(inspection) = sony::inspect(path, None) else {
        return ProbedSource::unprobed(AssetKind::Heif);
    };
    let presence = if inspection.embedded_jpeg.is_some() {
        Presence::Present
    } else {
        Presence::Absent
    };
    ProbedSource {
        facts: SourceFacts {
            kind: AssetKind::Heif,
            vendor: if inspection.is_sony {
                Vendor::Sony
            } else {
                Vendor::Other
            },
            heif_fast_jpeg: presence,
        },
        heif_fast_jpeg: inspection.embedded_jpeg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn non_sony_input_has_no_fast_representation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("generic.heic");
        fs::write(&path, b"generic HEIF bytes").unwrap();

        let probed = heif(&path);
        assert_eq!(probed.facts.vendor, Vendor::Other);
        assert_eq!(probed.facts.heif_fast_jpeg, Presence::Absent);
        assert!(probed.heif_fast_jpeg.is_none());
    }

    #[test]
    fn probe_does_not_scan_past_the_bounded_header_window() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("late-shif.heic");
        let mut bytes = vec![0; 2 * 1024 * 1024 + 4];
        bytes[2 * 1024 * 1024..].copy_from_slice(b"SHIF");
        fs::write(&path, bytes).unwrap();

        let probed = heif(&path);
        assert_eq!(probed.facts.vendor, Vendor::Other);
        assert_eq!(probed.facts.heif_fast_jpeg, Presence::Absent);
    }

    #[test]
    fn repository_fixture_is_recognized_by_representation_fact() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let probed = heif(&path);

        assert_eq!(probed.facts.vendor, Vendor::Sony);
        assert_eq!(probed.facts.heif_fast_jpeg, Presence::Present);
        let jpeg = probed.heif_fast_jpeg.unwrap();
        assert_eq!((jpeg.width, jpeg.height), (120, 160));
    }
}
