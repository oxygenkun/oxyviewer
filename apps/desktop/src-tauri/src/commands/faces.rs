//! Face analysis and people commands.
//!
//! These are the only way the frontend can touch face data. Every mutation
//! routes through the Host library so the [person/user-data
//! split](../jobs/faces.rs) stays enforceable: the UI can never write analyzer
//! output directly, and a user decision can never be expressed as "delete the
//! machine row".

use std::path::PathBuf;

use oxy_domain::{
    CustomTagId, FaceAnalysisProgress, FaceAnalysisRequest, FaceAnalyzerSettings, FaceCluster,
    FaceDecision, FaceLibraryStats, FaceReviewFilter, FaceReviewPage, Person,
};
use tauri::State;

use crate::AppState;

/// Feature availability plus the effective settings, so the UI can disable the
/// people entry point without probing the filesystem.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FaceCapability {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    pub running: bool,
    pub settings: FaceAnalyzerSettings,
    pub stats: FaceLibraryStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<FaceAnalysisProgress>,
    /// File that holds persons and confirmations. It is user data, not cache.
    pub people_store_path: String,
}

#[tauri::command]
pub(crate) fn get_face_capability(state: State<'_, AppState>) -> Result<FaceCapability, String> {
    let available = state.face_queue.is_available();
    Ok(FaceCapability {
        available,
        unavailable_reason: (!available).then(|| {
            "Face models are not installed. Run `pnpm faces:prepare` or install the model pack."
                .to_owned()
        }),
        running: state.face_queue.is_running(),
        settings: state
            .library
            .face_analyzer_settings()
            .map_err(|error| error.to_string())?,
        stats: crate::jobs::faces::library_stats(&state.library)?,
        progress: state.face_queue.progress(),
        people_store_path: state.people.path().to_string_lossy().to_string(),
    })
}

/// Measured accept/reject score distributions and a suggested threshold.
#[tauri::command]
pub(crate) fn get_face_calibration(
    state: State<'_, AppState>,
) -> Result<oxy_domain::FaceCalibration, String> {
    state.people.calibration()
}

#[tauri::command]
pub(crate) fn update_face_analyzer_settings(
    settings: FaceAnalyzerSettings,
    state: State<'_, AppState>,
) -> Result<FaceAnalyzerSettings, String> {
    let previous = state
        .library
        .face_analyzer_settings()
        .map_err(|error| error.to_string())?;
    let stored = state
        .library
        .update_face_analyzer_settings(&settings)
        .map_err(|error| error.to_string())?;
    // Detection-affecting parameters change what the analyzer produces, so the
    // cached analyzer and every checkpoint that depends on it must be dropped.
    // Matching and clustering thresholds only need the cheap derived refresh.
    state.face_queue.invalidate_analyzer();
    if detection_changed(&previous, &stored) {
        // The next run re-analyzes; nothing is deleted here, and user decisions
        // are never affected.
    } else {
        let _ = state.face_queue.refresh_without_detection();
    }
    Ok(stored)
}

fn detection_changed(previous: &FaceAnalyzerSettings, next: &FaceAnalyzerSettings) -> bool {
    previous.detection_confidence != next.detection_confidence
        || previous.nms_threshold != next.nms_threshold
        || previous.min_face_pixels != next.min_face_pixels
        || previous.max_faces_per_asset != next.max_faces_per_asset
        || previous.detect_small_faces != next.detect_small_faces
}

#[tauri::command]
pub(crate) fn start_face_analysis(
    request: FaceAnalysisRequest,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.face_queue.start(request)
}

#[tauri::command]
pub(crate) fn cancel_face_analysis(job_id: String, state: State<'_, AppState>) -> bool {
    state.face_queue.cancel(&job_id)
}

#[tauri::command]
pub(crate) fn list_persons(state: State<'_, AppState>) -> Result<Vec<Person>, String> {
    state.people.persons()
}

#[tauri::command]
pub(crate) fn create_person(
    person_id: String,
    display_name: String,
    linked_tag_id: Option<CustomTagId>,
    state: State<'_, AppState>,
) -> Result<Person, String> {
    state
        .people
        .create_person(&person_id, &display_name, linked_tag_id)
}

#[tauri::command]
pub(crate) fn rename_person(
    person_id: String,
    display_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.people.rename_person(&person_id, &display_name)
}

#[tauri::command]
pub(crate) fn link_person_tag(
    person_id: String,
    tag_id: Option<CustomTagId>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.people.link_person_tag(&person_id, tag_id)
}

#[tauri::command]
pub(crate) fn delete_person(
    person_id: String,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    state.people.delete_person(&person_id)
}

/// Folds one person into another. Reversible while the operation stays in the
/// durable journal.
#[tauri::command]
pub(crate) fn merge_persons(
    source_person_id: String,
    target_person_id: String,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    state
        .people
        .merge_persons(&source_person_id, &target_person_id)
}

