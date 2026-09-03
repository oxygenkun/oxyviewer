//! Tag definitions and lookup tables (V1-V5, V8-V10).
//!
//! Data-driven tag tables for EXIF IFD0, ExifIFD, GPS IFD, and TIFF baseline.
//! Each tag has an ID, name, group, and optional print converter for display.

use crate::tiff::value::TagValue;

/// V9: Tag group (Group0=format, Group1=IFD, Group2=category).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagGroup {
    /// IFD0 - primary image metadata.
    Ifd0,
    /// IFD1 - thumbnail metadata.
    Ifd1,
    /// ExifIFD - camera/exposure settings.
    ExifIfd,
    /// GPS IFD - geolocation data.
    GpsIfd,
    /// Interoperability IFD.
    InteropIfd,
}

/// V1: Tag definition - compile-time descriptor for a known tag.
#[derive(Debug, Clone, Copy)]
pub struct TagDef {
    /// Tag ID (e.g., 0x010F for Make).
    pub id: u16,
    /// Human-readable tag name.
    pub name: &'static str,
    /// Which IFD group this tag belongs to.
    pub group: TagGroup,
    /// Optional print converter: maps a `TagValue` to a display string.
    /// If `None`, the default `TagValue::display()` is used.
    pub print_conv: Option<fn(&TagValue) -> Option<String>>,
}

/// V8: Apply PrintConv - convert a tag value to its display string.
pub fn print_value(tag_def: &TagDef, value: &TagValue) -> String {
    if let Some(conv) = tag_def.print_conv {
        if let Some(s) = conv(value) {
            return s;
        }
    }
    value.display()
}

/// V10: Look up a tag definition by ID and group.
pub fn find_tag(id: u16, group: TagGroup) -> Option<&'static TagDef> {
    let table = match group {
        TagGroup::Ifd0 | TagGroup::Ifd1 => &IFD0_TAGS[..],
        TagGroup::ExifIfd => &EXIF_IFD_TAGS[..],
        TagGroup::GpsIfd => &GPS_TAGS[..],
        TagGroup::InteropIfd => &INTEROP_TAGS[..],
    };
    table.iter().find(|t| t.id == id)
}

/// V10: Look up a tag definition by name (case-insensitive search across all groups).
pub fn find_tag_by_name(name: &str) -> Option<&'static TagDef> {
    let lower = name.to_ascii_lowercase();
    ALL_TAGS
        .iter()
        .find(|t| t.name.to_ascii_lowercase() == lower)
}

// -- Print converters (V8) ----------------------------------------------

fn print_orientation(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "Horizontal (normal)",
            2 => "Mirror horizontal",
            3 => "Rotate 180",
            4 => "Mirror vertical",
            5 => "Mirror horizontal and rotate 270 CW",
            6 => "Rotate 90 CW",
            7 => "Mirror horizontal and rotate 90 CW",
            8 => "Rotate 270 CW",
            _ => return None,
        }
        .into(),
    )
}

fn print_resolution_unit(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "None",
            2 => "inches",
            3 => "cm",
            _ => return None,
        }
        .into(),
    )
}

fn print_predictor(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "None",
            2 => "Horizontal differencing",
            3 => "Floating point",
            34892 => "Horizontal difference X2",
            34893 => "Horizontal difference X4",
            34894 => "Floating point X2",
            34895 => "Floating point X4",
            _ => return None,
        }
        .into(),
    )
}

fn print_extra_samples(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Unspecified",
            1 => "Associated Alpha",
            2 => "Unassociated Alpha",
            _ => return None,
        }
        .into(),
    )
}

fn sample_format_name(n: u32) -> &'static str {
    match n {
        1 => "Unsigned",
        2 => "Signed",
        3 => "Float",
        4 => "Undefined",
        5 => "Complex int",
        6 => "Complex float",
        _ => "Unknown",
    }
}

fn print_sample_format(v: &TagValue) -> Option<String> {
    if let TagValue::U16Array(arr) = v {
        let parts: Vec<&str> = arr.iter().map(|&n| sample_format_name(n as u32)).collect();
        return Some(parts.join("; "));
    }
    Some(sample_format_name(v.to_u32()?).into())
}

fn print_lens_info(v: &TagValue) -> Option<String> {
    // LensInfo is 4 rationals: min_fl, max_fl, min_fnum, max_fnum
    // Format: "min_fl[-max_fl]mm f/min_fnum[-max_fnum]"
    if let TagValue::RationalArray(arr) = v {
        if arr.len() >= 4 {
            let fmt_val = |n: u32, d: u32| -> Option<String> {
                // 0/0 is how "unknown" is written in a LensInfo rational, and
                // renders as "?" below. Only the denominator is tested: the
                // second half of the original condition (n == 0 && d == 0) was
                // already covered by the first and never changed the outcome.
                // A zero numerator over a non-zero denominator is left alone
                // rather than folded in here, because that would change what
                // is printed and no file in the corpora exercises it.
                if d == 0 {
                    return None;
                }
                let f = n as f64 / d as f64;
                Some(format_sig_digits(f, 10))
            };
            let min_fl = fmt_val(arr[0].0, arr[0].1);
            let max_fl = fmt_val(arr[1].0, arr[1].1);
            let min_fn = fmt_val(arr[2].0, arr[2].1);
            let max_fn = fmt_val(arr[3].0, arr[3].1);

            let mut result = match &min_fl {
                Some(s) => s.clone(),
                None => "?".into(),
            };
            // Add max focal if different and non-zero
            if let (Some(mn), Some(mx)) = (&min_fl, &max_fl) {
                if mn != mx && arr[1].0 != 0 {
                    result.push('-');
                    result.push_str(mx);
                }
            }
            result.push_str("mm f/");
            match &min_fn {
                Some(s) => result.push_str(s),
                None => result.push('?'),
            }
            if let (Some(mn), Some(mx)) = (&min_fn, &max_fn) {
                if mn != mx {
                    result.push('-');
                    result.push_str(mx);
                }
            }
            return Some(result);
        }
    }
    None
}

fn print_new_subfile_type(v: &TagValue) -> Option<String> {
    let bits = v.to_u32()?;
    let mut parts = Vec::new();
    if bits & 1 != 0 {
        parts.push("Reduced-resolution image");
    }
    if bits & 2 != 0 {
        parts.push("Single page of multi-page");
    }
    if bits & 4 != 0 {
        parts.push("Transparency mask");
    }
    if parts.is_empty() {
        Some("Full-resolution image".into())
    } else {
        Some(parts.join(", "))
    }
}

fn print_compression(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "Uncompressed",
            2 => "CCITT 1D",
            3 => "T4/Group 3 Fax",
            4 => "T6/Group 4 Fax",
            5 => "LZW",
            6 => "JPEG (old-style)",
            7 => "JPEG",
            8 => "Adobe Deflate",
            32773 => "PackBits",
            _ => return None,
        }
        .into(),
    )
}

fn print_fill_order(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "Normal",
            2 => "Reversed",
            _ => return None,
        }
        .into(),
    )
}

fn print_photometric(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "WhiteIsZero",
            1 => "BlackIsZero",
            2 => "RGB",
            3 => "RGB Palette",
            4 => "Transparency Mask",
            5 => "CMYK",
            6 => "YCbCr",
            8 => "CIELab",
            9 => "ICCLab",
            10 => "ITULab",
            32803 => "Color Filter Array",
            32844 => "Pixar LogL",
            32845 => "Pixar LogLuv",
            34892 => "Linear Raw",
            51177 => "Depth",
            _ => return None,
        }
        .into(),
    )
}

