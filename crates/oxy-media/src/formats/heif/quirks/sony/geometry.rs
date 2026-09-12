//! Bounded primary-item geometry and conservative Sony JPEG padding detection.

use image::{DynamicImage, GenericImageView};
use oxy_domain::{PreviewContentRect, PreviewDisplaySize, PreviewGeometry};

struct BoxView<'a> {
    kind: &'a [u8],
    body: &'a [u8],
}

fn boxes(mut bytes: &[u8]) -> Option<Vec<BoxView<'_>>> {
    let mut result = Vec::new();
    while !bytes.is_empty() {
        let kind = bytes.get(4..8)?;
        let size = number(bytes, 0, 4)?;
        let (size, header) = match size {
            0 => (bytes.len(), 8),
            1 => (usize::try_from(number(bytes, 8, 8)?).ok()?, 16),
            size => (usize::try_from(size).ok()?, 8),
        };
        result.push(BoxView {
            kind,
            body: bytes.get(header..size)?,
        });
        bytes = bytes.get(size..)?;
    }
    Some(result)
}

fn number(bytes: &[u8], start: usize, len: usize) -> Option<u64> {
    bytes
        .get(start..start.checked_add(len)?)?
        .iter()
        .try_fold(0_u64, |n, b| n.checked_mul(256)?.checked_add(u64::from(*b)))
}

/// Only consume a complete top-level meta box. The following mdat can extend
/// beyond the bounded prefix and must not force reading image payloads.
pub(super) fn primary(bytes: &[u8]) -> Option<(PreviewDisplaySize, u8)> {
    let mut offset = 0;
    let meta = loop {
        let size = number(bytes, offset, 4)?;
        let (size, header) = if size == 1 {
            (usize::try_from(number(bytes, offset + 8, 8)?).ok()?, 16)
        } else {
            (usize::try_from(size).ok()?, 8)
        };
        if size < header {
            return None;
        }
        let end = offset.checked_add(size)?;
        let data = bytes.get(offset + header..end)?;
        if bytes.get(offset + 4..offset + 8)? == b"meta" {
            break data;
        }
        offset = end;
    };
    let children = boxes(meta.get(4..)?)?;
    let pitm = children.iter().find(|b| b.kind == b"pitm")?.body;
    let id = number(
        pitm,
        4,
        match *pitm.first()? {
            0 => 2,
            1 => 4,
            _ => return None,
        },
    )?;
    let iprp = boxes(children.iter().find(|b| b.kind == b"iprp")?.body)?;
    let properties = boxes(iprp.iter().find(|b| b.kind == b"ipco")?.body)?;
    let mut associations = Vec::new();
    for ipma in iprp.iter().filter(|b| b.kind == b"ipma") {
        let data = ipma.body;
        let version = *data.first()?;
        if version > 1 {
            return None;
        }
        let flags = number(data, 1, 3)?;
        if flags > 1 {
            return None;
        }
        let wide = flags == 1;
        let mut cursor = 8;
        let entries = number(data, 4, 4)?;
        for _ in 0..entries {
            let id_size = if version == 0 { 2 } else { 4 };
            let item = number(data, cursor, id_size)?;
            cursor += id_size;
            let count = *data.get(cursor)?;
            cursor += 1;
            for _ in 0..count {
                let len = if wide { 2 } else { 1 };
                let index = number(data, cursor, len)? & if wide { 0x7fff } else { 0x7f };
                cursor += len;
                if item == id && index != 0 {
                    associations.push(properties.get(usize::try_from(index - 1).ok()?)?);
                }
            }
        }
    }
    let mut dimensions = None;
    let mut rotation = None;
    for property in associations {
        match property.kind {
            b"ispe" => {
                if dimensions.is_some() || property.body.len() != 12 {
                    return None;
                }
                dimensions = Some(PreviewDisplaySize {
                    width: u32::try_from(number(property.body, 4, 4)?).ok()?,
                    height: u32::try_from(number(property.body, 8, 4)?).ok()?,
                });
            }
            b"irot" => {
                if rotation.is_some() || property.body.len() != 1 || property.body[0] > 3 {
                    return None;
                }
                rotation = Some(property.body[0]);
            }
            // These transforms require additional coordinate handling.
            b"imir" | b"clap" => return None,
            _ => {}
        }
    }
    let dimensions = dimensions?;
    (dimensions.width > 0 && dimensions.height > 0).then_some((dimensions, rotation.unwrap_or(0)))
}

