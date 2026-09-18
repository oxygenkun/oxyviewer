//! End-to-end people flow over the real persistence layer.
//!
//! This is the product loop the feature exists for, exercised without Tauri:
//!
//! ```text
//! detect -> embed -> store -> label a person -> a new asset matches
//!        -> candidate -> review -> confirm / correct / not-a-face
//! ```
//!
//! `reference_parity.rs` proves the analyzer agrees with OpenCV. This test
//! proves the Host invariants hold once the analyzer's output is stored:
//! machine output is replaced freely, user decisions are not, and an
//! undecided face never becomes an assertion.
//!
//! Needs the pinned models (`pnpm faces:prepare`); skips when they are absent.

use std::path::{Path, PathBuf};

use oxy_domain::{
    FaceAnalyzerSettings, FaceCluster, FaceDecision, FaceReviewFilter, FaceReviewState, PersonId,
    face_source_revision_for_path,
};
use oxy_faces::{
    ClusterInput, FaceAnalyzer, FaceModelPaths, PersonGallery, RgbImage, cluster_embeddings,
    match_person,
};
use oxy_library::{DECISION_REBIND_IOU, Library, StoredFace};

/// The analyzer plus the fingerprints it actually reports. Embeddings and
/// observations are keyed by these, so a test that invents its own strings
/// silently reads an empty table.
struct Rig {
    analyzer: FaceAnalyzer,
    detector: String,
    embedder: String,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn model_paths() -> Option<FaceModelPaths> {
    let directory = std::env::var_os("OXY_FACE_MODEL_DIR").map_or_else(
        || manifest_dir().join("../../target/native/face-models"),
        PathBuf::from,
    );
    let detector = directory.join("face_detection_yunet_2023mar.onnx");
    let embedder = directory.join("face_recognition_sface_2021dec.onnx");
    (detector.is_file() && embedder.is_file()).then(|| FaceModelPaths::new(detector, embedder, 640))
}

fn load_fixture() -> RgbImage {
    load_image_at(&manifest_dir().join("tests/data/fixtures/two_people.jpg"))
}

fn load_image_at(path: &Path) -> RgbImage {
    let image = image::open(path)
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()))
        .to_rgb8();
    RgbImage::new(image.width(), image.height(), image.into_raw()).expect("RGB8 buffer")
}

/// One asset's analysis, reproducing exactly what the background queue does:
/// read the revision, decode, analyze with the stale-source guard, then store.
fn analyze_file(library: &Library, rig: &Rig, path: &Path) -> Option<usize> {
    let revision = face_source_revision_for_path(path).expect("revision");
    let image = load_image_at(path);
    let analyzed = rig
        .analyzer
        .analyze_verified(&path.to_string_lossy(), path, &revision, &image)
        .expect("analysis succeeds")?;
    let faces: Vec<StoredFace> = analyzed
        .into_iter()
        .map(|face| StoredFace {
            observation: face.observation,
            embedding: face.embedding,
        })
        .collect();
    let count = faces.len();
    library
        .replace_asset_faces(
            path,
            &path.to_string_lossy(),
            &revision,
            &rig.detector,
            &rig.embedder,
            &faces,
        )
        .expect("faces stored");
    Some(count)
}

/// Every observation across the library, so a test can prove there are no
/// duplicates rather than only that a count looks right.
fn all_observations(library: &Library) -> Vec<String> {
    let mut ids = Vec::new();
    for target in library
        .face_analysis_targets("any", true, 10_000)
        .expect("targets")
    {
        for observation in library
            .face_observations_for_asset(&target.path)
            .expect("observations")
        {
            ids.push(observation.observation_id);
        }
    }
    // `force` makes the library enumerate every indexed asset regardless of its
    // checkpoint, which is what a duplicate check needs to see.
    ids
}

fn rig() -> Option<Rig> {
    let paths = model_paths()?;
    let analyzer = FaceAnalyzer::load(
        &paths,
        FaceAnalyzerSettings {
            detection_confidence: 0.9,
            nms_threshold: 0.3,
            max_faces_per_asset: 5000,
            min_face_pixels: 8,
            ..FaceAnalyzerSettings::default()
        },
    )
    .expect("pinned models load");
    Some(Rig {
        detector: analyzer.detector_fingerprint(),
        embedder: analyzer.embedder_fingerprint().to_owned(),
        analyzer,
    })
}

