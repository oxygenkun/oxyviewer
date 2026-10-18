use super::{
    MetadataError, MetadataFacade, MetadataReader, NativeMetadataReader, embedded_command,
    exiftool_output,
    exiftool_session::{Session, WriteError},
    read_sidecar,
    types::MetadataPatch,
};
use crate::files::sidecar_path;
use oxy_media::types::EditableMetadata;
use std::{ffi::OsString, path::Path};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FileSync {
    Updated,
    Unchanged,
    Cancelled,
}

impl MetadataFacade {
    pub(super) fn sync_embedded_file(
        &self,
        path: &Path,
        session: &mut Option<Session>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<FileSync, WriteError> {
        if !sidecar_path(path).is_file() {
            return Err(MetadataError::SidecarUnavailable(path.to_path_buf()).into());
        }
        let desired = read_sidecar(path)?;
        // Read the actual embedded values. The normal application facade overlays
        // the sidecar and would incorrectly classify every file as unchanged.
        if let Ok(document) = NativeMetadataReader.read(path, None)
            && document.diagnostics.is_empty()
            && editable_matches(&desired, &document.editable)
        {
            return Ok(FileSync::Unchanged);
        }
        if cancelled() {
            return Ok(FileSync::Cancelled);
        }
        let patch = MetadataPatch {
            rating: Some(desired.rating),
            color_label: Some(desired.color_label),
            pick_label: Some(desired.pick_label),
            keywords: Some(desired.keywords),
            hierarchical_keywords: Some(desired.hierarchical_keywords),
            ..MetadataPatch::default()
        };
        let executable = self.exiftool();
        let mut command = embedded_command(path, &patch, executable.as_deref())?;
        let args = command.get_args().map(OsString::from).collect::<Vec<_>>();
        if Session::supports(&args) {
            if session
                .as_ref()
                .is_some_and(|session| !session.is_for(executable.as_deref()))
            {
                *session = None;
            }
            if session.is_none() {
                *session = Some(Session::start(executable.as_deref())?);
            }
            if cancelled() {
                return Ok(FileSync::Cancelled);
            }
            session
                .as_mut()
                .expect("initialized session")
                .execute(&args)?;
        } else {
            let output = exiftool_output(&mut command).map_err(|error| WriteError {
                error,
                uncertain: true,
            })?;
            if !output.status.success() {
                return Err(WriteError {
                    error: MetadataError::Read(
                        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                    ),
                    uncertain: true,
                });
            }
        }
        self.invalidate_summary_path(path);
        Ok(FileSync::Updated)
    }
}

fn editable_matches(desired: &EditableMetadata, embedded: &EditableMetadata) -> bool {
    // Sony's explicit zero/None sentinels and absent optional markings represent
    // the same user state. Sort lists without deduplicating embedded values, so
    // previously duplicated keywords are repaired by the next explicit sync.
    desired.rating.unwrap_or(0) == embedded.rating.unwrap_or(0)
        && desired.color_label == embedded.color_label
        && desired.pick_label == embedded.pick_label
        && sorted(&desired.keywords) == sorted(&embedded.keywords)
        && sorted(&desired.hierarchical_keywords) == sorted(&embedded.hierarchical_keywords)
}

fn sorted(values: &[String]) -> Vec<&str> {
    let mut values = values.iter().map(String::as_str).collect::<Vec<_>>();
    values.sort_unstable();
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_media::types::{AssetKind, PickLabel};
    use std::fs;

    fn exercise(path: &Path, kind: AssetKind) {
        if super::super::probe_exiftool(None).is_err() {
            eprintln!("skipping embedded write round trip: ExifTool unavailable");
            return;
        }
        let metadata = MetadataFacade::default();
        let patch = MetadataPatch {
            rating: Some(Some(4)),
            color_label: Some(Some("Blue".into())),
            pick_label: Some(Some(PickLabel::Accepted)),
            keywords: Some(vec!["alpha".into(), "中文,标签".into()]),
            hierarchical_keywords: Some(vec!["人物|甲".into(), "地点|乙".into()]),
            ..MetadataPatch::default()
        };
        metadata.patch_metadata(path, kind, &patch).unwrap();
        let mut session = None;
        assert_eq!(
            metadata
                .sync_embedded_file(path, &mut session, &|| false)
                .unwrap(),
            FileSync::Updated
        );
        let embedded = NativeMetadataReader.read(path, None).unwrap();
        assert_eq!(embedded.editable.rating, Some(4));
        assert_eq!(embedded.editable.color_label.as_deref(), Some("Blue"));
        assert_eq!(embedded.editable.pick_label, Some(PickLabel::Accepted));
        assert_eq!(
            sorted(&embedded.editable.keywords),
            sorted(patch.keywords.as_ref().unwrap())
        );
        assert_eq!(
            sorted(&embedded.editable.hierarchical_keywords),
            sorted(patch.hierarchical_keywords.as_ref().unwrap())
        );
        let first = fs::read(path).unwrap();
        let before = fs::metadata(path).unwrap().modified().unwrap();
        assert_eq!(
            metadata
                .sync_embedded_file(path, &mut session, &|| false)
                .unwrap(),
            FileSync::Unchanged
        );
        assert_eq!(fs::read(path).unwrap(), first);
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), before);

