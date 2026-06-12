use crate::{ImageDimensions, MediaError};
use image::{DynamicImage, ImageBuffer, Rgba};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    hash::Hash,
    path::Path,
    process::{Command, Stdio},
    sync::{LazyLock, Mutex},
    time::SystemTime,
};

const BACKEND: &str = "FFmpeg HEIF tile-grid";
static CAPABILITY: LazyLock<Result<(), String>> =
    LazyLock::new(|| capability_probe().map_err(|error| error.to_string()));
static GRID_CACHE: LazyLock<Mutex<HashMap<GridCacheKey, TileGrid>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GridCacheKey {
    path: std::path::PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Tile {
    stream_index: u64,
    x: u64,
    y: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TileGrid {
    width: u32,
    height: u32,
    tiles: Vec<Tile>,
}

pub fn capability() -> Result<(), MediaError> {
    CAPABILITY.clone().map_err(native_error)
}

fn capability_probe() -> Result<(), MediaError> {
    command_supports("ffmpeg", "-filters", "xstack")?;
    command_supports("ffprobe", "-h", "-show_stream_groups")?;
    Ok(())
}

pub fn can_decode(path: &Path) -> Result<(), MediaError> {
    capability()?;
    cached_grid(path).map(|_| ())
}

pub fn decode_full_rgba8(
    path: &Path,
    display_size: ImageDimensions,
) -> Result<DynamicImage, MediaError> {
    let grid = cached_grid(path)?;
    let (filter, width, height) = filter_for_grid(&grid, display_size)?;
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-filter_complex",
            &filter,
            "-map",
            "[out]",
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| native_error(format!("start ffmpeg: {error}")))?;
    if !output.status.success() {
        return Err(native_error(format!(
            "decode failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let expected_len = rgba_buffer_len(width, height)?;
    if output.stdout.len() != expected_len {
        return Err(native_error(format!(
            "decoded RGBA length mismatch: expected {expected_len}, got {}",
            output.stdout.len()
        )));
    }
    let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_vec(width, height, output.stdout)
        .ok_or_else(|| native_error("decoded image buffer dimensions do not match"))?;
    Ok(DynamicImage::ImageRgba8(image))
}

fn command_supports(
    command: &'static str,
    argument: &'static str,
    expected: &'static str,
) -> Result<(), MediaError> {
    let output = Command::new(command)
        .arg(argument)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| native_error(format!("{command} is unavailable: {error}")))?;
    if output.status.success()
        && (String::from_utf8_lossy(&output.stdout).contains(expected)
            || String::from_utf8_lossy(&output.stderr).contains(expected))
    {
        Ok(())
    } else {
        Err(native_error(format!(
            "{command} does not expose required {expected} support"
        )))
    }
}

fn probe_grid(path: &Path) -> Result<TileGrid, MediaError> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_stream_groups", "-of", "json"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| native_error(format!("start ffprobe: {error}")))?;
    if !output.status.success() {
        return Err(native_error(format!(
            "ffprobe failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    parse_grid(&output.stdout)
}

fn cached_grid(path: &Path) -> Result<TileGrid, MediaError> {
    let metadata = fs::metadata(path)?;
    let key = GridCacheKey {
        path: path.to_owned(),
        len: metadata.len(),
        modified: metadata.modified().ok(),
    };
    if let Some(grid) = GRID_CACHE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&key)
        .cloned()
    {
        return Ok(grid);
    }
    let grid = probe_grid(path)?;
    GRID_CACHE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(key, grid.clone());
    Ok(grid)
}

fn parse_grid(json: &[u8]) -> Result<TileGrid, MediaError> {
    let root: Value = serde_json::from_slice(json)
        .map_err(|error| native_error(format!("parse ffprobe JSON: {error}")))?;
    let groups = root["stream_groups"]
        .as_array()
        .ok_or_else(|| native_error("ffprobe returned no stream groups"))?;
    let group = groups
        .iter()
        .find(|group| group["type"].as_str() == Some("Tile Grid"))
        .ok_or_else(|| native_error("HEIF has no FFmpeg tile-grid stream group"))?;
    let component = group["components"]
        .as_array()
        .and_then(|components| components.first())
        .ok_or_else(|| native_error("tile-grid stream group has no component"))?;
    let width = json_u32(component, "width")?;
    let height = json_u32(component, "height")?;
    let subcomponents = component["subcomponents"]
        .as_array()
        .ok_or_else(|| native_error("tile-grid component has no tiles"))?;
    let tiles = subcomponents
        .iter()
        .map(|tile| {
            Ok(Tile {
                stream_index: json_u64(tile, "stream_index")?,
                x: json_u64(tile, "tile_horizontal_offset")?,
                y: json_u64(tile, "tile_vertical_offset")?,
            })
        })
        .collect::<Result<Vec<_>, MediaError>>()?;
    if tiles.is_empty() {
        return Err(native_error("tile-grid component has no tiles"));
    }
    Ok(TileGrid {
        width,
        height,
        tiles,
    })
}

fn filter_for_grid(
    grid: &TileGrid,
    display_size: ImageDimensions,
) -> Result<(String, u32, u32), MediaError> {
    let inputs = grid
        .tiles
        .iter()
        .map(|tile| format!("[0:{}]", tile.stream_index))
        .collect::<String>();
    let layout = grid
        .tiles
        .iter()
        .map(|tile| format!("{}_{}", tile.x, tile.y))
        .collect::<Vec<_>>()
        .join("|");
    let mut filter = format!(
        "{inputs}xstack=inputs={}:layout={layout},crop={}:{}",
        grid.tiles.len(),
        grid.width,
        grid.height
    );
    let output_size = if grid.width == display_size.width && grid.height == display_size.height {
        display_size
    } else if grid.width == display_size.height && grid.height == display_size.width {
        filter.push_str(",transpose=clock");
        display_size
    } else {
        return Err(native_error(format!(
            "tile-grid size {}x{} does not match display size {}x{}",
            grid.width, grid.height, display_size.width, display_size.height
        )));
    };
    filter.push_str(",format=rgba[out]");
    Ok((filter, output_size.width, output_size.height))
}

fn rgba_buffer_len(width: u32, height: u32) -> Result<usize, MediaError> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| native_error("decoded RGBA buffer size overflow"))
}

fn json_u32(value: &Value, key: &str) -> Result<u32, MediaError> {
    json_u64(value, key)?
        .try_into()
        .map_err(|_| native_error(format!("ffprobe {key} exceeds u32")))
}

fn json_u64(value: &Value, key: &str) -> Result<u64, MediaError> {
    value[key]
        .as_u64()
        .ok_or_else(|| native_error(format!("ffprobe tile-grid is missing {key}")))
}

fn native_error(message: impl Into<String>) -> MediaError {
    MediaError::NativeDecode {
        backend: BACKEND,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID_JSON: &[u8] = br#"{
      "stream_groups": [{
        "type": "Tile Grid",
        "components": [{
          "width": 7008,
          "height": 4672,
          "subcomponents": [
            {"stream_index": 0, "tile_horizontal_offset": 0, "tile_vertical_offset": 0},
            {"stream_index": 1, "tile_horizontal_offset": 3520, "tile_vertical_offset": 0}
          ]
        }]
      }]
    }"#;

    #[test]
    fn parses_grid_and_builds_dynamic_rotated_filter() {
        let grid = parse_grid(GRID_JSON).unwrap();
        assert_eq!(grid.tiles.len(), 2);
        let (filter, width, height) = filter_for_grid(
            &grid,
            ImageDimensions {
                width: 4672,
                height: 7008,
            },
        )
        .unwrap();
        assert_eq!((width, height), (4672, 7008));
        assert_eq!(
            filter,
            "[0:0][0:1]xstack=inputs=2:layout=0_0|3520_0,crop=7008:4672,transpose=clock,format=rgba[out]"
        );
    }

    #[test]
    fn decodes_repository_hif_fixture_when_ffmpeg_is_available() {
        if capability().is_err() {
            return;
        }
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let image = decode_full_rgba8(
            &fixture,
            ImageDimensions {
                width: 4672,
                height: 7008,
            },
        )
        .unwrap();
        assert_eq!((image.width(), image.height()), (4672, 7008));
    }

    #[test]
    #[ignore = "decodes the full fixture through both FFmpeg and libheif"]
    fn fixture_orientation_matches_libheif() {
        if capability().is_err() {
            return;
        }
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let ffmpeg = decode_full_rgba8(
            &fixture,
            ImageDimensions {
                width: 4672,
                height: 7008,
            },
        )
        .unwrap()
        .thumbnail_exact(64, 96)
        .into_rgb8();
        let libheif = crate::heif::decode_full_rgb8(&fixture)
            .unwrap()
            .thumbnail_exact(64, 96)
            .into_rgb8();
        let current = mean_absolute_error(ffmpeg.as_raw(), libheif.as_raw());
        let rotated = image::imageops::rotate180(&ffmpeg);
        let opposite = mean_absolute_error(rotated.as_raw(), libheif.as_raw());
        assert!(
            current < opposite,
            "FFmpeg orientation should match libheif: current={current}, opposite={opposite}"
        );
    }

    fn mean_absolute_error(left: &[u8], right: &[u8]) -> f64 {
        left.iter()
            .zip(right)
            .map(|(left, right)| (f64::from(*left) - f64::from(*right)).abs())
            .sum::<f64>()
            / left.len() as f64
    }
}
