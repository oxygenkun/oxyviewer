use super::*;

const MIB: usize = 1024 * 1024;

fn encoded(registry: &ResourceRegistry, len: usize) -> Result<ResourceHandle, MediaError> {
    registry.register_encoded(
        vec![1; len].into(),
        "image/jpeg",
        DisplayDimensions {
            width: 1,
            height: 1,
        },
        ArtifactRepresentation::Embedded,
    )
}

#[test]
fn dynamic_budget_and_protocol_limits_are_independent() {
    for (ram, expected) in [(4_u64, 1_u64), (8, 1), (16, 2), (32, 4), (64, 8)] {
        assert_eq!(
            resource_memory_budget(Some(ram * 1024 * 1024 * 1024)),
            usize::try_from(expected * 1024 * 1024 * 1024).unwrap_or(usize::MAX)
        );
    }
    assert_eq!(resource_memory_budget(None), 1024 * MIB);
    assert_eq!(
        resource_memory_budget(Some(u64::MAX)),
        usize::try_from(u64::MAX / 8).unwrap_or(usize::MAX)
    );
    let limits =
        ResourceRegistryLimits::new(512, resource_memory_budget(Some(64 * 1024 * 1024 * 1024)));
    assert_eq!(limits.max_materialized_responses, 4);
    assert_eq!(limits.max_materialized_bytes, 128 * MIB);
    assert_eq!(limits.publish_grace, Duration::from_secs(5));
    assert_eq!(limits.ui_lease, Duration::from_secs(30));
    let registry = ResourceRegistry::new(limits);
    let mut responses = Vec::new();
    for _ in 0..4 {
        responses.push(MaterializedReservation::acquire(&registry.inner, 32 * MIB).unwrap());
    }
    assert!(matches!(
        MaterializedReservation::acquire(&registry.inner, 1),
        Err(MediaError::ResourceBudgetExhausted {
            budget: "materialized count",
            current: 4,
            limit: 4,
            requested: 1
        })
    ));
    responses.pop();
    assert!(matches!(
        MaterializedReservation::acquire(&registry.inner, 32 * MIB + 1),
        Err(MediaError::ResourceBudgetExhausted {
            budget: "materialized bytes",
            ..
        })
    ));
    drop(responses);
    assert_eq!(registry.stats().materialized_bytes, 0);
    assert_eq!(registry.stats().materialized_responses, 0);
}

#[test]
fn encoded_128_mib_pressure_releases_reservations() {
    let registry = ResourceRegistry::new(ResourceRegistryLimits::new(512, 128 * MIB));
    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(
            encoded(&registry, 32 * MIB)
                .unwrap()
                .descriptor
                .resource_id
                .clone(),
        );
    }
    assert_eq!(registry.stats().encoded_bytes, 128 * MIB);
    assert!(
        matches!(encoded(&registry, 1), Err(MediaError::ResourceBudgetExhausted {
        budget: "encoded memory", current, limit, requested: 1,
    }) if current == 128 * MIB && limit == 128 * MIB)
    );
    registry.release(&ids[0]);
    let next = encoded(&registry, 32 * MIB).unwrap();
    assert!(!registry.contains(&ids[0]));
    assert_eq!(registry.stats().encoded_bytes, 128 * MIB);
    assert_eq!(registry.len(), 4);
    drop(next);
}

#[test]
fn protected_64_file_regression_and_512_entry_production_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.jpg");
    fs::write(&path, [1; 10]).unwrap();
    for capacity in [64, 512] {
        let registry = ResourceRegistry::new(ResourceRegistryLimits::new(capacity, 1));
        for _ in 0..64 {
            registry
                .register_file(
                    &path,
                    "image/jpeg",
                    DisplayDimensions {
                        width: 1,
                        height: 1,
                    },
                    ArtifactRepresentation::Original,
                )
                .unwrap();
        }
        let staged = dir.path().join(format!("staged-{capacity}.jpg"));
        fs::write(&staged, [2; 20]).unwrap();
        let result = registry.register_owned_staged_file(
            Arc::new(OwnedStagedFile::new(staged.clone())),
            "image/jpeg",
            DisplayDimensions {
                width: 1,
                height: 1,
            },
            ArtifactRepresentation::Decoded,
        );
        if capacity == 64 {
            assert!(matches!(
                result,
                Err(MediaError::ResourceBudgetExhausted {
                    budget: "entry",
                    current: 64,
                    limit: 64,
                    requested: 1
                })
            ));
            assert!(!staged.exists());
        } else {
            assert!(result.is_ok());
            assert_eq!(registry.stats().staged_files, 1);
            assert_eq!(registry.stats().staged_bytes, 20);
        }
        assert_eq!(registry.stats().encoded_bytes, 0);
    }
}

