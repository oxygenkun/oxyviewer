//! Typed application events. The desktop adapter owns serialization and delivery.
use crate::browse::types::{DirectoryBrowseProgress, DirectoryTreeSnapshot};
use crate::library::types::LibraryIndexUpdate;
use crate::resources::types::{ImageProjection, MetadataProjection};

#[derive(Debug, Clone)]
pub enum CoreEvent {
    ImageProjectionUpdated(Box<ImageProjection>),
    MetadataProjectionUpdated(MetadataProjection),
    DirectoryTreeUpdated(DirectoryTreeSnapshot),
    DirectoryBrowseProgress(DirectoryBrowseProgress),
    LibraryDirectoriesIndexed(LibraryIndexUpdate),
    LibraryIndexed(LibraryIndexUpdate),
    Shutdown,
}
