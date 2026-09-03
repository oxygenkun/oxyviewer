//! File type detection via magic bytes (F3).

/// Detected file type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    Jpeg,
    Png,
    Gif,
    Bmp,
    Tiff,
    WebP,
    Heif,
    Pdf,
    Icc,
    QuickTime,
    // RAW camera formats
    /// Canon RAW v2 (TIFF-based, "CR" signature at offset 8)
    Cr2,
    /// Canon RAW v3 (ISOBMFF-based, ftyp brand "crx ")
    Cr3,
    /// Nikon Electronic Format (TIFF-based)
    Nef,
    /// Sony Alpha RAW (TIFF-based)
    Arw,
    /// Adobe Digital Negative (TIFF-based)
    Dng,
    /// Olympus RAW Format (TIFF-based, may have non-standard magic)
    Orf,
    /// Panasonic RAW 2 (TIFF variant, magic 0x55)
    Rw2,
    /// Pentax Electronic Format (TIFF-based)
    Pef,
    /// Fujifilm RAW Format (custom header with embedded JPEG)
    Raf,
    /// Samsung RAW (TIFF-based)
    Srw,
}

impl FileType {
    /// Detect file type from the first bytes of a file. Returns `None` if unrecognized.
    ///
    /// Requires at least 12 bytes for reliable detection; works with fewer for some formats.
    /// For TIFF-based RAW formats that share TIFF magic bytes (NEF, ARW, DNG, ORF, PEF, SRW),
    /// use [`FileType::detect_with_ext`] to disambiguate by file extension.
    pub fn detect(data: &[u8]) -> Option<FileType> {
        if data.len() < 4 {
            return None;
        }

        // JPEG: FF D8 FF
        if data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
            return Some(FileType::Jpeg);
        }

        // PNG: 89 50 4E 47 0D 0A 1A 0A
        if data.len() >= 8 && data[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
            return Some(FileType::Png);
        }

        // GIF: GIF87a or GIF89a
        if data.len() >= 6 && &data[..3] == b"GIF" {
            if &data[3..6] == b"87a" || &data[3..6] == b"89a" {
                return Some(FileType::Gif);
            }
        }

        // BMP: BM
        if data[0] == b'B' && data[1] == b'M' {
            return Some(FileType::Bmp);
        }

        // Fujifilm RAF: "FUJIFILM" at offset 0
        if data.len() >= 8 && &data[..8] == b"FUJIFILM" {
            return Some(FileType::Raf);
        }

        // TIFF-based formats (check specific RAW signatures before generic TIFF)
        if data.len() >= 4 {
            let is_tiff_le =
                data[0] == b'I' && data[1] == b'I' && data[2] == 0x2A && data[3] == 0x00;
            let is_tiff_be =
                data[0] == b'M' && data[1] == b'M' && data[2] == 0x00 && data[3] == 0x2A;

            // Canon CR2: TIFF + "CR" at offset 8
            if is_tiff_le && data.len() >= 10 && data[8] == b'C' && data[9] == b'R' {
                return Some(FileType::Cr2);
            }

            // Panasonic RW2: II 55 00 (non-standard TIFF magic)
            if data[0] == b'I' && data[1] == b'I' && data[2] == 0x55 && data[3] == 0x00 {
                return Some(FileType::Rw2);
            }

            // Olympus ORF: non-standard byte order marker OR/SR instead of 0x2A
            if data[0] == b'I' && data[1] == b'I' {
                let magic = u16::from_le_bytes([data[2], data[3]]);
                if magic == 0x4F52 || magic == 0x5352 {
                    // 'OR' or 'SR' - Olympus RAW
                    return Some(FileType::Orf);
                }
            }

            // Generic TIFF (including NEF, ARW, DNG, PEF, SRW - disambiguate with detect_with_ext)
            if is_tiff_le || is_tiff_be {
                return Some(FileType::Tiff);
            }

            // BigTIFF: II 2B 00 or MM 00 2B
            if data[0] == b'I' && data[1] == b'I' && data[2] == 0x2B && data[3] == 0x00 {
                return Some(FileType::Tiff);
            }
            if data[0] == b'M' && data[1] == b'M' && data[2] == 0x00 && data[3] == 0x2B {
                return Some(FileType::Tiff);
            }
        }

