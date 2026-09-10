use crate::{ImageDimensions, MediaError};
use image::{DynamicImage, RgbaImage};
use serde_json::Value;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::{
    collections::HashMap,
    fs,
    hash::Hash,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{LazyLock, Mutex},
    time::SystemTime,
};

const BACKEND: &str = "FFmpeg HEIF tile-grid";
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(target_os = "windows")]
const HIGH_PRIORITY_CLASS: u32 = 0x0000_0080;
static CAPABILITY: LazyLock<Result<(), String>> =
    LazyLock::new(|| capability_probe().map_err(|error| error.to_string()));
static GRID_CACHE: LazyLock<Mutex<HashMap<GridCacheKey, TileGrid>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static COMMANDS: LazyLock<(PathBuf, PathBuf)> = LazyLock::new(resolve_commands);

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
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TileGrid {
    width: u32,
    height: u32,
    rotation: i32,
    tiles: Vec<Tile>,
    previews: Vec<PreviewStream>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewStream {
    index: u64,
    width: u32,
    height: u32,
}

#[derive(Debug)]
#[cfg_attr(any(target_os = "macos", target_os = "linux"), allow(dead_code))]
pub struct EncodedTile {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub jpeg: Vec<u8>,
}

pub fn capability() -> Result<(), MediaError> {
    CAPABILITY.clone().map_err(native_error)
}

fn capability_probe() -> Result<(), MediaError> {
    command_supports(&COMMANDS.0, "-decoders", "hevc")?;
    command_supports(&COMMANDS.1, "-h", "-show_stream_groups")?;
    Ok(())
}

pub fn can_decode(path: &Path) -> Result<(), MediaError> {
    // Parsing this specific file is both cheaper and stronger evidence than
    // launching two additional generic capability probes. The actual decode
    // still reports a precise error and falls back to libheif if FFmpeg is
    // absent or incompatible.
    cached_grid(path).map(|_| ())
}

#[cfg_attr(any(target_os = "macos", target_os = "linux"), allow(dead_code))]
pub fn tile_count(path: &Path) -> Result<usize, MediaError> {
    cached_grid(path).map(|grid| grid.tiles.len())
}

/// Lets FFmpeg keep each HEVC grid component compressed for delivery to the
/// WebView. Six high-quality JPEG tiles are much cheaper to cross the custom
/// protocol boundary than 35 raw RGBA tiles (roughly 131 MB for the fixture).
#[cfg_attr(any(target_os = "macos", target_os = "linux"), allow(dead_code))]
pub fn decode_full_jpeg_tiles(
    path: &Path,
    display_size: ImageDimensions,
    display_sharpening: bool,
) -> Result<Vec<EncodedTile>, MediaError> {
    let grid = cached_grid(path)?;
    filter_for_grid(&grid, display_size)?;
    let output_dir = tempfile::tempdir()?;
    let mut command = media_command(&COMMANDS.0);
    command
        .args(["-v", "error", "-threads", "2", "-i"])
        .arg(path);
    let mut outputs = Vec::with_capacity(grid.tiles.len());
    for (position, tile) in grid.tiles.iter().enumerate() {
        let output_path = output_dir.path().join(format!("{position}.jpg"));
        let (mut filter, x, y, width, height) = oriented_tile(&grid, tile)?;
        if display_sharpening {
            filter.push_str(",unsharp=3:3:0.2:3:3:0");
        }
        command
            .args([
                "-map",
                &format!("0:{}", tile.stream_index),
                "-frames:v",
                "1",
            ])
            .args(["-vf", &filter])
            .args(["-pix_fmt", "yuvj444p", "-c:v", "mjpeg", "-q:v", "2"])
            .arg(&output_path);
        outputs.push((output_path, x, y, width, height));
    }
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| native_error(format!("start ffmpeg JPEG tile decode: {error}")))?;
    if !output.status.success() {
        return Err(native_error(format!(
            "JPEG tile decode failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut tiles = outputs
        .into_iter()
        .map(|(path, x, y, width, height)| {
            Ok(EncodedTile {
                x,
                y,
                width,
                height,
                jpeg: fs::read(path)?,
            })
        })
        .collect::<Result<Vec<_>, MediaError>>()?;
    let center = (
        display_size.width as i64 / 2,
        display_size.height as i64 / 2,
    );
    tiles.sort_by_key(|tile| {
        let tile_center = (
            tile.x as i64 + tile.width as i64 / 2,
            tile.y as i64 + tile.height as i64 / 2,
        );
        (tile_center.0 - center.0).abs() + (tile_center.1 - center.1).abs()
    });
    Ok(tiles)
}

pub fn decode_full_rgba8(
    path: &Path,
    display_size: ImageDimensions,
    display_sharpening: bool,
) -> Result<DynamicImage, MediaError> {
    let grid = cached_grid(path)?;
    filter_for_grid(&grid, display_size)?;
    let (output_dir, output_paths) = decode_tile_bitmaps(path, &grid, display_sharpening)?;
    let mut image = RgbaImage::new(display_size.width, display_size.height);
    for (tile, output_path) in grid.tiles.iter().zip(&output_paths) {
        let (_, x, y, width, height) = oriented_tile(&grid, tile)?;
        copy_bmp_tile(&mut image, output_path, x, y, width, height)?;
    }
    drop(output_dir);
    Ok(DynamicImage::ImageRgba8(image))
}

/// Convert the primary HEIF tile grid straight to one JPEG in FFmpeg. The
/// decoded frame never crosses into a Rust `RgbaImage`.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn transcode_full_jpeg(
    path: &Path,
    destination: &Path,
    display_size: ImageDimensions,
    quality: u8,
) -> Result<(), MediaError> {
    let grid = cached_grid(path)?;
    let (filter, _, _) = filter_for_grid(&grid, display_size)?;
    let filter = filter.replace(",format=rgba[out]", ",format=yuvj444p[out]");
    let qscale = ((100_u16.saturating_sub(u16::from(quality))) / 4 + 1)
        .clamp(1, 31)
        .to_string();
    let output = media_command(&COMMANDS.0)
        .args(["-v", "error", "-threads", "2", "-i"])
        .arg(path)
        .args([
            "-filter_complex",
            &filter,
            "-map",
            "[out]",
            "-frames:v",
            "1",
        ])
        .args(["-c:v", "mjpeg", "-q:v", &qscale, "-y"])
        .arg(destination)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| native_error(format!("start ffmpeg HEIF to JPEG conversion: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(native_error(format!(
            "HEIF to JPEG conversion failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn decode_tile_bitmaps(
    path: &Path,
    grid: &TileGrid,
    display_sharpening: bool,
) -> Result<(tempfile::TempDir, Vec<PathBuf>), MediaError> {
    let output_dir = tempfile::tempdir()?;
    let mut command = media_command(&COMMANDS.0);
    command
        .args(["-v", "error", "-threads", "2", "-i"])
        .arg(path);
    let mut output_paths = Vec::with_capacity(grid.tiles.len());
    for (position, tile) in grid.tiles.iter().enumerate() {
        let output_path = output_dir.path().join(format!("{position}.bmp"));
        let (mut filter, _, _, _, _) = oriented_tile(grid, tile)?;
        if display_sharpening {
            filter.push_str(",unsharp=3:3:0.2:3:3:0");
        }
        command
            .args([
                "-map",
                &format!("0:{}", tile.stream_index),
                "-frames:v",
                "1",
            ])
            .args(["-vf", &filter])
            .args(["-pix_fmt", "bgra", "-c:v", "bmp"])
            .arg(&output_path);
        output_paths.push(output_path);
    }
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| native_error(format!("start ffmpeg tile decode: {error}")))?;
    if !output.status.success() {
        return Err(native_error(format!(
            "decode failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok((output_dir, output_paths))
}

fn copy_bmp_tile(
    destination: &mut RgbaImage,
    path: &Path,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), MediaError> {
    let bmp = fs::read(path)?;
    if bmp.get(0..2) != Some(b"BM") || bmp.len() < 54 {
        return Err(native_error("FFmpeg tile output is not a BMP image"));
    }
    let data_offset = bmp_u32(&bmp, 10)? as usize;
    let bmp_width = bmp_i32(&bmp, 18)?;
    let signed_height = bmp_i32(&bmp, 22)?;
    if bmp_width <= 0
        || signed_height == 0
        || bmp_u16(&bmp, 28)? != 32
        || bmp_u32(&bmp, 30)? != 0
        || u32::try_from(bmp_width).ok() != Some(width)
        || signed_height.unsigned_abs() != height
    {
        return Err(native_error(
            "FFmpeg produced an unexpected BMP tile layout",
        ));
    }
    if x.checked_add(width)
        .is_none_or(|right| right > destination.width())
        || y.checked_add(height)
            .is_none_or(|bottom| bottom > destination.height())
    {
        return Err(native_error(
            "decoded tile lies outside the HEIF display canvas",
        ));
    }
    let source_stride = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| native_error("source tile stride overflow"))?;
    let destination_stride = usize::try_from(destination.width())
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| native_error("destination stride overflow"))?;
    let height = usize::try_from(height).map_err(|_| native_error("tile height exceeds usize"))?;
    let x = usize::try_from(x).map_err(|_| native_error("tile x offset exceeds usize"))?;
    let y = usize::try_from(y).map_err(|_| native_error("tile y offset exceeds usize"))?;
    let pixel_bytes = source_stride
        .checked_mul(height)
        .ok_or_else(|| native_error("BMP tile size overflow"))?;
    if bmp.len() < data_offset.saturating_add(pixel_bytes) {
        return Err(native_error("FFmpeg BMP tile is truncated"));
    }
    for row in 0..height {
        let source_row = if signed_height > 0 {
            height - row - 1
        } else {
            row
        };
        let source_start = data_offset + source_row * source_stride;
        let destination_start = (y + row) * destination_stride + x * 4;
        let source = &bmp[source_start..source_start + source_stride];
        let target =
            &mut destination.as_mut()[destination_start..destination_start + source_stride];
        for (bgra, rgba) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
            rgba.copy_from_slice(&[bgra[2], bgra[1], bgra[0], bgra[3]]);
        }
    }
    Ok(())
}

fn bmp_u16(bytes: &[u8], offset: usize) -> Result<u16, MediaError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| native_error("truncated BMP header"))
}

fn bmp_u32(bytes: &[u8], offset: usize) -> Result<u32, MediaError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| native_error("truncated BMP header"))
}

fn bmp_i32(bytes: &[u8], offset: usize) -> Result<i32, MediaError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(i32::from_le_bytes)
        .ok_or_else(|| native_error("truncated BMP header"))
}