fn analyze_into(
    library: &Library,
    rig: &Rig,
    image: &RgbImage,
    asset: &str,
    revision: &str,
) -> Vec<StoredFace> {
    let path = Path::new(asset);
    let analyzed = rig
        .analyzer
        .analyze(asset, path, revision, image)
        .expect("analysis succeeds");
    let faces: Vec<StoredFace> = analyzed
        .into_iter()
        .map(|face| StoredFace {
            observation: face.observation,
            embedding: face.embedding,
        })
        .collect();
    library
        .replace_asset_faces(path, asset, revision, &rig.detector, &rig.embedder, &faces)
        .expect("faces stored");
    faces
}

fn gallery_for(library: &Library, rig: &Rig, person_id: &str, name: &str) -> PersonGallery {
    let confirmed = library
        .confirmed_face_embeddings(&rig.embedder)
        .expect("gallery query");
    let examples = confirmed
        .into_iter()
        .filter(|(id, _)| id == person_id)
        .map(|(_, embedding)| embedding)
        .collect::<Vec<_>>();
    PersonGallery::new(person_id, name, examples)
}

#[test]
fn detect_label_then_confirm_or_correct_a_new_asset() {
    let Some(rig) = rig() else {
        eprintln!("skipping: run `pnpm faces:prepare` to enable this test");
        return;
    };
    let image = load_fixture();
    let library = Library::in_memory().expect("in-memory library");

    // --- First asset: two faces, both unknown ------------------------------
    let first = analyze_into(
        &library,
        &rig,
        &image,
        "/photos/first.jpg",
        "100:200:faces-v1",
    );
    assert_eq!(first.len(), 2, "the fixture has exactly two faces");

    let unknown = library
        .face_review_page(FaceReviewFilter::Unknown, 0, 50)
        .expect("review page");
    assert_eq!(unknown.items.len(), 2);
    assert!(
        unknown
            .items
            .iter()
            .all(|item| item.state == FaceReviewState::Unknown)
    );

    // --- The user names face 0 --------------------------------------------
    let person = library
        .create_person("person-alice", "Alice", None)
        .expect("person created");
    let alice_face = first[0].observation.observation_id.clone();
    library
        .record_face_decision(
            &alice_face,
            &FaceDecision::ConfirmPerson {
                person_id: person.person_id.clone(),
            },
            None,
        )
        .expect("confirm stored");

    // The other face stays unknown: naming one face must not name the asset.
    let unknown = library
        .face_review_page(FaceReviewFilter::Unknown, 0, 50)
        .expect("review page");
    assert_eq!(unknown.items.len(), 1);
    assert_eq!(
        unknown.items[0].observation_id,
        first[1].observation.observation_id
    );

    // --- A new asset arrives with the same two faces -----------------------
    analyze_into(
        &library,
        &rig,
        &image,
        "/photos/second.jpg",
        "300:400:faces-v1",
    );

    // Candidate matching proposes Alice for the matching face and nothing for
    // the stranger, so the review queue is "pending" rather than "unknown".
    let gallery = gallery_for(&library, &rig, &person.person_id, "Alice");
    let second_faces = library
        .face_observations_for_asset(Path::new("/photos/second.jpg"))
        .expect("stored observations");
    let mut candidates = Vec::new();
    for observation in &second_faces {
        let embedding = library
            .unresolved_face_embeddings(&rig.embedder, 100)
            .expect("embeddings")
            .into_iter()
            .find(|(id, _)| id == &observation.observation_id)
            .map(|(_, embedding)| embedding)
            .expect("every new observation has an embedding");
        if let Some(matched) = match_person(&embedding, std::slice::from_ref(&gallery)) {
            if matched.is_candidate(0.363) {
                candidates.push((observation.observation_id.clone(), matched));
            }
        }
    }
    assert_eq!(
        candidates.len(),
        1,
        "only the known face should be proposed"
    );
    assert_eq!(candidates[0].1.person_id, person.person_id);
    assert!(
        candidates[0].1.similarity > 0.9,
        "the same photo must match itself strongly, got {}",
        candidates[0].1.similarity
    );

    let proposed = candidates[0].0.clone();
    library
        .replace_face_candidates(
            "matcher-v1",
            &[oxy_domain::FaceCandidate {
                observation_id: proposed.clone(),
                person_id: person.person_id.clone(),
                person_name: person.display_name.clone(),
                similarity: candidates[0].1.similarity,
                matcher_fingerprint: "matcher-v1".into(),
            }],
        )
        .expect("candidates stored");

    // The proposal is pending, not an assertion.
    let pending = library
        .face_review_page(FaceReviewFilter::Pending, 0, 50)
        .expect("pending page");
    assert_eq!(pending.items.len(), 1);
    assert_eq!(pending.items[0].observation_id, proposed);
    assert_eq!(pending.items[0].state, FaceReviewState::Pending);
    assert_eq!(
        pending.items[0]
            .candidate
            .as_ref()
            .map(|candidate| candidate.person_id.clone()),
        Some(person.person_id.clone())
    );
    assert_eq!(
        library.persons().expect("persons")[0].face_count,
        1,
        "a pending candidate must not count as a confirmed face"
    );

    // --- The user confirms the proposal ------------------------------------
    library
        .record_face_decision(
            &proposed,
            &FaceDecision::ConfirmPerson {
                person_id: person.person_id.clone(),
            },
            Some(candidates[0].1.similarity),
        )
        .expect("confirm stored");
    assert_eq!(library.persons().expect("persons")[0].face_count, 2);
    assert!(
        library
            .face_review_page(FaceReviewFilter::Pending, 0, 50)
            .expect("pending page")
            .items
            .is_empty()
    );

    // --- The user corrects a wrong match ----------------------------------
    // A third asset where the stranger's face is proposed for Alice: rejecting
    // it must suppress the proposal even when the matcher runs again.
    let stranger = second_faces
        .iter()
        .find(|observation| {
            observation.observation_id != proposed && observation.observation_id != alice_face
        })
        .map(|observation| observation.observation_id.clone())
        .expect("the stranger face exists");
    library
        .replace_face_candidates(
            "matcher-v1",
            &[oxy_domain::FaceCandidate {
                observation_id: stranger.clone(),
                person_id: person.person_id.clone(),
                person_name: "Alice".into(),
                similarity: 0.9,
                matcher_fingerprint: "matcher-v1".into(),
            }],
        )
        .expect("candidates stored");
    library
        .record_face_decision(
            &stranger,
            &FaceDecision::RejectPerson {
                // Last use of `person`: the identity is moved into the
                // rejection, which is what makes the rejection durable.
                person_id: person.person_id,
            },
            Some(0.9),
        )
        .expect("rejection stored");

    // The rejection is a user fact, so the cached candidate is cleared and the
    // face is reported as rejected rather than pending or unknown.
    assert!(
        library
            .face_review_page(FaceReviewFilter::Pending, 0, 50)
            .expect("pending page")
            .items
            .is_empty()
    );
    let rejected = library
        .face_review_page(FaceReviewFilter::Rejected, 0, 50)
        .expect("rejected page");
    assert_eq!(rejected.items.len(), 1);
    assert_eq!(rejected.items[0].state, FaceReviewState::Rejected);

    // Marking a region as not-a-face is equally durable.
    library
        .record_face_decision(&alice_face, &FaceDecision::NotFace, None)
        .expect("not-face stored");
    assert_eq!(
        library.persons().expect("persons")[0].face_count,
        1,
        "marking not-a-face removes the confirmation"
    );
}

