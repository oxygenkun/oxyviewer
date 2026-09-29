//! Only this stage publishes detections/features and progress. Existing ledger
//! methods reobserve source identity and fence generation/claim in transactions.
use super::operations::PersonOperation;
use crate::{People, features::PersonFeature};
use oxy_domain::{DetectedPersonInstance, PersonAnalysisRun, PersonAnalysisTask};

pub(super) struct FaceOutput {
    pub detections: Vec<DetectedPersonInstance>,
    pub features: Result<Vec<PersonFeature>, String>,
}
pub(super) struct InferredAsset {
    pub task: PersonAnalysisTask,
    pub output: Result<FaceOutput, String>,
}

fn progress(operation: &PersonOperation, run: PersonAnalysisRun, error: Option<String>) {
    operation.update(|s| {
        s.completed = run.completed_tasks + run.failed_tasks;
        s.total = run.total_tasks;
        s.run = Some(run);
        if error.is_some() {
            s.error = error;
        }
    });
}

fn failed(
    people: &People,
    task: &PersonAnalysisTask,
    mut error: String,
    operation: &PersonOperation,
) -> Result<(), String> {
    while error.len() > 2048 {
        error.pop();
    }
    if error.trim().is_empty() {
        error = "识别失败".into();
    }
    let run = people
        .fail_person_analysis_task(task, &error)
        .map_err(|e| e.to_string())?;
    progress(operation, run, Some(error));
    Ok(())
}

pub(super) fn save(
    people: &People,
    asset: InferredAsset,
    encoder: &str,
    operation: &PersonOperation,
) -> Result<(), String> {
    let task = asset.task;
    if operation.is_cancelled()
        || !people
            .person_analysis_is_current(&task.run_id)
            .map_err(|e| e.to_string())?
    {
        return Ok(());
    }
    let output = match asset.output {
        Ok(output) => output,
        Err(error) => return failed(people, &task, error, operation),
    };
    match people.complete_person_analysis_task_with_detections(&task, &[], Some(&output.detections))
    {
        Ok(run) => progress(operation, run, None),
        Err(error) => return failed(people, &task, error.to_string(), operation),
    }
    if operation.is_cancelled() {
        return Ok(());
    }
    let encoder_task = people
        .claim_person_analysis_stage(&task.run_id, encoder, Some(&task.asset_path))
        .map_err(|e| e.to_string())?
        .ok_or("无法领取匹配的特征保存步骤")?;
    let result = output.features.and_then(|features| {
        if operation.is_cancelled() {
            return Err("已取消".into());
        }
        people
            .complete_person_analysis_task(&encoder_task, &features)
            .map_err(|e| e.to_string())
    });
    if operation.is_cancelled() {
        return Ok(());
    }
    match result {
        Ok(run) => {
            progress(operation, run, None);
            Ok(())
        }
        Err(error) => failed(people, &encoder_task, error, operation),
    }
}
