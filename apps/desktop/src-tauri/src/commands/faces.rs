//! Face analysis and people commands.
//!
//! These are the only way the frontend can touch face data. Every mutation
//! routes through the Host library so the [person/user-data
//! split](../jobs/faces.rs) stays enforceable: the UI can never write analyzer
//! output directly, and a user decision can never be expressed as "delete the
//! machine row".

use std::{path::PathBuf, sync::Arc};

use oxy_domain::{
    CustomTagId, FaceAnalysisRequest, FaceAnalyzerSettings, FaceCapability, FaceCluster,
    FaceDecision, FaceReviewFilter, FaceReviewPage, Person,
};
use tauri::{AppHandle, Emitter, State};

use crate::AppState;

#[tauri::command]
pub(crate) fn get_face_capability(state: State<'_, AppState>) -> Result<FaceCapability, String> {
    let available = state.face_queue.is_available();
    Ok(FaceCapability {
        available,
        unavailable_reason: None,
        running: state.face_queue.is_running(),
        settings: state
            .library
            .face_analyzer_settings()
            .map_err(|error| error.to_string())?,
        stats: crate::jobs::faces::library_stats(&state.library)?,
        progress: state.face_queue.progress(),
        models: crate::providers::face_models::statuses(&state.face_queue.app_data_dir()?),
        people_store_path: state.people.path().to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub(crate) async fn install_face_model(
    model_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<FaceCapability, String> {
    if state.face_queue.is_running() {
        return Err("wait for face analysis to finish before installing models".into());
    }
    let model = crate::providers::face_models::ManagedFaceModel::parse(&model_id)
        .ok_or_else(|| format!("unknown face model: {model_id}"))?;
    let data_dir = state.face_queue.app_data_dir()?;
    let activates_managed_pair = crate::providers::face_models::installed_pair(&data_dir).is_none();
    let install_result = tauri::async_runtime::spawn_blocking({
        let data_dir = data_dir.clone();
        let app = app.clone();
        move || {
            crate::providers::face_models::install(&data_dir, model, |progress| {
                let _ = app.emit(
                    crate::providers::face_models::FACE_MODEL_DOWNLOAD_PROGRESS_EVENT,
                    progress,
                );
            })
        }
    })
    .await
    .map_err(|error| error.to_string())?;
    if let Err(message) = install_result {
        let _ = app.emit(
            crate::providers::face_models::FACE_MODEL_DOWNLOAD_PROGRESS_EVENT,
            oxy_domain::FaceModelDownloadProgress {
                model_id: model.id().to_owned(),
                stage: oxy_domain::FaceModelDownloadStage::Failed,
                downloaded_bytes: 0,
                total_bytes: model.download_size(),
                message: Some(message.clone()),
            },
        );
        return Err(message);
    }
    if let Some(paths) = crate::providers::face_models::installed_pair(&data_dir) {
        state.face_queue.install_models(paths)?;
        if activates_managed_pair {
            state.face_queue.clear_cache()?;
        }
    }
    get_face_capability(state)
}

#[tauri::command]
pub(crate) fn clear_face_analysis_data(state: State<'_, AppState>) -> Result<(), String> {
    state.face_queue.clear_cache()?;
    state.face_crops.clear();
    Ok(())
}

#[tauri::command]
pub(crate) fn delete_people_annotations(state: State<'_, AppState>) -> Result<(), String> {
    state.face_queue.clear_cache()?;
    state.people.delete_all_annotations()?;
    state.face_crops.clear();
    Ok(())
}

#[tauri::command]
pub(crate) fn get_face_sync_status(state: State<'_, AppState>) -> Vec<oxy_domain::FaceSyncStatus> {
    state.people.sync_status()
}

#[tauri::command]
pub(crate) fn resolve_face_sync_conflict(
    path: PathBuf,
    use_remote: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.people.resolve_sync_conflict(&path, use_remote)
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
    state
        .face_queue
        .invalidate_analyzer(detection_changed(&previous, &settings.sanitized()))?;
    let stored = state
        .library
        .update_face_analyzer_settings(&settings)
        .map_err(|error| error.to_string())?;
    // Detection-affecting parameters change what the analyzer produces, so the
    // cached analyzer and every checkpoint that depends on it must be dropped.
    // Matching and clustering thresholds only need the cheap derived refresh.

    if detection_changed(&previous, &stored) {
        // The next run re-analyzes; nothing is deleted here, and user decisions
        // are never affected.
    } else {
        state.face_queue.refresh_without_detection()?;
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
pub(crate) async fn start_face_analysis(
    request: FaceAnalysisRequest,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let queue = state.face_queue.clone();
    tauri::async_runtime::spawn_blocking(move || queue.start(request))
        .await
        .map_err(|error| error.to_string())?
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
pub(crate) async fn set_person_tag_path(
    person_id: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || people.set_person_tag_path(&person_id, &path))
        .await
        .map_err(|error| error.to_string())?
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
pub(crate) async fn set_face_clarity(
    observation_ids: Vec<String>,
    blurry: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let people = std::sync::Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || people.set_face_clarity(&observation_ids, blurry))
        .await
        .map_err(|error| error.to_string())?
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
    cursor: usize,
    page_size: usize,
    state: State<'_, AppState>,
) -> Result<FaceReviewPage, String> {
    let mut page = state
        .library
        .face_review_page(FaceReviewFilter::All, cursor, page_size.clamp(1, 500))
        .map_err(|error| error.to_string())?;
    state.people.apply_clarity_marks(&mut page.items);
    Ok(page)
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
    let mut items = state
        .library
        .face_reviews_for_asset(&path)
        .map_err(|error| error.to_string())?;
    state.people.apply_clarity_marks(&mut items);
    Ok(items)
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