fn print_planar_config(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "Chunky",
            2 => "Planar",
            _ => return None,
        }
        .into(),
    )
}

fn print_ycbcr_positioning(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "Centered",
            2 => "Co-sited",
            _ => return None,
        }
        .into(),
    )
}

fn print_exposure_program(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Not Defined",
            1 => "Manual",
            2 => "Program AE",
            3 => "Aperture-priority AE",
            4 => "Shutter speed priority AE",
            5 => "Creative (Slow speed)",
            6 => "Action (High speed)",
            7 => "Portrait",
            8 => "Landscape",
            _ => return None,
        }
        .into(),
    )
}

fn print_metering_mode(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Unknown",
            1 => "Average",
            2 => "Center-weighted average",
            3 => "Spot",
            4 => "Multi-spot",
            5 => "Multi-segment",
            6 => "Partial",
            255 => "Other",
            _ => return None,
        }
        .into(),
    )
}

fn print_light_source(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Unknown",
            1 => "Daylight",
            2 => "Fluorescent",
            3 => "Tungsten (Incandescent)",
            4 => "Flash",
            9 => "Fine Weather",
            10 => "Cloudy",
            11 => "Shade",
            12 => "Daylight Fluorescent",
            13 => "Day White Fluorescent",
            14 => "Cool White Fluorescent",
            15 => "White Fluorescent",
            16 => "Warm White Fluorescent",
            17 => "Standard Light A",
            18 => "Standard Light B",
            19 => "Standard Light C",
            20 => "D55",
            21 => "D65",
            22 => "D75",
            23 => "D50",
            24 => "ISO Studio Tungsten",
            255 => "Other",
            _ => return None,
        }
        .into(),
    )
}

#[allow(dead_code)] // EXIF Flash printer; not yet wired into the tag-print dispatch table.
fn print_flash(v: &TagValue) -> Option<String> {
    let bits = v.to_u32()?;
    let fired = bits & 1 != 0;
    let ret = (bits >> 1) & 3;
    let mode = (bits >> 3) & 3;
    let function = (bits >> 5) & 1 != 0;
    let red_eye = (bits >> 6) & 1 != 0;

    let mut parts = Vec::new();
    if fired {
        parts.push("Fired");
    } else {
        parts.push("Did not fire");
    }
    match ret {
        2 => parts.push("strobe return not detected"),
        3 => parts.push("strobe return detected"),
        _ => {}
    }
    match mode {
        1 => parts.push("compulsory firing"),
        2 => parts.push("compulsory suppression"),
        3 => parts.push("auto"),
        _ => {}
    }
    if function {
        parts.push("no flash function");
    }
    if red_eye {
        parts.push("red-eye reduction");
    }

    Some(parts.join(", "))
}

fn print_color_space(v: &TagValue) -> Option<String> {
    let n = v.to_u32()?;
    Some(match n {
        1 => "sRGB".into(),
        2 => "Adobe RGB".into(),
        0xFFFF => "Uncalibrated".into(),
        _ => format!("Unknown ({})", n),
    })
}

fn print_sensing_method(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "Not defined",
            2 => "One-chip color area",
            3 => "Two-chip color area",
            4 => "Three-chip color area",
            5 => "Color sequential area",
            7 => "Trilinear",
            8 => "Color sequential linear",
            _ => return None,
        }
        .into(),
    )
}

fn print_file_source(v: &TagValue) -> Option<String> {
    // Sigma incorrectly writes 4 bytes: [3, 0, 0, 0]
    if let TagValue::Bytes(b) = v {
        if b.as_slice() == [3, 0, 0, 0] {
            return Some("Sigma Digital Camera".into());
        }
    }
    let val = match v {
        TagValue::U8(n) => *n as u32,
        TagValue::Bytes(b) if !b.is_empty() => b[0] as u32,
        _ => return None,
    };
    Some(
        match val {
            1 => "Film Scanner",
            2 => "Reflection Print Scanner",
            3 => "Digital Camera",
            n => return Some(format!("Unknown ({})", n)),
        }
        .into(),
    )
}

fn print_scene_type(v: &TagValue) -> Option<String> {
    let val = match v {
        TagValue::U8(n) => *n as u32,
        TagValue::Bytes(b) if !b.is_empty() => b[0] as u32,
        _ => return None,
    };
    Some(match val {
        1 => "Directly photographed".into(),
        n => format!("Unknown ({})", n),
    })
}

fn print_custom_rendered(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Normal",
            1 => "Custom",
            2 => "HDR (no original saved)",
            3 => "HDR (original saved)",
            4 => "Original (for HDR)",
            6 => "Panorama",
            7 => "Portrait HDR",
            8 => "Portrait",
            _ => return None,
        }
        .into(),
    )
}

fn print_exposure_mode(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Auto",
            1 => "Manual",
            2 => "Auto bracket",
            _ => return None,
        }
        .into(),
    )
}

fn print_white_balance(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Auto",
            1 => "Manual",
            _ => return None,
        }
        .into(),
    )
}

fn print_scene_capture_type(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Standard",
            1 => "Landscape",
            2 => "Portrait",
            3 => "Night",
            _ => return None,
        }
        .into(),
    )
}

fn print_composite_image(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Unknown",
            1 => "Not a Composite Image",
            2 => "General Composite Image",
            3 => "Composite Image Captured While Shooting",
            _ => return None,
        }
        .into(),
    )
}

fn print_gain_control(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "None",
            1 => "Low gain up",
            2 => "High gain up",
            3 => "Low gain down",
            4 => "High gain down",
            _ => return None,
        }
        .into(),
    )
}

fn print_contrast(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Normal",
            1 => "Low",
            2 => "High",
            _ => return None,
        }
        .into(),
    )
}

fn print_saturation(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Normal",
            1 => "Low",
            2 => "High",
            _ => return None,
        }
        .into(),
    )
}

fn print_sharpness(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Normal",
            1 => "Soft",
            2 => "Hard",
            _ => return None,
        }
        .into(),
    )
}

fn print_subject_distance_range(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Unknown",
            1 => "Macro",
            2 => "Close",
            3 => "Distant",
            _ => return None,
        }
        .into(),
    )
}

fn print_sensitivity_type(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "Unknown",
            1 => "Standard Output Sensitivity",
            2 => "Recommended Exposure Index",
            3 => "ISO Speed",
            4 => "Standard Output Sensitivity and Recommended Exposure Index",
            5 => "Standard Output Sensitivity and ISO Speed",
            6 => "Recommended Exposure Index and ISO Speed",
            7 => "Standard Output Sensitivity, Recommended Exposure Index and ISO Speed",
            _ => return None,
        }
        .into(),
    )
}

#[allow(dead_code)] // Companion to `print_flash`; pending wire-up in print dispatch.
fn print_focal_plane_res_unit(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "None",
            2 => "inches",
            3 => "centimeters",
            4 => "millimeters",
            5 => "micrometers",
            _ => return None,
        }
        .into(),
    )
}

fn print_gps_altitude_ref(v: &TagValue) -> Option<String> {
    let val = match v {
        TagValue::U8(n) => *n as u32,
        TagValue::Bytes(b) if !b.is_empty() => b[0] as u32,
        _ => return None,
    };
    Some(
        match val {
            0 => "Above Sea Level",
            1 => "Below Sea Level",
            _ => return None,
        }
        .into(),
    )
}

