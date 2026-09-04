//! IPTC IIM parser (I1-I5).
//!
//! Parses IPTC-IIM datasets from JPEG APP13 Photoshop IRB segments.

use crate::core::Result;

/// A parsed IPTC dataset.
#[derive(Debug, Clone)]
pub struct Dataset {
    /// Record number (usually 2 for Application Record).
    pub record: u8,
    /// Dataset number within the record.
    pub dataset: u8,
    /// Raw value bytes.
    pub value: Vec<u8>,
}

impl Dataset {
    /// Dataset value as UTF-8 string.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.value).ok()
    }

    /// Dataset value as string, treating non-UTF8 bytes as Latin-1.
    pub fn as_string_lossy(&self) -> String {
        match std::str::from_utf8(&self.value) {
            Ok(s) => s.to_string(),
            Err(_) => self.value.iter().map(|&b| b as char).collect(),
        }
    }

    /// Human-readable name for Application Record (record 2) datasets.
    pub fn name(&self) -> &'static str {
        if self.record == 2 {
            app_record_name(self.dataset)
        } else if self.record == 1 && self.dataset == 90 {
            "CodedCharacterSet"
        } else {
            "Unknown"
        }
    }
}

/// Parsed IPTC data from a JPEG file.
#[derive(Debug, Clone)]
pub struct IptcData {
    /// All parsed datasets.
    pub datasets: Vec<Dataset>,
    /// Character encoding detected from record 1:90.
    pub encoding: IptcEncoding,
}

/// Character encoding for IPTC text - I5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IptcEncoding {
    /// Latin-1 (default when no coded character set is specified).
    Latin1,
    /// UTF-8 (ESC sequence 0x1B 0x25 0x47).
    Utf8,
    /// Other/unknown.
    Other,
}

impl IptcData {
    /// Get all values for a given record:dataset pair.
    pub fn values(&self, record: u8, dataset: u8) -> Vec<&Dataset> {
        self.datasets
            .iter()
            .filter(|d| d.record == record && d.dataset == dataset)
            .collect()
    }

    /// Get a single value for a given record:dataset pair.
    pub fn value(&self, record: u8, dataset: u8) -> Option<&Dataset> {
        self.datasets
            .iter()
            .find(|d| d.record == record && d.dataset == dataset)
    }

    /// Get a single value as string.
    pub fn value_str(&self, record: u8, dataset: u8) -> Option<&str> {
        self.value(record, dataset)?.as_str()
    }

    /// Get all keyword values (2:25) - I4.
    pub fn keywords(&self) -> Vec<&str> {
        self.values(2, 25)
            .iter()
            .filter_map(|d| d.as_str())
            .collect()
    }

    /// Get caption/abstract (2:120).
    pub fn caption(&self) -> Option<&str> {
        self.value_str(2, 120)
    }

    /// Get byline/author (2:80).
    pub fn byline(&self) -> Option<&str> {
        self.value_str(2, 80)
    }

    /// Get headline (2:105).
    pub fn headline(&self) -> Option<&str> {
        self.value_str(2, 105)
    }

    /// Get city (2:90).
    pub fn city(&self) -> Option<&str> {
        self.value_str(2, 90)
    }

    /// Get country name (2:101).
    pub fn country(&self) -> Option<&str> {
        self.value_str(2, 101)
    }

    /// Get object name / title (2:5).
    pub fn object_name(&self) -> Option<&str> {
        self.value_str(2, 5)
    }
}

/// I1: Locate and extract IPTC data from a JPEG APP13 segment.
///
/// The `app13_data` should be the segment payload (after the segment header).
/// APP13 contains Photoshop IRBs (Image Resource Blocks), and IPTC is in
/// resource ID 0x0404.
pub fn extract_from_app13(app13_data: &[u8]) -> Option<Vec<u8>> {
    // Check for "Photoshop 3.0\0" header (14 bytes)
    let header = b"Photoshop 3.0\0";
    if app13_data.len() < header.len() || &app13_data[..header.len()] != header {
        return None;
    }

    let mut pos = header.len();

    // Parse IRBs (Image Resource Blocks)
    while pos + 12 <= app13_data.len() {
        // IRB signature: "8BIM"
        if &app13_data[pos..pos + 4] != b"8BIM" {
            break;
        }
        pos += 4;

        // Resource ID (2 bytes, big-endian)
        if pos + 2 > app13_data.len() {
            break;
        }
        let resource_id = u16::from_be_bytes([app13_data[pos], app13_data[pos + 1]]);
        pos += 2;

        // Pascal string (name): first byte is length, then that many bytes, padded to even
        if pos >= app13_data.len() {
            break;
        }
        let name_len = app13_data[pos] as usize;
        pos += 1;
        let padded_name_len = if (name_len + 1) % 2 == 0 {
            name_len
        } else {
            name_len + 1
        };
        pos += padded_name_len;

        // Data size (4 bytes, big-endian)
        if pos + 4 > app13_data.len() {
            break;
        }
        let data_size = u32::from_be_bytes([
            app13_data[pos],
            app13_data[pos + 1],
            app13_data[pos + 2],
            app13_data[pos + 3],
        ]) as usize;
        pos += 4;

        // Resource data
        if pos + data_size > app13_data.len() {
            break;
        }

        if resource_id == 0x0404 {
            // I1: Found IPTC data!
            return Some(app13_data[pos..pos + data_size].to_vec());
        }

        pos += data_size;
        // Data is padded to even size
        if data_size % 2 == 1 {
            pos += 1;
        }
    }

    None
}

