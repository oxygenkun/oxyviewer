use super::*;
use oxy_runtime::CancellationToken;

fn check(token: &CancellationToken) -> Result<(), PeopleError> {
    if token.is_cancelled() {
        Err(failure("任务已取消"))
    } else {
        Ok(())
    }
}

impl People {
    pub fn run_people_grouping(
        &self,
        input: &RunPeopleGrouping,
        token: &CancellationToken,
    ) -> Result<FolderPeopleWorkspace, PeopleError> {
        if !input.similarity.is_finite()
            || !(0.0..=1.0).contains(&input.similarity)
            || input.person_ids.as_ref().is_some_and(Vec::is_empty)
        {
            return Err(failure("请选择人物并使用 0–1 范围的候选相似度"));
        }
        self.initialize_global_people()?;
        let folder = input.folder_path.canonicalize()?;
        let (_, encoder, space) = crate::clusters::parameters()?;
        let (people, refs) = {
            let c = self.store.read();
            (
                db::catalog(&c)?,
                db::reference_vectors(&c, &space, &encoder)?,
            )
        };
        if input
            .person_ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| !people.iter().any(|p| p.id == *id)))
        {
            return Err(PeopleError::MissingPersonRecord);
        }
        let mut gallery = Vec::new();
        let mut used_references = Vec::new();
        let geometry_matches = |r: &db::ReferenceVector| {
            r.tuple
                .face_box
                .is_some_and(|b| b.iter().zip(r.face_box).all(|(a, b)| (a - b).abs() <= 1e-9))
        };
        for r in &refs {
            check(token)?;
            if input
                .person_ids
                .as_ref()
                .is_some_and(|ids| !ids.contains(&r.person_id))
            {
                continue;
            }
            if !geometry_matches(r)
                || refs
                    .iter()
                    .filter(|candidate| {
                        candidate.person_id == r.person_id
                            && candidate.tuple.id == r.tuple.id
                            && geometry_matches(candidate)
                    })
                    .count()
                    != 1
            {
                continue;
            }
            if oxy_fs::observe_file(&r.tuple.asset_path)
                .is_ok_and(|s| s.revision_id() == r.tuple.source_identity_revision)
                && let Some(v) = crate::clusters::vector(Some(&r.vector))
            {
                gallery.push((r.person_id.clone(), v));
                used_references.push(r);
            }
        }
        if let Some(ids) = &input.person_ids {
            let missing: Vec<_> = people
                .iter()
                .filter(|p| ids.contains(&p.id) && !gallery.iter().any(|(id, _)| *id == p.id))
                .map(|p| p.display_name.as_str())
                .collect();
            if !missing.is_empty() {
                return Err(failure(&format!(
                    "这些人物没有当前模型可用的人脸参考：{}。请确认参考实例并识别参考所在文件夹；人体信息保留供人工审阅。",
                    missing.join("、")
                )));
            }
        }
        // Reuses the bounded conservative core policy. Retrieval never expands its
        // gallery using unconfirmed candidates or clothing similarity.
        let snapshot = self.cluster_folder_evidence(&folder, token, input.person_ids.is_some())?;
        let mut suggestions = convert(&snapshot)?;
        let (detector, _, _) = crate::clusters::parameters()?;
        let evidence = repo::person_clusters::evidence(
            &self.store.read(),
            &folder.to_string_lossy(),
            &snapshot.run_id,
            &detector,
            &space,
            &encoder,
        )?;
        let mut vectors = HashMap::new();
        for e in &evidence {
            if let Some(v) = crate::clusters::vector(e.vector.as_deref()) {
                let id = format!(
                    "det:{}",
                    hash(&(
                        std::path::PathBuf::from(&e.asset_path),
                        &e.instance_id,
                        &e.source_revision
                    ))?
                );
                vectors.insert(id, v);
            }
        }
        let mut groups = Vec::new();
        for g in suggestions.groups {
            let mut unknown = Vec::new();
            for t in g.members {
                check(token)?;
                let mut scores = BTreeMap::<String, f32>::new();
                if let Some(v) = vectors.get(&t.id) {
                    for (person, reference) in &gallery {
                        let score = v.iter().zip(reference).map(|(a, b)| a * b).sum::<f32>();
                        let old = scores.entry(person.clone()).or_insert(-1.0);
                        *old = old.max(score);
                    }
                }
                let mut ranked: Vec<_> = scores
                    .into_iter()
                    .filter(|(_, s)| *s >= input.similarity)
                    .collect();
                ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                if input.person_ids.is_none() {
                    if ranked.len() > 1 && ranked[0].1 - ranked[1].1 < 0.05 {
                        ranked.clear();
                    }
                    ranked.truncate(1);
                }
                if ranked.is_empty() {
                    if input.person_ids.is_none() {
                        unknown.push(t);
                    }
                } else {
                    for (person, score) in ranked {
                        let mut member = t.clone();
                        member.person_id = Some(person.clone());
                        member.decision = Some(PersonReviewDecision::Pending);
                        member.score = Some(score);
                        groups.push(PersonTupleGroup {
                            id: format!("person:{person}"),
                            person_id: Some(person),
                            members: vec![member],
                            cover: g.cover.clone(),
                        });
                    }
                }
            }
            if !unknown.is_empty() {
                groups.push(PersonTupleGroup {
                    id: g.id,
                    person_id: None,
                    members: unknown,
                    cover: g.cover,
                });
            }
        }
        suggestions.groups = groups;
        suggestions.notice = Some(format!(
            "人脸候选阈值 {:.3}；结果均需人工核对。人体信息用于实例标注，不按衣着自动合并身份。",
            input.similarity
        ));
        for r in used_references {
            check(token)?;
            if !oxy_fs::observe_file(&r.tuple.asset_path)
                .is_ok_and(|s| s.revision_id() == r.tuple.source_identity_revision)
            {
                return Err(failure("参考源图发生变化，请重新运行"));
            }
        }
        for t in suggestions.groups.iter().flat_map(|g| &g.members) {
            check(token)?;
            if oxy_fs::observe_file(&t.asset_path)?.revision_id() != t.source_identity_revision {
                return Err(PeopleError::PersonAnalysisConflict);
            }
        }
        let mut c = self.store.write();
        let tx = c.transaction().map_err(StoreError::from)?;
        check(token)?;
        if db::catalog(&tx)? != people
            || db::reference_vectors(&tx, &space, &encoder)? != refs
            || repo::person_cache::analysis_head(&tx, &folder.to_string_lossy())?
                .is_none_or(|(_, id)| id != snapshot.run_id)
            || repo::person_clusters::evidence(
                &tx,
                &folder.to_string_lossy(),
                &snapshot.run_id,
                &detector,
                &space,
                &encoder,
            )? != evidence
        {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        db::save_suggestions(
            &tx,
            &folder.to_string_lossy(),
            &snapshot.run_id,
            &serde_json::to_string(&suggestions)?,
        )?;
        tx.commit().map_err(StoreError::from)?;
        drop(c);
        self.folder_people_workspace(&folder)
    }
}
