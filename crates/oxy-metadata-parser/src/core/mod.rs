//! Foundation layer: byte readers, error types, file detection, recursion guard.

mod error;
mod magic;
mod mmap;
mod reader;
mod recursion;
pub mod value;

pub use error::{Error, Result};
pub use magic::FileType;
pub use mmap::MappedFile;
pub use reader::Reader;
pub use recursion::RecursionGuard;
pub use value::TagValue;
