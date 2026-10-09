//! Conservative folder suggestions. Computation never writes user facts.
use crate::{People, PeopleError, environment::catalog};
use oxy_domain::{
    AssetSummary, PersonAnalysisState, PersonCluster, PersonClusterFilter, PersonClusterMember,
    PersonClusterSnapshot,
};
use oxy_runtime::CancellationToken;
use oxy_store::{StoreError, repo};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

mod adoption;
#[cfg(test)]
pub(crate) mod tests;

// Experimental suggestion policy, not an identity acceptance threshold.
pub const ALGORITHM: &str = "adaface-complete-link-0.50-v2";
const MIN_SIMILARITY: f32 = 0.50;
const MAX_FACES: usize = 5_000;

fn cancelled(token: &CancellationToken) -> Result<(), PeopleError> {
    if token.is_cancelled() {
        return Err(PeopleError::Cluster("聚类已取消".into()));
    }
    Ok(())
}

fn digest(value: &impl serde::Serialize) -> Result<String, PeopleError> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

fn current_run(
    connection: &oxy_store::Connection,
    folder: &str,
) -> Result<oxy_domain::PersonAnalysisRun, PeopleError> {
    let (_, id) = repo::person_cache::analysis_head(connection, folder)?
        .ok_or_else(|| PeopleError::Cluster("没有识别结果，请先识别当前文件夹".into()))?;
    let run = repo::person_cache::analysis_run(connection, &id)?
        .ok_or(PeopleError::MissingPersonAnalysis)?;
    if run.state != PersonAnalysisState::Completed {
        return Err(PeopleError::Cluster(
            "请先完成当前文件夹的识别任务，再聚类".into(),
        ));
    }
    Ok(run)
}

pub(crate) fn parameters() -> Result<(String, String, String), PeopleError> {
    let manifest = catalog::manifest();
    let detector = crate::stage_fingerprint(&manifest, catalog::DETECTOR)
        .map_err(|e| PeopleError::Cluster(e.to_string()))?;
    let encoder = crate::stage_fingerprint(&manifest, catalog::ENCODER)
        .map_err(|e| PeopleError::Cluster(e.to_string()))?;
    let space = manifest
        .stages
        .iter()
        .find(|s| s.stage_id == catalog::ENCODER)
        .and_then(|s| s.feature_space_id.clone())
        .ok_or(PeopleError::InvalidPersonFeature)?;
    Ok((detector, encoder, space))
}

pub(crate) fn vector(bytes: Option<&[u8]>) -> Option<Vec<f32>> {
    let bytes = bytes?;
    if bytes.len() != 512 * 4 {
        return None;
    }
    let values: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let norm: f32 = values.iter().map(|x| x * x).sum();
    (values.iter().all(|x| x.is_finite()) && (norm - 1.0).abs() < 0.002).then_some(values)
}

struct Face {
    member: PersonClusterMember,
    values: Option<Vec<f32>>,
    quality: f64,
}

fn group(
    mut faces: Vec<Face>,
    token: &CancellationToken,
) -> Result<(Vec<PersonCluster>, Vec<PersonClusterMember>), PeopleError> {
    faces.sort_by(|a, b| {
        b.quality
            .total_cmp(&a.quality)
            .then_with(|| a.member.asset_path.cmp(&b.member.asset_path))
            .then_with(|| a.member.instance_id.cmp(&b.member.instance_id))
    });
    // Bounded dense similarities plus compact pair indices: at most ~200 MB
    // for 5000 faces. No decoding, GPU session, or user writes in this stage.
    let n = faces.len();
    let mut similarities = vec![-1.0f32; n * n];
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for i in 0..n {
        cancelled(token)?;
        let Some(a) = &faces[i].values else {
            continue;
        };
        for j in 0..i {
            if faces[i].member.asset_path == faces[j].member.asset_path {
                continue;
            }
            let Some(b) = &faces[j].values else {
                continue;
            };
            let score = a.iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
            similarities[i * n + j] = score;
            similarities[j * n + i] = score;
            if score >= MIN_SIMILARITY {
                edges.push((i as u32, j as u32));
            }
        }
    }
    edges.sort_unstable_by(|&(a, b), &(c, d)| {
        similarities[c as usize * n + d as usize]
            .total_cmp(&similarities[a as usize * n + b as usize])
            .then((a, b).cmp(&(c, d)))
    });
    let mut owner: Vec<_> = (0..n).collect();
    let mut groups: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    for (i, j) in edges {
        cancelled(token)?;
        let (a, b) = (owner[i as usize], owner[j as usize]);
        if a == b {
            continue;
        }
        // Strongest evidence first, but every cross-pair must agree. A bridge
        // cannot transitively merge dissimilar people or two faces in one photo.
        if !groups[a].iter().all(|&i| {
            groups[b]
                .iter()
                .all(|&j| similarities[i * n + j] >= MIN_SIMILARITY)
        }) {
            continue;
        }
        let merged = std::mem::take(&mut groups[b]);
        for &member in &merged {
            owner[member] = a;
        }
        groups[a].extend(merged);
    }
    let mut clusters = Vec::new();
    let mut ungrouped = Vec::new();
    for members in groups.into_iter().filter(|g| !g.is_empty()) {
        let mut members: Vec<_> = members
            .into_iter()
            .map(|i| faces[i].member.clone())
            .collect();
        if members.len() == 1 {
            ungrouped.append(&mut members);
            continue;
        }
        // Stable id independent of input enumeration and quality ordering.
        let mut keys: Vec<_> = members
            .iter()
            .map(|m| (&m.asset_path, &m.instance_id))
            .collect();
        keys.sort();
        clusters.push(PersonCluster {
            id: digest(&keys)?,
            cover: None,
            people: Vec::new(),
            members,
        });
    }
    clusters.sort_by(|a, b| b.members.len().cmp(&a.members.len()).then(a.id.cmp(&b.id)));
    Ok((clusters, ungrouped))
}