        // External embedded changes must not be hidden by the sidecar overlay.
        let external = MetadataPatch {
            rating: Some(Some(2)),
            ..MetadataPatch::default()
        };
        let command = embedded_command(path, &external, None).unwrap();
        let args = command.get_args().map(OsString::from).collect::<Vec<_>>();
        if Session::supports(&args) {
            session.as_mut().unwrap().execute(&args).unwrap();
        } else {
            assert!(
                exiftool_output(&mut embedded_command(path, &external, None).unwrap())
                    .unwrap()
                    .status
                    .success()
            );
        }
        assert_eq!(
            metadata
                .sync_embedded_file(path, &mut session, &|| false)
                .unwrap(),
            FileSync::Updated
        );
        metadata
            .patch_metadata(
                path,
                kind,
                &MetadataPatch {
                    rating: Some(None),
                    color_label: Some(None),
                    pick_label: Some(None),
                    keywords: Some(vec!["replacement".into()]),
                    hierarchical_keywords: Some(Vec::new()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert_eq!(
            metadata
                .sync_embedded_file(path, &mut session, &|| false)
                .unwrap(),
            FileSync::Updated
        );
        let embedded = NativeMetadataReader.read(path, None).unwrap().editable;
        assert_eq!(embedded.rating.unwrap_or(0), 0);
        assert_eq!(embedded.color_label, None);
        assert_eq!(embedded.pick_label, None);
        assert_eq!(embedded.keywords, ["replacement"]);
        assert!(embedded.hierarchical_keywords.is_empty());
        assert_eq!(
            metadata
                .sync_embedded_file(path, &mut session, &|| false)
                .unwrap(),
            FileSync::Unchanged
        );
        metadata
            .patch_metadata(
                path,
                kind,
                &MetadataPatch {
                    keywords: Some(Vec::new()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert_eq!(
            metadata
                .sync_embedded_file(path, &mut session, &|| false)
                .unwrap(),
            FileSync::Updated
        );
        assert!(
            NativeMetadataReader
                .read(path, None)
                .unwrap()
                .editable
                .keywords
                .is_empty()
        );
    }

    #[test]
    fn jpeg_round_trip_replaces_lists_skips_equal_values_and_detects_external_changes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("photo.jpg");
        image::RgbImage::new(8, 8).save(&path).unwrap();
        exercise(&path, AssetKind::Jpeg);
    }

    #[test]
    fn hif_round_trip_preserves_sony_markings_and_does_not_duplicate_keywords() {
        let Some(fixture) = super::super::sony_hif_fixture() else {
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("photo.HIF");
        fs::copy(fixture, &path).unwrap();
        exercise(&path, AssetKind::Heif);
    }

    #[cfg(unix)]
    #[test]
    fn newline_in_filename_uses_safe_argv_transport() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("line\nbreak.jpg");
        image::RgbImage::new(8, 8).save(&path).unwrap();
        exercise(&path, AssetKind::Jpeg);
    }

    #[test]
    fn list_order_is_irrelevant_but_existing_duplicates_require_repair() {
        let desired = EditableMetadata {
            keywords: vec!["a".into(), "b".into()],
            ..EditableMetadata::default()
        };
        let mut actual = desired.clone();
        actual.keywords.reverse();
        actual.rating = Some(0);
        assert!(editable_matches(&desired, &actual));
        actual.keywords.push("a".into());
        assert!(!editable_matches(&desired, &actual));
    }
}