pub(super) fn detect(
    image: &DynamicImage,
    size: PreviewDisplaySize,
    turn: u8,
) -> Option<PreviewGeometry> {
    let (width, height) = image.dimensions();
    // The validated SHIF fast origin is a landscape 160x120 JPEG.
    // Other layouts stay on the existing path until real samples establish them.
    if (width, height) != (160, 120) || size.width < size.height {
        return None;
    }
    let mut rect = PreviewContentRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    if u64::from(width) * u64::from(size.height) > u64::from(height) * u64::from(size.width) {
        rect.width =
            u32::try_from(u64::from(height) * u64::from(size.width) / u64::from(size.height))
                .ok()?
                & !1;
        rect.x = (width - rect.width) / 2;
    } else {
        rect.height =
            u32::try_from(u64::from(width) * u64::from(size.height) / u64::from(size.width))
                .ok()?
                & !1;
        rect.y = (height - rect.height) / 2;
    }
    if rect.width < 32 || rect.height < 32 {
        return None;
    }
    let rgb = image.to_rgb8();
    let line = |vertical: bool, position: u32| {
        let length = if vertical { height } else { width };
        let mut sum = 0_u64;
        let mut peak = 0;
        for index in 0..length {
            let pixel = if vertical {
                rgb.get_pixel(position, index)
            } else {
                rgb.get_pixel(index, position)
            };
            for channel in pixel.0 {
                sum += u64::from(channel);
                peak = peak.max(channel);
            }
        }
        (sum as f64 / f64::from(length * 3), peak)
    };
    // Validate every inferred padding line, plus a visible transition on both
    // content edges. Ambiguous dark scenes are intentionally left untouched.
    for (vertical, start, end, limit) in [
        (true, rect.x, rect.x + rect.width, width),
        (false, rect.y, rect.y + rect.height, height),
    ] {
        if start == 0 && end == limit {
            continue;
        }
        for position in (0..start).chain(end..limit) {
            let (mean, peak) = line(vertical, position);
            if mean > 5.0 || peak > 24 {
                return None;
            }
        }
        for position in [start, end - 1] {
            let (mean, peak) = line(vertical, position);
            if mean < 12.0 || peak < 32 {
                return None;
            }
        }
    }
    let display_size = match turn {
        0 => size,
        1 => {
            rect = PreviewContentRect {
                x: rect.y,
                y: width - rect.x - rect.width,
                width: rect.height,
                height: rect.width,
            };
            PreviewDisplaySize {
                width: size.height,
                height: size.width,
            }
        }
        2 => {
            rect.x = width - rect.x - rect.width;
            rect.y = height - rect.y - rect.height;
            size
        }
        3 => {
            rect = PreviewContentRect {
                x: height - rect.y - rect.height,
                y: rect.x,
                width: rect.height,
                height: rect.width,
            };
            PreviewDisplaySize {
                width: size.height,
                height: size.width,
            }
        }
        _ => return None,
    };
    Some(PreviewGeometry {
        display_size,
        content_rect: rect,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = u32::try_from(body.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(body);
        bytes
    }

    fn dimensions(width: u32, height: u32) -> Vec<u8> {
        let mut body = vec![0; 4];
        body.extend_from_slice(&width.to_be_bytes());
        body.extend_from_slice(&height.to_be_bytes());
        atom(b"ispe", &body)
    }

    #[test]
    fn primary_uses_associations_instead_of_largest_size_or_first_rotation() {
        let properties = [
            dimensions(9999, 9999),
            atom(b"irot", &[1]),
            dimensions(7008, 4672),
            atom(b"irot", &[3]),
        ]
        .concat();
        // Primary item 7 references only properties 3 and 4. Cover both the
        // short and wide forms of item IDs and property associations.
        for wide in [false, true] {
            let pitm = if wide {
                vec![1, 0, 0, 0, 0, 0, 0, 7]
            } else {
                vec![0, 0, 0, 0, 0, 7]
            };
            let mut ipma = vec![u8::from(wide), 0, 0, u8::from(wide), 0, 0, 0, 1];
            ipma.extend_from_slice(if wide { &[0, 0, 0, 7] } else { &[0, 7] });
            ipma.push(2);
            ipma.extend_from_slice(if wide {
                &[0x80, 3, 0x80, 4]
            } else {
                &[0x83, 0x84]
            });
            let iprp = atom(
                b"iprp",
                &[atom(b"ipco", &properties), atom(b"ipma", &ipma)].concat(),
            );
            let bytes = atom(b"meta", &[vec![0; 4], atom(b"pitm", &pitm), iprp].concat());
            assert_eq!(
                primary(&bytes),
                Some((
                    PreviewDisplaySize {
                        width: 7008,
                        height: 4672
                    },
                    3
                ))
            );
            for end in 0..bytes.len() {
                assert!(primary(&bytes[..end]).is_none());
            }
        }
    }

    #[test]
    fn recognizes_square_and_wide_padding_but_rejects_nonblack_edges() {
        for (size, rect) in [
            (
                PreviewDisplaySize {
                    width: 4000,
                    height: 4000,
                },
                PreviewContentRect {
                    x: 20,
                    y: 0,
                    width: 120,
                    height: 120,
                },
            ),
            (
                PreviewDisplaySize {
                    width: 6400,
                    height: 3600,
                },
                PreviewContentRect {
                    x: 0,
                    y: 15,
                    width: 160,
                    height: 90,
                },
            ),
        ] {
            let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(160, 120, |x, y| {
                let inside = x >= rect.x
                    && x < rect.x + rect.width
                    && y >= rect.y
                    && y < rect.y + rect.height;
                image::Rgb(if inside { [80, 100, 120] } else { [0, 0, 0] })
            }));
            assert_eq!(detect(&image, size, 0).unwrap().content_rect, rect);
            let mut altered = image.to_rgb8();
            altered.put_pixel(0, 0, image::Rgb([90, 90, 90]));
            assert!(detect(&DynamicImage::ImageRgb8(altered), size, 0).is_none());
        }
    }

    #[test]
    fn detects_padding_and_rotates_geometry_without_reencoding() {
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(160, 120, |_, y| {
            image::Rgb(if (7..113).contains(&y) {
                [80, 100, 120]
            } else {
                [1, 2, 3]
            })
        }));
        for turn in 0..4 {
            let result = detect(
                &image,
                PreviewDisplaySize {
                    width: 7008,
                    height: 4672,
                },
                turn,
            )
            .unwrap();
            let rect = result.content_rect;
            if turn % 2 == 0 {
                assert_eq!((rect.x, rect.y, rect.width, rect.height), (0, 7, 160, 106));
            } else {
                assert_eq!((rect.x, rect.y, rect.width, rect.height), (7, 0, 106, 160));
                assert_eq!(result.display_size.width, 4672);
            }
        }
        assert!(
            detect(
                &DynamicImage::new_rgb8(160, 120),
                PreviewDisplaySize {
                    width: 7008,
                    height: 4672
                },
                0
            )
            .is_none()
        );
        let full = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            160,
            120,
            image::Rgb([80, 80, 80]),
        ));
        assert!(
            detect(
                &full,
                PreviewDisplaySize {
                    width: 7008,
                    height: 4672
                },
                0
            )
            .is_none()
        );
    }

    #[test]
    fn real_fixture_primary_and_padding() {
        let Some(path) = crate::sony_hif_fixture() else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let (size, turn) = primary(&bytes[..256 * 1024]).unwrap();
        assert_eq!((size.width, size.height, turn), (7008, 4672, 3));
        assert!(primary(&bytes[..1000]).is_none());
        let image = image::load_from_memory(&bytes[155648..155648 + 8294]).unwrap();
        let result = detect(&image, size, turn).unwrap();
        assert_eq!(
            result.content_rect,
            PreviewContentRect {
                x: 7,
                y: 0,
                width: 106,
                height: 160
            }
        );
    }
}
