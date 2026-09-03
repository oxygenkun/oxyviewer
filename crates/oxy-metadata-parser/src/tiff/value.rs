//! Turning raw IFD bytes into a `TagValue` (V6, RawConv).
//!
//! The value type itself lives in [`crate::core::value`], because it is
//! reachable from the public API and must exist even with every image format
//! disabled. What is TIFF-specific - reading an `IfdEntry`'s bytes with the
//! right endianness and data type - is here.

use crate::tiff::{DataType, IfdEntry};

pub use crate::core::value::TagValue;

impl TagValue {
    /// V6: Extract a typed value from a raw IFD entry (RawConv).
    pub fn from_entry(entry: &IfdEntry<'_>, big_endian: bool) -> Option<TagValue> {
        let data = entry.data;
        let count = entry.count as usize;

        // Windows XP tags (0x9C9B-0x9C9F) are UCS-2LE encoded strings stored as BYTE
        if entry.tag >= 0x9C9B && entry.tag <= 0x9C9F && entry.data_type == DataType::Byte {
            return Some(TagValue::Ascii(decode_ucs2le(data)));
        }

        match entry.data_type {
            DataType::Byte => {
                if count == 1 {
                    data.first().map(|&b| TagValue::U8(b))
                } else {
                    Some(TagValue::Bytes(data[..count.min(data.len())].to_vec()))
                }
            }
            DataType::Ascii => {
                // Truncate at first null byte (like ExifTool), then trim trailing whitespace
                let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
                std::str::from_utf8(&data[..end])
                    .ok()
                    .map(|s| TagValue::Ascii(s.trim_end().to_string()))
            }
            DataType::Short => {
                if count == 1 && data.len() >= 2 {
                    Some(TagValue::U16(read_u16(data, 0, big_endian)))
                } else {
                    let vals: Vec<u16> = (0..count)
                        .take_while(|&i| i * 2 + 2 <= data.len())
                        .map(|i| read_u16(data, i * 2, big_endian))
                        .collect();
                    Some(TagValue::U16Array(vals))
                }
            }
            DataType::Long | DataType::Ifd => {
                if count == 1 && data.len() >= 4 {
                    Some(TagValue::U32(read_u32(data, 0, big_endian)))
                } else {
                    let vals: Vec<u32> = (0..count)
                        .take_while(|&i| i * 4 + 4 <= data.len())
                        .map(|i| read_u32(data, i * 4, big_endian))
                        .collect();
                    Some(TagValue::U32Array(vals))
                }
            }
            DataType::Rational => {
                if count == 1 && data.len() >= 8 {
                    let num = read_u32(data, 0, big_endian);
                    let den = read_u32(data, 4, big_endian);
                    Some(TagValue::Rational(num, den))
                } else {
                    let vals: Vec<(u32, u32)> = (0..count)
                        .take_while(|&i| i * 8 + 8 <= data.len())
                        .map(|i| {
                            let off = i * 8;
                            (
                                read_u32(data, off, big_endian),
                                read_u32(data, off + 4, big_endian),
                            )
                        })
                        .collect();
                    Some(TagValue::RationalArray(vals))
                }
            }
            DataType::SByte => {
                if count == 1 {
                    data.first().map(|&b| TagValue::I8(b as i8))
                } else {
                    Some(TagValue::Bytes(data[..count.min(data.len())].to_vec()))
                }
            }
            DataType::Undefined => Some(TagValue::Bytes(data[..count.min(data.len())].to_vec())),
            DataType::SShort => {
                if count == 1 && data.len() >= 2 {
                    Some(TagValue::I16(read_i16(data, 0, big_endian)))
                } else {
                    let vals: Vec<i16> = (0..count)
                        .take_while(|&i| i * 2 + 2 <= data.len())
                        .map(|i| read_i16(data, i * 2, big_endian))
                        .collect();
                    Some(TagValue::I16Array(vals))
                }
            }
            DataType::SLong => {
                if count == 1 && data.len() >= 4 {
                    Some(TagValue::I32(read_i32(data, 0, big_endian)))
                } else {
                    let vals: Vec<i32> = (0..count)
                        .take_while(|&i| i * 4 + 4 <= data.len())
                        .map(|i| read_i32(data, i * 4, big_endian))
                        .collect();
                    Some(TagValue::I32Array(vals))
                }
            }
            DataType::SRational => {
                if count == 1 && data.len() >= 8 {
                    let num = read_i32(data, 0, big_endian);
                    let den = read_i32(data, 4, big_endian);
                    Some(TagValue::SRational(num, den))
                } else {
                    let vals: Vec<(i32, i32)> = (0..count)
                        .take_while(|&i| i * 8 + 8 <= data.len())
                        .map(|i| {
                            let off = i * 8;
                            (
                                read_i32(data, off, big_endian),
                                read_i32(data, off + 4, big_endian),
                            )
                        })
                        .collect();
                    Some(TagValue::SRationalArray(vals))
                }
            }
            DataType::Float => {
                if count == 1 && data.len() >= 4 {
                    let bits = read_u32(data, 0, big_endian);
                    Some(TagValue::F32(f32::from_bits(bits)))
                } else {
                    let vals: Vec<f32> = (0..count)
                        .take_while(|&i| i * 4 + 4 <= data.len())
                        .map(|i| f32::from_bits(read_u32(data, i * 4, big_endian)))
                        .collect();
                    Some(TagValue::F32Array(vals))
                }
            }
            DataType::Double => {
                if count == 1 && data.len() >= 8 {
                    let bits = read_u64(data, 0, big_endian);
                    Some(TagValue::F64(f64::from_bits(bits)))
                } else {
                    let vals: Vec<f64> = (0..count)
                        .take_while(|&i| i * 8 + 8 <= data.len())
                        .map(|i| f64::from_bits(read_u64(data, i * 8, big_endian)))
                        .collect();
                    Some(TagValue::F64Array(vals))
                }
            }
            DataType::Long8 | DataType::Ifd8 => {
                if count == 1 && data.len() >= 8 {
                    Some(TagValue::U64(read_u64(data, 0, big_endian)))
                } else {
                    let vals: Vec<u64> = (0..count)
                        .take_while(|&i| i * 8 + 8 <= data.len())
                        .map(|i| read_u64(data, i * 8, big_endian))
                        .collect();
                    Some(TagValue::U64Array(vals))
                }
            }
            DataType::SLong8 => {
                if count == 1 && data.len() >= 8 {
                    Some(TagValue::I64(read_u64(data, 0, big_endian) as i64))
                } else {
                    // Return as U64Array for simplicity
                    let vals: Vec<u64> = (0..count)
                        .take_while(|&i| i * 8 + 8 <= data.len())
                        .map(|i| read_u64(data, i * 8, big_endian))
                        .collect();
                    Some(TagValue::U64Array(vals))
                }
            }
        }
    }
}