#[test]
fn clustering_groups_repeat_faces_and_marks_boundaries() {
    let Some(rig) = rig() else {
        eprintln!("skipping: run `pnpm faces:prepare` to enable this test");
        return;
    };
    let image = load_fixture();
    let library = Library::in_memory().expect("in-memory library");

    // Three assets with the same two faces: the same-person pair must cluster.
    for (index, asset) in ["/photos/a.jpg", "/photos/b.jpg", "/photos/c.jpg"]
        .iter()
        .enumerate()
    {
        analyze_into(
            &library,
            &rig,
            &image,
            asset,
            &format!("{}:200:faces-v1", 100 + index),
        );
    }

    let embeddings = library
        .undecided_face_embeddings(&rig.embedder, 100)
        .expect("embeddings");
    assert_eq!(embeddings.len(), 6, "three assets with two faces each");
    let inputs: Vec<ClusterInput> = embeddings
        .into_iter()
        .map(|(observation_id, embedding)| ClusterInput {
            observation_id,
            embedding,
        })
        .collect();

    let clusters: Vec<FaceCluster> =
        cluster_embeddings(&inputs, rig.analyzer.settings().cluster_threshold, 2)
            .expect("clustering");
    assert_eq!(
        clusters.len(),
        2,
        "the same face across three photos is one cluster, and the other face is another"
    );
    for cluster in &clusters {
        assert_eq!(cluster.member_count, 3);
        assert!(
            cluster.outlier_observation_ids.is_empty(),
            "identical crops must not be flagged as boundary members"
        );
        assert!(cluster.cohesion > 0.95, "cohesion {}", cluster.cohesion);
    }

    // Clusters are cache: storing them must not create persons or decisions.
    library
        .replace_face_clusters("cluster-v1", &clusters)
        .expect("clusters stored");
    assert!(library.persons().expect("persons").is_empty());
    assert!(library.face_decisions().expect("decisions").is_empty());
    assert_eq!(
        library.face_clusters("cluster-v1").expect("clusters").len(),
        2
    );
    assert_eq!(
        library
            .face_clusters_current()
            .expect("current clusters")
            .len(),
        2,
        "the most recent pass is what a reader sees"
    );
}

