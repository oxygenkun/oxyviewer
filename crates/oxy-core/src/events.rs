//! Typed application events. The desktop adapter owns serialization and delivery.
use crate::browse::types::{DirectoryBrowseProgress, DirectoryTreeSnapshot};
use crate::library::types::LibraryIndexUpdate;
use crate::resources::types::{HeifStatusEvent, ImageProjection, MetadataProjection};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum CoreEvent {
    ImageProjectionUpdated(Box<ImageProjection>),
    MetadataProjectionUpdated(MetadataProjection),
    EmbeddedSyncProgress(crate::metadata::types::EmbeddedSyncProgress),
    AssetTagsUpdated(PathBuf),
    HeifTilePublished(Box<oxy_media::HeifTilePublication>),
    HeifStatusUpdated(HeifStatusEvent),
    DirectoryTreeUpdated(DirectoryTreeSnapshot),
    DirectoryBrowseProgress(DirectoryBrowseProgress),
    LibraryDirectoriesIndexed(LibraryIndexUpdate),
    LibraryIndexed(LibraryIndexUpdate),
    Shutdown,
}