/// Decode UCS-2LE (UTF-16LE) bytes to a UTF-8 string, trimming null terminators.
fn decode_ucs2le(data: &[u8]) -> String {
    let u16s: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    // Trim trailing nulls
    let end = u16s.iter().position(|&c| c == 0).unwrap_or(u16s.len());
    String::from_utf16_lossy(&u16s[..end])
}

fn read_u16(data: &[u8], off: usize, big_endian: bool) -> u16 {
    if big_endian {
        u16::from_be_bytes([data[off], data[off + 1]])
    } else {
        u16::from_le_bytes([data[off], data[off + 1]])
    }
}

fn read_i16(data: &[u8], off: usize, big_endian: bool) -> i16 {
    read_u16(data, off, big_endian) as i16
}

fn read_u32(data: &[u8], off: usize, big_endian: bool) -> u32 {
    if big_endian {
        u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    } else {
        u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    }
}

fn read_i32(data: &[u8], off: usize, big_endian: bool) -> i32 {
    read_u32(data, off, big_endian) as i32
}

fn read_u64(data: &[u8], off: usize, big_endian: bool) -> u64 {
    if big_endian {
        u64::from_be_bytes(data[off..off + 8].try_into().unwrap())
    } else {
        u64::from_le_bytes(data[off..off + 8].try_into().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry<'a>(tag: u16, data_type: DataType, count: u64, data: &'a [u8]) -> IfdEntry<'a> {
        IfdEntry {
            tag,
            data_type,
            raw_type: data_type as u16,
            count,
            data,
            inline: data.len() <= 4,
        }
    }

    #[test]
    fn v6_raw_conv_u16() {
        let bytes = 1920u16.to_le_bytes();
        let entry = make_entry(256, DataType::Short, 1, &bytes);
        let val = TagValue::from_entry(&entry, false).unwrap();
        assert_eq!(val, TagValue::U16(1920));
    }

    #[test]
    fn v6_raw_conv_u32() {
        let bytes = 100000u32.to_le_bytes();
        let entry = make_entry(256, DataType::Long, 1, &bytes);
        let val = TagValue::from_entry(&entry, false).unwrap();
        assert_eq!(val, TagValue::U32(100000));
    }

    #[test]
    fn v6_raw_conv_ascii() {
        let bytes = b"Canon\0";
        let entry = make_entry(271, DataType::Ascii, 6, bytes);
        let val = TagValue::from_entry(&entry, false).unwrap();
        assert_eq!(val, TagValue::Ascii("Canon".into()));
    }

    #[test]
    fn v6_raw_conv_rational() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&100u32.to_le_bytes());
        let entry = make_entry(0x829A, DataType::Rational, 1, &bytes);
        let val = TagValue::from_entry(&entry, false).unwrap();
        assert_eq!(val, TagValue::Rational(1, 100));
    }

    #[test]
    fn v6_raw_conv_srational() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(-1i32).to_le_bytes());
        bytes.extend_from_slice(&3i32.to_le_bytes());
        let entry = make_entry(0x9204, DataType::SRational, 1, &bytes);
        let val = TagValue::from_entry(&entry, false).unwrap();
        assert_eq!(val, TagValue::SRational(-1, 3));
    }

    #[test]
    fn v6_raw_conv_be() {
        let bytes = 1920u16.to_be_bytes();
        let entry = make_entry(256, DataType::Short, 1, &bytes);
        let val = TagValue::from_entry(&entry, true).unwrap();
        assert_eq!(val, TagValue::U16(1920));
    }

    #[test]
    fn v6_raw_conv_u16_array() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        let entry = make_entry(258, DataType::Short, 3, &bytes);
        let val = TagValue::from_entry(&entry, false).unwrap();
        assert_eq!(val, TagValue::U16Array(vec![8, 8, 8]));
    }

    // 3.14 / 2.718... here are arbitrary floats chosen to have an inexact
    // binary expansion, not approximations of a constant.
    #[allow(clippy::approx_constant)]
    #[test]
    fn v6_raw_conv_double() {
        let bytes = 3.14159f64.to_le_bytes();
        let entry = make_entry(1, DataType::Double, 1, &bytes);
        let val = TagValue::from_entry(&entry, false).unwrap();
        assert_eq!(val, TagValue::F64(3.14159));
    }

    #[test]
    fn v6_raw_conv_byte() {
        let bytes = [42u8];
        let entry = make_entry(1, DataType::Byte, 1, &bytes);
        let val = TagValue::from_entry(&entry, false).unwrap();
        assert_eq!(val, TagValue::U8(42));
    }

    #[test]
    fn v6_raw_conv_undefined() {
        let bytes = b"0232";
        let entry = make_entry(0x9000, DataType::Undefined, 4, bytes);
        let val = TagValue::from_entry(&entry, false).unwrap();
        assert_eq!(val, TagValue::Bytes(b"0232".to_vec()));
    }

    #[test]
    fn v7_rational_to_f64() {
        let val = TagValue::Rational(1, 100);
        assert_eq!(val.to_f64(), Some(0.01));
    }

    #[test]
    fn v7_srational_to_f64() {
        let val = TagValue::SRational(-1, 3);
        let f = val.to_f64().unwrap();
        assert!((f - (-1.0 / 3.0)).abs() < 1e-10);
    }

    #[test]
    fn v7_rational_div_zero() {
        let val = TagValue::Rational(1, 0);
        assert_eq!(val.to_f64(), None);
    }

    #[test]
    fn v7_to_u32() {
        assert_eq!(TagValue::U16(100).to_u32(), Some(100));
        assert_eq!(TagValue::U32(200).to_u32(), Some(200));
        assert_eq!(TagValue::I32(-1).to_u32(), None);
    }

    #[test]
    fn v8_display_rational() {
        assert_eq!(TagValue::Rational(1, 100).display(), "1/100");
        assert_eq!(TagValue::Rational(72, 1).display(), "72");
        assert_eq!(TagValue::Rational(1, 0).display(), "1/0");
    }

    #[test]
    fn v8_display_ascii() {
        assert_eq!(TagValue::Ascii("Canon".into()).display(), "Canon");
    }

    #[test]
    fn v8_display_bytes() {
        assert_eq!(
            TagValue::Bytes(vec![0x30, 0x32, 0x33, 0x32]).display(),
            "30 32 33 32"
        );
    }

    #[test]
    fn v8_display_array() {
        assert_eq!(TagValue::U16Array(vec![8, 8, 8]).display(), "8 8 8");
    }
}
