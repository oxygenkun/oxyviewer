//! GIF parser (G1-G5).

use crate::core::{Error, Reader, Result};

/// Parsed GIF file metadata.
#[derive(Debug)]
pub struct GifInfo<'a> {
    /// GIF version: "87a" or "89a".
    pub version: &'a str,
    /// Logical screen width (G5).
    pub width: u16,
    /// Logical screen height (G5).
    pub height: u16,
    /// Global color table flag.
    pub has_global_color_table: bool,
    /// Background color index.
    pub background_color_index: u8,
    /// Pixel aspect ratio.
    pub pixel_aspect_ratio: u8,
    /// Comment extensions (G2).
    pub comments: Vec<String>,
    /// Application extensions (G3).
    pub app_extensions: Vec<AppExtension<'a>>,
    /// XMP data if found (G4).
    pub xmp_data: Option<&'a [u8]>,
}

/// A GIF application extension (G3).
#[derive(Debug, Clone)]
pub struct AppExtension<'a> {
    pub identifier: [u8; 8],
    pub auth_code: [u8; 3],
    pub data: &'a [u8],
}

impl<'a> AppExtension<'a> {
    pub fn identifier_str(&self) -> &str {
        std::str::from_utf8(&self.identifier)
            .unwrap_or("")
            .trim_end_matches('\0')
    }

    /// Is this a NETSCAPE animation extension?
    pub fn is_netscape(&self) -> bool {
        &self.identifier == b"NETSCAPE" && &self.auth_code == b"2.0"
    }
}

/// Parse a GIF file (G1-G5).
pub fn parse_gif<'a>(data: &'a [u8]) -> Result<GifInfo<'a>> {
    let mut reader = Reader::new(data);

    // G1: Header - "GIF87a" or "GIF89a"
    let header = reader.read_bytes(6)?;
    if &header[..3] != b"GIF" {
        return Err(Error::Format("not a GIF: invalid header".into()));
    }

    let version = std::str::from_utf8(&header[3..6])
        .map_err(|_| Error::Format("invalid GIF version".into()))?;
    if version != "87a" && version != "89a" {
        return Err(Error::Format(format!("unsupported GIF version: {version}")));
    }

    // G5: Logical screen descriptor
    let width = reader.read_u16_le()?;
    let height = reader.read_u16_le()?;
    let packed = reader.read_u8()?;
    let background_color_index = reader.read_u8()?;
    let pixel_aspect_ratio = reader.read_u8()?;

    let has_global_color_table = packed & 0x80 != 0;
    let color_table_size = if has_global_color_table {
        3 * (1 << ((packed & 0x07) + 1))
    } else {
        0
    };

    // Skip global color table
    reader.skip(color_table_size)?;

    let mut comments = Vec::new();
    let mut app_extensions = Vec::new();
    let mut xmp_data = None;

    // Parse blocks
    loop {
        if reader.remaining() == 0 {
            break;
        }

        let block_type = reader.read_u8()?;

        match block_type {
            // Image descriptor
            0x2C => {
                // Skip image: left(2) + top(2) + width(2) + height(2) + packed(1)
                reader.skip(8)?;
                let img_packed = reader.read_u8()?;
                let has_local_table = img_packed & 0x80 != 0;
                if has_local_table {
                    let local_size = 3 * (1 << ((img_packed & 0x07) + 1));
                    reader.skip(local_size)?;
                }
                // LZW minimum code size
                reader.skip(1)?;
                // Skip sub-blocks
                skip_sub_blocks(&mut reader)?;
            }

            // Extension
            0x21 => {
                if reader.remaining() == 0 {
                    break;
                }
                let label = reader.read_u8()?;

                match label {
                    // G2: Comment extension
                    0xFE => {
                        let comment_data = read_sub_blocks(&mut reader)?;
                        if let Ok(s) = String::from_utf8(comment_data) {
                            comments.push(s);
                        }
                    }

                    // G3: Application extension
                    0xFF => {
                        // Block size should be 11
                        let block_size = reader.read_u8()?;
                        if block_size >= 11 && reader.remaining() >= 11 {
                            let app_bytes = reader.read_bytes(11)?;
                            let mut identifier = [0u8; 8];
                            let mut auth_code = [0u8; 3];
                            identifier.copy_from_slice(&app_bytes[..8]);
                            auth_code.copy_from_slice(&app_bytes[8..11]);

                            // Skip remaining block_size - 11 bytes if any
                            if block_size > 11 {
                                reader.skip(block_size as usize - 11)?;
                            }

                            let ext_data_start = reader.position();
                            let _ext_data = read_sub_blocks(&mut reader)?;

                            // G4: XMP in application extension
                            if &identifier == b"XMP Data" && &auth_code == b"XMP" {
                                // XMP data follows - the raw bytes between the application
                                // extension header and the sub-block terminator contain XMP
                                xmp_data = Some(
                                    reader
                                        .slice(
                                            ext_data_start,
                                            reader
                                                .position()
                                                .saturating_sub(ext_data_start)
                                                .saturating_sub(1),
                                        )
                                        .unwrap_or(&[]),
                                );
                            }

                            app_extensions.push(AppExtension {
                                identifier,
                                auth_code,
                                data: reader
                                    .slice(
                                        ext_data_start,
                                        reader.position().saturating_sub(ext_data_start),
                                    )
                                    .unwrap_or(&[]),
                            });
                        } else {
                            skip_sub_blocks(&mut reader)?;
                        }
                    }

                    // Other extensions (graphic control, plain text)
                    _ => {
                        skip_sub_blocks(&mut reader)?;
                    }
                }
            }

            // Trailer
            0x3B => break,

            // Unknown - try to skip
            _ => break,
        }
    }

    // Reborrow version from original data
    let version_ref = std::str::from_utf8(&data[3..6]).unwrap();

    Ok(GifInfo {
        version: version_ref,
        width,
        height,
        has_global_color_table,
        background_color_index,
        pixel_aspect_ratio,
        comments,
        app_extensions,
        xmp_data,
    })
}

