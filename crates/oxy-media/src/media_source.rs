use crate::{MediaError, backends::libheif, pipeline::raw};
use image::ImageReader;
use oxy_domain::{AssetKind, PreviewKind, PreviewResult, RenderLevel};
use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}

fn raster_dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    let raster = || -> Result<ImageDimensions, MediaError> {
        let reader = ImageReader::new(BufReader::new(File::open(path)?)).with_guessed_format()?;
        let (width, height) = reader.into_dimensions()?;
        Ok(ImageDimensions { width, height })
    };

    raster()
}

pub fn dimensions(path: &Path, kind: AssetKind) -> Result<ImageDimensions, MediaError> {
    match kind {
        AssetKind::Jpeg | AssetKind::Png | AssetKind::Webp | AssetKind::Tiff => {
            raster_dimensions(path)
        }
        AssetKind::Heif => libheif::dimensions(path),
        AssetKind::Raw => raw::dimensions(path),
    }
}

pub(crate) fn preview_result(
    path: PathBuf,
    kind: PreviewKind,
    level: RenderLevel,
) -> Result<PreviewResult, MediaError> {
    let size = raster_dimensions(&path)?;
    Ok(PreviewResult {
        path,
        width: size.width,
        height: size.height,
        kind,
        render_level: level,
        diagnostics: None,
    })
}

pub(crate) fn cached_preview_result(
    path: PathBuf,
    kind: PreviewKind,
    level: RenderLevel,
) -> Result<Option<PreviewResult>, MediaError> {
    if !path.is_file() {
        return Ok(None);
    }
    match preview_result(path.clone(), kind, level) {
        Ok(result) => Ok(Some(result)),
        Err(_) => match std::fs::remove_file(path) {
            Ok(()) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        },
    }
}

pub(crate) fn has_complete_jpeg_markers(path: &Path) -> Result<bool, std::io::Error> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() < 4 {
        return Ok(false);
    }
    let mut start = [0_u8; 2];
    file.read_exact(&mut start)?;
    file.seek(SeekFrom::End(-2))?;
    let mut end = [0_u8; 2];
    file.read_exact(&mut end)?;
    Ok(start == [0xff, 0xd8] && end == [0xff, 0xd9])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupted_cached_artifact_is_removed_for_rebuild() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("corrupted.jpg");
        std::fs::write(&path, b"not a jpeg").unwrap();

        let result =
            cached_preview_result(path.clone(), PreviewKind::Decoded, RenderLevel::Preview)
                .unwrap();

        assert!(result.is_none());
        assert!(!path.exists());
    }
}