        // WebP: RIFF....WEBP
        if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
            return Some(FileType::WebP);
        }

        // HEIF/HEIC: ISOBMFF with ftyp box, brands: heic, heix, hevc, hevx, heim, heis,
        //   hevm, hevs, mif1, msf1, avif, avis
        if data.len() >= 12 && &data[4..8] == b"ftyp" {
            let brand = &data[8..12];
            if brand == b"heic"
                || brand == b"heix"
                || brand == b"hevc"
                || brand == b"hevx"
                || brand == b"heim"
                || brand == b"heis"
                || brand == b"hevm"
                || brand == b"hevs"
                || brand == b"mif1"
                || brand == b"msf1"
                || brand == b"avif"
                || brand == b"avis"
            {
                return Some(FileType::Heif);
            }
        }

        // Canon CR3: ISOBMFF with ftyp brand "crx "
        if data.len() >= 12 && &data[4..8] == b"ftyp" && &data[8..12] == b"crx " {
            return Some(FileType::Cr3);
        }

        // QuickTime/MP4/MOV: ISOBMFF with ftyp box, non-HEIF brands
        #[cfg(feature = "quicktime")]
        if data.len() >= 12 && &data[4..8] == b"ftyp" {
            // Any ftyp that wasn't caught by HEIF or CR3 above is likely MP4/MOV
            let brand: [u8; 4] = [data[8], data[9], data[10], data[11]];
            if crate::quicktime::is_quicktime_brand(&brand) {
                return Some(FileType::QuickTime);
            }
        }

        // PDF: %PDF-
        if data.len() >= 5 && &data[..5] == b"%PDF-" {
            return Some(FileType::Pdf);
        }

        // ICC profile: size (4 bytes) + 32 bytes + signature "acsp" at offset 36
        if data.len() >= 40 && &data[36..40] == b"acsp" {
            return Some(FileType::Icc);
        }

        None
    }

    /// Detect file type using both magic bytes and file extension hint.
    ///
    /// For TIFF-based RAW formats (NEF, ARW, DNG, PEF, SRW) that share identical
    /// magic bytes with plain TIFF, the extension is used to identify the specific format.
    pub fn detect_with_ext(data: &[u8], ext: &str) -> Option<FileType> {
        let detected = Self::detect(data)?;

        // Only refine TIFF detection - other formats have unique signatures
        if detected == FileType::Tiff {
            match ext.to_ascii_lowercase().as_str() {
                "nef" | "nrw" => return Some(FileType::Nef),
                "arw" | "srf" | "sr2" => return Some(FileType::Arw),
                "dng" => return Some(FileType::Dng),
                "orf" => return Some(FileType::Orf),
                "pef" => return Some(FileType::Pef),
                "srw" => return Some(FileType::Srw),
                "cr2" => return Some(FileType::Cr2), // fallback if CR signature missing
                "rw2" | "rwl" => return Some(FileType::Rw2),
                _ => {}
            }
        }

        Some(detected)
    }

    /// Whether this file type is a TIFF-based RAW format (parsed via TIFF/IFD).
    pub fn is_tiff_based(&self) -> bool {
        matches!(
            self,
            FileType::Tiff
                | FileType::Cr2
                | FileType::Nef
                | FileType::Arw
                | FileType::Dng
                | FileType::Orf
                | FileType::Rw2
                | FileType::Pef
                | FileType::Srw
        )
    }

    /// Whether this is a camera RAW format.
    pub fn is_raw(&self) -> bool {
        matches!(
            self,
            FileType::Cr2
                | FileType::Cr3
                | FileType::Nef
                | FileType::Arw
                | FileType::Dng
                | FileType::Orf
                | FileType::Rw2
                | FileType::Pef
                | FileType::Raf
                | FileType::Srw
        )
    }
}

