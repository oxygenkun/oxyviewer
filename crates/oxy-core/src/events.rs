//! Typed application events. The desktop adapter owns serialization and delivery.
use crate::contracts::{
    DirectoryTreeSnapshot, ImageProjection, LibraryIndexUpdate, MetadataProjection,
};

#[derive(Debug, Clone)]
pub enum CoreEvent {
    ImageProjectionUpdated(Box<ImageProjection>),
    MetadataProjectionUpdated(MetadataProjection),
    DirectoryTreeUpdated(DirectoryTreeSnapshot),
    LibraryDirectoriesIndexed(LibraryIndexUpdate),
    LibraryIndexed(LibraryIndexUpdate),
    Shutdown,
}