fn print_gps_status(v: &TagValue) -> Option<String> {
    match v.as_ascii() {
        Some("A") => Some("Measurement Active".into()),
        Some("V") => Some("Measurement Void".into()),
        _ => None,
    }
}

fn print_cfa_pattern(v: &TagValue) -> Option<String> {
    let bytes = match v {
        TagValue::Bytes(b) => b.as_slice(),
        _ => return None,
    };
    if bytes.len() < 4 {
        return None;
    }
    // First 4 bytes: 2 shorts (big-endian) for columns and rows
    // Try big-endian first
    let cols = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    let rows = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
    if cols == 0 || rows == 0 || 4 + cols * rows != bytes.len() {
        return None;
    }
    let colors = ["Red", "Green", "Blue", "Cyan", "Magenta", "Yellow", "White"];
    let mut result = String::new();
    for r in 0..rows {
        result.push('[');
        for c in 0..cols {
            if c > 0 {
                result.push(',');
            }
            let idx = bytes[4 + r * cols + c] as usize;
            result.push_str(colors.get(idx).unwrap_or(&"Unknown"));
        }
        result.push(']');
    }
    Some(result)
}

fn print_gps_measure_mode(v: &TagValue) -> Option<String> {
    match v.as_ascii() {
        Some("2") => Some("2-Dimensional Measurement".into()),
        Some("3") => Some("3-Dimensional Measurement".into()),
        _ => None,
    }
}

fn print_gps_speed_ref(v: &TagValue) -> Option<String> {
    match v.as_ascii() {
        Some("K") => Some("km/h".into()),
        Some("M") => Some("mph".into()),
        Some("N") => Some("knots".into()),
        Some(s) => Some(format!("Unknown ({})", s)),
        None => None,
    }
}

fn print_gps_direction_ref(v: &TagValue) -> Option<String> {
    match v.as_ascii() {
        Some("T") => Some("True North".into()),
        Some("M") => Some("Magnetic North".into()),
        Some(s) => Some(format!("Unknown ({})", s)),
        None => None,
    }
}

fn print_gps_distance_ref(v: &TagValue) -> Option<String> {
    match v.as_ascii() {
        Some("K") => Some("Kilometers".into()),
        Some("M") => Some("Miles".into()),
        Some("N") => Some("Nautical Miles".into()),
        Some(s) => Some(format!("Unknown ({})", s)),
        None => None,
    }
}

fn print_gps_differential(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            0 => "No Correction",
            1 => "Differential Corrected",
            _ => return None,
        }
        .into(),
    )
}

// -- UNDEFINED byte -> ASCII print converters ----------------------------

/// ExifVersion, FlashpixVersion: 4 ASCII bytes like "0221" stored as UNDEFINED.
fn print_version_bytes(v: &TagValue) -> Option<String> {
    match v {
        TagValue::Bytes(b) if b.len() == 4 => {
            // If all bytes are ASCII digits, display as-is (e.g. "0221")
            // Otherwise replace non-printable bytes with '.' (matches ExifTool)
            let s: String = b
                .iter()
                .map(|&c| {
                    if c.is_ascii_graphic() || c == b' ' {
                        c as char
                    } else {
                        '.'
                    }
                })
                .collect();
            Some(s)
        }
        TagValue::Ascii(s) => Some(s.clone()),
        _ => None,
    }
}

/// ComponentsConfiguration: bytes [1,2,3,0] -> "Y, Cb, Cr, -"
fn print_components_config(v: &TagValue) -> Option<String> {
    let bytes = match v {
        TagValue::Bytes(b) => b.as_slice(),
        _ => return None,
    };
    let parts: Vec<&str> = bytes
        .iter()
        .map(|b| match b {
            0 => "-",
            1 => "Y",
            2 => "Cb",
            3 => "Cr",
            4 => "R",
            5 => "G",
            6 => "B",
            _ => "?",
        })
        .collect();
    Some(parts.join(", "))
}

/// InteropVersion: 4 ASCII bytes like "0100" stored as UNDEFINED.
fn print_interop_version(v: &TagValue) -> Option<String> {
    print_version_bytes(v)
}

/// GPSVersionID: bytes [2,2,0,0] -> "2.2.0.0"
fn print_gps_version_id(v: &TagValue) -> Option<String> {
    match v {
        TagValue::Bytes(b) if b.len() == 4 => Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])),
        _ => None,
    }
}

/// UserComment: 8-byte charset prefix + text. Empty = ""
fn print_user_comment(v: &TagValue) -> Option<String> {
    let bytes = match v {
        TagValue::Bytes(b) => b.as_slice(),
        _ => return None,
    };
    if bytes.len() < 8 {
        return Some(String::new());
    }
    let prefix = &bytes[..8];
    let payload = &bytes[8..];
    // Check if payload is all zeros/whitespace (empty comment)
    if payload
        .iter()
        .all(|&b| b == 0 || b == b' ' || b == b'\t' || b == b'\n' || b == b'\r')
    {
        return Some(String::new());
    }
    if prefix == b"ASCII\0\0\0" {
        // Truncate at first null byte
        let end = payload
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(payload.len());
        Some(
            String::from_utf8_lossy(&payload[..end])
                .trim_end()
                .to_string(),
        )
    } else if prefix == b"UNICODE\0" {
        // UTF-16 BE
        let chars: Vec<u16> = payload
            .chunks(2)
            .filter_map(|c| {
                if c.len() == 2 {
                    Some(u16::from_be_bytes([c[0], c[1]]))
                } else {
                    None
                }
            })
            .collect();
        let end = chars.iter().rposition(|&c| c != 0).map_or(0, |p| p + 1);
        Some(String::from_utf16_lossy(&chars[..end]))
    } else if prefix == b"\0\0\0\0\0\0\0\0" {
        // Undefined encoding, try ASCII - truncate at first null
        let end = payload
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(payload.len());
        let s = String::from_utf8_lossy(&payload[..end])
            .trim_end()
            .to_string();
        Some(s)
    } else {
        // Unknown prefix - treat entire data (including prefix) as raw text
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let s = String::from_utf8_lossy(&bytes[..end])
            .trim_end()
            .to_string();
        Some(s)
    }
}

// -- Rational display converters ----------------------------------------

/// FocalLength: rational -> "21.3 mm"
fn print_focal_length(v: &TagValue) -> Option<String> {
    let f = v.to_f64()?;
    Some(format!("{} mm", format_decimal(f)))
}

/// FocalLengthIn35mmFormat: integer -> "102 mm"
fn print_focal_length_35mm(v: &TagValue) -> Option<String> {
    let n = v.to_u32()?;
    Some(format!("{n} mm"))
}

/// ShutterSpeedValue: APEX -> exposure time fraction
fn print_shutter_speed_value(v: &TagValue) -> Option<String> {
    let apex = v.to_f64()?;
    if !apex.is_finite() || apex.abs() >= 100.0 {
        return Some("0".into());
    }
    let time = 2.0_f64.powf(-apex);
    format_exposure_time(time)
}

/// ApertureValue / MaxApertureValue: APEX -> f-number
fn print_aperture_value(v: &TagValue) -> Option<String> {
    let apex = v.to_f64()?;
    let fnum = 2.0_f64.powf(apex / 2.0);
    Some(format_decimal(fnum))
}

