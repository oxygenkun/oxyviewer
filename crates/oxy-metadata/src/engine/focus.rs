//! Camera AF coordinates, normalized before the display-space EXIF transform.
use oxy_domain::FocusInfo;
use oxy_metadata_parser::Tag;

pub(super) fn from_tags(tags: &[Tag], display: Option<(u32, u32)>) -> Option<FocusInfo> {
    let make = value(tags, "EXIF", "Make")?;
    if make.eq_ignore_ascii_case("Canon") {
        canon(tags, display)
    } else if make.eq_ignore_ascii_case("Panasonic") {
        panasonic(tags, display).or_else(|| panasonic_faces(tags, display))
    } else {
        None
    }
}

fn value<'a>(tags: &'a [Tag], group: &str, name: &str) -> Option<&'a str> {
    tags.iter()
        .find(|tag| tag.group == group && tag.name == name)
        .map(|tag| tag.value.as_str())
}

fn numbers(tags: &[Tag], name: &str) -> Vec<i64> {
    value(tags, "MakerNotes", name)
        .unwrap_or_default()
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn positive(tags: &[Tag], name: &str) -> Option<u32> {
    u32::try_from(*numbers(tags, name).first()?)
        .ok()
        .filter(|v| *v > 0)
}

fn canon(tags: &[Tag], display: Option<(u32, u32)>) -> Option<FocusInfo> {
    // https://exiftool.org/TagNames/Canon.html#AFInfo2
    // EOS uses a centered, upward-positive Y axis; PowerShot uses downward Y.
    let width = positive(tags, "AFImageWidth")?;
    let height = positive(tags, "AFImageHeight")?;
    let valid = positive(tags, "ValidAFPoints")? as usize;
    let xs = numbers(tags, "AFAreaXPositions");
    let ys = numbers(tags, "AFAreaYPositions");
    let widths = numbers(tags, "AFAreaWidths");
    let heights = numbers(tags, "AFAreaHeights");
    let indices = numbers(tags, "AFPointsInFocus");
    // A selected point does not prove focus lock. Never draw every candidate
    // when the in-focus mask is empty (including manual-focus shots).
    let orientation = super::focus_orientation(tags, width, height, display);
    let eos = value(tags, "EXIF", "Model").is_some_and(|model| model.contains("EOS"));
    let mut result = None::<FocusInfo>;
    for index in indices {
        let Ok(index) = usize::try_from(index) else {
            continue;
        };
        if index >= valid || index >= xs.len() || index >= ys.len() {
            continue;
        }
        let x = i64::from(width) / 2 + xs[index];
        let y = i64::from(height) / 2 + if eos { -ys[index] } else { ys[index] };
        if !(0..=i64::from(width)).contains(&x) || !(0..=i64::from(height)).contains(&y) {
            continue;
        }
        let frame_edge = |values: &[i64], fallback| {
            values
                .get(index)
                .and_then(|v| u32::try_from(*v).ok())
                .filter(|v| *v > 0)
                .or_else(|| positive(tags, fallback))
        };
        let frame = frame_edge(&widths, "AFAreaWidth").zip(frame_edge(&heights, "AFAreaHeight"));
        let point = crate::orient_focus_info(width, height, x as u32, y as u32, frame, orientation);
        if let Some(result) = &mut result {
            for region in point.regions {
                if !result.regions.contains(&region) {
                    result.regions.push(region);
                }
            }
        } else {
            result = Some(point);
        }
    }
    result
}

fn panasonic(tags: &[Tag], display: Option<(u32, u32)>) -> Option<FocusInfo> {
    // Tag 0x004d is two unsigned rationals in [0,1], not an integer pixel pair.
    let position = value(tags, "MakerNotes", "AFPointPosition")?;
    let coordinates = position
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let [x, y] = coordinates.as_slice() else {
        return None;
    };
    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y) {
        return None;
    }
    // Prefer camera image dimensions, independent of the current preview size.
    let exif_edge = |name| {
        value(tags, "EXIF", name)
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0)
    };
    let width = positive(tags, "PanasonicImageWidth").or_else(|| exif_edge("ExifImageWidth"))?;
    let height = positive(tags, "PanasonicImageHeight").or_else(|| exif_edge("ExifImageHeight"))?;
    Some(crate::orient_focus_info(
        width,
        height,
        (x * f64::from(width)).round() as u32,
        (y * f64::from(height)).round() as u32,
        None,
        super::focus_orientation(tags, width, height, display),
    ))
}