fn oriented_tile(grid: &TileGrid, tile: &Tile) -> Result<(String, u32, u32, u32, u32), MediaError> {
    let x: u32 = tile
        .x
        .try_into()
        .map_err(|_| native_error("tile x exceeds u32"))?;
    let y: u32 = tile
        .y
        .try_into()
        .map_err(|_| native_error("tile y exceeds u32"))?;
    let width = tile.width.min(
        grid.width
            .checked_sub(x)
            .ok_or_else(|| native_error("tile x outside grid"))?,
    );
    let height = tile.height.min(
        grid.height
            .checked_sub(y)
            .ok_or_else(|| native_error("tile y outside grid"))?,
    );
    let crop = format!("crop={width}:{height}:0:0");
    match grid.rotation {
        -90 | 270 => Ok((
            format!("{crop},transpose=clock"),
            grid.height - y - height,
            x,
            height,
            width,
        )),
        90 | -270 => Ok((
            format!("{crop},transpose=cclock"),
            y,
            grid.width - x - width,
            height,
            width,
        )),
        180 | -180 => Ok((
            format!("{crop},hflip,vflip"),
            grid.width - x - width,
            grid.height - y - height,
            width,
            height,
        )),
        _ => Ok((crop, x, y, width, height)),
    }
}

/// Decode the smallest non-tile image that can satisfy a progressive preview.
/// Sony HIF files commonly carry a medium-sized camera-rendered HEVC image in
/// addition to the primary tile grid. Decoding that single stream avoids
/// paying for all six full-resolution tiles just to paint the first frame.
#[cfg_attr(any(target_os = "macos", target_os = "linux"), allow(dead_code))]
pub fn decode_scaled_preview(path: &Path, max_size: u32) -> Result<DynamicImage, MediaError> {
    let grid = cached_grid(path)?;
    let stream = grid
        .previews
        .iter()
        .filter(|stream| stream.width.max(stream.height) >= max_size)
        .min_by_key(|stream| stream.width.max(stream.height))
        .ok_or_else(|| native_error("HEIF has no sufficiently large independent preview stream"))?;
    let map = format!("0:{}", stream.index);
    let scale = format!("scale={max_size}:{max_size}:force_original_aspect_ratio=decrease");
    let output = media_command(&COMMANDS.0)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            &map,
            "-frames:v",
            "1",
            "-vf",
            &scale,
            "-q:v",
            "2",
            "-c:v",
            "mjpeg",
            "-f",
            "image2pipe",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| native_error(format!("start ffmpeg preview: {error}")))?;
    if !output.status.success() {
        return Err(native_error(format!(
            "preview decode failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    image::load_from_memory(&output.stdout)
        .map_err(|error| native_error(format!("decode ffmpeg preview bitmap: {error}")))
}

fn command_supports(
    command: &Path,
    argument: &'static str,
    expected: &'static str,
) -> Result<(), MediaError> {
    let output = media_command(command)
        .arg(argument)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| native_error(format!("{} is unavailable: {error}", command.display())))?;
    if output.status.success()
        && (String::from_utf8_lossy(&output.stdout).contains(expected)
            || String::from_utf8_lossy(&output.stderr).contains(expected))
    {
        Ok(())
    } else {
        Err(native_error(format!(
            "{} does not expose required {expected} support",
            command.display()
        )))
    }
}

fn command_pair(directory: &Path, prefix: &str) -> (PathBuf, PathBuf) {
    let extension = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    (
        directory.join(format!("{prefix}ffmpeg{extension}")),
        directory.join(format!("{prefix}ffprobe{extension}")),
    )
}

fn bundled_pair(directory: &Path) -> Option<(PathBuf, PathBuf)> {
    let pair = command_pair(directory, "oxy-");
    // A damaged/incomplete bundle must fail instead of mixing with PATH tools.
    (pair.0.is_file() || pair.1.is_file()).then_some(pair)
}

fn resolve_commands() -> (PathBuf, PathBuf) {
    // Explicit diagnostic override applies to the whole pair, even if missing.
    if let Some(directory) = std::env::var_os("OXY_FFMPEG_DIR") {
        let directory = PathBuf::from(directory);
        return bundled_pair(&directory).unwrap_or_else(|| command_pair(&directory, ""));
    }
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(directory) = current_exe.parent()
    {
        if let Some(pair) = bundled_pair(directory) {
            return pair;
        }
        if !cfg!(debug_assertions) {
            return command_pair(directory, "oxy-");
        }
    }
    // System tools are a development convenience, never a release dependency.
    #[cfg(all(target_os = "windows", debug_assertions))]
    {
        let mut directories = Vec::new();
        if let Some(scoop) = std::env::var_os("SCOOP") {
            directories.push(PathBuf::from(scoop).join("apps/ffmpeg/current/bin"));
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            directories.push(PathBuf::from(profile).join("scoop/apps/ffmpeg/current/bin"));
        }
        for directory in directories {
            let pair = command_pair(&directory, "");
            if pair.0.is_file() && pair.1.is_file() {
                return pair;
            }
        }
    }
    command_pair(
        Path::new(""),
        if cfg!(debug_assertions) { "" } else { "oxy-" },
    )
}

fn media_command(executable: &Path) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new(executable);
        command.creation_flags(CREATE_NO_WINDOW | HIGH_PRIORITY_CLASS);
        command
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        Command::new(executable)
    }
}