/// ExposureCompensation: signed rational -> fraction like "0", "+1", "-2/3"
fn print_exposure_compensation(v: &TagValue) -> Option<String> {
    let f = v.to_f64()?;
    let val = f * 1.00001; // avoid round-off errors (matches ExifTool)
    if val == 0.0 || val.abs() < 0.0001 {
        return Some("0".into());
    }
    let iv = val as i64;
    if iv != 0 && (iv as f64 / val).abs() > 0.999 {
        return Some(format!("{:+}", iv));
    }
    let v2 = (val * 2.0) as i64;
    if v2 != 0 && (v2 as f64 / (val * 2.0)).abs() > 0.999 {
        return Some(format!("{:+}/2", v2));
    }
    let v3 = (val * 3.0) as i64;
    if v3 != 0 && (v3 as f64 / (val * 3.0)).abs() > 0.999 {
        return Some(format!("{:+}/3", v3));
    }
    // Fallback: 3 significant digits with sign (like ExifTool's %+.3g)
    let s = format_sig_digits(val, 3);
    if val > 0.0 && !s.starts_with('+') {
        Some(format!("+{}", s))
    } else {
        Some(s)
    }
}

/// ExposureTime: rational -> "1/125" or "0.8"
fn print_exposure_time(v: &TagValue) -> Option<String> {
    match v {
        TagValue::Rational(n, d) => {
            if *d == 0 {
                return None;
            }
            let f = *n as f64 / *d as f64;
            format_exposure_time(f)
        }
        _ => {
            let f = v.to_f64()?;
            format_exposure_time(f)
        }
    }
}

/// Format exposure time matching ExifTool's PrintExposureTime:
/// - Sub-0.25s: "1/N" format
/// - 0.25s and above: decimal with 1dp
pub(crate) fn format_exposure_time(f: f64) -> Option<String> {
    if f >= 1.0 {
        Some(format_decimal(f))
    } else if f > 0.0 && f < 0.25001 {
        let recip = (0.5 + 1.0 / f) as u64;
        Some(format!("1/{recip}"))
    } else if f > 0.0 {
        // >= 0.25s: use 1 decimal place, strip trailing .0
        let s = format!("{:.1}", f);
        Some(s.strip_suffix(".0").unwrap_or(&s).to_string())
    } else {
        Some("0".into())
    }
}

/// FNumber: rational -> decimal (2 dp for <1.0, 1 dp for >=1.0)
fn print_fnumber(v: &TagValue) -> Option<String> {
    let f = v.to_f64()?;
    if f < 1.0 {
        Some(format!("{:.2}", f))
    } else {
        Some(format_decimal(f))
    }
}

/// CompressedBitsPerPixel: rational -> decimal with full precision
fn print_compressed_bpp(v: &TagValue) -> Option<String> {
    if let TagValue::Rational(0, 0) = v {
        return Some("undef".into());
    }
    let f = v.to_f64()?;
    Some(format_decimal_full(f))
}

/// Single or multi-value rational -> decimal (10 sig digits)
fn print_resolution(v: &TagValue) -> Option<String> {
    match v {
        TagValue::Rational(n, 0) => Some(if *n == 0 { "undef" } else { "inf" }.into()),
        TagValue::RationalArray(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .map(|&(n, d)| {
                    if d == 0 {
                        if n == 0 { "undef".into() } else { "inf".into() }
                    } else {
                        format_decimal_full(n as f64 / d as f64)
                    }
                })
                .collect();
            Some(parts.join(" "))
        }
        _ => {
            let f = v.to_f64()?;
            Some(format_decimal_full(f))
        }
    }
}

/// SubjectDistance: rational -> "m" suffix
fn print_subject_distance(v: &TagValue) -> Option<String> {
    if let TagValue::Rational(0, 0) = v {
        return Some("undef".into());
    }
    let f = v.to_f64()?;
    Some(format!("{} m", format_decimal_full(f)))
}

/// DigitalZoomRatio: rational -> decimal with full precision
fn print_digital_zoom(v: &TagValue) -> Option<String> {
    if let TagValue::Rational(0, 0) = v {
        return Some("undef".into());
    }
    let f = v.to_f64()?;
    Some(format_decimal_full(f))
}

/// GPSTimeStamp: 3 rationals -> "HH:MM:SS.ss"
fn print_gps_timestamp(v: &TagValue) -> Option<String> {
    match v {
        TagValue::RationalArray(arr) if arr.len() == 3 => {
            let h = if arr[0].1 == 0 {
                0.0
            } else {
                arr[0].0 as f64 / arr[0].1 as f64
            };
            let m = if arr[1].1 == 0 {
                0.0
            } else {
                arr[1].0 as f64 / arr[1].1 as f64
            };
            let s = if arr[2].1 == 0 {
                0.0
            } else {
                arr[2].0 as f64 / arr[2].1 as f64
            };
            // Format seconds with fractional part if needed
            let h = h as u32;
            let m = m as u32;
            if (s - s.floor()).abs() < 1e-9 {
                Some(format!("{h:02}:{m:02}:{:02}", s as u32))
            } else {
                let s_str = format_decimal_full(s);
                // Pad seconds to at least 2 digits
                if s < 10.0 {
                    Some(format!("{h:02}:{m:02}:0{s_str}"))
                } else {
                    Some(format!("{h:02}:{m:02}:{s_str}"))
                }
            }
        }
        _ => None,
    }
}

// -- GPS print converters -----------------------------------------------

fn print_gps_latitude_ref(v: &TagValue) -> Option<String> {
    match v.as_ascii() {
        Some("N") => Some("North".into()),
        Some("S") => Some("South".into()),
        Some(s) => Some(format!("Unknown ({})", s)),
        None => None,
    }
}

fn print_gps_longitude_ref(v: &TagValue) -> Option<String> {
    match v.as_ascii() {
        Some("E") => Some("East".into()),
        Some("W") => Some("West".into()),
        Some(s) => Some(format!("Unknown ({})", s)),
        None => None,
    }
}

/// GPSAltitude: rational -> "123.4 m"
fn print_gps_altitude(v: &TagValue) -> Option<String> {
    let f = v.to_f64()?;
    Some(format!("{} m", format_decimal_full(f)))
}

/// GPSHPositioningError: rational -> "123.456 m"
fn print_gps_hpe(v: &TagValue) -> Option<String> {
    let f = v.to_f64()?;
    Some(format!("{} m", format_decimal_full(f)))
}

// -- Interop print converters -------------------------------------------

fn print_interop_index(v: &TagValue) -> Option<String> {
    match v.as_ascii() {
        Some("R98") => Some("R98 - DCF basic file (sRGB)".into()),
        Some("THM") => Some("THM - DCF thumbnail file".into()),
        Some("R03") => Some("R03 - DCF option file (Adobe RGB)".into()),
        Some(s) => Some(s.to_string()),
        _ => None,
    }
}

// -- Flash print converter (ExifTool-compatible order) ------------------