impl People {
    /// Explicit background work. Fresh discovery and source validation stay off folder open.
    pub fn cluster_folder(
        &self,
        folder: &Path,
        token: &CancellationToken,
    ) -> Result<PersonClusterSnapshot, PeopleError> {
        self.cluster_folder_evidence(folder, token, false)
    }

    pub(crate) fn cluster_folder_evidence(
        &self,
        folder: &Path,
        token: &CancellationToken,
        instances_only: bool,
    ) -> Result<PersonClusterSnapshot, PeopleError> {
        let folder = folder.canonicalize()?;
        let folder_text = folder.to_string_lossy();
        let (detector, encoder, space) = parameters()?;
        let (run, evidence, completed) = {
            let connection = self.store.read();
            let run = current_run(&connection, &folder_text)?;
            let evidence = repo::person_clusters::evidence(
                &connection,
                &folder_text,
                &run.run_id,
                &detector,
                &space,
                &encoder,
            )?;
            let completed = repo::person_clusters::completed_sources(
                &connection,
                &run.run_id,
                catalog::DETECTOR,
            )?;
            (run, evidence, completed)
        };
        if run.pipeline_fingerprint
            != crate::pipeline_fingerprint(&catalog::manifest())
                .map_err(|e| PeopleError::Cluster(e.to_string()))?
        {
            return Err(PeopleError::Cluster(
                "识别结果来自旧版本模型，请重新识别当前文件夹".into(),
            ));
        }
        if evidence.len() > MAX_FACES {
            return Err(PeopleError::Cluster(
                "当前文件夹人脸超过 5000 个，请分批处理".into(),
            ));
        }
        let assets =
            oxy_fs::scan_assets_with_progress_and_cancel(&folder, |_| {}, || token.is_cancelled())?;
        let mut current = HashMap::new();
        for asset in &assets {
            cancelled(token)?;
            let observation = oxy_fs::observe_file(&asset.path)?;
            current.insert(
                asset.path.clone(),
                (
                    observation.revision_id(),
                    format!("{}:{}", asset.size_bytes, asset.modified_at_ms),
                ),
            );
        }
        let detected_paths: HashSet<_> = evidence.iter().map(|e| e.asset_path.as_str()).collect();
        let completed: HashMap<_, _> = completed.into_iter().collect();
        let no_face_count = current
            .iter()
            .filter(|(p, (r, _))| {
                completed.get(p.to_string_lossy().as_ref()) == Some(r)
                    && !detected_paths.contains(p.to_string_lossy().as_ref())
            })
            .count();
        let mut unavailable_count = current
            .iter()
            .filter(|(p, (r, _))| completed.get(p.to_string_lossy().as_ref()) != Some(r))
            .count();
        let mut faces = Vec::new();
        for item in &evidence {
            let path = std::path::PathBuf::from(&item.asset_path);
            let Some((revision, summary)) = current.get(&path) else {
                continue;
            };
            if revision != &item.source_revision {
                continue;
            }
            let values = vector(item.vector.as_deref());
            if values.is_none() {
                unavailable_count += 1;
            }
            let face_box: [f64; 4] = serde_json::from_str(&item.face_box)?;
            faces.push(Face {
                member: PersonClusterMember {
                    asset_path: path,
                    instance_id: item.instance_id.clone(),
                    source_revision: revision.clone(),
                    summary_revision: summary.clone(),
                    face_box,
                },
                values,
                quality: item.face_score * (face_box[2] * face_box[3]).sqrt(),
            });
        }
        if faces.is_empty() && no_face_count == 0 && !assets.is_empty() {
            return Err(PeopleError::Cluster(
                "没有兼容且有效的人脸特征，请重新识别当前文件夹".into(),
            ));
        }
        let (mut clusters, ungrouped) = if instances_only {
            (Vec::new(), faces.into_iter().map(|f| f.member).collect())
        } else {
            group(faces, token)?
        };
        for cluster in &mut clusters {
            cluster.cover = assets
                .iter()
                .find(|a| a.path == cluster.members[0].asset_path)
                .cloned();
        }
        let mut snapshot = PersonClusterSnapshot {
            snapshot_id: String::new(),
            folder_path: folder.clone(),
            run_id: run.run_id.clone(),
            pipeline_fingerprint: run.pipeline_fingerprint.clone(),
            algorithm: ALGORITHM.into(),
            clusters,
            ungrouped,
            no_face_count,
            unavailable_count,
        };
        snapshot.snapshot_id = digest(&snapshot)?;
        // Reobserve before publication. No filesystem I/O under the database lock.
        for member in snapshot
            .clusters
            .iter()
            .flat_map(|c| &c.members)
            .chain(&snapshot.ungrouped)
        {
            cancelled(token)?;
            if oxy_fs::observe_file(&member.asset_path)?.revision_id() != member.source_revision {
                return Err(PeopleError::PersonAnalysisConflict);
            }
        }
        let json = serde_json::to_string(&snapshot)?;
        let mut connection = self.store.write();
        let tx = connection.transaction().map_err(StoreError::from)?;
        cancelled(token)?;
        if current_run(&tx, &folder_text)?.run_id != run.run_id
            || repo::person_clusters::evidence(
                &tx,
                &folder_text,
                &run.run_id,
                &detector,
                &space,
                &encoder,
            )? != evidence
        {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        if !instances_only {
            repo::person_clusters::save_snapshot(&tx, &folder_text, &run.run_id, &json)?;
        }
        tx.commit().map_err(StoreError::from)?;
        Ok(snapshot)
    }

    pub fn person_clusters(
        &self,
        folder: &Path,
    ) -> Result<Option<PersonClusterSnapshot>, PeopleError> {
        let raw = repo::person_clusters::snapshot(&self.store.read(), &folder.to_string_lossy())?;
        let snapshot: Option<PersonClusterSnapshot> =
            raw.map(|s| serde_json::from_str(&s)).transpose()?;
        let fingerprint = crate::pipeline_fingerprint(&catalog::manifest())
            .map_err(|e| PeopleError::Cluster(e.to_string()))?;
        let mut snapshot =
            snapshot.filter(|s| s.algorithm == ALGORITHM && s.pipeline_fingerprint == fingerprint);
        if let Some(snapshot) = &mut snapshot {
            let connection = self.store.read();
            let people = repo::people::list_folder_people(&connection, &folder.to_string_lossy())?;
            let links = repo::people::cluster_links(&connection, &folder.to_string_lossy())?;
            for cluster in &mut snapshot.clusters {
                cluster.people = people
                    .iter()
                    .filter(|person| {
                        links
                            .iter()
                            .any(|(id, subject)| id == &cluster.id && subject == &person.id)
                    })
                    .cloned()
                    .collect();
            }
        }
        Ok(snapshot)
    }

    pub fn filter_assets_by_cluster(
        &self,
        folder: &Path,
        assets: &[AssetSummary],
        filter: &PersonClusterFilter,
    ) -> Result<Vec<AssetSummary>, PeopleError> {
        let Some(snapshot) = self
            .person_clusters(folder)?
            .filter(|s| s.snapshot_id == filter.snapshot_id)
        else {
            return Ok(Vec::new());
        };
        let members = if filter.cluster_id == "ungrouped" {
            &snapshot.ungrouped
        } else {
            let Some(cluster) = snapshot.clusters.iter().find(|c| c.id == filter.cluster_id) else {
                return Ok(Vec::new());
            };
            &cluster.members
        };
        let versions: HashMap<_, _> = members
            .iter()
            .map(|m| (&m.asset_path, &m.summary_revision))
            .collect();
        Ok(assets
            .iter()
            .filter(|a| {
                versions
                    .get(&a.path)
                    .is_some_and(|r| **r == format!("{}:{}", a.size_bytes, a.modified_at_ms))
            })
            .cloned()
            .collect())
    }
}