fn probe_grid(path: &Path) -> Result<TileGrid, MediaError> {
    let output = media_command(&COMMANDS.1)
        .args([
            "-v",
            "error",
            "-show_stream_groups",
            "-show_streams",
            "-of",
            "json",
        ])
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
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned()
    {
        return Ok(grid);
    }
    let grid = probe_grid(path)?;
    GRID_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    let stream_dimensions = root["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|stream| {
            Some((
                stream["index"].as_u64()?,
                (
                    stream["width"].as_u64()?.try_into().ok()?,
                    stream["height"].as_u64()?.try_into().ok()?,
                ),
            ))
        })
        .collect::<HashMap<_, _>>();
    let tiles = subcomponents
        .iter()
        .map(|tile| {
            let stream_index = json_u64(tile, "stream_index")?;
            let (width, height) =
                stream_dimensions
                    .get(&stream_index)
                    .copied()
                    .ok_or_else(|| {
                        native_error(format!(
                            "ffprobe omitted dimensions for tile stream {stream_index}"
                        ))
                    })?;
            Ok(Tile {
                stream_index,
                x: json_u64(tile, "tile_horizontal_offset")?,
                y: json_u64(tile, "tile_vertical_offset")?,
                width,
                height,
            })
        })
        .collect::<Result<Vec<_>, MediaError>>()?;
    let tile_indices = tiles
        .iter()
        .map(|tile| tile.stream_index)
        .collect::<std::collections::HashSet<_>>();
    let previews = root["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|stream| stream["codec_type"].as_str() == Some("video"))
        .filter_map(|stream| {
            let index = stream["index"].as_u64()?;
            let width = stream["width"].as_u64()?.try_into().ok()?;
            let height = stream["height"].as_u64()?.try_into().ok()?;
            (!tile_indices.contains(&index)).then_some(PreviewStream {
                index,
                width,
                height,
            })
        })
        .collect();
    // The display transform belongs to the primary Tile Grid component. An
    // auxiliary preview often repeats it, but that is not guaranteed and some
    // cameras give the preview a different transform. Prefer the component so
    // full-resolution tiles cannot inherit an unrelated preview orientation.
    let rotation = rotation_from_side_data(component)
        .or_else(|| rotation_from_side_data(group))
        .or_else(|| {
            groups
                .iter()
                .flat_map(|group| group["streams"].as_array().into_iter().flatten())
                .chain(root["streams"].as_array().into_iter().flatten())
                .filter(|stream| stream["codec_type"].as_str() == Some("video"))
                .filter(|stream| stream["disposition"]["dependent"].as_u64() == Some(0))
                .find_map(rotation_from_side_data)
        })
        .unwrap_or(0)
        .try_into()
        .map_err(|_| native_error("ffprobe display rotation exceeds i32"))?;
    if tiles.is_empty() {
        return Err(native_error("tile-grid component has no tiles"));
    }
    Ok(TileGrid {
        width,
        height,
        rotation,
        tiles,
        previews,
    })
}