#[test]
fn a_detector_change_rebinds_confirmations_and_keeps_the_person() {
    let Some(rig) = rig() else {
        eprintln!("skipping: run `pnpm faces:prepare` to enable this test");
        return;
    };
    let image = load_fixture();
    let library = Library::in_memory().expect("in-memory library");
    let first = analyze_into(&library, &rig, &image, "/photos/a.jpg", "100:200:faces-v1");
    let target = first[0].observation.observation_id.clone();
    let region = first[0].observation.bbox;
    library
        .create_person("person-alice", "Alice", None)
        .expect("person");
    library
        .record_face_decision(
            &target,
            &FaceDecision::ConfirmPerson {
                person_id: "person-alice".into(),
            },
            None,
        )
        .expect("confirm");

    // Simulate a new detector: same face, nudged box, new observation ids.
    let nudged: Vec<StoredFace> = first
        .iter()
        .enumerate()
        .map(|(index, face)| StoredFace {
            observation: oxy_domain::FaceObservation {
                observation_id: format!("next-detector-{index}"),
                bbox: oxy_domain::NormalizedRect::new(
                    face.observation.bbox.x + 0.004,
                    face.observation.bbox.y - 0.003,
                    face.observation.bbox.width + 0.006,
                    face.observation.bbox.height + 0.005,
                ),
                detector_fingerprint: "yunet/next/detect-v2".into(),
                ..face.observation.clone()
            },
            embedding: face.embedding.clone(),
        })
        .collect();
    library
        .replace_asset_faces(
            Path::new("/photos/a.jpg"),
            "/photos/a.jpg",
            "100:200:faces-v1",
            "yunet/next/detect-v2",
            &rig.embedder,
            &nudged,
        )
        .expect("re-analysis stored");

    let decisions = library.face_decisions().expect("decisions");
    assert_eq!(decisions.len(), 1);
    let rebound = decisions[0]
        .observation_id
        .as_deref()
        .expect("the decision follows the face");
    assert_ne!(
        rebound, target,
        "the new detector has its own observation ids"
    );
    assert!(rebound.starts_with("next-detector-"));
    assert!(
        region.iou(nudged[0].observation.bbox) > DECISION_REBIND_IOU,
        "the test's nudge must stay within the re-bind threshold"
    );
    assert_eq!(library.persons().expect("persons")[0].face_count, 1);
    let confirmed = library
        .face_review_page(FaceReviewFilter::Confirmed, 0, 10)
        .expect("confirmed page");
    assert_eq!(confirmed.items.len(), 1);
    assert_eq!(
        confirmed.items[0].confirmed_person_name.as_deref(),
        Some("Alice")
    );
}

