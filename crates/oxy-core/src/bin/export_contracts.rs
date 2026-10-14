//! Generate or check the single TypeScript contract artifact from Rust types.
use std::{collections::BTreeMap, error::Error, path::PathBuf};
use ts_rs::{Config, TS};

fn register<T: TS>(types: &mut BTreeMap<String, String>, cfg: &Config) {
    types.insert(T::ident(cfg), T::decl(cfg));
}

fn main() -> Result<(), Box<dyn Error>> {
    let check = std::env::args().skip(1).any(|arg| arg == "--check");
    let cfg = Config::new().with_large_int("number");
    let mut types = BTreeMap::new();
    register::<oxy_core::contracts::AboutLink>(&mut types, &cfg);
    register::<oxy_media::types::AccelerationKind>(&mut types, &cfg);
    register::<oxy_media::types::AnalysisRequirement>(&mut types, &cfg);
    register::<oxy_core::contracts::AppInfo>(&mut types, &cfg);
    register::<oxy_media::types::ArtifactFacts>(&mut types, &cfg);
    register::<oxy_core::contracts::AssetDetails>(&mut types, &cfg);
    register::<oxy_core::contracts::AssetDetailsResult>(&mut types, &cfg);
    register::<oxy_media::types::AssetKind>(&mut types, &cfg);
    register::<oxy_core::contracts::AssetQuery>(&mut types, &cfg);
    register::<oxy_core::contracts::AssetSort>(&mut types, &cfg);
    register::<oxy_core::contracts::AssetSummary>(&mut types, &cfg);
    register::<oxy_core::contracts::AssetTagAssignment>(&mut types, &cfg);
    register::<oxy_core::contracts::AssetTagAssignmentsByPath>(&mut types, &cfg);
    register::<oxy_core::contracts::BeginPersonAnalysis>(&mut types, &cfg);
    register::<oxy_core::contracts::BrowsePage>(&mut types, &cfg);
    register::<oxy_core::contracts::BurstFrame>(&mut types, &cfg);
    register::<oxy_core::contracts::BurstGroup>(&mut types, &cfg);
    register::<oxy_media::types::ByteIntegrity>(&mut types, &cfg);
    register::<oxy_core::contracts::CacheSettings>(&mut types, &cfg);
    register::<oxy_core::contracts::CacheSettingsUpdate>(&mut types, &cfg);
    register::<oxy_media::types::CaptureMetadata>(&mut types, &cfg);
    register::<oxy_core::contracts::CustomTag>(&mut types, &cfg);
    register::<oxy_core::contracts::DebugQueueItem>(&mut types, &cfg);
    register::<oxy_core::contracts::DebugQueueSnapshot>(&mut types, &cfg);
    register::<oxy_core::contracts::DebugQueueState>(&mut types, &cfg);
    register::<oxy_media::types::DetailCoverage>(&mut types, &cfg);
    register::<oxy_core::contracts::DetectedPersonInstance>(&mut types, &cfg);
    register::<oxy_core::contracts::DirectoryBrowseProgress>(&mut types, &cfg);
    register::<oxy_core::contracts::DirectorySearchMatch>(&mut types, &cfg);
    register::<oxy_core::contracts::DirectorySummary>(&mut types, &cfg);
    register::<oxy_core::contracts::DirectoryTreeNode>(&mut types, &cfg);
    register::<oxy_core::contracts::DirectoryTreeSnapshot>(&mut types, &cfg);
    register::<oxy_media::types::DisplayDimensions>(&mut types, &cfg);
    register::<oxy_media::types::EditableMetadata>(&mut types, &cfg);
    register::<oxy_media::types::EncodedDimensions>(&mut types, &cfg);
    register::<oxy_core::metadata::provider::ExiftoolStatus>(&mut types, &cfg);
    register::<oxy_core::contracts::ExternalAppSettings>(&mut types, &cfg);
    register::<oxy_core::contracts::ExternalApplication>(&mut types, &cfg);
    register::<oxy_core::contracts::ExternalOpenResult>(&mut types, &cfg);
    register::<oxy_core::contracts::FileDeletionMode>(&mut types, &cfg);
    register::<oxy_core::contracts::FileOperation>(&mut types, &cfg);
    register::<oxy_core::contracts::FileOperationResult>(&mut types, &cfg);
    register::<oxy_core::contracts::FileOperationRecovery>(&mut types, &cfg);
    register::<oxy_media::types::FocusInfo>(&mut types, &cfg);
    register::<oxy_media::types::FocusRegion>(&mut types, &cfg);
    register::<oxy_core::contracts::FolderPeopleWorkspace>(&mut types, &cfg);
    register::<oxy_core::contracts::FolderSession>(&mut types, &cfg);
    register::<oxy_core::contracts::GlobalPerson>(&mut types, &cfg);
    register::<oxy_core::contracts::GlobalPersonFolder>(&mut types, &cfg);
    register::<oxy_core::contracts::GlobalPersonFolders>(&mut types, &cfg);
    register::<oxy_core::contracts::GlobalPersonGallery>(&mut types, &cfg);
    register::<oxy_media::types::HeifBackendKind>(&mut types, &cfg);
    register::<oxy_media::types::HeifCapabilities>(&mut types, &cfg);
    register::<oxy_media::types::HeifDecodeSession>(&mut types, &cfg);
    register::<oxy_media::types::HeifDecodeStatus>(&mut types, &cfg);
    register::<oxy_media::types::HeifDiagnostics>(&mut types, &cfg);
    register::<oxy_core::contracts::HeifFullPresentation>(&mut types, &cfg);
    register::<oxy_core::contracts::HeifStatusEvent>(&mut types, &cfg);
    register::<oxy_media::types::HeifTilePayload>(&mut types, &cfg);
    register::<oxy_core::contracts::HeifTileReady>(&mut types, &cfg);
    register::<oxy_media::types::ImageOperation>(&mut types, &cfg);
    register::<oxy_media::types::ImageOrigin>(&mut types, &cfg);
    register::<oxy_core::contracts::ImageProjection>(&mut types, &cfg);
    register::<oxy_core::contracts::JobPriority>(&mut types, &cfg);
    register::<oxy_core::contracts::JobProgress>(&mut types, &cfg);
    register::<oxy_core::contracts::LibraryIndexUpdate>(&mut types, &cfg);
    register::<oxy_media::types::MediaPersistence>(&mut types, &cfg);
    register::<oxy_media::types::MediaResourceDescriptor>(&mut types, &cfg);
    register::<oxy_media::types::MediaSatisfaction>(&mut types, &cfg);
    register::<oxy_core::contracts::MetadataCapability>(&mut types, &cfg);
    register::<oxy_core::contracts::MetadataPatch>(&mut types, &cfg);
    register::<oxy_core::contracts::MetadataProjection>(&mut types, &cfg);
    register::<oxy_core::contracts::MetadataProvider>(&mut types, &cfg);
    register::<oxy_core::contracts::MetadataRequestPriority>(&mut types, &cfg);
    register::<oxy_core::contracts::OmittedScheduleAction>(&mut types, &cfg);
    register::<oxy_core::contracts::Page<String>>(&mut types, &cfg);
    register::<oxy_core::contracts::PerfScenario>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonAnalysisRun>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonAnalysisState>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonAnalysisTask>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonAnalysisTaskInput>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonCluster>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonClusterMember>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonClusterSnapshot>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonDistanceMetric>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonModelArtifact>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonModelArtifactFormat>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonModelLicenseStatus>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonModelStatus>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonOperationState>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonOperationStatus>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonReviewDecision>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonStageKind>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonStageManifest>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonTuple>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonTupleFilter>(&mut types, &cfg);
    register::<oxy_core::contracts::PersonTupleGroup>(&mut types, &cfg);
    register::<oxy_media::types::PickLabel>(&mut types, &cfg);
    register::<oxy_core::contracts::PipelineManifest>(&mut types, &cfg);
    register::<oxy_media::types::PixelDimensions>(&mut types, &cfg);
    register::<oxy_media::types::PreviewContentRect>(&mut types, &cfg);
    register::<oxy_media::types::PreviewDiagnostics>(&mut types, &cfg);
    register::<oxy_media::types::PreviewDisplaySize>(&mut types, &cfg);
    register::<oxy_media::types::PreviewGeometry>(&mut types, &cfg);
    register::<oxy_media::types::PreviewKind>(&mut types, &cfg);
    register::<oxy_core::contracts::PreviewOmittedPolicy>(&mut types, &cfg);
    register::<oxy_media::types::PreviewPriority>(&mut types, &cfg);
    register::<oxy_media::types::PreviewResult>(&mut types, &cfg);
    register::<oxy_core::contracts::PreviewScheduleIntent>(&mut types, &cfg);
    register::<oxy_media::types::RawDecoderStatus>(&mut types, &cfg);
    register::<oxy_media::types::RawSystemAttempt>(&mut types, &cfg);
    register::<oxy_media::types::RawSystemAttemptState>(&mut types, &cfg);
    register::<oxy_media::types::RawSystemAvailability>(&mut types, &cfg);
    register::<oxy_media::types::RawSystemCodec>(&mut types, &cfg);
    register::<oxy_core::contracts::RelocationEntry>(&mut types, &cfg);
    register::<oxy_core::contracts::RelocationMatch>(&mut types, &cfg);
    register::<oxy_media::types::RenderLevel>(&mut types, &cfg);
    register::<oxy_core::contracts::ResourceLoadStatus>(&mut types, &cfg);
    register::<oxy_media::types::ResourceRegistryStats>(&mut types, &cfg);
    register::<oxy_core::contracts::ReviewPersonTuples>(&mut types, &cfg);
    register::<oxy_core::contracts::RootRelocationPlan>(&mut types, &cfg);
    register::<oxy_core::contracts::RootRelocationResult>(&mut types, &cfg);
    register::<oxy_core::contracts::RunPeopleGrouping>(&mut types, &cfg);
    register::<oxy_media::types::Sampling>(&mut types, &cfg);
    register::<oxy_core::contracts::SaveGlobalPerson>(&mut types, &cfg);
    register::<oxy_core::contracts::SavePersonTupleGeometry>(&mut types, &cfg);
    register::<oxy_core::contracts::SchedulePlacement>(&mut types, &cfg);
    register::<oxy_core::contracts::SetGlobalPersonReference>(&mut types, &cfg);
    register::<oxy_core::contracts::SetGlobalPersonTag>(&mut types, &cfg);
    register::<oxy_core::contracts::SortDirection>(&mut types, &cfg);
    register::<oxy_media::types::SourceImage>(&mut types, &cfg);
    register::<oxy_core::contracts::TagDeleteImpact>(&mut types, &cfg);
    register::<oxy_core::contracts::TagMatchMode>(&mut types, &cfg);
    register::<oxy_core::contracts::TagSyncStatus>(&mut types, &cfg);
    register::<oxy_core::contracts::UpdateStatus>(&mut types, &cfg);
    for (name, value) in [
        (
            "AssetId",
            <oxy_core::contracts::AssetId as TS>::inline(&cfg),
        ),
        ("JobId", <oxy_core::contracts::JobId as TS>::inline(&cfg)),
        (
            "CustomTagId",
            <oxy_core::contracts::CustomTagId as TS>::inline(&cfg),
        ),
        (
            "SessionId",
            <oxy_media::types::SessionId as TS>::inline(&cfg),
        ),
    ] {
        types.insert(name.into(), format!("type {name} = {value};"));
    }
    let output = "// Generated by pnpm contracts:generate. Do not edit.\n\n".to_owned()
        + &types
            .values()
            .map(|declaration| format!("export {declaration}\n"))
            .collect::<String>();
    let output = output
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/generated/contracts.ts");
    if check {
        if std::fs::read_to_string(&path)?.replace("\r\n", "\n") != output {
            return Err("Rust/TypeScript contracts differ; run pnpm contracts:generate".into());
        }
    } else {
        std::fs::create_dir_all(path.parent().ok_or("contract output has no parent")?)?;
        std::fs::write(path, output)?;
    }
    Ok(())
}
