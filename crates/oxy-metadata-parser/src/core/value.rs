//! The typed value a tag carries, and the format-independent half of the
//! conversion pipeline.
//!
//! This lives in `core` rather than beside the TIFF reader because it is
//! reachable from the public API - `Tag::typed_value` is a `TagValue` - and so
//! has to exist in every feature configuration, including one with no image
//! formats enabled at all. Turning raw IFD bytes into one of these is TIFF's
//! job and stays there, in `tiff::value`.
//!
//! Three-stage pipeline:
//! - **RawConv** (V6): raw IFD bytes -> `TagValue` (`tiff::value::from_entry`)
//! - **ValueConv** (V7): `TagValue` -> logical representation (rational -> f64)
//! - **PrintConv** (V8): logical value -> display string ("1" -> "Landscape")

/// A typed value extracted from an IFD entry (V6-V7).
#[derive(Debug, Clone, PartialEq)]
pub enum TagValue {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Rational(u32, u32),
    SRational(i32, i32),
    Ascii(String),
    Bytes(Vec<u8>),
    U16Array(Vec<u16>),
    U32Array(Vec<u32>),
    U64Array(Vec<u64>),
    I16Array(Vec<i16>),
    I32Array(Vec<i32>),
    F32Array(Vec<f32>),
    F64Array(Vec<f64>),
    RationalArray(Vec<(u32, u32)>),
    SRationalArray(Vec<(i32, i32)>),
}
impl TagValue {
    /// V7: Convert to f64 (ValueConv). For rationals, computes num/den.
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            TagValue::U8(v) => Some(*v as f64),
            TagValue::U16(v) => Some(*v as f64),
            TagValue::U32(v) => Some(*v as f64),
            TagValue::U64(v) => Some(*v as f64),
            TagValue::I8(v) => Some(*v as f64),
            TagValue::I16(v) => Some(*v as f64),
            TagValue::I32(v) => Some(*v as f64),
            TagValue::I64(v) => Some(*v as f64),
            TagValue::F32(v) => Some(*v as f64),
            TagValue::F64(v) => Some(*v),
            TagValue::Rational(n, d) => {
                if *d == 0 {
                    None
                } else {
                    Some(*n as f64 / *d as f64)
                }
            }
            TagValue::SRational(n, d) => {
                if *d == 0 {
                    None
                } else {
                    Some(*n as f64 / *d as f64)
                }
            }
            _ => None,
        }
    }

    /// V7: Convert to u32 (ValueConv).
    pub fn to_u32(&self) -> Option<u32> {
        match self {
            TagValue::U8(v) => Some(*v as u32),
            TagValue::U16(v) => Some(*v as u32),
            TagValue::U32(v) => Some(*v),
            TagValue::U64(v) => u32::try_from(*v).ok(),
            TagValue::I8(v) if *v >= 0 => Some(*v as u32),
            TagValue::I16(v) if *v >= 0 => Some(*v as u32),
            TagValue::I32(v) if *v >= 0 => Some(*v as u32),
            _ => None,
        }
    }

    /// Get as ASCII string reference.
    pub fn as_ascii(&self) -> Option<&str> {
        match self {
            TagValue::Ascii(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// V8: Format for display (default PrintConv).
    pub fn display(&self) -> String {
        match self {
            TagValue::U8(v) => v.to_string(),
            TagValue::U16(v) => v.to_string(),
            TagValue::U32(v) => v.to_string(),
            TagValue::U64(v) => v.to_string(),
            TagValue::I8(v) => v.to_string(),
            TagValue::I16(v) => v.to_string(),
            TagValue::I32(v) => v.to_string(),
            TagValue::I64(v) => v.to_string(),
            TagValue::F32(v) => format!("{v:.6}"),
            TagValue::F64(v) => format!("{v:.6}"),
            TagValue::Rational(n, d) => {
                if *d == 0 {
                    if *n == 0 {
                        "undef".into()
                    } else {
                        format!("{n}/0")
                    }
                } else if *n % *d == 0 {
                    format!("{}", *n / *d)
                } else {
                    format!("{n}/{d}")
                }
            }
            TagValue::SRational(n, d) => {
                if *d == 0 {
                    if *n == 0 {
                        "undef".into()
                    } else {
                        format!("{n}/0")
                    }
                } else if *n % *d == 0 {
                    format!("{}", *n / *d)
                } else {
                    format!("{n}/{d}")
                }
            }
            TagValue::Ascii(s) => s.clone(),
            TagValue::Bytes(b) => {
                if b.len() <= 16 {
                    b.iter()
                        .map(|x| format!("{x:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    let prefix: String = b[..16]
                        .iter()
                        .map(|x| format!("{x:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("{prefix} ... ({} bytes)", b.len())
                }
            }
            TagValue::U16Array(a) => format_array(a),
            TagValue::U32Array(a) => format_array(a),
            TagValue::U64Array(a) => format_array(a),
            TagValue::I16Array(a) => format_array(a),
            TagValue::I32Array(a) => format_array(a),
            TagValue::F32Array(a) => a
                .iter()
                .map(|v| format!("{v:.6}"))
                .collect::<Vec<_>>()
                .join(" "),
            TagValue::F64Array(a) => a
                .iter()
                .map(|v| {
                    // Use full precision, strip trailing zeros
                    let s = format!("{v}");
                    s
                })
                .collect::<Vec<_>>()
                .join(" "),
            TagValue::RationalArray(a) => a
                .iter()
                .map(|(n, d)| format!("{n}/{d}"))
                .collect::<Vec<_>>()
                .join(" "),
            TagValue::SRationalArray(a) => a
                .iter()
                .map(|(n, d)| format!("{n}/{d}"))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

fn format_array<T: std::fmt::Display>(a: &[T]) -> String {
    a.iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}
