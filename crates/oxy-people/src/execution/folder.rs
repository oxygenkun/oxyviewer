//! Folder enrollment and lifetime of the three-stage analysis pipeline.
use crate::People;
use oxy_domain::PersonAnalysisTask;
/// Register this face-only pipeline and run it. Folder clustering is not part
/// of this pipeline, so sealing after the two per-image stages is complete.
#[cfg(any(windows, target_os = "macos"))]
pub fn analyse_folder(
    people: &People,
    catalog: &oxy_fs::FsCatalog,
    session_id: &str,
    request: &oxy_domain::BeginPersonAnalysis,
    models: &mut crate::inference::face::OnnxFaceModels,
    operation: &crate::execution::operations::PersonOperation,
    prepare: impl FnMut(
        &PersonAnalysisTask,
        oxy_domain::AnalysisRequirement,
    ) -> Result<image::RgbImage, String>
    + Send,
) -> Result<(), String> {
    struct Sink<'a>(&'a People);
    impl crate::execution::enrollment::AnalysisTaskSink for Sink<'_> {
        type Error = crate::PeopleError;
        fn enqueue(
            &mut self,
            id: &str,
            tasks: &[oxy_domain::PersonAnalysisTaskInput],
        ) -> Result<usize, Self::Error> {
            self.0.enqueue_person_analysis_tasks(id, tasks)
        }
    }
    let manifest = crate::environment::catalog::manifest();
    let run = people
        .begin_person_analysis(request)
        .map_err(|error| error.to_string())?;
    operation.update(|status| status.run = Some(run.clone()));
    let result = (|| {
        crate::execution::enrollment::enroll_folder_snapshot(
            catalog,
            session_id,
            &run,
            &manifest,
            &mut Sink(people),
            || operation.is_cancelled(),
        )
        .map_err(|error| error.to_string())?;
        people
            .seal_person_analysis_tasks(&run.run_id)
            .map_err(|error| error.to_string())?;
        operation.update(|status| status.state = oxy_domain::PersonOperationState::Analysing);
        let final_run = super::face::execute(people, &run, &manifest, models, operation, prepare)?;
        if final_run.state == oxy_domain::PersonAnalysisState::Cancelled {
            return Err("识别任务已取消或被替代".into());
        }
        operation.update(|status| {
            status.detail = format!(
                "识别结束：完成 {} 个步骤，失败 {} 个（{}）。检测结果需人工核对。",
                final_run.completed_tasks,
                final_run.failed_tasks,
                models.providers().detector.description()
            );
            status.run = Some(final_run.clone());
        });
        if final_run.failed_tasks > 0 {
            return Err("部分照片识别失败，请检查格式或错误详情。".into());
        }
        Ok(())
    })();
    if result.is_err()
        && people
            .person_analysis_is_current(&run.run_id)
            .unwrap_or(false)
    {
        // Enumeration/infrastructure failure must not leave a queued run that
        // appears active forever. Per-image failures already have ledger rows.
        let _ = people.cancel_person_analysis(&run.run_id, &format!("worker-abort:{}", run.run_id));
    }
    result
}