/// I2: Parse IPTC dataset structure from raw IPTC bytes.
pub fn parse_iptc(data: &[u8]) -> Result<IptcData> {
    let mut datasets = Vec::new();
    let mut pos = 0;

    while pos + 5 <= data.len() {
        // I2: Tag marker 0x1C
        if data[pos] != 0x1C {
            break;
        }

        let record = data[pos + 1];
        let dataset = data[pos + 2];
        let size = u16::from_be_bytes([data[pos + 3], data[pos + 4]]) as usize;
        pos += 5;

        // Extended dataset size (size > 32767): first bit is set
        // For simplicity, handle standard size only (covers 99%+ of real files)
        if pos + size > data.len() {
            break;
        }

        let value = data[pos..pos + size].to_vec();
        datasets.push(Dataset {
            record,
            dataset,
            value,
        });
        pos += size;
    }

    // I5: Detect encoding from record 1:90 (Coded Character Set)
    let encoding = detect_encoding(&datasets);

    Ok(IptcData { datasets, encoding })
}

/// I5: Detect character encoding from coded character set dataset.
fn detect_encoding(datasets: &[Dataset]) -> IptcEncoding {
    for ds in datasets {
        if ds.record == 1 && ds.dataset == 90 {
            // UTF-8 escape sequence: ESC % G (0x1B 0x25 0x47)
            if ds.value.len() >= 3
                && ds.value[0] == 0x1B
                && ds.value[1] == 0x25
                && ds.value[2] == 0x47
            {
                return IptcEncoding::Utf8;
            }
            return IptcEncoding::Other;
        }
    }
    IptcEncoding::Latin1
}