/// Read all data from sub-blocks into a Vec.
fn read_sub_blocks(reader: &mut Reader<'_>) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    loop {
        if reader.remaining() == 0 {
            break;
        }
        let size = reader.read_u8()? as usize;
        if size == 0 {
            break; // block terminator
        }
        let actual = size.min(reader.remaining());
        let block = reader.read_bytes(actual)?;
        data.extend_from_slice(block);
    }
    Ok(data)
}

/// Skip over sub-blocks without reading.
fn skip_sub_blocks(reader: &mut Reader<'_>) -> Result<()> {
    loop {
        if reader.remaining() == 0 {
            break;
        }
        let size = reader.read_u8()? as usize;
        if size == 0 {
            break;
        }
        let actual = size.min(reader.remaining());
        reader.skip(actual)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal GIF89a with logical screen descriptor.
    fn build_gif(width: u16, height: u16, extensions: &[u8]) -> Vec<u8> {
        let mut data = b"GIF89a".to_vec();
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        data.push(0x00); // packed: no global color table
        data.push(0); // background color index
        data.push(0); // pixel aspect ratio
        data.extend_from_slice(extensions);
        data.push(0x3B); // trailer
        data
    }

    #[test]
    fn g1_header_89a() {
        let data = build_gif(320, 240, &[]);
        let info = parse_gif(&data).unwrap();
        assert_eq!(info.version, "89a");
    }

    #[test]
    fn g1_header_87a() {
        let mut data = b"GIF87a".to_vec();
        data.extend_from_slice(&100u16.to_le_bytes());
        data.extend_from_slice(&100u16.to_le_bytes());
        data.push(0);
        data.push(0);
        data.push(0);
        data.push(0x3B);
        let info = parse_gif(&data).unwrap();
        assert_eq!(info.version, "87a");
    }

    #[test]
    fn g1_not_gif() {
        assert!(parse_gif(b"NOT GIF").is_err());
    }

    #[test]
    fn g2_comment() {
        let mut ext = vec![0x21, 0xFE]; // comment extension
        let comment = b"Hello World";
        ext.push(comment.len() as u8);
        ext.extend_from_slice(comment);
        ext.push(0); // terminator

        let data = build_gif(100, 100, &ext);
        let info = parse_gif(&data).unwrap();
        assert_eq!(info.comments, vec!["Hello World"]);
    }

    #[test]
    fn g3_app_extension_netscape() {
        let mut ext = vec![0x21, 0xFF]; // application extension
        ext.push(11); // block size
        ext.extend_from_slice(b"NETSCAPE2.0");
        // Sub-block data
        ext.push(3); // sub-block size
        ext.push(1); // sub-block ID
        ext.extend_from_slice(&0u16.to_le_bytes()); // loop count
        ext.push(0); // terminator

        let data = build_gif(100, 100, &ext);
        let info = parse_gif(&data).unwrap();
        assert_eq!(info.app_extensions.len(), 1);
        assert!(info.app_extensions[0].is_netscape());
    }

    #[test]
    fn g5_dimensions() {
        let data = build_gif(640, 480, &[]);
        let info = parse_gif(&data).unwrap();
        assert_eq!(info.width, 640);
        assert_eq!(info.height, 480);
    }
}
