use crate::{MediaError, backends::libheif, pipeline::raw};
use image::ImageReader;
use oxy_domain::{PreviewKind, PreviewResult};
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

pub fn dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    let raster = || -> Result<ImageDimensions, MediaError> {
        let reader = ImageReader::new(BufReader::new(File::open(path)?)).with_guessed_format()?;
        let (width, height) = reader.into_dimensions()?;
        Ok(ImageDimensions { width, height })
    };

    raster()
        .or_else(|_| libheif::dimensions(path))
        .or_else(|_| raw::dimensions(path))
}

pub(crate) fn preview_result(
    path: PathBuf,
    kind: PreviewKind,
) -> Result<PreviewResult, MediaError> {
    let size = dimensions(&path)?;
    Ok(PreviewResult {
        path,
        width: size.width,
        height: size.height,
        kind,
        render_level: None,
        diagnostics: None,
    })
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