/// Detaches specific faces from a person without touching the rest.
#[tauri::command]
pub(crate) fn remove_faces_from_person(
    person_id: String,
    observation_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    state
        .people
        .remove_faces_from_person(&person_id, &observation_ids)
}

/// Confirms specific faces for a person, replacing previous answers.
#[tauri::command]
pub(crate) fn assign_faces_to_person(
    person_id: String,
    observation_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    state
        .people
        .assign_faces_to_person(&person_id, &observation_ids)
}

#[tauri::command]
pub(crate) fn get_person_undo(
    state: State<'_, AppState>,
) -> Result<Option<oxy_userdata::UndoableOperation>, String> {
    Ok(state.people.undoable())
}

#[tauri::command]
pub(crate) fn undo_person_operation(
    state: State<'_, AppState>,
) -> Result<Option<oxy_userdata::UndoableOperation>, String> {
    state.people.undo()
}

#[tauri::command]
pub(crate) fn decide_face(
    observation_id: String,
    decision: FaceDecision,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Durable store first: a confirmation the application cannot persist must
    // not become visible.
    state.people.decide(&observation_id, &decision)
}

#[tauri::command]
pub(crate) fn clear_face_decision(
    observation_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.people.clear_decision(&observation_id)
}

#[tauri::command]
pub(crate) fn get_face_review_page(
    filter: FaceReviewFilter,
    cursor: usize,
    page_size: usize,
    state: State<'_, AppState>,
) -> Result<FaceReviewPage, String> {
    state
        .library
        .face_review_page(filter, cursor, page_size.clamp(1, 500))
        .map_err(|error| error.to_string())
}

/// Face crops as `oxy-media://` resources, so no image bytes cross JSON IPC.
///
/// Observation ids that no longer exist are skipped instead of failing, which
/// keeps a review panel usable after a re-analysis.
/// Review rows for one asset, so the loupe can draw boxes with the user's
/// current answer instead of re-deriving it in the frontend.
#[tauri::command]
pub(crate) fn get_asset_face_reviews(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Vec<oxy_domain::FaceReviewItem>, String> {
    state
        .library
        .face_reviews_for_asset(&path)
        .map_err(|error| error.to_string())
}

/// The asset one face belongs to, so the workbench can reveal it in the main
/// window's loupe. `None` is an ordinary answer: a re-analysis can retire the
/// observation a click refers to.
#[tauri::command]
pub(crate) fn resolve_face_observation(
    observation_id: String,
    state: State<'_, AppState>,
) -> Result<Option<oxy_domain::FaceAssetReveal>, String> {
    let observation = state
        .library
        .face_observation(&observation_id)
        .map_err(|error| error.to_string())?;
    Ok(observation.map(|observation| oxy_domain::FaceAssetReveal {
        observation_id: observation.observation_id,
        asset_id: observation.asset_id,
        asset_path: observation.asset_path,
    }))
}

#[tauri::command]
pub(crate) async fn get_face_crops(
    observation_ids: Vec<String>,
    size: Option<u32>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::state::face_crops::FaceCropResource>, String> {
    let size = size.unwrap_or(crate::state::face_crops::DEFAULT_CROP_SIZE);
    let crops = std::sync::Arc::clone(&state.face_crops);
    tauri::async_runtime::spawn_blocking(move || crops.crops(&observation_ids, size))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn get_face_clusters(state: State<'_, AppState>) -> Result<Vec<FaceCluster>, String> {
    state
        .library
        .face_clusters_current()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_asset_face_observations(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Vec<oxy_domain::FaceObservation>, String> {
    state
        .library
        .face_observations_for_asset(&path)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_detection_parameters_force_re_analysis() {
        let base = FaceAnalyzerSettings::default();

        // Matcher and cluster thresholds are downstream of detection: changing
        // them must never invalidate stored detections.
        let matcher = FaceAnalyzerSettings {
            match_sensitivity: oxy_domain::FaceMatchSensitivity::Loose,
            match_threshold: 0.8,
            cluster_threshold: 0.5,
            auto_accept_threshold: Some(0.7),
            ..base
        };
        assert!(!detection_changed(&base, &matcher));

        // Detection parameters must invalidate everything downstream.
        for changed in [
            FaceAnalyzerSettings {
                detection_confidence: 0.7,
                ..base
            },
            FaceAnalyzerSettings {
                nms_threshold: 0.5,
                ..base
            },
            FaceAnalyzerSettings {
                min_face_pixels: 64,
                ..base
            },
            FaceAnalyzerSettings {
                max_faces_per_asset: 8,
                ..base
            },
            FaceAnalyzerSettings {
                detect_small_faces: !base.detect_small_faces,
                ..base
            },
        ] {
            assert!(
                detection_changed(&base, &changed),
                "a detection-affecting change must invalidate the analyzer"
            );
        }
    }
}
