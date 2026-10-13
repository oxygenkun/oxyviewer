//! Application services and state, independent of the desktop framework.

pub mod app;
pub use app::{AppPaths, OxyApp};
pub mod browse;
pub mod contracts;
pub mod events;
pub mod files;
pub mod jobs;
pub mod library;
pub mod metadata;
pub mod people;
pub mod resources;
pub mod runtime;
pub mod storage;
pub mod tags;

pub mod settings;