fn print_flash_exiftool(v: &TagValue) -> Option<String> {
    let bits = v.to_u32()?;
    // ExifTool uses a flat lookup table for flash values
    Some(
        match bits {
            0x00 => "No Flash",
            0x01 => "Fired",
            0x05 => "Fired, Return not detected",
            0x07 => "Fired, Return detected",
            0x08 => "On, Did not fire",
            0x09 => "On, Fired",
            0x0d => "On, Return not detected",
            0x0f => "On, Return detected",
            0x10 => "Off, Did not fire",
            0x14 => "Off, Did not fire, Return not detected",
            0x18 => "Auto, Did not fire",
            0x19 => "Auto, Fired",
            0x1d => "Auto, Fired, Return not detected",
            0x1f => "Auto, Fired, Return detected",
            0x20 => "No flash function",
            0x30 => "Off, No flash function",
            0x41 => "Fired, Red-eye reduction",
            0x45 => "Fired, Red-eye reduction, Return not detected",
            0x47 => "Fired, Red-eye reduction, Return detected",
            0x49 => "On, Red-eye reduction",
            0x4d => "On, Red-eye reduction, Return not detected",
            0x4f => "On, Red-eye reduction, Return detected",
            0x50 => "Off, Red-eye reduction",
            0x58 => "Auto, Did not fire, Red-eye reduction",
            0x59 => "Auto, Fired, Red-eye reduction",
            0x5d => "Auto, Fired, Red-eye reduction, Return not detected",
            0x5f => "Auto, Fired, Red-eye reduction, Return detected",
            _ => return Some(format!("Unknown (0x{:x})", bits)),
        }
        .into(),
    )
}

// -- FocalPlaneResolutionUnit (ExifTool-compatible) ---------------------

fn print_focal_plane_res_unit_et(v: &TagValue) -> Option<String> {
    Some(
        match v.to_u32()? {
            1 => "None",
            2 => "inches",
            3 => "cm",
            4 => "mm",
            5 => "um",
            _ => return None,
        }
        .into(),
    )
}

// -- Helpers ------------------------------------------------------------

/// Format a float with 1 decimal place, rounding like ExifTool.
/// 14.0 -> "14.0", 2.8 -> "2.8", 4.25 -> "4.2"
fn format_decimal(f: f64) -> String {
    format!("{:.1}", f)
}