/// I3: Human-readable name for Application Record 2 datasets.
fn app_record_name(dataset: u8) -> &'static str {
    match dataset {
        0 => "ApplicationRecordVersion",
        3 => "ObjectTypeReference",
        4 => "ObjectAttributeReference",
        5 => "ObjectName",
        7 => "EditStatus",
        8 => "EditorialUpdate",
        10 => "Urgency",
        12 => "SubjectReference",
        15 => "Category",
        20 => "SupplementalCategories",
        22 => "FixtureIdentifier",
        25 => "Keywords",
        26 => "ContentLocationCode",
        27 => "ContentLocationName",
        30 => "ReleaseDate",
        35 => "ReleaseTime",
        37 => "ExpirationDate",
        38 => "ExpirationTime",
        40 => "SpecialInstructions",
        42 => "ActionAdvised",
        45 => "ReferenceService",
        47 => "ReferenceDate",
        50 => "ReferenceNumber",
        55 => "DateCreated",
        60 => "TimeCreated",
        62 => "DigitalCreationDate",
        63 => "DigitalCreationTime",
        65 => "OriginatingProgram",
        70 => "ProgramVersion",
        75 => "ObjectCycle",
        80 => "By-line",
        85 => "By-lineTitle",
        90 => "City",
        92 => "Sub-location",
        95 => "Province-State",
        100 => "Country-PrimaryLocationCode",
        101 => "Country-PrimaryLocationName",
        103 => "OriginalTransmissionReference",
        105 => "Headline",
        110 => "Credit",
        115 => "Source",
        116 => "CopyrightNotice",
        118 => "Contact",
        120 => "Caption-Abstract",
        122 => "Writer-Editor",
        125 => "RasterizedCaption",
        130 => "ImageType",
        131 => "ImageOrientation",
        135 => "LanguageIdentifier",
        150 => "AudioType",
        151 => "AudioSamplingRate",
        152 => "AudioSamplingResolution",
        153 => "AudioDuration",
        154 => "AudioOutcue",
        200 => "ObjectPreviewFileFormat",
        201 => "ObjectPreviewFileVersion",
        202 => "ObjectPreviewData",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_iptc_dataset(record: u8, dataset: u8, value: &[u8]) -> Vec<u8> {
        let mut data = vec![0x1C, record, dataset];
        data.extend_from_slice(&(value.len() as u16).to_be_bytes());
        data.extend_from_slice(value);
        data
    }

    fn build_iptc_datasets(entries: &[(u8, u8, &[u8])]) -> Vec<u8> {
        let mut data = Vec::new();
        for &(rec, ds, val) in entries {
            data.extend_from_slice(&build_iptc_dataset(rec, ds, val));
        }
        data
    }

    fn build_app13(iptc_data: &[u8]) -> Vec<u8> {
        let mut data = b"Photoshop 3.0\0".to_vec();
        // IRB: 8BIM + resource ID 0x0404 + pascal string (empty) + data size + data
        data.extend_from_slice(b"8BIM");
        data.extend_from_slice(&0x0404u16.to_be_bytes());
        data.push(0); // pascal string length = 0
        data.push(0); // padding to even
        data.extend_from_slice(&(iptc_data.len() as u32).to_be_bytes());
        data.extend_from_slice(iptc_data);
        data
    }

    #[test]
    fn i1_extract_from_app13() {
        let iptc = build_iptc_datasets(&[(2, 5, b"Test Title")]);
        let app13 = build_app13(&iptc);
        let extracted = extract_from_app13(&app13).unwrap();
        assert_eq!(extracted, iptc);
    }

    #[test]
    fn i1_no_photoshop_header() {
        assert!(extract_from_app13(b"NOT PHOTOSHOP").is_none());
    }

    #[test]
    fn i1_no_iptc_resource() {
        let mut data = b"Photoshop 3.0\0".to_vec();
        data.extend_from_slice(b"8BIM");
        data.extend_from_slice(&0x0400u16.to_be_bytes()); // wrong resource ID
        data.push(0);
        data.push(0);
        data.extend_from_slice(&4u32.to_be_bytes());
        data.extend_from_slice(b"test");
        assert!(extract_from_app13(&data).is_none());
    }

    #[test]
    fn i2_parse_dataset_structure() {
        let data = build_iptc_datasets(&[
            (2, 5, b"My Title"),
            (2, 80, b"John Doe"),
            (2, 120, b"A photo caption"),
        ]);
        let iptc = parse_iptc(&data).unwrap();
        assert_eq!(iptc.datasets.len(), 3);
        assert_eq!(iptc.datasets[0].record, 2);
        assert_eq!(iptc.datasets[0].dataset, 5);
        assert_eq!(iptc.datasets[0].as_str(), Some("My Title"));
    }

    #[test]
    fn i3_app_record_names() {
        let data = build_iptc_datasets(&[
            (2, 5, b"Title"),
            (2, 25, b"keyword1"),
            (2, 80, b"Author"),
            (2, 90, b"City"),
            (2, 101, b"Country"),
            (2, 105, b"Headline"),
            (2, 116, b"Copyright"),
            (2, 120, b"Caption"),
        ]);
        let iptc = parse_iptc(&data).unwrap();
        assert_eq!(iptc.datasets[0].name(), "ObjectName");
        assert_eq!(iptc.datasets[1].name(), "Keywords");
        assert_eq!(iptc.datasets[2].name(), "By-line");
        assert_eq!(iptc.datasets[3].name(), "City");
        assert_eq!(iptc.datasets[4].name(), "Country-PrimaryLocationName");
        assert_eq!(iptc.datasets[5].name(), "Headline");
        assert_eq!(iptc.datasets[6].name(), "CopyrightNotice");
        assert_eq!(iptc.datasets[7].name(), "Caption-Abstract");
    }

    #[test]
    fn i3_helper_methods() {
        let data = build_iptc_datasets(&[
            (2, 5, b"My Photo"),
            (2, 80, b"Jane Doe"),
            (2, 105, b"Sunset"),
            (2, 90, b"New York"),
            (2, 101, b"USA"),
            (2, 120, b"Beautiful sunset over NYC"),
        ]);
        let iptc = parse_iptc(&data).unwrap();
        assert_eq!(iptc.object_name(), Some("My Photo"));
        assert_eq!(iptc.byline(), Some("Jane Doe"));
        assert_eq!(iptc.headline(), Some("Sunset"));
        assert_eq!(iptc.city(), Some("New York"));
        assert_eq!(iptc.country(), Some("USA"));
        assert_eq!(iptc.caption(), Some("Beautiful sunset over NYC"));
    }

    #[test]
    fn i4_repeatable_keywords() {
        let data = build_iptc_datasets(&[
            (2, 25, b"sunset"),
            (2, 25, b"landscape"),
            (2, 25, b"nature"),
        ]);
        let iptc = parse_iptc(&data).unwrap();
        let kw = iptc.keywords();
        assert_eq!(kw, vec!["sunset", "landscape", "nature"]);
    }

    #[test]
    fn i5_encoding_utf8() {
        let data = build_iptc_datasets(&[
            (1, 90, &[0x1B, 0x25, 0x47]), // ESC % G = UTF-8
            (2, 120, b"UTF-8 caption"),
        ]);
        let iptc = parse_iptc(&data).unwrap();
        assert_eq!(iptc.encoding, IptcEncoding::Utf8);
    }

    #[test]
    fn i5_encoding_default_latin1() {
        let data = build_iptc_datasets(&[(2, 120, b"No charset tag")]);
        let iptc = parse_iptc(&data).unwrap();
        assert_eq!(iptc.encoding, IptcEncoding::Latin1);
    }

    #[test]
    fn i2_empty_input() {
        let iptc = parse_iptc(&[]).unwrap();
        assert!(iptc.datasets.is_empty());
    }

    #[test]
    fn i2_truncated_dataset() {
        // Only 3 bytes - too short for a complete dataset
        let data = [0x1C, 2, 5];
        let iptc = parse_iptc(&data).unwrap();
        assert!(iptc.datasets.is_empty());
    }
}