#[test]
fn grace_expiry_renew_release_and_read_protection() {
    let registry = ResourceRegistry::new(ResourceRegistryLimits::new(1, 128));
    let first = encoded(&registry, 1).unwrap();
    let id = first.descriptor.resource_id.clone();
    drop(first);
    // Deterministic expiry, no wall-clock sleeps or test-only production limits.
    registry
        .inner
        .state
        .lock()
        .unwrap()
        .entries
        .get_mut(&id)
        .unwrap()
        .ui_lease_until = Instant::now();
    assert!(registry.renew(&id));
    assert!(encoded(&registry, 1).is_err());
    let read = registry.resolve(&id).unwrap();
    registry.release(&id);
    assert!(encoded(&registry, 1).is_err());
    drop(read);
    assert!(encoded(&registry, 1).is_ok());
    assert!(!registry.contains(&id));
    let id = registry
        .inner
        .state
        .lock()
        .unwrap()
        .lru
        .back()
        .unwrap()
        .clone();
    registry
        .inner
        .state
        .lock()
        .unwrap()
        .entries
        .get_mut(&id)
        .unwrap()
        .ui_lease_until = Instant::now();
    assert!(encoded(&registry, 1).is_ok());
    assert!(!registry.contains(&id));
}

#[test]
fn scrolling_600_file_publications_reclaims_released_working_set() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.jpg");
    fs::write(&path, [1; 10]).unwrap();
    let registry = ResourceRegistry::new(ResourceRegistryLimits::new(512, 128 * MIB));
    let mut displayed = VecDeque::new();
    for _ in 0..600 {
        let handle = registry
            .register_file(
                &path,
                "image/jpeg",
                DisplayDimensions {
                    width: 1,
                    height: 1,
                },
                ArtifactRepresentation::Original,
            )
            .unwrap();
        let id = handle.descriptor.resource_id.clone();
        assert!(registry.renew(&id));
        displayed.push_back(id);
        if displayed.len() > 100 {
            registry.release(&displayed.pop_front().unwrap());
        }
        assert!(registry.len() <= 101);
        assert_eq!(registry.stats().encoded_bytes, 0);
    }
}

#[test]
fn staged_read_blocks_eviction_and_managed_transition() {
    let directory = tempfile::tempdir().unwrap();
    let staged = directory.path().join("staged.jpg");
    let managed = directory.path().join("managed.jpg");
    fs::write(&staged, [1; 10]).unwrap();
    fs::write(&managed, [1; 10]).unwrap();
    let registry = ResourceRegistry::new(ResourceRegistryLimits::new(1, 128));
    let handle = registry
        .register_owned_staged_file(
            Arc::new(OwnedStagedFile::new(staged.clone())),
            "image/jpeg",
            DisplayDimensions {
                width: 1,
                height: 1,
            },
            ArtifactRepresentation::Decoded,
        )
        .unwrap();
    let id = handle.descriptor.resource_id.clone();
    drop(handle);
    let read = registry.resolve(&id).unwrap();
    registry.release(&id);
    assert!(encoded(&registry, 1).is_err());
    assert!(
        !registry
            .transition_to_managed_file(&id, &managed, &mut None)
            .unwrap()
    );
    assert!(staged.exists());
    drop(read);
    assert!(
        registry
            .transition_to_managed_file(&id, &managed, &mut None)
            .unwrap()
    );
    assert!(!staged.exists());
    assert_eq!(registry.stats().staged_files, 0);
    assert_eq!(registry.stats().encoded_bytes, 0);
}

#[test]
fn republished_grace_does_not_shorten_an_active_ui_lease() {
    let registry = ResourceRegistry::new(ResourceRegistryLimits::new(1, 128));
    let handle = encoded(&registry, 1).unwrap();
    let id = &handle.descriptor.resource_id;
    let until = registry.inner.state.lock().unwrap().entries[id].ui_lease_until;
    assert!(until.duration_since(Instant::now()) <= Duration::from_secs(5));
    assert!(registry.renew(id));
    let active = registry.inner.state.lock().unwrap().entries[id].ui_lease_until;
    assert!(active > until);
    assert!(registry.refresh_publication_grace(id));
    assert_eq!(
        registry.inner.state.lock().unwrap().entries[id].ui_lease_until,
        active
    );
    registry.release(id);
    assert!(registry.refresh_publication_grace(id));
    let grace = registry.inner.state.lock().unwrap().entries[id].ui_lease_until;
    assert!(grace < active);
}