fn rotation_from_side_data(value: &Value) -> Option<i64> {
    value["side_data_list"]
        .as_array()?
        .iter()
        .find_map(|side_data| side_data["rotation"].as_i64())
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
        match grid.rotation {
            // ffprobe reports display-matrix rotation using the opposite sign
            // from FFmpeg's transpose filter direction. A reported -90 degree
            // transform therefore needs a clockwise pixel rotation.
            -90 | 270 => filter.push_str(",transpose=clock"),
            90 | -270 => filter.push_str(",transpose=cclock"),
            rotation => {
                return Err(native_error(format!(
                    "tile-grid requires a quarter turn but display rotation is {rotation}"
                )));
            }
        }
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

    #[test]
    fn bundled_commands_are_resolved_as_one_pair() {
        let directory = tempfile::tempdir().unwrap();
        assert!(super::bundled_pair(directory.path()).is_none());
        let expected = super::command_pair(directory.path(), "oxy-");
        std::fs::write(&expected.0, b"test executable").unwrap();
        // Missing ffprobe stays in the same directory; no system fallback.
        assert_eq!(
            super::bundled_pair(directory.path()),
            Some(expected.clone())
        );
        std::fs::write(&expected.1, b"test executable").unwrap();
        assert_eq!(super::bundled_pair(directory.path()), Some(expected));
    }

    const GRID_JSON: &[u8] = br#"{
      "stream_groups": [{
        "type": "Tile Grid",
        "components": [{
          "width": 7008,
          "height": 4672,
          "side_data_list": [{
            "side_data_type": "Display Matrix",
            "rotation": -90
          }],
          "subcomponents": [
            {"stream_index": 0, "tile_horizontal_offset": 0, "tile_vertical_offset": 0},
            {"stream_index": 1, "tile_horizontal_offset": 3520, "tile_vertical_offset": 0}
          ]
        }]
      }],
      "streams": [{
        "index": 0,
        "codec_type": "video",
        "width": 3520,
        "height": 1600,
        "disposition": {"dependent": 1}
      }, {
        "index": 1,
        "codec_type": "video",
        "width": 3520,
        "height": 1600
      }, {
        "index": 6,
        "codec_type": "video",
        "width": 1664,
        "height": 1088,
        "disposition": {"dependent": 0},
        "side_data_list": [{"rotation": 90}]
      }]
    }"#;

    #[test]
    fn parses_grid_and_builds_dynamic_rotated_filter() {
        let grid = parse_grid(GRID_JSON).unwrap();
        assert_eq!(grid.tiles.len(), 2);
        assert_eq!(grid.previews.len(), 1);
        assert_eq!(grid.rotation, -90);
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
    fn falls_back_to_independent_preview_rotation_when_component_has_none() {
        let mut root: Value = serde_json::from_slice(GRID_JSON).unwrap();
        root["stream_groups"][0]["components"][0]
            .as_object_mut()
            .unwrap()
            .remove("side_data_list");
        let json = serde_json::to_vec(&root).unwrap();

        let grid = parse_grid(&json).unwrap();

        assert_eq!(grid.rotation, 90);
    }

    #[test]
    fn uses_counterclockwise_pixels_for_positive_display_matrix_rotation() {
        let mut grid = parse_grid(GRID_JSON).unwrap();
        grid.rotation = 90;
        let (filter, _, _) = filter_for_grid(
            &grid,
            ImageDimensions {
                width: 4672,
                height: 7008,
            },
        )
        .unwrap();
        assert!(filter.contains("transpose=cclock"));
    }

    #[test]
    fn decodes_repository_hif_fixture_when_ffmpeg_is_available() {
        if capability().is_err() {
            return;
        }
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let image = decode_full_rgba8(
            &fixture,
            ImageDimensions {
                width: 4672,
                height: 7008,
            },
            false,
        )
        .unwrap();
        assert_eq!((image.width(), image.height()), (4672, 7008));
    }

    #[test]
    fn transcodes_repository_hif_directly_to_jpeg_when_ffmpeg_is_available() {
        if capability().is_err() {
            return;
        }
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let output = tempfile::Builder::new().suffix(".jpg").tempfile().unwrap();
        transcode_full_jpeg(
            &fixture,
            output.path(),
            ImageDimensions {
                width: 4672,
                height: 7008,
            },
            95,
        )
        .unwrap();
        let image = image::ImageReader::open(output.path())
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((image.width(), image.height()), (4672, 7008));
    }

    #[test]
    fn decodes_fast_auxiliary_preview_when_ffmpeg_is_available() {
        if capability().is_err() {
            return;
        }
        let Some(path) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let image = decode_scaled_preview(&path, 512).unwrap();
        assert_eq!(image.width().max(image.height()), 512);
    }

    #[test]
    #[ignore = "decodes the full fixture through both FFmpeg and libheif"]
    fn fixture_orientation_matches_libheif() {
        if capability().is_err() {
            return;
        }
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let ffmpeg = decode_full_rgba8(
            &fixture,
            ImageDimensions {
                width: 4672,
                height: 7008,
            },
            false,
        )
        .unwrap()
        .thumbnail_exact(64, 96)
        .into_rgb8();
        let libheif = crate::backends::libheif::decode_full_rgb8(&fixture)
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