impl std::fmt::Display for FileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileType::Jpeg => write!(f, "JPEG"),
            FileType::Png => write!(f, "PNG"),
            FileType::Gif => write!(f, "GIF"),
            FileType::Bmp => write!(f, "BMP"),
            FileType::Tiff => write!(f, "TIFF"),
            FileType::WebP => write!(f, "WebP"),
            FileType::Heif => write!(f, "HEIF/HEIC"),
            FileType::Pdf => write!(f, "PDF"),
            FileType::Icc => write!(f, "ICC"),
            FileType::QuickTime => write!(f, "QuickTime/MP4"),
            FileType::Cr2 => write!(f, "CR2"),
            FileType::Cr3 => write!(f, "CR3"),
            FileType::Nef => write!(f, "NEF"),
            FileType::Arw => write!(f, "ARW"),
            FileType::Dng => write!(f, "DNG"),
            FileType::Orf => write!(f, "ORF"),
            FileType::Rw2 => write!(f, "RW2"),
            FileType::Pef => write!(f, "PEF"),
            FileType::Raf => write!(f, "RAF"),
            FileType::Srw => write!(f, "SRW"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_jpeg() {
        assert_eq!(
            FileType::detect(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(FileType::Jpeg)
        );
        assert_eq!(
            FileType::detect(&[0xFF, 0xD8, 0xFF, 0xE1]),
            Some(FileType::Jpeg)
        );
    }

    #[test]
    fn detect_png() {
        let sig = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        ];
        assert_eq!(FileType::detect(&sig), Some(FileType::Png));
    }

    #[test]
    fn detect_gif87a() {
        assert_eq!(FileType::detect(b"GIF87a\x00\x00"), Some(FileType::Gif));
    }

    #[test]
    fn detect_gif89a() {
        assert_eq!(FileType::detect(b"GIF89a\x00\x00"), Some(FileType::Gif));
    }

    #[test]
    fn detect_bmp() {
        assert_eq!(FileType::detect(b"BM\x00\x00"), Some(FileType::Bmp));
    }

    #[test]
    fn detect_tiff_le() {
        assert_eq!(
            FileType::detect(&[b'I', b'I', 0x2A, 0x00]),
            Some(FileType::Tiff)
        );
    }

    #[test]
    fn detect_tiff_be() {
        assert_eq!(
            FileType::detect(&[b'M', b'M', 0x00, 0x2A]),
            Some(FileType::Tiff)
        );
    }

    #[test]
    fn detect_bigtiff() {
        assert_eq!(
            FileType::detect(&[b'I', b'I', 0x2B, 0x00]),
            Some(FileType::Tiff)
        );
        assert_eq!(
            FileType::detect(&[b'M', b'M', 0x00, 0x2B]),
            Some(FileType::Tiff)
        );
    }

    #[test]
    fn detect_webp() {
        let mut data = *b"RIFF\x00\x00\x00\x00WEBP";
        assert_eq!(FileType::detect(&data), Some(FileType::WebP));
        // Wrong fourcc
        data[8..12].copy_from_slice(b"AVI ");
        assert_ne!(FileType::detect(&data), Some(FileType::WebP));
    }

    #[test]
    fn detect_heif() {
        let mut data = [0u8; 12];
        data[4..8].copy_from_slice(b"ftyp");
        data[8..12].copy_from_slice(b"heic");
        assert_eq!(FileType::detect(&data), Some(FileType::Heif));

        data[8..12].copy_from_slice(b"avif");
        assert_eq!(FileType::detect(&data), Some(FileType::Heif));
    }

    #[test]
    fn detect_pdf() {
        assert_eq!(FileType::detect(b"%PDF-1.7\x00"), Some(FileType::Pdf));
    }

    #[test]
    fn detect_icc() {
        let mut data = [0u8; 44];
        data[36..40].copy_from_slice(b"acsp");
        assert_eq!(FileType::detect(&data), Some(FileType::Icc));
    }

    #[test]
    fn detect_cr2() {
        // CR2: II 2A 00 + offset bytes + "CR" at offset 8
        let mut data = [0u8; 12];
        data[0..2].copy_from_slice(b"II");
        data[2] = 0x2A;
        data[3] = 0x00;
        data[8] = b'C';
        data[9] = b'R';
        assert_eq!(FileType::detect(&data), Some(FileType::Cr2));
    }

    #[test]
    fn detect_rw2() {
        // RW2: II 55 00
        assert_eq!(
            FileType::detect(&[b'I', b'I', 0x55, 0x00]),
            Some(FileType::Rw2)
        );
    }

    #[test]
    fn detect_orf() {
        // ORF with 'OR' magic
        let data = [b'I', b'I', 0x52, 0x4F]; // 0x4F52 LE = 'OR'
        assert_eq!(FileType::detect(&data), Some(FileType::Orf));
    }

    #[test]
    fn detect_raf() {
        assert_eq!(
            FileType::detect(b"FUJIFILMCCD-RAW 0201"),
            Some(FileType::Raf)
        );
    }

    #[test]
    fn detect_cr3() {
        let mut data = [0u8; 12];
        data[4..8].copy_from_slice(b"ftyp");
        data[8..12].copy_from_slice(b"crx ");
        assert_eq!(FileType::detect(&data), Some(FileType::Cr3));
    }

    #[test]
    fn detect_with_ext_nef() {
        // Plain TIFF magic, but .nef extension -> NEF
        let data = [b'I', b'I', 0x2A, 0x00];
        assert_eq!(FileType::detect_with_ext(&data, "nef"), Some(FileType::Nef));
        assert_eq!(FileType::detect_with_ext(&data, "NEF"), Some(FileType::Nef));
        // Without extension hint, stays TIFF
        assert_eq!(FileType::detect(&data), Some(FileType::Tiff));
    }

    #[test]
    fn detect_with_ext_dng() {
        let data = [b'M', b'M', 0x00, 0x2A];
        assert_eq!(FileType::detect_with_ext(&data, "dng"), Some(FileType::Dng));
    }

    #[test]
    fn detect_with_ext_arw() {
        let data = [b'I', b'I', 0x2A, 0x00];
        assert_eq!(FileType::detect_with_ext(&data, "arw"), Some(FileType::Arw));
    }

    #[test]
    fn detect_unknown() {
        assert_eq!(
            FileType::detect(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            None
        );
    }

    #[test]
    fn detect_too_short() {
        assert_eq!(FileType::detect(&[0xFF, 0xD8]), None);
    }

    #[test]
    fn display() {
        assert_eq!(FileType::Jpeg.to_string(), "JPEG");
        assert_eq!(FileType::Pdf.to_string(), "PDF");
        assert_eq!(FileType::Cr2.to_string(), "CR2");
        assert_eq!(FileType::Nef.to_string(), "NEF");
        assert_eq!(FileType::Raf.to_string(), "RAF");
    }

    #[test]
    fn is_raw() {
        assert!(FileType::Cr2.is_raw());
        assert!(FileType::Raf.is_raw());
        assert!(!FileType::Jpeg.is_raw());
        assert!(!FileType::Tiff.is_raw());
    }

    #[test]
    fn is_tiff_based() {
        assert!(FileType::Cr2.is_tiff_based());
        assert!(FileType::Nef.is_tiff_based());
        assert!(FileType::Tiff.is_tiff_based());
        assert!(!FileType::Raf.is_tiff_based());
        assert!(!FileType::Cr3.is_tiff_based());
    }
}
