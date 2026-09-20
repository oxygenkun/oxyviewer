//! Durable people data for the Host.
//!
//! Owns the split of authority between the two stores:
//!
//! * [`oxy_userdata::PersonStore`] (a JSON file under the app data directory) is
//!   authoritative for persons and decisions.
//! * `oxy_library` (SQLite) is a queryable projection that a cache clear may
//!   destroy at any time.
//!
//! Every mutation is durable first and projected second, so a crash can only
//! cost a projection refresh. [`PeopleService::sync_bindings`] repairs the one
//! thing the projection knows and the durable file cannot: which observation a
//! decision is currently attached to.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxy_domain::{
    CustomTagId, DecisionRecord, FaceCalibration, FaceDecision, Person, PersonRecord,
};
use oxy_library::{DECISION_REBIND_IOU, Library, StoredDecision};
use oxy_userdata::{PersonStore, StoreOrigin, UndoableOperation};

pub(crate) struct PeopleService {
    library: Arc<Library>,
    store: PersonStore,
    projection: std::sync::Mutex<()>,
    operations: std::sync::Mutex<()>,
    dirty: std::sync::atomic::AtomicBool,
}

struct PeopleMutation<'a> {
    _guard: std::sync::MutexGuard<'a, ()>,
    dirty: &'a std::sync::atomic::AtomicBool,
}
impl Drop for PeopleMutation<'_> {
    fn drop(&mut self) {
        self.dirty.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl PeopleService {
    fn mutation(&self) -> Result<PeopleMutation<'_>, String> {
        Ok(PeopleMutation {
            _guard: self
                .operations
                .lock()
                .map_err(|_| "people operation poisoned")?,
            dirty: &self.dirty,
        })
    }

    /// Loads the durable store and makes the SQLite projection agree with it.
    ///
    /// On first run the store does not exist, so it is seeded from whatever
    /// user data the cache already holds. That is the only migration this
    /// feature needs, and it runs once.
    pub(crate) fn load(library: Arc<Library>, path: PathBuf) -> Result<Self, String> {
        let (store, origin) = PersonStore::load(path).map_err(|error| error.to_string())?;
        let service = Self {
            library,
            store,
            projection: std::sync::Mutex::new(()),
            operations: std::sync::Mutex::new(()),
            dirty: std::sync::atomic::AtomicBool::new(false),
        };
        // `PersonStore::load` is strict: a file that is corrupt, unreadable, or
        // from a newer build is an error above, so only creation can need
        // seeding.
        match origin {
            StoreOrigin::Created => {
                service.seed_from_cache()?;
                service.project()?;
            }
            _ => service.project()?,
        }
        Ok(service)
    }

    /// Low-priority sidecar import/sync. The durable document owns pending work,
    /// so losing SQLite cannot lose a queued annotation.
    pub(crate) fn start_sync(
        self: &Arc<Self>,
        jobs: Arc<oxy_runtime::JobRegistry>,
        files: Arc<oxy_fs::FsCatalog>,
        metadata: oxy_metadata::MetadataFacade,
        app: tauri::AppHandle,
    ) -> Result<(), String> {
        let service = Arc::clone(self);
        let ticket = jobs.register(oxy_domain::JobPriority::LibraryIndex);
        let job_id = ticket.id.clone();
        let registry = Arc::clone(&jobs);
        let worker = std::thread::Builder::new()
            .name("oxy-people-sync".into())
            .spawn(move || {
                let token = ticket.cancellation_token();
                let mut iteration = 0u64;
                while !token.is_cancelled() {
                    for (path, state) in service.store.sidecar_states() {
                        if token.is_cancelled() {
                            break;
                        }
                        if state.conflict.is_some()
                            || (state.base == state.desired && state.last_error.is_none())
                            || (state.last_error.is_some() && iteration % 30 != 0)
                        {
                            continue;
                        }
                        if !service
                            .library
                            .foreground
                            .wait_for_background_cancellable(&token)
                        {
                            break;
                        }
                        if service.sync_asset(&path).is_err() {
                            let _ = service.store.sidecar_error(&path, "sidecarUnavailable");
                        }
                    }
                    if !token.is_cancelled() {
                        let _ = service.import_page(&token);
                    }
                    if service
                        .dirty
                        .swap(false, std::sync::atomic::Ordering::AcqRel)
                    {
                        match service.library.refresh_person_tag_projection() {
                            Ok(true) => crate::commands::schedule_tag_xmp_sync(
                                Arc::clone(&service.library),
                                Arc::clone(&files),
                                metadata.clone(),
                            ),
                            Ok(false) => {}
                            Err(_) => service
                                .dirty
                                .store(true, std::sync::atomic::Ordering::Release),
                        }
                        use tauri::Emitter;
                        if let Ok(stats) = service.library.face_library_stats() {
                            let _ = app.emit(crate::jobs::faces::FACE_LIBRARY_UPDATED_EVENT, stats);
                        }
                    }
                    iteration = iteration.wrapping_add(1);
                    for _ in 0..40 {
                        if token.is_cancelled() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                }
                registry.finish(&ticket.id);
            })
            .map_err(|error| {
                jobs.finish(&job_id);
                error.to_string()
            })?;
        jobs.track_worker(job_id, worker);
        Ok(())
    }

    fn import_page(&self, token: &oxy_runtime::CancellationToken) -> Result<(), String> {
        const KEY: &str = "people.sidecar.import.v1";
        let run = self
            .library
            .resume_face_run(KEY)
            .map_err(|error| error.to_string())?;
        let page = self
            .library
            .face_analysis_page(
                &oxy_domain::FaceAnalysisRequest::library(false),
                run.cursor.as_deref(),
            )
            .map_err(|error| error.to_string())?;
        for asset in &page {
            if !self
                .library
                .foreground
                .wait_for_background_cancellable(token)
            {
                return Ok(());
            }
            if self.sync_asset(&asset.path).is_err() {
                let _ = self.store.sidecar_error(&asset.path, "sidecarUnavailable");
            }
        }
        if let Some(asset) = page.last() {
            self.library
                .checkpoint_face_run(KEY, run.generation, &asset.path)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn sync_asset(&self, path: &Path) -> Result<(), String> {
        let roots = self.library.roots().map_err(|error| error.to_string())?;
        let root = roots
            .iter()
            .find(|root| {
                oxy_fs::FsCatalog::authorize_path(root, path)
                    .is_ok_and(|canonical| canonical == path)
            })
            .ok_or("asset outside authorized roots")?;
        let sidecar = oxy_fs::sidecar_path(path);
        if sidecar.exists() {
            oxy_fs::FsCatalog::authorize_path(root, &sidecar)
                .map_err(|_| "sidecar outside authorized root")?;
        }
        let remote = oxy_metadata::read_face_sidecar(path).map_err(|error| error.to_string())?;
        let state = {
            let _guard = self
                .operations
                .lock()
                .map_err(|_| "people operation poisoned")?;
            if self
                .store
                .merge_sidecar(path, remote.clone())
                .map_err(|error| error.to_string())?
            {
                self.project()?;
            }
            self.store.sidecar_state(path)
        };
        let Some(state) = state else {
            return Ok(());
        };
        if state.conflict.is_some() || state.desired == remote {
            return Ok(());
        }
        oxy_fs::FsCatalog::authorize_path(root, path).map_err(|_| "asset authorization expired")?;
        if sidecar.exists() {
            oxy_fs::FsCatalog::authorize_path(root, &sidecar)
                .map_err(|_| "sidecar authorization expired")?;
        }
        oxy_metadata::write_face_sidecar(path, &remote, &state.desired)
            .map_err(|error| error.to_string())?;
        self.store
            .acknowledge_sidecar(path, state.desired)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn delete_all_annotations(&self) -> Result<(), String> {
        let _mutation = self.mutation()?;
        self.store
            .delete_all_annotations()
            .map_err(|error| error.to_string())?;
        self.project()
    }

    pub(crate) fn sync_status(&self) -> Vec<oxy_domain::FaceSyncStatus> {
        self.store.sidecar_status()
    }

    pub(crate) fn resolve_sync_conflict(
        &self,
        path: &Path,
        use_remote: bool,
    ) -> Result<(), String> {
        let _mutation = self.mutation()?;
        self.store
            .resolve_sidecar(path, use_remote)
            .map_err(|error| error.to_string())?;
        self.project()
    }

    pub(crate) fn path(&self) -> &Path {
        self.store.path()
    }

    fn seed_from_cache(&self) -> Result<(), String> {
        let (persons, decisions) = self
            .library
            .export_user_data()
            .map_err(|error| error.to_string())?;
        if persons.is_empty() && decisions.is_empty() {
            return Ok(());
        }
        self.store
            .replace_all(persons, decisions)
            .map_err(|error| error.to_string())?;
        // The cache and the store now agree by construction, so no projection
        // is needed; re-projecting would be a no-op.
        Ok(())
    }

    /// Rewrites the SQLite projection from the durable store.
    fn project(&self) -> Result<(), String> {
        let _guard = self
            .projection
            .lock()
            .map_err(|_| "people projection poisoned")?;
        self.prepare_person_tags()?;
        let (persons, decisions) = self.store.user_data();
        self.library
            .replace_user_data(&persons, &decisions)
            .map_err(|error| error.to_string())?;
        self.sync_bindings_inner()?;
        self.dirty.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Repair links from durable paths, including people created before tag projection.
    fn prepare_person_tags(&self) -> Result<(), String> {
        let tags = self
            .library
            .custom_tags()
            .map_err(|error| error.to_string())?;
        for mut person in self.store.persons() {
            let path = person
                .tag_path
                .clone()
                .or_else(|| {
                    tags.iter()
                        .find(|tag| Some(tag.id) == person.linked_tag_id)
                        .map(|tag| tag.path.clone())
                })
                .unwrap_or_else(|| format!("人物|{}", person.display_name));
            let tag = self
                .library
                .ensure_custom_tag_path(&path)
                .map_err(|error| error.to_string())?;
            if person.tag_path.as_ref() != Some(&tag.path) || person.linked_tag_id != Some(tag.id) {
                person.tag_path = Some(tag.path);
                person.linked_tag_id = Some(tag.id);
                self.store
                    .upsert_person(person)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    pub(crate) fn set_person_tag_path(&self, person_id: &str, path: &str) -> Result<(), String> {
        let _mutation = self.mutation()?;
        let mut person = self.store.person(person_id).ok_or("person was not found")?;
        let tag = self
            .library
            .ensure_custom_tag_path(path)
            .map_err(|error| error.to_string())?;
        person.tag_path = Some(tag.path);
        person.linked_tag_id = Some(tag.id);
        person.updated_at_ms = now_ms();
        self.store
            .upsert_person(person)
            .map_err(|error| error.to_string())?;
        self.library
            .link_person_tag(person_id, Some(tag.id))
            .map_err(|error| error.to_string())
    }

    /// Keep durable classification paths in step with edits made in the tag tree.
    pub(crate) fn capture_tag_paths(&self) -> Result<(), String> {
        let _mutation = self.mutation()?;
        let tags = self
            .library
            .custom_tags()
            .map_err(|error| error.to_string())?;
        for mut person in self.store.persons() {
            if let Some(tag) = tags.iter().find(|tag| Some(tag.id) == person.linked_tag_id)
                && person.tag_path.as_ref() != Some(&tag.path)
            {
                person.tag_path = Some(tag.path.clone());
                self.store
                    .upsert_person(person)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    pub(crate) fn persons(&self) -> Result<Vec<Person>, String> {
        self.library.persons().map_err(|error| error.to_string())
    }

    pub(crate) fn create_person(
        &self,
        person_id: &str,
        display_name: &str,
        linked_tag_id: Option<CustomTagId>,
    ) -> Result<Person, String> {
        let _mutation = self.mutation()?;
        let now = now_ms();
        let tag = if let Some(id) = linked_tag_id {
            self.library
                .custom_tags()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|tag| tag.id == id)
                .ok_or("tag was not found")?
        } else {
            self.library
                .ensure_custom_tag_path(&format!("人物|{}", display_name.trim()))
                .map_err(|error| error.to_string())?
        };
        self.store
            .upsert_person(PersonRecord {
                tag_path: Some(tag.path),
                person_id: person_id.to_string(),
                display_name: display_name.trim().to_string(),
                linked_tag_id: Some(tag.id),
                created_at_ms: now,
                updated_at_ms: now,
            })
            .map_err(|error| error.to_string())?;
        self.library
            .create_person(person_id, display_name, Some(tag.id))
            .map_err(|error| error.to_string())?;
        self.library
            .persons()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|person| person.person_id == person_id)
            .ok_or_else(|| "person was not projected".to_owned())
    }

    pub(crate) fn rename_person(&self, person_id: &str, display_name: &str) -> Result<(), String> {
        let _mutation = self.mutation()?;
        let mut person = self.store.person(person_id).ok_or("person was not found")?;
        let parent = person
            .tag_path
            .as_deref()
            .and_then(|path| path.rsplit_once('|').map(|(parent, _)| parent))
            .unwrap_or("人物");
        let tag = self
            .library
            .ensure_custom_tag_path(&format!("{parent}|{}", display_name.trim()))
            .map_err(|error| error.to_string())?;
        person.display_name = display_name.trim().to_owned();
        person.updated_at_ms = now_ms();
        person.tag_path = Some(tag.path);
        person.linked_tag_id = Some(tag.id);
        self.store
            .upsert_person(person)
            .map_err(|error| error.to_string())?;
        self.library
            .rename_person(person_id, display_name)
            .map_err(|error| error.to_string())?;
        self.library
            .link_person_tag(person_id, Some(tag.id))
            .map_err(|error| error.to_string())
    }

    pub(crate) fn link_person_tag(
        &self,
        person_id: &str,
        tag_id: Option<CustomTagId>,
    ) -> Result<(), String> {
        let person = self.store.person(person_id).ok_or("person was not found")?;
        let path = if let Some(id) = tag_id {
            self.library
                .custom_tags()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|tag| tag.id == id)
                .ok_or("tag was not found")?
                .path
        } else {
            format!("人物|{}", person.display_name)
        };
        self.set_person_tag_path(person_id, &path)
    }

    pub(crate) fn delete_person(&self, person_id: &str) -> Result<usize, String> {
        let _mutation = self.mutation()?;
        let removed = self
            .store
            .remove_person(person_id)
            .map_err(|error| error.to_string())?;
        self.library
            .delete_person(person_id)
            .map_err(|error| error.to_string())?;
        Ok(removed)
    }

    /// Records the user's answer about one face.
    ///
    /// The region and asset come from the stored observation, so the durable
    /// record describes the face the user actually saw rather than whatever the
    /// detector produces next time.
    pub(crate) fn decide(
        &self,
        observation_id: &str,
        decision: &FaceDecision,
    ) -> Result<(), String> {
        let _mutation = self.mutation()?;
        let observation = self
            .library
            .face_observation(observation_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("face observation was not found: {observation_id}"))?;
        // Capture the score behind the proposal before the decision clears it:
        // this is the only moment the application learns how the current
        // threshold behaved on a real face.
        let proposed_similarity = self
            .library
            .pending_candidate_for(observation_id)
            .map_err(|error| error.to_string())?
            .map(|candidate| candidate.similarity);
        self.store
            .set_decision(DecisionRecord {
                observation_id: Some(observation.observation_id.clone()),
                asset_id: observation.asset_id.clone(),
                asset_path: observation.asset_path.clone(),
                region: observation.bbox,
                decision: decision.clone(),
                created_at_ms: now_ms(),
                proposed_similarity,
            })
            .map_err(|error| error.to_string())?;
        self.library
            .record_face_decision(observation_id, decision, proposed_similarity)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn set_face_clarity(
        &self,
        observation_ids: &[String],
        blurry: Option<bool>,
    ) -> Result<(), String> {
        let _mutation = self.mutation()?;
        let mut regions = Vec::with_capacity(observation_ids.len());
        for id in observation_ids {
            let observation = self
                .library
                .face_observation(id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("face observation was not found: {id}"))?;
            regions.push((observation.asset_path, observation.bbox));
        }
        self.store
            .set_clarity_marks(&regions, blurry)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn apply_clarity_marks(&self, items: &mut [oxy_domain::FaceReviewItem]) {
        self.store.apply_clarity_marks(items);
    }

    /// How the current threshold behaves on this library, measured from the
    /// user's own answers rather than from a public benchmark.
    pub(crate) fn calibration(&self) -> Result<FaceCalibration, String> {
        let samples = self.store.score_samples();
        let settings = self
            .library
            .face_analyzer_settings()
            .map_err(|error| error.to_string())?;
        let accepted: Vec<f32> = samples
            .iter()
            .filter(|sample| sample.accepted)
            .map(|sample| sample.similarity)
            .collect();
        let rejected: Vec<f32> = samples
            .iter()
            .filter(|sample| !sample.accepted)
            .map(|sample| sample.similarity)
            .collect();
        Ok(FaceCalibration {
            recommended_threshold: oxy_faces::recommend_threshold(&accepted, &rejected),
            separable: oxy_faces::populations_separate(&accepted, &rejected),
            current_threshold: settings.effective_match_threshold(),
            accepted_scores: accepted,
            rejected_scores: rejected,
        })
    }

    pub(crate) fn clear_decision(&self, observation_id: &str) -> Result<(), String> {
        let _mutation = self.mutation()?;
        self.store
            .clear_decision(observation_id)
            .map_err(|error| error.to_string())?;
        self.library
            .clear_face_decision(observation_id)
            .map_err(|error| error.to_string())
    }

    /// Folds one person into another. Every face confirmed for the source now
    /// belongs to the target; nothing has to be re-confirmed by hand.
    pub(crate) fn merge_persons(
        &self,
        source_person_id: &str,
        target_person_id: &str,
    ) -> Result<usize, String> {
        let _mutation = self.mutation()?;
        let moved = self
            .store
            .merge_persons(source_person_id, target_person_id)
            .map_err(|error| error.to_string())?;
        self.project()?;
        Ok(moved)
    }

    /// Removes the user's confirmation of specific faces for one person, so a
    /// wrongly attached face is not part of that person any more.
    pub(crate) fn remove_faces_from_person(
        &self,
        person_id: &str,
        observation_ids: &[String],
    ) -> Result<usize, String> {
        let _mutation = self.mutation()?;
        let removed = self
            .store
            .remove_faces_from_person(person_id, observation_ids)
            .map_err(|error| error.to_string())?;
        if removed > 0 {
            self.project()?;
        }
        Ok(removed)
    }

    /// Confirms a set of faces for one person, replacing any previous answer.
    pub(crate) fn assign_faces_to_person(
        &self,
        person_id: &str,
        observation_ids: &[String],
    ) -> Result<usize, String> {
        let _mutation = self.mutation()?;
        let mut observations = Vec::with_capacity(observation_ids.len());
        for observation_id in observation_ids {
            let observation = self
                .library
                .face_observation(observation_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("face observation was not found: {observation_id}"))?;
            observations.push((
                observation.observation_id.clone(),
                observation.asset_id.clone(),
                observation.asset_path.clone(),
                observation.bbox,
            ));
        }
        let previous = self
            .store
            .assign_faces_to_person(person_id, &observations, now_ms())
            .map_err(|error| error.to_string())?;
        if !observations.is_empty() {
            self.project()?;
        }
        Ok(previous)
    }

    pub(crate) fn undoable(&self) -> Option<UndoableOperation> {
        self.store.undoable()
    }

    /// Reverses the newest person operation, in the durable store first.
    pub(crate) fn undo(&self) -> Result<Option<UndoableOperation>, String> {
        let _mutation = self.mutation()?;
        let undone = self.store.undo().map_err(|error| error.to_string())?;
        if undone.is_some() {
            self.project()?;
        }
        Ok(undone)
    }

    /// Follows a renamed or moved asset.
    ///
    /// The durable store moves first (it is authoritative) and the projection is
    /// rebuilt, while the machine cache moves separately. A confirmation made on
    /// the old path therefore describes the same bytes at the new one.
    pub(crate) fn move_asset(&self, source: &Path, destination: &Path) -> Result<(), String> {
        let _mutation = self.mutation()?;
        let asset_id = oxy_fs::stable_asset_id(destination);
        self.store
            .move_decisions(source, destination, &asset_id)
            .map_err(|error| error.to_string())?;
        self.library
            .move_asset_face_state(source, destination)
            .map_err(|error| error.to_string())?;
        self.project()
    }

    /// Follows a copied asset: the confirmation goes with the copy, and the
    /// copy's faces are re-derived at its new location.
    pub(crate) fn copy_asset(&self, source: &Path, destination: &Path) -> Result<(), String> {
        let _mutation = self.mutation()?;
        let asset_id = oxy_fs::stable_asset_id(destination);
        self.store
            .copy_decisions(source, destination, &asset_id)
            .map_err(|error| error.to_string())?;
        self.project()
    }

    /// Drops the face data for an asset that no longer exists.
    pub(crate) fn remove_asset(&self, path: &Path) -> Result<(), String> {
        let _mutation = self.mutation()?;
        self.store
            .remove_decisions(path)
            .map_err(|error| error.to_string())?;
        self.library
            .remove_asset_face_state(path)
            .map_err(|error| error.to_string())?;
        self.project()
    }

    /// Copies the observation bindings the projection computed back into the
    /// durable file.
    ///
    /// Analysis re-binds decisions to its own new observation ids, which only
    /// the projection knows. Without this the durable file would keep pointing
    /// at ids from a previous detector, and the next start would show every
    /// confirmation as unbound until analysis ran again.
    pub(crate) fn sync_bindings(&self) -> Result<usize, String> {
        let _mutation = self.mutation()?;
        self.sync_bindings_inner()
    }

    fn sync_bindings_inner(&self) -> Result<usize, String> {
        let records = self.store.decisions();
        if records.is_empty() {
            return Ok(0);
        }
        let projected = self
            .library
            .face_decisions()
            .map_err(|error| error.to_string())?;
        let mut by_asset: HashMap<PathBuf, Vec<&StoredDecision>> = HashMap::new();
        for decision in &projected {
            by_asset
                .entry(decision.asset_path.clone())
                .or_default()
                .push(decision);
        }

        let mut updated = 0usize;
        let mut next = records;
        for record in &mut next {
            let binding = by_asset
                .get(&record.asset_path)
                .and_then(|candidates| {
                    candidates
                        .iter()
                        .filter(|candidate| {
                            candidate.decision == record.decision
                                && candidate.region.iou(record.region) >= DECISION_REBIND_IOU
                        })
                        .max_by(|left, right| {
                            left.region
                                .iou(record.region)
                                .partial_cmp(&right.region.iou(record.region))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                })
                .and_then(|candidate| candidate.observation_id.clone());
            if binding != record.observation_id {
                record.observation_id = binding;
                updated += 1;
            }
        }
        if updated > 0 {
            self.store
                .replace_all(self.store.persons(), next)
                .map_err(|error| error.to_string())?;
        }
        Ok(updated)
    }

    /// Region a decision occupies, used by diagnostics and tests.
    #[cfg(test)]
    pub(crate) fn decision_count(&self) -> usize {
        self.store.decisions().len()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_domain::{FaceDecision, FaceObservation, NormalizedRect};
    use oxy_library::StoredFace;

    const DETECTOR: &str = "scrfd/test/detect-v1";
    const EMBEDDER: &str = "adaface/test/align-v1";

    fn rect(x: f32, y: f32, width: f32, height: f32) -> NormalizedRect {
        NormalizedRect::new(x, y, width, height)
    }

    fn store_face(library: &Library, id: &str, bbox: NormalizedRect) {
        store_faces(library, &[(id.to_string(), bbox)]);
    }

    /// Replaces the asset's faces in one call: `replace_asset_faces` is a full
    /// replacement, so storing two faces separately would keep only the second.
    fn store_faces(library: &Library, faces: &[(String, NormalizedRect)]) {
        let faces: Vec<StoredFace> = faces
            .iter()
            .enumerate()
            .map(|(index, (id, bbox))| StoredFace {
                observation: FaceObservation {
                    observation_id: id.clone(),
                    asset_id: "asset-1".into(),
                    asset_path: PathBuf::from("/photos/a.jpg"),
                    source_revision: "100:200:faces-v1".into(),
                    local_index: index as u32,
                    bbox: *bbox,
                    landmarks: Vec::new(),
                    detection_score: 0.97,
                    detector_fingerprint: DETECTOR.into(),
                },
                embedding: vec![1.0, 0.0],
                face_pixels: 128,
                clarity: 0.8,
            })
            .collect();
        library
            .replace_asset_faces(
                Path::new("/photos/a.jpg"),
                "asset-1",
                "100:200:faces-v1",
                DETECTOR,
                EMBEDDER,
                &faces,
            )
            .unwrap();
    }

    fn setup() -> (Arc<Library>, PeopleService, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let library = Arc::new(Library::in_memory().unwrap());
        let service =
            PeopleService::load(library.clone(), directory.path().join("people.json")).unwrap();
        (library, service, directory)
    }

    #[test]
    fn importing_sidecar_binds_existing_faces_and_can_be_cleared_without_inference() {
        let (library, service, _directory) = setup();
        let region = rect(0.1, 0.1, 0.2, 0.2);
        store_face(&library, "obs-1", region);
        let path = Path::new("/photos/a.jpg");
        service
            .store
            .merge_sidecar(
                path,
                oxy_domain::PortableFaceFacts {
                    facts: vec![oxy_domain::PortableFaceFact {
                        person_tag_path: None,
                        id: "fact".into(),
                        revision: "r1".into(),
                        region,
                        decision: Some(FaceDecision::ConfirmPerson {
                            person_id: "alice".into(),
                        }),
                        person_name: Some("Alice".into()),
                    }],
                },
            )
            .unwrap();
        service.project().unwrap();
        assert!(service.store.decision_for("obs-1").is_some());
        assert_eq!(service.persons().unwrap()[0].face_count, 1);
        service
            .dirty
            .store(false, std::sync::atomic::Ordering::Release);
        service.clear_decision("obs-1").unwrap();
        assert_eq!(service.decision_count(), 0);
        assert!(service.dirty.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn sidecar_sync_recovers_human_facts_without_either_local_store() {
        let (library, service, directory) = setup();
        let root = directory.path().canonicalize().unwrap();
        library.add_root(&root).unwrap();
        let asset = root.join("photo.jpg");
        std::fs::write(&asset, b"source").unwrap();
        service.create_person("alice", "Alice", None).unwrap();
        service
            .store
            .set_decision(DecisionRecord {
                observation_id: None,
                asset_id: oxy_fs::stable_asset_id(&asset),
                asset_path: asset.clone(),
                region: rect(0.1, 0.1, 0.2, 0.2),
                decision: FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
                created_at_ms: 1,
                proposed_similarity: None,
            })
            .unwrap();
        service
            .set_person_tag_path("alice", "人物|家人|Alice")
            .unwrap();
        assert!(service.sync_status()[0].pending);
        service.sync_asset(&asset).unwrap();
        assert!(service.sync_status().is_empty());
        let xml = std::fs::read_to_string(oxy_fs::sidecar_path(&asset)).unwrap();
        assert!(!xml.contains("embedding") && !xml.contains("observation"));
        let fresh_library = Arc::new(Library::in_memory().unwrap());
        fresh_library.add_root(&root).unwrap();
        let fresh =
            PeopleService::load(fresh_library.clone(), root.join("fresh-people.json")).unwrap();
        fresh.sync_asset(&asset).unwrap();
        assert_eq!(fresh.persons().unwrap()[0].display_name, "Alice");
        fresh_library.refresh_person_tag_projection().unwrap();
        assert_eq!(
            fresh_library.tag_xmp_payload(&asset).unwrap().hierarchical,
            ["人物|家人|Alice"]
        );
        assert_eq!(fresh.decision_count(), 1);
        fresh_library.clear_face_cache().unwrap();
        assert_eq!(fresh.decision_count(), 1);
        fresh.delete_all_annotations().unwrap();
        fresh.sync_asset(&asset).unwrap();
        assert!(
            oxy_metadata::read_face_sidecar(&asset)
                .unwrap()
                .facts
                .iter()
                .all(|fact| fact.decision.is_none())
        );
    }

    #[test]
    fn manual_quality_validates_entire_batch_before_persisting() {
        let (library, service, _directory) = setup();
        store_face(&library, "obs-1", rect(0.1, 0.1, 0.2, 0.2));
        assert!(
            service
                .set_face_clarity(&["obs-1".into(), "missing".into()], Some(true))
                .is_err()
        );
        let mut page = library
            .face_review_page(oxy_domain::FaceReviewFilter::All, 0, 20)
            .unwrap();
        service.apply_clarity_marks(&mut page.items);
        assert_eq!(page.items[0].manual_blurry, None);
        service
            .set_face_clarity(&["obs-1".into()], Some(false))
            .unwrap();
        service.apply_clarity_marks(&mut page.items);
        assert_eq!(page.items[0].manual_blurry, Some(false));
    }

    #[test]
    fn a_confirmation_is_durable_before_it_is_projected() {
        let (library, service, directory) = setup();
        store_face(&library, "obs-1", rect(0.1, 0.1, 0.2, 0.2));
        service.create_person("p1", "Alice", None).unwrap();
        service
            .decide(
                "obs-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "p1".into(),
                },
            )
            .unwrap();
        assert_eq!(service.decision_count(), 1);
        assert_eq!(library.persons().unwrap()[0].face_count, 1);

        // A brand-new cache, loaded from the same durable file, must already
        // know about Alice and the confirmation.
        let fresh = Arc::new(Library::in_memory().unwrap());
        let reloaded =
            PeopleService::load(fresh.clone(), directory.path().join("people.json")).unwrap();
        assert_eq!(reloaded.persons().unwrap().len(), 1);
        assert_eq!(reloaded.persons().unwrap()[0].display_name, "Alice");
        assert_eq!(reloaded.decision_count(), 1);
        assert_eq!(fresh.face_decisions().unwrap().len(), 1);
    }

    #[test]
    fn the_first_run_seeds_the_store_from_existing_cache_data() {
        let (library, _service, directory) = setup();
        store_face(&library, "obs-1", rect(0.1, 0.1, 0.2, 0.2));
        library.create_person("p1", "Alice", None).unwrap();
        library
            .record_face_decision(
                "obs-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "p1".into(),
                },
                None,
            )
            .unwrap();

        // `_service` created the file empty; a second load sees it as Loaded and
        // would project it. The migration path is the one that seeds it.
        let path = directory.path().join("migrated.json");
        let fresh = Arc::new(Library::in_memory().unwrap());
        let (store, origin) = PersonStore::load(&path).unwrap();
        assert_eq!(origin, StoreOrigin::Created);
        let (persons, decisions) = library.export_user_data().unwrap();
        store.replace_all(persons, decisions).unwrap();

        let service = PeopleService::load(fresh.clone(), path).unwrap();
        assert_eq!(service.persons().unwrap().len(), 1);
        assert_eq!(fresh.face_decisions().unwrap().len(), 1);
    }

    #[test]
    fn clearing_a_decision_removes_it_from_both_stores() {
        let (library, service, _directory) = setup();
        store_face(&library, "obs-1", rect(0.1, 0.1, 0.2, 0.2));
        service.decide("obs-1", &FaceDecision::NotFace).unwrap();
        assert_eq!(service.decision_count(), 1);

        service.clear_decision("obs-1").unwrap();
        assert_eq!(service.decision_count(), 0);
        assert!(library.face_decisions().unwrap().is_empty());
        assert_eq!(
            library
                .face_review_page(oxy_domain::FaceReviewFilter::Unknown, 0, 10)
                .unwrap()
                .items
                .len(),
            1,
            "the face returns to the queue"
        );
    }

    #[test]
    fn merging_and_undoing_survives_the_projection() {
        let (library, service, _directory) = setup();
        store_faces(
            &library,
            &[
                ("obs-1".to_string(), rect(0.10, 0.10, 0.20, 0.20)),
                ("obs-2".to_string(), rect(0.40, 0.10, 0.20, 0.20)),
            ],
        );
        service.create_person("alice", "Alice", None).unwrap();
        service.create_person("alica", "Alica", None).unwrap();
        service
            .decide(
                "obs-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
            )
            .unwrap();
        service
            .decide(
                "obs-2",
                &FaceDecision::ConfirmPerson {
                    person_id: "alica".into(),
                },
            )
            .unwrap();
        // The faces belong to two observations on one asset, so re-analysis
        // must be simulated before the second one exists.
        assert_eq!(service.persons().unwrap().len(), 2);

        let moved = service.merge_persons("alica", "alice").unwrap();
        assert_eq!(moved, 1);
        // The projection follows the durable store, not the other way round.
        let persons = service.persons().unwrap();
        assert_eq!(persons.len(), 1);
        assert_eq!(persons[0].person_id, "alice");
        assert_eq!(persons[0].face_count, 2);
        assert!(
            library
                .face_decisions()
                .unwrap()
                .iter()
                .all(|record| record.decision.person_id() == Some("alice"))
        );

        let undoable = service.undoable().expect("merge is undoable");
        assert_eq!(undoable.kind, "mergePersons");
        assert_eq!(undoable.other_person_name.as_deref(), Some("Alica"));
        service.undo().unwrap();
        assert_eq!(service.persons().unwrap().len(), 2);
        assert_eq!(
            library
                .persons()
                .unwrap()
                .iter()
                .filter(|person| person.display_name == "Alica")
                .count(),
            1,
            "the projection restores the person too"
        );
    }

    #[test]
    fn removing_faces_from_a_person_keeps_the_other_photos() {
        let (library, service, _directory) = setup();
        store_face(&library, "obs-1", rect(0.10, 0.10, 0.20, 0.20));
        service.create_person("alice", "Alice", None).unwrap();
        service
            .decide(
                "obs-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
            )
            .unwrap();
        assert_eq!(service.persons().unwrap()[0].face_count, 1);

        let removed = service
            .remove_faces_from_person("alice", &["obs-1".to_string()])
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(service.persons().unwrap()[0].face_count, 0);
        assert!(library.face_decisions().unwrap().is_empty());
        // The face is back in the queue rather than silently dropped.
        assert_eq!(
            library
                .face_review_page(oxy_domain::FaceReviewFilter::Unknown, 0, 10)
                .unwrap()
                .items
                .len(),
            1
        );
        assert_eq!(
            service.undoable().map(|operation| operation.kind),
            Some("removeFaces".to_string())
        );
    }

    #[test]
    fn calibration_learns_from_the_users_own_answers() {
        let (library, service, _directory) = setup();
        let faces: Vec<(String, NormalizedRect)> = (0..6)
            .map(|index| {
                (
                    format!("obs-{index}"),
                    rect(0.05 * index as f32, 0.1, 0.05, 0.05),
                )
            })
            .collect();
        store_faces(&library, &faces);
        service.create_person("alice", "Alice", None).unwrap();

        // Six answered proposals: three clearly the same person, three clearly
        // not. Without recording the score there would be nothing to learn from.
        for (index, (similarity, accepted)) in [
            (0.70, true),
            (0.65, true),
            (0.60, true),
            (0.40, false),
            (0.35, false),
            (0.30, false),
        ]
        .into_iter()
        .enumerate()
        {
            let observation_id = format!("obs-{index}");
            library
                .replace_face_candidates(
                    "matcher-v1",
                    &[oxy_domain::FaceCandidate {
                        observation_id: observation_id.clone(),
                        person_id: "alice".into(),
                        person_name: "Alice".into(),
                        similarity,
                        matcher_fingerprint: "matcher-v1".into(),
                    }],
                )
                .unwrap();
            let decision = if accepted {
                FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                }
            } else {
                FaceDecision::RejectPerson {
                    person_id: "alice".into(),
                }
            };
            service.decide(&observation_id, &decision).unwrap();
        }

        let calibration = service.calibration().unwrap();
        assert_eq!(calibration.accepted_scores.len(), 3);
        assert_eq!(calibration.rejected_scores.len(), 3);
        assert!(calibration.separable, "the two populations do not overlap");
        let recommended = calibration
            .recommended_threshold
            .expect("enough evidence to recommend");
        assert!(
            recommended > 0.40 && recommended < 0.60,
            "the recommendation must sit between the populations, got {recommended}"
        );
        assert_eq!(
            calibration.current_threshold,
            oxy_domain::FaceAnalyzerSettings::default().effective_match_threshold()
        );
    }

    #[test]
    fn calibration_stays_silent_while_there_is_too_little_evidence() {
        let (library, service, _directory) = setup();
        store_face(&library, "obs-1", rect(0.1, 0.1, 0.2, 0.2));
        service.create_person("alice", "Alice", None).unwrap();
        library
            .replace_face_candidates(
                "matcher-v1",
                &[oxy_domain::FaceCandidate {
                    observation_id: "obs-1".into(),
                    person_id: "alice".into(),
                    person_name: "Alice".into(),
                    similarity: 0.7,
                    matcher_fingerprint: "matcher-v1".into(),
                }],
            )
            .unwrap();
        service
            .decide(
                "obs-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
            )
            .unwrap();

        let calibration = service.calibration().unwrap();
        assert_eq!(calibration.accepted_scores, vec![0.7]);
        assert!(calibration.rejected_scores.is_empty());
        assert_eq!(
            calibration.recommended_threshold, None,
            "one example is not evidence"
        );
    }

    #[test]
    fn a_face_named_from_scratch_contributes_no_score_sample() {
        let (library, service, _directory) = setup();
        store_face(&library, "obs-1", rect(0.1, 0.1, 0.2, 0.2));
        service.create_person("alice", "Alice", None).unwrap();
        // No candidate was ever proposed, so there is no score to learn from.
        service
            .decide(
                "obs-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
            )
            .unwrap();
        let calibration = service.calibration().unwrap();
        assert!(calibration.accepted_scores.is_empty());
        assert!(calibration.rejected_scores.is_empty());
        assert_eq!(calibration.recommended_threshold, None);
    }

    #[test]
    fn sync_bindings_copies_the_rebound_observation_into_the_durable_file() {
        let (library, service, directory) = setup();
        store_face(&library, "obs-1", rect(0.10, 0.10, 0.20, 0.20));
        service.create_person("p1", "Alice", None).unwrap();
        service
            .decide(
                "obs-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "p1".into(),
                },
            )
            .unwrap();

        // A new detector renames the observation and nudges its box.
        store_face(&library, "obs-2", rect(0.104, 0.098, 0.204, 0.198));
        assert_eq!(
            library.face_decisions().unwrap()[0]
                .observation_id
                .as_deref(),
            Some("obs-2"),
            "the cache re-binds immediately"
        );
        // The durable file still holds the previous binding until the run ends.
        assert_eq!(
            service.decision_count(),
            1,
            "the decision itself was never lost"
        );
        assert_eq!(service.sync_bindings().unwrap(), 1);

        let reloaded = PeopleService::load(
            Arc::new(Library::in_memory().unwrap()),
            directory.path().join("people.json"),
        )
        .unwrap();
        let (_, decisions) = reloaded.library.export_user_data().unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].observation_id.as_deref(), Some("obs-2"));
    }
    #[test]
    fn person_classification_drives_tags_and_survives_cache_rebuild() {
        let (library, service, directory) = setup();
        store_face(&library, "obs-1", rect(0.1, 0.1, 0.2, 0.2));
        service.create_person("alice", "Alice", None).unwrap();
        service
            .set_person_tag_path("alice", "人物|家人|Alice")
            .unwrap();
        service
            .decide(
                "obs-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
            )
            .unwrap();
        assert!(library.refresh_person_tag_projection().unwrap());
        let path = Path::new("/photos/a.jpg");
        let payload = library.tag_xmp_payload(path).unwrap();
        assert!(payload.subjects.contains(&"Alice".into()));
        assert_eq!(payload.hierarchical, ["人物|家人|Alice"]);
        let tag = service.persons().unwrap()[0].linked_tag_id.unwrap();
        assert!(
            library
                .asset_tag_assignments(&[path.into()])
                .unwrap()
                .iter()
                .any(|item| item.tag.id == tag && item.assigned_count == 1)
        );
        service.rename_person("alice", "Alicia").unwrap();
        library.refresh_person_tag_projection().unwrap();
        assert_eq!(
            library.tag_xmp_payload(path).unwrap().hierarchical,
            ["人物|家人|Alicia"]
        );
        let fresh_library = Arc::new(Library::in_memory().unwrap());
        // Reused IDs must not link a person to an unrelated cache entry.
        fresh_library.create_custom_tag(None, "unrelated").unwrap();
        let fresh =
            PeopleService::load(fresh_library.clone(), directory.path().join("people.json"))
                .unwrap();
        fresh_library.refresh_person_tag_projection().unwrap();
        assert_eq!(
            fresh_library.tag_xmp_payload(path).unwrap().hierarchical,
            ["人物|家人|Alicia"]
        );
        assert_eq!(fresh.persons().unwrap()[0].display_name, "Alicia");
        service.clear_decision("obs-1").unwrap();
        library.refresh_person_tag_projection().unwrap();
        assert!(
            library
                .tag_xmp_payload(path)
                .unwrap()
                .hierarchical
                .is_empty()
        );
    }

    #[test]
    fn tag_tree_moves_update_durable_person_classification() {
        let (library, service, directory) = setup();
        let person = service.create_person("alice", "Alice", None).unwrap();
        let parent = library.ensure_custom_tag_path("人物|朋友").unwrap();
        library
            .update_custom_tag(person.linked_tag_id.unwrap(), Some(parent.id), "Alice")
            .unwrap();
        service.capture_tag_paths().unwrap();
        let fresh_library = Arc::new(Library::in_memory().unwrap());
        let fresh =
            PeopleService::load(fresh_library.clone(), directory.path().join("people.json"))
                .unwrap();
        let id = fresh.persons().unwrap()[0].linked_tag_id;
        assert!(
            fresh_library
                .custom_tags()
                .unwrap()
                .iter()
                .any(|tag| Some(tag.id) == id && tag.path == "人物|朋友|Alice")
        );
    }
}