/// Format a float with full precision like ExifTool does for resolution values.
fn format_sig_digits(f: f64, sig: usize) -> String {
    // Equivalent to C's sprintf("%.Ng", f) - N significant digits, trailing zeros trimmed
    if f == 0.0 {
        return "0".into();
    }
    let magnitude = f.abs().log10().floor() as i32;
    let decimal_places = (sig as i32 - 1 - magnitude).max(0) as usize;
    let s = format!("{:.prec$}", f, prec = decimal_places);
    if s.contains('.') {
        let s = s.trim_end_matches('0');
        s.trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn format_decimal_full(f: f64) -> String {
    // 10 significant digits, trailing zeros trimmed - the precision these
    // resolution values are conventionally printed at.
    format_sig_digits(f, 10)
}

#[allow(dead_code)] // Used by `format_sig_digits` callers that aren't yet wired.
fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

// -- Tag tables (V2-V5) -------------------------------------------------

macro_rules! tag {
    ($id:expr, $name:expr, $group:expr) => {
        TagDef {
            id: $id,
            name: $name,
            group: $group,
            print_conv: None,
        }
    };
    ($id:expr, $name:expr, $group:expr, $conv:expr) => {
        TagDef {
            id: $id,
            name: $name,
            group: $group,
            print_conv: Some($conv),
        }
    };
}

/// V2 + V5: IFD0 / TIFF baseline tag table.
pub static IFD0_TAGS: &[TagDef] = &[
    // TIFF baseline (V5)
    tag!(
        0x00FE,
        "SubfileType",
        TagGroup::Ifd0,
        print_new_subfile_type
    ),
    tag!(0x00FF, "OldSubfileType", TagGroup::Ifd0),
    tag!(0x0100, "ImageWidth", TagGroup::Ifd0),
    tag!(0x0101, "ImageHeight", TagGroup::Ifd0),
    tag!(0x0102, "BitsPerSample", TagGroup::Ifd0),
    tag!(0x0103, "Compression", TagGroup::Ifd0, print_compression),
    tag!(
        0x0106,
        "PhotometricInterpretation",
        TagGroup::Ifd0,
        print_photometric
    ),
    tag!(0x010A, "FillOrder", TagGroup::Ifd0, print_fill_order),
    tag!(0x010D, "DocumentName", TagGroup::Ifd0),
    tag!(0x010E, "ImageDescription", TagGroup::Ifd0),
    tag!(0x010F, "Make", TagGroup::Ifd0),
    tag!(0x0110, "Model", TagGroup::Ifd0),
    tag!(0x0111, "StripOffsets", TagGroup::Ifd0),
    tag!(0x0112, "Orientation", TagGroup::Ifd0, print_orientation),
    tag!(0x0115, "SamplesPerPixel", TagGroup::Ifd0),
    tag!(0x0116, "RowsPerStrip", TagGroup::Ifd0),
    tag!(0x0117, "StripByteCounts", TagGroup::Ifd0),
    tag!(0x0118, "MinSampleValue", TagGroup::Ifd0),
    tag!(0x0119, "MaxSampleValue", TagGroup::Ifd0),
    tag!(0x011A, "XResolution", TagGroup::Ifd0, print_resolution),
    tag!(0x011B, "YResolution", TagGroup::Ifd0, print_resolution),
    tag!(
        0x011C,
        "PlanarConfiguration",
        TagGroup::Ifd0,
        print_planar_config
    ),
    tag!(0x011E, "XPosition", TagGroup::Ifd0, print_resolution),
    tag!(0x011F, "YPosition", TagGroup::Ifd0, print_resolution),
    tag!(
        0x0128,
        "ResolutionUnit",
        TagGroup::Ifd0,
        print_resolution_unit
    ),
    tag!(0x0131, "Software", TagGroup::Ifd0),
    tag!(0x0132, "DateTime", TagGroup::Ifd0),
    tag!(0x013B, "Artist", TagGroup::Ifd0),
    tag!(0x013C, "HostComputer", TagGroup::Ifd0),
    tag!(0x013D, "Predictor", TagGroup::Ifd0, print_predictor),
    tag!(0x013E, "WhitePoint", TagGroup::Ifd0, print_resolution),
    tag!(
        0x013F,
        "PrimaryChromaticities",
        TagGroup::Ifd0,
        print_resolution
    ),
    tag!(0x011D, "PageName", TagGroup::Ifd0),
    tag!(0x0129, "PageNumber", TagGroup::Ifd0),
    tag!(0x0140, "ColorMap", TagGroup::Ifd0),
    tag!(0x0142, "TileWidth", TagGroup::Ifd0),
    tag!(0x0143, "TileLength", TagGroup::Ifd0),
    tag!(0x0144, "TileOffsets", TagGroup::Ifd0),
    tag!(0x0145, "TileByteCounts", TagGroup::Ifd0),
    tag!(0x014A, "SubIFDs", TagGroup::Ifd0),
    tag!(0x015B, "JPEGTables", TagGroup::Ifd0),
    tag!(0x0152, "ExtraSamples", TagGroup::Ifd0, print_extra_samples),
    tag!(0x0153, "SampleFormat", TagGroup::Ifd0, print_sample_format),
    tag!(
        0x0211,
        "YCbCrCoefficients",
        TagGroup::Ifd0,
        print_resolution
    ),
    tag!(0x0212, "YCbCrSubSampling", TagGroup::Ifd0),
    tag!(
        0x0213,
        "YCbCrPositioning",
        TagGroup::Ifd0,
        print_ycbcr_positioning
    ),
    tag!(
        0x0214,
        "ReferenceBlackWhite",
        TagGroup::Ifd0,
        print_resolution
    ),
    tag!(0x0301, "Gamma", TagGroup::Ifd0, print_resolution),
    tag!(0x80E3, "Matteing", TagGroup::Ifd0),
    tag!(0x85D8, "ModelTransform", TagGroup::Ifd0),
    tag!(
        0x9216,
        "TIFF-EPStandardID",
        TagGroup::Ifd0,
        print_gps_version_id
    ),
    tag!(0x8298, "Copyright", TagGroup::Ifd0),
    // EXIF/GPS IFD pointers
    tag!(0x8769, "ExifIFD", TagGroup::Ifd0),
    tag!(0x8825, "GPSIFD", TagGroup::Ifd0),
    // Windows XP tags (UCS-2 encoded)
    tag!(0x9C9B, "XPTitle", TagGroup::Ifd0),
    tag!(0x9C9C, "XPComment", TagGroup::Ifd0),
    tag!(0x9C9D, "XPAuthor", TagGroup::Ifd0),
    tag!(0x9C9E, "XPKeywords", TagGroup::Ifd0),
    tag!(0x9C9F, "XPSubject", TagGroup::Ifd0),
];

/// V3: ExifIFD tag table.
pub static EXIF_IFD_TAGS: &[TagDef] = &[
    tag!(
        0x829A,
        "ExposureTime",
        TagGroup::ExifIfd,
        print_exposure_time
    ),
    tag!(0x829D, "FNumber", TagGroup::ExifIfd, print_fnumber),
    tag!(
        0x8822,
        "ExposureProgram",
        TagGroup::ExifIfd,
        print_exposure_program
    ),
    tag!(0x8824, "SpectralSensitivity", TagGroup::ExifIfd),
    tag!(0x8827, "ISO", TagGroup::ExifIfd),
    tag!(0x8828, "OECF", TagGroup::ExifIfd),
    tag!(
        0x8830,
        "SensitivityType",
        TagGroup::ExifIfd,
        print_sensitivity_type
    ),
    tag!(0x8831, "StandardOutputSensitivity", TagGroup::ExifIfd),
    tag!(0x8832, "RecommendedExposureIndex", TagGroup::ExifIfd),
    tag!(0x8833, "ISOSpeed", TagGroup::ExifIfd),
    tag!(
        0x9000,
        "ExifVersion",
        TagGroup::ExifIfd,
        print_version_bytes
    ),
    tag!(0x9003, "DateTimeOriginal", TagGroup::ExifIfd),
    tag!(0x9004, "DateTimeDigitized", TagGroup::ExifIfd),
    tag!(0x9010, "OffsetTime", TagGroup::ExifIfd),
    tag!(0x9011, "OffsetTimeOriginal", TagGroup::ExifIfd),
    tag!(0x9012, "OffsetTimeDigitized", TagGroup::ExifIfd),
    tag!(
        0x9101,
        "ComponentsConfiguration",
        TagGroup::ExifIfd,
        print_components_config
    ),
    tag!(
        0x9102,
        "CompressedBitsPerPixel",
        TagGroup::ExifIfd,
        print_compressed_bpp
    ),
    tag!(
        0x9201,
        "ShutterSpeedValue",
        TagGroup::ExifIfd,
        print_shutter_speed_value
    ),
    tag!(
        0x9202,
        "ApertureValue",
        TagGroup::ExifIfd,
        print_aperture_value
    ),
    tag!(0x9203, "BrightnessValue", TagGroup::ExifIfd),
    tag!(
        0x9204,
        "ExposureCompensation",
        TagGroup::ExifIfd,
        print_exposure_compensation
    ),
    tag!(
        0x9205,
        "MaxApertureValue",
        TagGroup::ExifIfd,
        print_aperture_value
    ),
    tag!(
        0x9206,
        "SubjectDistance",
        TagGroup::ExifIfd,
        print_subject_distance
    ),
    tag!(
        0x9207,
        "MeteringMode",
        TagGroup::ExifIfd,
        print_metering_mode
    ),
    tag!(0x9208, "LightSource", TagGroup::ExifIfd, print_light_source),
    tag!(0x9209, "Flash", TagGroup::ExifIfd, print_flash_exiftool),
    tag!(0x920A, "FocalLength", TagGroup::ExifIfd, print_focal_length),
    tag!(0x9214, "SubjectArea", TagGroup::ExifIfd),
    tag!(0x927C, "MakerNote", TagGroup::ExifIfd),
    tag!(0x9286, "UserComment", TagGroup::ExifIfd, print_user_comment),
    tag!(0x9290, "SubSecTime", TagGroup::ExifIfd),
    tag!(0x9291, "SubSecTimeOriginal", TagGroup::ExifIfd),
    tag!(0x9292, "SubSecTimeDigitized", TagGroup::ExifIfd),
    tag!(
        0xA000,
        "FlashpixVersion",
        TagGroup::ExifIfd,
        print_version_bytes
    ),
    tag!(0xA001, "ColorSpace", TagGroup::ExifIfd, print_color_space),
    tag!(0xA002, "ExifImageWidth", TagGroup::ExifIfd),
    tag!(0xA003, "ExifImageHeight", TagGroup::ExifIfd),
    tag!(0xA004, "RelatedSoundFile", TagGroup::ExifIfd),
    tag!(0xA005, "InteropIFD", TagGroup::ExifIfd),
    tag!(0xA20B, "FlashEnergy", TagGroup::ExifIfd),
    tag!(
        0xA20E,
        "FocalPlaneXResolution",
        TagGroup::ExifIfd,
        print_resolution
    ),
    tag!(
        0xA20F,
        "FocalPlaneYResolution",
        TagGroup::ExifIfd,
        print_resolution
    ),
    tag!(
        0xA210,
        "FocalPlaneResolutionUnit",
        TagGroup::ExifIfd,
        print_focal_plane_res_unit_et
    ),
    tag!(0xA215, "ExposureIndex", TagGroup::ExifIfd, print_resolution),
    tag!(
        0xA217,
        "SensingMethod",
        TagGroup::ExifIfd,
        print_sensing_method
    ),
    tag!(0xA300, "FileSource", TagGroup::ExifIfd, print_file_source),
    tag!(0xA301, "SceneType", TagGroup::ExifIfd, print_scene_type),
    tag!(0xA302, "CFAPattern", TagGroup::ExifIfd, print_cfa_pattern),
    tag!(
        0xA401,
        "CustomRendered",
        TagGroup::ExifIfd,
        print_custom_rendered
    ),
    tag!(
        0xA402,
        "ExposureMode",
        TagGroup::ExifIfd,
        print_exposure_mode
    ),
    tag!(
        0xA403,
        "WhiteBalance",
        TagGroup::ExifIfd,
        print_white_balance
    ),
    tag!(
        0xA404,
        "DigitalZoomRatio",
        TagGroup::ExifIfd,
        print_digital_zoom
    ),
    tag!(
        0xA405,
        "FocalLengthIn35mmFormat",
        TagGroup::ExifIfd,
        print_focal_length_35mm
    ),
    tag!(
        0xA406,
        "SceneCaptureType",
        TagGroup::ExifIfd,
        print_scene_capture_type
    ),
    tag!(0xA407, "GainControl", TagGroup::ExifIfd, print_gain_control),
    tag!(0xA408, "Contrast", TagGroup::ExifIfd, print_contrast),
    tag!(0xA409, "Saturation", TagGroup::ExifIfd, print_saturation),
    tag!(0xA40A, "Sharpness", TagGroup::ExifIfd, print_sharpness),
    tag!(0xA40B, "DeviceSettingDescription", TagGroup::ExifIfd),
    tag!(
        0xA40C,
        "SubjectDistanceRange",
        TagGroup::ExifIfd,
        print_subject_distance_range
    ),
    tag!(0xA420, "ImageUniqueID", TagGroup::ExifIfd),
    tag!(0xA430, "CameraOwnerName", TagGroup::ExifIfd),
    tag!(0xA431, "SerialNumber", TagGroup::ExifIfd),
    tag!(0xA432, "LensInfo", TagGroup::ExifIfd, print_lens_info),
    tag!(0xA433, "LensMake", TagGroup::ExifIfd),
    tag!(0xA434, "LensModel", TagGroup::ExifIfd),
    tag!(0xA435, "LensSerialNumber", TagGroup::ExifIfd),
    // Not standard but used: ExifTool shows 0xA500 as "Gamma"
    tag!(0xA500, "Gamma", TagGroup::ExifIfd, print_resolution),
    tag!(
        0xA460,
        "CompositeImage",
        TagGroup::ExifIfd,
        print_composite_image
    ),
    tag!(0xA461, "CompositeImageCount", TagGroup::ExifIfd),
    tag!(0xA462, "CompositeImageExposureTimes", TagGroup::ExifIfd),
    tag!(0xEA1C, "Padding", TagGroup::ExifIfd),
    tag!(0xEA1D, "OffsetSchema", TagGroup::ExifIfd),
];

/// V4: GPS IFD tag table.
pub static GPS_TAGS: &[TagDef] = &[
    tag!(
        0x0000,
        "GPSVersionID",
        TagGroup::GpsIfd,
        print_gps_version_id
    ),
    tag!(
        0x0001,
        "GPSLatitudeRef",
        TagGroup::GpsIfd,
        print_gps_latitude_ref
    ),
    tag!(0x0002, "GPSLatitude", TagGroup::GpsIfd),
    tag!(
        0x0003,
        "GPSLongitudeRef",
        TagGroup::GpsIfd,
        print_gps_longitude_ref
    ),
    tag!(0x0004, "GPSLongitude", TagGroup::GpsIfd),
    tag!(
        0x0005,
        "GPSAltitudeRef",
        TagGroup::GpsIfd,
        print_gps_altitude_ref
    ),
    tag!(0x0006, "GPSAltitude", TagGroup::GpsIfd, print_gps_altitude),
    tag!(
        0x0007,
        "GPSTimeStamp",
        TagGroup::GpsIfd,
        print_gps_timestamp
    ),
    tag!(0x0008, "GPSSatellites", TagGroup::GpsIfd),
    tag!(0x0009, "GPSStatus", TagGroup::GpsIfd, print_gps_status),
    tag!(
        0x000A,
        "GPSMeasureMode",
        TagGroup::GpsIfd,
        print_gps_measure_mode
    ),
    tag!(0x000B, "GPSDOP", TagGroup::GpsIfd),
    tag!(0x000C, "GPSSpeedRef", TagGroup::GpsIfd, print_gps_speed_ref),
    tag!(0x000D, "GPSSpeed", TagGroup::GpsIfd, print_resolution),
    tag!(
        0x000E,
        "GPSTrackRef",
        TagGroup::GpsIfd,
        print_gps_direction_ref
    ),
    tag!(0x000F, "GPSTrack", TagGroup::GpsIfd, print_resolution),
    tag!(
        0x0010,
        "GPSImgDirectionRef",
        TagGroup::GpsIfd,
        print_gps_direction_ref
    ),
    tag!(
        0x0011,
        "GPSImgDirection",
        TagGroup::GpsIfd,
        print_resolution
    ),
    tag!(0x0012, "GPSMapDatum", TagGroup::GpsIfd),
    tag!(
        0x0013,
        "GPSDestLatitudeRef",
        TagGroup::GpsIfd,
        print_gps_latitude_ref
    ),
    tag!(0x0014, "GPSDestLatitude", TagGroup::GpsIfd),
    tag!(
        0x0015,
        "GPSDestLongitudeRef",
        TagGroup::GpsIfd,
        print_gps_longitude_ref
    ),
    tag!(0x0016, "GPSDestLongitude", TagGroup::GpsIfd),
    tag!(
        0x0017,
        "GPSDestBearingRef",
        TagGroup::GpsIfd,
        print_gps_direction_ref
    ),
    tag!(0x0018, "GPSDestBearing", TagGroup::GpsIfd, print_resolution),
    tag!(
        0x0019,
        "GPSDestDistanceRef",
        TagGroup::GpsIfd,
        print_gps_distance_ref
    ),
    tag!(
        0x001A,
        "GPSDestDistance",
        TagGroup::GpsIfd,
        print_resolution
    ),
    tag!(0x001B, "GPSProcessingMethod", TagGroup::GpsIfd),
    tag!(0x001C, "GPSAreaInformation", TagGroup::GpsIfd),
    tag!(0x001D, "GPSDateStamp", TagGroup::GpsIfd),
    tag!(
        0x001E,
        "GPSDifferential",
        TagGroup::GpsIfd,
        print_gps_differential
    ),
    tag!(
        0x001F,
        "GPSHPositioningError",
        TagGroup::GpsIfd,
        print_gps_hpe
    ),
];

/// Interoperability IFD tags.
pub static INTEROP_TAGS: &[TagDef] = &[
    tag!(
        0x0001,
        "InteropIndex",
        TagGroup::InteropIfd,
        print_interop_index
    ),
    tag!(
        0x0002,
        "InteropVersion",
        TagGroup::InteropIfd,
        print_interop_version
    ),
    tag!(0x1000, "RelatedImageFileFormat", TagGroup::InteropIfd),
    tag!(0x1001, "RelatedImageWidth", TagGroup::InteropIfd),
    tag!(0x1002, "RelatedImageHeight", TagGroup::InteropIfd),
];

/// Combined tag list for name-based lookup (V10).
static ALL_TAGS: &[TagDef] = {
    // We can't concatenate slices at compile time, so we use a macro-generated flat array.
    // For now, use a function-based approach via find_tag_by_name which searches all tables.
    // This static is a placeholder - the actual search iterates all tables.
    &[]
};

/// V10: Search all tag tables by name (used when ALL_TAGS is empty placeholder).
pub fn find_tag_by_name_all(name: &str) -> Option<&'static TagDef> {
    let lower = name.to_ascii_lowercase();
    for table in [IFD0_TAGS, EXIF_IFD_TAGS, GPS_TAGS, INTEROP_TAGS] {
        if let Some(t) = table.iter().find(|t| t.name.to_ascii_lowercase() == lower) {
            return Some(t);
        }
    }
    None
}

/// Return the tag name for a given ID and group, or a hex fallback.
pub fn tag_name(id: u16, group: TagGroup) -> String {
    find_tag(id, group)
        .map(|t| t.name.to_string())
        .unwrap_or_else(|| format!("Tag 0x{id:04X}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_tag_def_structure() {
        let tag = &IFD0_TAGS[0];
        assert_eq!(tag.id, 0x00FE);
        assert_eq!(tag.name, "SubfileType");
        assert_eq!(tag.group, TagGroup::Ifd0);
    }

    #[test]
    fn v2_ifd0_tag_count() {
        // At least 30 IFD0/TIFF baseline tags
        assert!(IFD0_TAGS.len() >= 30);
    }

    #[test]
    fn v3_exif_ifd_tag_count() {
        // At least 50 ExifIFD tags
        assert!(EXIF_IFD_TAGS.len() >= 50);
    }

    #[test]
    fn v4_gps_tag_count() {
        // At least 30 GPS tags
        assert!(GPS_TAGS.len() >= 30);
    }

    #[test]
    fn v5_tiff_baseline_tags() {
        // Key TIFF baseline tags are present
        assert!(find_tag(0x0100, TagGroup::Ifd0).is_some()); // ImageWidth
        assert!(find_tag(0x0101, TagGroup::Ifd0).is_some()); // ImageHeight
        assert!(find_tag(0x0102, TagGroup::Ifd0).is_some()); // BitsPerSample
        assert!(find_tag(0x0103, TagGroup::Ifd0).is_some()); // Compression
        assert!(find_tag(0x0106, TagGroup::Ifd0).is_some()); // PhotometricInterpretation
        assert!(find_tag(0x0112, TagGroup::Ifd0).is_some()); // Orientation
    }

    #[test]
    fn v8_print_conv_orientation() {
        assert_eq!(
            print_orientation(&TagValue::U16(1)),
            Some("Horizontal (normal)".into())
        );
        assert_eq!(
            print_orientation(&TagValue::U16(6)),
            Some("Rotate 90 CW".into())
        );
        assert_eq!(print_orientation(&TagValue::U16(99)), None);
    }

    #[test]
    fn v8_print_conv_compression() {
        assert_eq!(
            print_compression(&TagValue::U16(1)),
            Some("Uncompressed".into())
        );
        assert_eq!(
            print_compression(&TagValue::U16(6)),
            Some("JPEG (old-style)".into())
        );
    }

    #[test]
    fn v8_print_conv_exposure_program() {
        assert_eq!(
            print_exposure_program(&TagValue::U16(2)),
            Some("Program AE".into())
        );
        assert_eq!(
            print_exposure_program(&TagValue::U16(4)),
            Some("Shutter speed priority AE".into())
        );
    }

    #[test]
    fn v8_print_conv_flash() {
        // 0x0F = fired + return detected + compulsory
        assert_eq!(
            print_flash(&TagValue::U16(0x0F)),
            Some("Fired, strobe return detected, compulsory firing".into())
        );
        // 0x00 = did not fire
        assert_eq!(
            print_flash(&TagValue::U16(0x00)),
            Some("Did not fire".into())
        );
    }

    #[test]
    fn v8_print_conv_color_space() {
        assert_eq!(print_color_space(&TagValue::U16(1)), Some("sRGB".into()));
        assert_eq!(
            print_color_space(&TagValue::U16(0xFFFF)),
            Some("Uncalibrated".into())
        );
    }

    #[test]
    fn v8_print_value_with_conv() {
        let tag = find_tag(0x0112, TagGroup::Ifd0).unwrap(); // Orientation
        let val = TagValue::U16(6);
        assert_eq!(print_value(tag, &val), "Rotate 90 CW");
    }

    #[test]
    fn v8_print_value_no_conv() {
        let tag = find_tag(0x010F, TagGroup::Ifd0).unwrap(); // Make
        let val = TagValue::Ascii("Canon".into());
        assert_eq!(print_value(tag, &val), "Canon");
    }

    #[test]
    fn v9_tag_groups() {
        assert_eq!(
            find_tag(0x0100, TagGroup::Ifd0).unwrap().group,
            TagGroup::Ifd0
        );
        assert_eq!(
            find_tag(0x829A, TagGroup::ExifIfd).unwrap().group,
            TagGroup::ExifIfd
        );
        assert_eq!(
            find_tag(0x0002, TagGroup::GpsIfd).unwrap().group,
            TagGroup::GpsIfd
        );
    }

    #[test]
    fn v10_lookup_by_id() {
        let tag = find_tag(0x010F, TagGroup::Ifd0).unwrap();
        assert_eq!(tag.name, "Make");

        let tag = find_tag(0x829A, TagGroup::ExifIfd).unwrap();
        assert_eq!(tag.name, "ExposureTime");

        let tag = find_tag(0x0002, TagGroup::GpsIfd).unwrap();
        assert_eq!(tag.name, "GPSLatitude");
    }

    #[test]
    fn v10_lookup_by_name() {
        let tag = find_tag_by_name_all("Make").unwrap();
        assert_eq!(tag.id, 0x010F);

        let tag = find_tag_by_name_all("ExposureTime").unwrap();
        assert_eq!(tag.id, 0x829A);

        let tag = find_tag_by_name_all("GPSLatitude").unwrap();
        assert_eq!(tag.id, 0x0002);
    }

    #[test]
    fn v10_lookup_by_name_case_insensitive() {
        assert!(find_tag_by_name_all("make").is_some());
        assert!(find_tag_by_name_all("MAKE").is_some());
        assert!(find_tag_by_name_all("exposuretime").is_some());
    }

    #[test]
    fn v10_tag_name_known() {
        assert_eq!(tag_name(0x010F, TagGroup::Ifd0), "Make");
    }

    #[test]
    fn v10_tag_name_unknown() {
        assert_eq!(tag_name(0xFFFF, TagGroup::Ifd0), "Tag 0xFFFF");
    }

    #[test]
    fn v10_ifd1_uses_ifd0_table() {
        // IFD1 shares tags with IFD0
        let tag = find_tag(0x0103, TagGroup::Ifd1).unwrap();
        assert_eq!(tag.name, "Compression");
    }

    #[test]
    fn v8_print_gps_converters() {
        assert_eq!(
            print_gps_altitude_ref(&TagValue::U8(0)),
            Some("Above Sea Level".into())
        );
        assert_eq!(
            print_gps_altitude_ref(&TagValue::U8(1)),
            Some("Below Sea Level".into())
        );
        assert_eq!(
            print_gps_speed_ref(&TagValue::Ascii("K".into())),
            Some("km/h".into())
        );
        assert_eq!(
            print_gps_differential(&TagValue::U16(1)),
            Some("Differential Corrected".into())
        );
    }

    #[test]
    fn v8_print_scene_and_file_source() {
        assert_eq!(
            print_file_source(&TagValue::Bytes(vec![3])),
            Some("Digital Camera".into())
        );
        assert_eq!(
            print_scene_type(&TagValue::Bytes(vec![1])),
            Some("Directly photographed".into())
        );
    }

    #[test]
    fn shutter_speed_and_exposure_time() {
        // ShutterSpeedValue APEX ~5.91 -> 1/60
        assert_eq!(
            print_shutter_speed_value(&TagValue::SRational(62534, 10573)),
            Some("1/60".into())
        );
        // ExposureTime 8/10 -> 0.8 (not 1/1)
        assert_eq!(
            print_exposure_time(&TagValue::Rational(8, 10)),
            Some("0.8".into())
        );
        // ExposureTime 10/25 -> 0.4 (not 1/3)
        assert_eq!(
            print_exposure_time(&TagValue::Rational(10, 25)),
            Some("0.4".into())
        );
        // ExposureTime 1/125 -> 1/125
        assert_eq!(
            print_exposure_time(&TagValue::Rational(1, 125)),
            Some("1/125".into())
        );
    }
}