#[test]
fn analysis_resumes_from_checkpoints_without_duplicates() {
    let Some(rig) = rig() else {
        eprintln!("skipping: run `pnpm faces:prepare` to enable this test");
        return;
    };
    let fixture = manifest_dir().join("tests/data/fixtures/two_people.jpg");
    let directory = tempfile::tempdir().expect("temp dir");
    let mut paths = Vec::new();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        let destination = directory.path().join(name);
        std::fs::copy(&fixture, &destination).expect("fixture copy");
        paths.push(destination);
    }
    let library = Library::open(&directory.path().join("library.sqlite")).expect("library");
    library.add_root(directory.path()).expect("root");
    library.index_root(directory.path()).expect("index");

    let fingerprint = rig.detector.clone();
    let targets = library
        .face_analysis_targets(&fingerprint, false, 100)
        .expect("targets");
    assert_eq!(targets.len(), 3);
    let (analyzed, total) = library.face_analysis_counts(&fingerprint).expect("counts");
    assert_eq!((analyzed, total), (0, 3));

    // The job is killed after one asset: only that asset is checkpointed.
    let first = targets[0].clone();
    assert_eq!(analyze_file(&library, &rig, &first.path), Some(2));

    // A restarted run continues from the checkpoint instead of starting over.
    let remaining = library
        .face_analysis_targets(&fingerprint, false, 100)
        .expect("targets");
    assert_eq!(remaining.len(), 2, "resume, do not restart");
    assert!(
        !remaining.iter().any(|target| target.path == first.path),
        "a completed asset must not be visited again"
    );
    let (analyzed, total) = library.face_analysis_counts(&fingerprint).expect("counts");
    assert_eq!((analyzed, total), (1, 3));

    // Finish the run.
    for target in &remaining {
        assert_eq!(analyze_file(&library, &rig, target.path.as_path()), Some(2));
    }
    assert!(
        library
            .face_analysis_targets(&fingerprint, false, 100)
            .expect("targets")
            .is_empty(),
        "a completed library has nothing left to analyze"
    );

    let observations = all_observations(&library);
    assert_eq!(observations.len(), 6, "three assets with two faces each");
    let unique: std::collections::HashSet<&String> = observations.iter().collect();
    assert_eq!(unique.len(), 6, "no duplicate observations");

    // Re-running over the same bytes must not add or change anything: the
    // observation ids are content-derived, so this is idempotent.
    for target in &targets {
        assert_eq!(analyze_file(&library, &rig, target.path.as_path()), Some(2));
    }
    let mut reparsed = all_observations(&library);
    reparsed.sort();
    let mut before = observations.clone();
    before.sort();
    assert_eq!(reparsed, before, "re-analysis must be idempotent");
}

#[test]
fn a_result_is_discarded_when_the_source_changed_during_analysis() {
    let Some(rig) = rig() else {
        eprintln!("skipping: run `pnpm faces:prepare` to enable this test");
        return;
    };
    let fixture = manifest_dir().join("tests/data/fixtures/two_people.jpg");
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("a.jpg");
    std::fs::copy(&fixture, &path).expect("fixture copy");
    let library = Library::open(&directory.path().join("library.sqlite")).expect("library");
    library.add_root(directory.path()).expect("root");
    library.index_root(directory.path()).expect("index");

    // The revision the analyzer is told to expect, and the pixels decoded from
    // those bytes.
    let revision = face_source_revision_for_path(&path).expect("revision");
    let image = load_image_at(&path);

    // The file is replaced while the analysis is in flight.
    let mut replaced = std::fs::read(&path).expect("read");
    replaced.extend_from_slice(b"\n<!-- replaced while analyzing -->");
    std::fs::write(&path, &replaced).expect("replace");

    let discarded = rig
        .analyzer
        .analyze_verified("asset", &path, &revision, &image)
        .expect("analysis runs");
    assert!(
        discarded.is_none(),
        "a result computed from bytes that no longer exist must be discarded"
    );

    // Nothing was stored, so the asset has no checkpoint and will be visited
    // again by the next run.
    assert!(
        library
            .face_observations_for_asset(&path)
            .expect("observations")
            .is_empty()
    );
    let targets = library
        .face_analysis_targets(&rig.detector, false, 100)
        .expect("targets");
    assert_eq!(targets.len(), 1, "the refused asset stays in the queue");
    // The queue's token comes from the index and therefore lags a file that
    // changed after indexing. That is exactly why the analyzer re-reads the
    // revision from disk before decoding instead of trusting the queued token:
    // the guard compares against the bytes it actually decoded.
    assert_ne!(
        targets[0].source_revision,
        face_source_revision_for_path(&path).expect("revision"),
        "the indexed token must not be assumed to describe the current bytes"
    );

    // With the revision of the bytes actually on disk, the result is accepted.
    let current = face_source_revision_for_path(&path).expect("revision");
    assert_ne!(current, revision);
    let accepted = rig
        .analyzer
        .analyze_verified("asset", &path, &current, &image)
        .expect("analysis runs");
    assert!(accepted.is_some(), "a current revision must be accepted");
}

#[test]
fn a_person_id_is_not_a_cluster_id_or_a_name() {
    let library = Library::in_memory().expect("in-memory library");
    library
        .create_person("person-1", "Alice", None)
        .expect("person");
    // Renaming is a label change; the identity used by every confirmation is
    // the person id, so nothing else has to move.
    library
        .rename_person("person-1", "Alice Zhang")
        .expect("rename");
    let persons = library.persons().expect("persons");
    assert_eq!(persons[0].person_id, "person-1");
    assert_eq!(persons[0].display_name, "Alice Zhang");
    let _: PersonId = persons[0].person_id.clone();
}