fn panasonic_faces(tags: &[Tag], display: Option<(u32, u32)>) -> Option<FocusInfo> {
    let count = positive(tags, "NumFacePositions")?.min(5);
    let exif_edge = |name| {
        value(tags, "EXIF", name)
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0)
    };
    let width = positive(tags, "PanasonicImageWidth").or_else(|| exif_edge("ExifImageWidth"))?;
    let height = positive(tags, "PanasonicImageHeight").or_else(|| exif_edge("ExifImageHeight"))?;
    // FaceDetInfo uses the unrotated image scaled to 320 pixels wide.
    // Do not use the EXIF thumbnail's outer dimensions: a matching camera
    // JPEG may pad its 3:2 thumbnail to 160x120 while RW2 uses 160x106.
    // Both store identical face coordinates, excluding that padding.
    let canvas_width = 320.0;
    let canvas_height = canvas_width * f64::from(height) / f64::from(width);
    let orientation = super::focus_orientation(tags, width, height, display);
    let mut result = None::<FocusInfo>;
    for index in 1..=count {
        let coords = numbers(tags, &format!("Face{index}Position"));
        let [x, y, w, h] = coords.as_slice() else {
            continue;
        };
        if *x < 0
            || *y < 0
            || *w <= 0
            || *h <= 0
            || *x as f64 > canvas_width
            || *y as f64 > canvas_height
            || *w as f64 > canvas_width
            || *h as f64 > canvas_height
        {
            continue;
        }
        let scale_x = |v: i64| (v as f64 * f64::from(width) / canvas_width).round() as u32;
        let scale_y = |v: i64| (v as f64 * f64::from(height) / canvas_height).round() as u32;
        let point = crate::orient_focus_info(
            width,
            height,
            scale_x(*x),
            scale_y(*y),
            Some((scale_x(*w).max(1), scale_y(*h).max(1))),
            orientation,
        );
        if let Some(result) = &mut result {
            result.regions.extend(point.regions);
        } else {
            result = Some(point);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_panasonic_jpeg_raw_pair_has_matching_face_regions() {
        let Some(directory) = std::env::var_os("OXY_PANASONIC_PAIR_DIR") else {
            return;
        };
        let directory = std::path::Path::new(&directory);
        let read = |name| {
            let tags = oxy_metadata_parser::tags(directory.join(name)).unwrap();
            assert_eq!(
                value(&tags, "MakerNotes", "Face1Position"),
                Some("230 101 16 16")
            );
            from_tags(&tags, None).unwrap()
        };
        let jpeg = read("PANA5004.JPG");
        let raw = read("PANA5004.RW2");
        assert_eq!(jpeg, raw);
        assert_eq!(
            (jpeg.coordinate_width, jpeg.coordinate_height),
            (4000, 6000)
        );
        assert_eq!(jpeg.regions.len(), 1);
        let region = &jpeg.regions[0];
        assert_eq!((region.center_x, region.center_y), (1894, 1687));
        assert_eq!((region.width, region.height), (Some(300), Some(300)));
        eprintln!("PANA5004 JPG/RW2: {jpeg:?}");
    }

    #[test]
    fn real_camera_focus_fixture() {
        for variable in ["OXY_CR3_FIXTURE", "OXY_RW2_FIXTURE"] {
            let Some(path) = std::env::var_os(variable) else {
                continue;
            };
            let tags = oxy_metadata_parser::tags(std::path::Path::new(&path)).unwrap();
            for tag in tags.iter().filter(|tag| {
                matches!(
                    tag.name.as_str(),
                    "Orientation"
                        | "AFImageWidth"
                        | "AFImageHeight"
                        | "AFAreaXPositions"
                        | "AFAreaYPositions"
                        | "AFPointsInFocus"
                        | "AFPointPosition"
                        | "PanasonicImageWidth"
                        | "PanasonicImageHeight"
                        | "NumFacePositions"
                        | "Face1Position"
                        | "ThumbnailImageWidth"
                        | "ThumbnailImageHeight"
                )
            }) {
                eprintln!(
                    "{} {} {}",
                    tag.group,
                    tag.name,
                    tag.value.chars().take(80).collect::<String>()
                );
            }
            let focus = super::super::focus_from_tags(&tags, None);
            eprintln!("{variable}: {focus:?}");
            if variable == "OXY_CR3_FIXTURE" {
                assert!(
                    !focus
                        .expect("Canon fixture has an AF position")
                        .regions
                        .is_empty()
                );
            } else {
                let position =
                    value(&tags, "MakerNotes", "AFPointPosition").expect("Panasonic AF tag parsed");
                if position == "n/a" && positive(&tags, "NumFacePositions").is_some() {
                    assert!(focus.as_ref().is_some_and(|info| !info.regions.is_empty()));
                } else if position == "n/a" {
                    assert!(focus.is_none());
                } else {
                    assert!(focus.is_some());
                }
            }
        }
    }

    #[test]
    fn canon_rotates_only_in_focus_regions_and_preserves_signed_coordinates() {
        let tags = vec![
            Tag::new("EXIF", "Make", "Canon"),
            Tag::new("EXIF", "Model", "Canon EOS R6 Mark II"),
            Tag::new("MakerNotes", "AFImageWidth", "6000"),
            Tag::new("MakerNotes", "AFImageHeight", "4000"),
            Tag::new("MakerNotes", "ValidAFPoints", "3"),
            Tag::new("MakerNotes", "AFAreaXPositions", "-1200 100 300"),
            Tag::new("MakerNotes", "AFAreaYPositions", "400 -200 100"),
            Tag::new("MakerNotes", "AFAreaWidths", "100 200 300"),
            Tag::new("MakerNotes", "AFAreaHeights", "50 60 70"),
            Tag::new("MakerNotes", "AFPointsInFocus", "0,2,99"),
        ];
        let focus = from_tags(&tags, Some((4000, 6000))).unwrap();
        assert_eq!(
            (focus.coordinate_width, focus.coordinate_height),
            (4000, 6000)
        );
        assert_eq!(focus.regions.len(), 2);
        assert_eq!(
            (focus.regions[0].center_x, focus.regions[0].center_y),
            (2400, 1800)
        );
        assert_eq!(
            (focus.regions[0].width, focus.regions[0].height),
            (Some(50), Some(100))
        );
        let mut no_focus = tags;
        no_focus.last_mut().unwrap().value = "(none)".into();
        assert_eq!(from_tags(&no_focus, None), None);
    }

    #[test]
    fn panasonic_faces_ignore_thumbnail_padding_and_yield_to_valid_af() {
        let mut tags = vec![
            Tag::new("EXIF", "Make", "Panasonic"),
            Tag::with_typed(
                "EXIF",
                "Orientation",
                "Rotate 270 CW",
                oxy_metadata_parser::core::TagValue::U16(8),
            ),
            Tag::new("EXIF", "ThumbnailImageWidth", "160"),
            Tag::new("EXIF", "ThumbnailImageHeight", "106"),
            Tag::new("MakerNotes", "PanasonicImageWidth", "6000"),
            Tag::new("MakerNotes", "PanasonicImageHeight", "4000"),
            Tag::new("MakerNotes", "NumFacePositions", "3"),
            Tag::new("MakerNotes", "Face1Position", "213 76 13 13"),
            Tag::new("MakerNotes", "Face2Position", "100 50 20 10"),
            Tag::new("MakerNotes", "Face3Position", "65535 65535 0 0"),
            Tag::new("MakerNotes", "AFPointPosition", "n/a"),
        ];
        let info = from_tags(&tags, None).unwrap();
        assert_eq!(
            (info.coordinate_width, info.coordinate_height),
            (4000, 6000)
        );
        assert_eq!(info.regions.len(), 2);
        let region = &info.regions[0];
        assert_eq!((region.center_x, region.center_y), (1425, 2006));
        assert_eq!((region.width, region.height), (Some(244), Some(244)));

        // Companion JPEG has padding absent from the RAW thumbnail. Neither
        // that padding nor a missing thumbnail changes camera coordinates.
        tags[3].value = "120".into();
        assert_eq!(from_tags(&tags, None), Some(info.clone()));
        tags[2].name = "UnusedWidth".into();
        tags[3].name = "UnusedHeight".into();
        assert_eq!(from_tags(&tags, None), Some(info));

        tags.last_mut().unwrap().value = "0.5 0.5".into();
        let info = from_tags(&tags, None).unwrap();
        assert_eq!(info.regions.len(), 1);
        assert_eq!(
            (info.regions[0].center_x, info.regions[0].center_y),
            (2000, 3000)
        );
        tags.last_mut().unwrap().value = "n/a".into();
        tags[6].value = "0".into();
        assert!(from_tags(&tags, None).is_none());
    }

    #[test]
    fn panasonic_preserves_fractional_position_and_rejects_invalid_sentinels() {
        let mut tags = vec![
            Tag::new("EXIF", "Make", "Panasonic"),
            Tag::new("MakerNotes", "PanasonicImageWidth", "6000"),
            Tag::new("MakerNotes", "PanasonicImageHeight", "4000"),
            Tag::new("MakerNotes", "AFPointPosition", "0.3125 0.625"),
        ];
        let focus = from_tags(&tags, Some((4000, 6000))).unwrap();
        assert_eq!(
            (focus.regions[0].center_x, focus.regions[0].center_y),
            (1500, 1875)
        );
        assert_eq!(focus.regions[0].width, None);
        for invalid in ["n/a", "none", "16777216 16777216", "NaN 0.5", "-0.1 0.5"] {
            tags.last_mut().unwrap().value = invalid.into();
            assert_eq!(from_tags(&tags, None), None);
        }
    }
}
