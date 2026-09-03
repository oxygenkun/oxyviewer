//! XMP XML parser (X1-X12).
//!
//! Extracts XMP metadata properties from RDF/XML embedded in JPEG, PNG, PDF, etc.
//! Implements a minimal XML parser targeting the XMP/RDF subset - no external
//! dependency needed.

use crate::core::Result;

/// Well-known XMP namespace URIs.
pub mod ns {
    pub const DC: &str = "http://purl.org/dc/elements/1.1/";
    pub const XMP: &str = "http://ns.adobe.com/xap/1.0/";
    pub const EXIF: &str = "http://ns.adobe.com/exif/1.0/";
    pub const TIFF: &str = "http://ns.adobe.com/tiff/1.0/";
    pub const PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";
    pub const XMP_MM: &str = "http://ns.adobe.com/xap/1.0/mm/";
    pub const XMP_RIGHTS: &str = "http://ns.adobe.com/xap/1.0/rights/";
    pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    pub const XML: &str = "http://www.w3.org/XML/1998/namespace";
}

/// JPEG APP1 XMP prefix - X1.
pub const JPEG_XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// JPEG APP1 extended XMP prefix - X9.
pub const JPEG_XMP_EXT_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";

/// PNG XMP text chunk key - X2.
pub const PNG_XMP_KEY: &str = "XML:com.adobe.xmp";

/// Parsed XMP document.
#[derive(Debug, Clone)]
pub struct XmpData {
    /// All extracted properties (namespace, name, value).
    pub properties: Vec<XmpProperty>,
}

/// A single XMP property.
#[derive(Debug, Clone)]
pub struct XmpProperty {
    /// Namespace URI (e.g., `http://purl.org/dc/elements/1.1/`).
    pub namespace: String,
    /// Local property name (e.g., `title`, `creator`).
    pub name: String,
    /// Property value.
    pub value: XmpValue,
}

/// XMP property value.
#[derive(Debug, Clone, PartialEq)]
pub enum XmpValue {
    /// Simple text value.
    Simple(String),
    /// Language alternative (X10): vec of (lang, value) pairs.
    LangAlt(Vec<(String, String)>),
    /// Ordered array (X11): rdf:Seq.
    OrderedArray(Vec<String>),
    /// Unordered array (X11): rdf:Bag.
    UnorderedArray(Vec<String>),
    /// Struct (X12): nested properties.
    Struct(Vec<(String, String)>),
}

impl XmpValue {
    /// Get the primary text value (first item for arrays, x-default for lang alts).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            XmpValue::Simple(s) => Some(s),
            XmpValue::LangAlt(items) => {
                // Prefer x-default, else first
                items
                    .iter()
                    .find(|(lang, _)| lang == "x-default")
                    .or_else(|| items.first())
                    .map(|(_, v)| v.as_str())
            }
            XmpValue::OrderedArray(items) | XmpValue::UnorderedArray(items) => {
                items.first().map(|s| s.as_str())
            }
            XmpValue::Struct(_) => None,
        }
    }

    /// Get all string values (flattens arrays).
    pub fn all_strings(&self) -> Vec<&str> {
        match self {
            XmpValue::Simple(s) => vec![s.as_str()],
            XmpValue::LangAlt(items) => items.iter().map(|(_, v)| v.as_str()).collect(),
            XmpValue::OrderedArray(items) | XmpValue::UnorderedArray(items) => {
                items.iter().map(|s| s.as_str()).collect()
            }
            XmpValue::Struct(_) => vec![],
        }
    }
}

impl XmpData {
    /// Find a property by namespace and name.
    pub fn get(&self, namespace: &str, name: &str) -> Option<&XmpProperty> {
        self.properties
            .iter()
            .find(|p| p.namespace == namespace && p.name == name)
    }

    /// Get a simple string value.
    pub fn get_str(&self, namespace: &str, name: &str) -> Option<&str> {
        self.get(namespace, name)?.value.as_str()
    }

    /// X5: Dublin Core properties.
    pub fn dc_title(&self) -> Option<&str> {
        self.get_str(ns::DC, "title")
    }
    pub fn dc_creator(&self) -> Option<&str> {
        self.get_str(ns::DC, "creator")
    }
    pub fn dc_description(&self) -> Option<&str> {
        self.get_str(ns::DC, "description")
    }
    pub fn dc_subject(&self) -> Vec<&str> {
        self.get(ns::DC, "subject")
            .map(|p| p.value.all_strings())
            .unwrap_or_default()
    }

    /// X6: EXIF namespace.
    pub fn exif_value(&self, name: &str) -> Option<&str> {
        self.get_str(ns::EXIF, name)
    }

    /// X7: TIFF namespace.
    pub fn tiff_value(&self, name: &str) -> Option<&str> {
        self.get_str(ns::TIFF, name)
    }

    /// X8: Photoshop namespace.
    pub fn photoshop_value(&self, name: &str) -> Option<&str> {
        self.get_str(ns::PHOTOSHOP, name)
    }
}

/// X1/X2/X3: Locate XMP data in raw bytes.
/// Returns the XMP XML as a string slice.
pub fn locate_xmp(data: &[u8]) -> Option<&str> {
    // Look for <?xpacket or <x:xmpmeta or <rdf:RDF
    let markers: &[&[u8]] = &[b"<?xpacket begin", b"<x:xmpmeta", b"<rdf:RDF"];
    for marker in markers {
        if let Some(start) = data.windows(marker.len()).position(|w| w == *marker) {
            // Find end
            let end = find_xmp_end(&data[start..])
                .map(|e| start + e)
                .unwrap_or(data.len());
            return std::str::from_utf8(&data[start..end]).ok();
        }
    }
    None
}

fn find_xmp_end(data: &[u8]) -> Option<usize> {
    // Look for <?xpacket end
    let end_marker = b"<?xpacket end";
    if let Some(pos) = data.windows(end_marker.len()).position(|w| w == end_marker) {
        // Find closing ?>
        if let Some(gt) = data[pos..].iter().position(|&b| b == b'>') {
            return Some(pos + gt + 1);
        }
    }
    // Look for </x:xmpmeta>
    let end2 = b"</x:xmpmeta>";
    if let Some(pos) = data.windows(end2.len()).position(|w| w == end2) {
        return Some(pos + end2.len());
    }
    // Look for </rdf:RDF>
    let end3 = b"</rdf:RDF>";
    if let Some(pos) = data.windows(end3.len()).position(|w| w == end3) {
        return Some(pos + end3.len());
    }
    None
}

/// X4: Parse XMP XML string into structured properties.
pub fn parse_xmp(xml: &str) -> Result<XmpData> {
    let mut properties = Vec::new();

    // Parse namespace declarations and extract rdf:Description blocks
    // Collect namespace prefixes
    let ns_map = extract_namespaces(xml);

    // Extract x:xmptk (or x:xaptk) from <x:xmpmeta> (or <x:xapmeta>) element
    let meta_tags = [("<x:xmpmeta", "x:xmptk="), ("<x:xapmeta", "x:xaptk=")];
    for (meta_needle, tk_needle) in meta_tags {
        if let Some(meta_start) = xml.find(meta_needle) {
            let meta_end = xml[meta_start..]
                .find('>')
                .map(|e| meta_start + e)
                .unwrap_or(xml.len());
            let meta_tag = &xml[meta_start..meta_end + 1];
            if let Some(tk_pos) = meta_tag.find(tk_needle) {
                if let Some(value) = extract_quoted_value(&meta_tag[tk_pos + tk_needle.len()..]) {
                    properties.push(XmpProperty {
                        namespace: "adobe:ns:meta/".to_string(),
                        name: "XMPToolkit".to_string(),
                        value: XmpValue::Simple(decode_xml_entities(&value)),
                    });
                }
            }
            break;
        }
    }

    // Find rdf:Description elements and extract their attributes + child elements
    let mut pos = 0;
    while pos < xml.len() {
        if let Some(desc_start) = xml[pos..].find("<rdf:Description") {
            let abs_start = pos + desc_start;

            // Find the end of this element (could be self-closing or have children)
            if let Some((desc_content, desc_end)) =
                find_element_content(xml, abs_start, "rdf:Description")
            {
                // Extract attributes from the opening tag
                let tag_end = xml[abs_start..]
                    .find('>')
                    .map(|e| abs_start + e)
                    .unwrap_or(desc_end);
                let opening_tag = &xml[abs_start..tag_end + 1];

                extract_attributes(opening_tag, &ns_map, &mut properties);

                // Extract rdf:about or about attribute
                let about_search = [("rdf:about=", 10), (" about=", 7)];
                for (pattern, skip) in about_search {
                    if let Some(about_pos) = opening_tag.find(pattern) {
                        if let Some(value) = extract_quoted_value(&opening_tag[about_pos + skip..])
                        {
                            let value = value.trim().to_string();
                            if !value.is_empty() {
                                properties.push(XmpProperty {
                                    namespace: "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                                        .to_string(),
                                    name: "About".to_string(),
                                    value: XmpValue::Simple(value),
                                });
                            }
                        }
                        break;
                    }
                }

                // Extract child elements
                if !desc_content.is_empty() {
                    extract_child_elements(desc_content, &ns_map, &mut properties);
                }

                pos = desc_end;
            } else {
                pos = abs_start + 17; // skip past "<rdf:Description"
            }
        } else {
            break;
        }
    }

    Ok(XmpData { properties })
}

/// X9: Reassemble extended XMP from multiple JPEG APP1 segments.
///
/// Each segment's raw payload (after the `http://ns.adobe.com/xmp/extension/\0`
/// header) has the format:
///   - 32 bytes: MD5 GUID (hex ASCII)
///   - 4 bytes:  total extended data length (big-endian u32)
///   - 4 bytes:  this chunk's offset within the full data (big-endian u32)
///   - remaining: XMP data chunk
pub fn reassemble_extended_xmp(segments: &[(Vec<u8>, [u8; 32])]) -> Option<String> {
    if segments.is_empty() {
        return None;
    }

    // Sort by offset (if embedded in the data)
    // For now, just concatenate in order
    let mut result = Vec::new();
    for (data, _guid) in segments {
        result.extend_from_slice(data);
    }

    String::from_utf8(result).ok()
}

/// Extract and reassemble extended XMP directly from JPEG APP1 segment payloads.
///
/// Pass the raw `data` slice from each `Segment` with `App1Kind::ExtendedXmp`.
/// This function parses the 32-byte GUID + 4-byte total + 4-byte offset header,
/// sorts chunks by offset, validates GUID consistency, and returns the assembled XML.
pub fn reassemble_extended_xmp_from_segments(segment_payloads: &[&[u8]]) -> Option<String> {
    if segment_payloads.is_empty() {
        return None;
    }

    const EXT_HEADER_LEN: usize = 35; // b"http://ns.adobe.com/xmp/extension/\0"
    const CHUNK_HEADER_LEN: usize = 32 + 4 + 4; // GUID + total_len + offset

    struct Chunk<'a> {
        guid: &'a [u8],
        #[allow(dead_code)]
        total_len: u32,
        offset: u32,
        data: &'a [u8],
    }

    let mut chunks = Vec::with_capacity(segment_payloads.len());

    for &payload in segment_payloads {
        // Skip the extension header if present
        let rest = if payload.starts_with(JPEG_XMP_EXT_HEADER) {
            &payload[EXT_HEADER_LEN..]
        } else {
            payload
        };

        if rest.len() < CHUNK_HEADER_LEN {
            continue;
        }

        let guid = &rest[..32];
        let total_len = u32::from_be_bytes([rest[32], rest[33], rest[34], rest[35]]);
        let offset = u32::from_be_bytes([rest[36], rest[37], rest[38], rest[39]]);
        let data = &rest[CHUNK_HEADER_LEN..];

        chunks.push(Chunk {
            guid,
            total_len,
            offset,
            data,
        });
    }

    if chunks.is_empty() {
        return None;
    }

    // Validate: all GUIDs should match
    let first_guid = chunks[0].guid;
    if !chunks.iter().all(|c| c.guid == first_guid) {
        return None; // GUID mismatch across segments
    }

    // Sort by offset
    chunks.sort_by_key(|c| c.offset);

    // Assemble
    let total = chunks[0].total_len as usize;
    let mut result = Vec::with_capacity(total);
    for chunk in &chunks {
        result.extend_from_slice(chunk.data);
    }

    String::from_utf8(result).ok()
}

// -- Internal XML helpers ------------------------------------------------

/// Extract namespace prefix -> URI mappings from the XML.
fn extract_namespaces(xml: &str) -> Vec<(String, String)> {
    let mut ns_map = Vec::new();

    // Match xmlns:prefix="uri" patterns
    let mut pos = 0;
    while let Some(idx) = xml[pos..].find("xmlns:") {
        let abs = pos + idx + 6;
        if let Some(eq) = xml[abs..].find('=') {
            let prefix = xml[abs..abs + eq].trim().to_string();
            let after_eq = abs + eq + 1;
            if let Some(uri) = extract_quoted_value(&xml[after_eq..]) {
                ns_map.push((prefix, uri));
            }
        }
        pos = abs + 1;
    }

    ns_map
}

/// Resolve a prefixed name (e.g., "dc:title") to (namespace_uri, local_name).
///
/// The local name is UNESCAPED on the way out (see [`decode_xml_name_escapes`]),
/// so every consumer sees the property's real name. Callers that need to match
/// the closing tag must keep using the raw text - this returns the decoded form.
fn resolve_prefixed_name<'a>(
    name: &str,
    ns_map: &'a [(String, String)],
) -> Option<(String, String)> {
    if let Some(colon) = name.find(':') {
        let prefix = &name[..colon];
        let local = &name[colon + 1..];
        // Skip rdf: and xml: attributes
        if prefix == "rdf" || prefix == "xml" || prefix == "xmlns" {
            return None;
        }
        let local = decode_xml_name_escapes(local);
        for (p, uri) in ns_map {
            if p == prefix {
                return Some((uri.clone(), local));
            }
        }
        // Unknown prefix - use prefix as namespace
        Some((prefix.to_string(), local))
    } else {
        None
    }
}

/// Undo the escaping writers use for property names XML will not accept.
///
/// A custom property can be called anything - Word's document properties reach
/// XMP with spaces and `#` in them - but an XML element name cannot contain
/// either. Acrobat (and the writers that follow it) escape each offending
/// character as U+2182 ROMAN NUMERAL TEN THOUSAND followed by four hex digits.
/// U+2182 is chosen because it IS legal in an XML name (XML 1.0 allows the
/// 0x2070-0x218F block) while being a character no real property name uses.
///
/// Observed in the wild on the OPF test corpus, where
///
/// ```text
/// <pdfx:Digitalↂ0020preservationↂ0020testingↂ0020propertyↂ0020ↂ00231>
/// ```
///
/// is the property "Digital preservation testing property #1". Left encoded it
/// reads as corruption, which is exactly how it looked in a metadata table.
///
/// A lone marker, a short tail, or non-hex digits are left ALONE rather than
/// dropped: a name we cannot decode is still the name the file used, and
/// silently eating characters would be worse than showing the escape.
fn decode_xml_name_escapes(name: &str) -> String {
    const MARKER: char = '\u{2182}';
    if !name.contains(MARKER) {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len());
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c != MARKER {
            out.push(c);
            continue;
        }
        // Peek exactly four hex digits without consuming them until we know
        // the whole escape is well formed.
        let hex: String = chars.clone().take(4).collect();
        let decoded = (hex.len() == 4 && hex.chars().all(|h| h.is_ascii_hexdigit()))
            .then(|| u32::from_str_radix(&hex, 16).ok())
            .flatten()
            .and_then(char::from_u32);
        match decoded {
            Some(ch) => {
                for _ in 0..4 {
                    chars.next();
                }
                out.push(ch);
            }
            None => out.push(MARKER),
        }
    }
    out
}

/// Extract attributes from an rdf:Description opening tag as properties.
fn extract_attributes(tag: &str, ns_map: &[(String, String)], props: &mut Vec<XmpProperty>) {
    // Parse attributes: name="value"
    let mut pos = 0;
    while pos < tag.len() {
        // Find next attribute-like pattern
        if let Some(eq_pos) = tag[pos..].find('=') {
            let abs_eq = pos + eq_pos;
            // Get attribute name (word before =)
            let name_start = tag[..abs_eq]
                .rfind(|c: char| c.is_whitespace())
                .map_or(0, |p| p + 1);
            let attr_name = tag[name_start..abs_eq].trim();

            if let Some(value) = extract_quoted_value(&tag[abs_eq + 1..]) {
                if let Some((namespace, name)) = resolve_prefixed_name(attr_name, ns_map) {
                    props.push(XmpProperty {
                        namespace,
                        name,
                        value: XmpValue::Simple(decode_xml_entities(&value)),
                    });
                }
                // Skip past the closing quote
                let quote_char = tag.as_bytes().get(abs_eq + 1).copied().unwrap_or(b'"');
                let value_end = tag[abs_eq + 2..]
                    .find(quote_char as char)
                    .map(|p| abs_eq + 2 + p + 1)
                    .unwrap_or(abs_eq + 2);
                pos = value_end;
            } else {
                pos = abs_eq + 1;
            }
        } else {
            break;
        }
    }
}

/// Extract child elements from rdf:Description content.
fn extract_child_elements(
    content: &str,
    ns_map: &[(String, String)],
    props: &mut Vec<XmpProperty>,
) {
    let mut pos = 0;

    while pos < content.len() {
        // Find next element start
        if let Some(lt) = content[pos..].find('<') {
            let abs_lt = pos + lt;
            // Skip comments, processing instructions, closing tags
            if content[abs_lt..].starts_with("</")
                || content[abs_lt..].starts_with("<?")
                || content[abs_lt..].starts_with("<!--")
            {
                pos = abs_lt + 1;
                continue;
            }

            // Get tag name
            let tag_name_end = content[abs_lt + 1..]
                .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .map(|p| abs_lt + 1 + p)
                .unwrap_or(content.len());
            let tag_name = &content[abs_lt + 1..tag_name_end];

            if tag_name.is_empty() {
                pos = abs_lt + 1;
                continue;
            }

            if let Some((namespace, name)) = resolve_prefixed_name(tag_name, ns_map) {
                // Check for rdf:resource or rdf:parseType on the opening tag
                let tag_close = content[abs_lt..]
                    .find('>')
                    .map(|e| abs_lt + e)
                    .unwrap_or(content.len());
                let opening_tag = &content[abs_lt..tag_close + 1];

                // Check for rdf:resource attribute (e.g., <xmpMM:DocumentID rdf:resource="..."/>)
                if let Some(res_val) = extract_rdf_resource(opening_tag) {
                    props.push(XmpProperty {
                        namespace,
                        name,
                        value: XmpValue::Simple(decode_xml_entities(&res_val)),
                    });
                    // Skip past the element
                    if let Some((_, elem_end)) = find_element_content(content, abs_lt, tag_name) {
                        pos = elem_end;
                    } else {
                        pos = tag_close + 1;
                    }
                }
                // Check for rdf:parseType="Resource" (inline struct)
                else if opening_tag.contains("rdf:parseType") && opening_tag.contains("Resource")
                {
                    if let Some((inner, elem_end)) = find_element_content(content, abs_lt, tag_name)
                    {
                        // Flatten struct fields as separate top-level properties (ExifTool style)
                        flatten_struct_children(inner, ns_map, &name, props);
                        pos = elem_end;
                    } else {
                        pos = abs_lt + 1;
                    }
                } else if let Some((inner, elem_end)) =
                    find_element_content(content, abs_lt, tag_name)
                {
                    if inner.is_empty() {
                        // Self-closing tag - check for struct attributes like
                        // <xapMM:DerivedFrom stRef:instanceID="..." stRef:documentID="..."/>
                        let struct_attrs = extract_struct_attributes(opening_tag, ns_map);
                        if !struct_attrs.is_empty() {
                            // Flatten struct attributes as ParentFieldName properties
                            for (attr_ns, attr_name, attr_value) in &struct_attrs {
                                let cap_name = capitalize_first_char(attr_name);
                                props.push(XmpProperty {
                                    namespace: attr_ns.clone(),
                                    name: format!("{name}{cap_name}"),
                                    value: XmpValue::Simple(decode_xml_entities(attr_value)),
                                });
                            }
                        }
                    } else {
                        // Check for Seq/Bag of struct items (self-closing rdf:li with attributes)
                        // ExifTool flattens these as HistoryAction, HistoryWhen, etc.
                        if let Some(flattened) = try_flatten_seq_of_structs(inner, ns_map, &name) {
                            for prop in flattened {
                                props.push(prop);
                            }
                        } else {
                            let value = parse_element_value(inner, ns_map);
                            // If value is a Struct, also flatten its fields as ParentFieldName properties
                            if let XmpValue::Struct(fields) = &value {
                                flatten_struct_fields(fields, ns_map, &name, &namespace, props);
                            }
                            props.push(XmpProperty {
                                namespace,
                                name,
                                value,
                            });
                        }
                    }
                    pos = elem_end;
                } else {
                    pos = abs_lt + 1;
                }
            } else {
                pos = abs_lt + 1;
            }
        } else {
            break;
        }
    }
}

/// Parse the value of an element, detecting arrays and language alternatives.
fn parse_element_value(inner: &str, _ns_map: &[(String, String)]) -> XmpValue {
    let trimmed = inner.trim();

    // X10: Language alternative - rdf:Alt with xml:lang attributes
    if trimmed.contains("<rdf:Alt") {
        let items = extract_rdf_list_items(trimmed, "rdf:Alt");
        let lang_items: Vec<(String, String)> = items
            .into_iter()
            .map(|(attrs, text)| {
                let lang = extract_xml_lang(&attrs).unwrap_or_else(|| "x-default".to_string());
                (lang, decode_xml_entities(&text))
            })
            .collect();
        if !lang_items.is_empty() {
            return XmpValue::LangAlt(lang_items);
        }
    }

    // X11: Ordered array - rdf:Seq
    if trimmed.contains("<rdf:Seq") {
        let items = extract_rdf_list_items(trimmed, "rdf:Seq");
        let strings: Vec<String> = items
            .into_iter()
            .map(|(_, text)| decode_xml_entities(&text))
            .collect();
        if !strings.is_empty() {
            return XmpValue::OrderedArray(strings);
        }
    }

    // X11: Unordered array - rdf:Bag
    if trimmed.contains("<rdf:Bag") {
        let items = extract_rdf_list_items(trimmed, "rdf:Bag");
        let strings: Vec<String> = items
            .into_iter()
            .map(|(_, text)| decode_xml_entities(&text))
            .collect();
        if !strings.is_empty() {
            return XmpValue::UnorderedArray(strings);
        }
    }

    // X12: Struct - rdf:Description (nested)
    if trimmed.contains("<rdf:Description") {
        let mut struct_fields = Vec::new();
        // Extract attributes and simple child elements
        if let Some(desc_start) = trimmed.find("<rdf:Description") {
            if let Some((desc_inner, _)) =
                find_element_content(trimmed, desc_start, "rdf:Description")
            {
                // Extract attributes
                let tag_end = trimmed[desc_start..]
                    .find('>')
                    .map(|e| desc_start + e)
                    .unwrap_or(trimmed.len());
                let opening = &trimmed[desc_start..tag_end + 1];
                let mut pos = 0;
                while pos < opening.len() {
                    if let Some(eq_pos) = opening[pos..].find('=') {
                        let abs_eq = pos + eq_pos;
                        let name_start = opening[..abs_eq]
                            .rfind(|c: char| c.is_whitespace())
                            .map_or(0, |p| p + 1);
                        let attr_name = opening[name_start..abs_eq].trim();
                        if let Some(value) = extract_quoted_value(&opening[abs_eq + 1..]) {
                            if let Some(colon) = attr_name.find(':') {
                                let prefix = &attr_name[..colon];
                                if prefix != "rdf" && prefix != "xml" && prefix != "xmlns" {
                                    struct_fields
                                        .push((attr_name.to_string(), decode_xml_entities(&value)));
                                }
                            }
                        }
                        pos = abs_eq + 2;
                    } else {
                        break;
                    }
                }
                // Extract child elements
                let mut child_pos = 0;
                while child_pos < desc_inner.len() {
                    if let Some(lt) = desc_inner[child_pos..].find('<') {
                        let abs = child_pos + lt;
                        if desc_inner[abs..].starts_with("</") {
                            child_pos = abs + 1;
                            continue;
                        }
                        let name_end = desc_inner[abs + 1..]
                            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                            .map(|p| abs + 1 + p)
                            .unwrap_or(desc_inner.len());
                        let tn = &desc_inner[abs + 1..name_end];
                        if !tn.is_empty() {
                            if let Some((text, end)) = find_element_content(desc_inner, abs, tn) {
                                struct_fields
                                    .push((tn.to_string(), decode_xml_entities(text.trim())));
                                child_pos = end;
                                continue;
                            }
                        }
                        child_pos = abs + 1;
                    } else {
                        break;
                    }
                }
            }
        }
        if !struct_fields.is_empty() {
            return XmpValue::Struct(struct_fields);
        }
    }

    // Simple text value
    XmpValue::Simple(decode_xml_entities(trimmed))
}

/// Extract rdf:resource="..." attribute value from an opening tag.
fn extract_rdf_resource(tag: &str) -> Option<String> {
    let marker = "rdf:resource=";
    let idx = tag.find(marker)?;
    extract_quoted_value(&tag[idx + marker.len()..])
}

/// Extract namespace-prefixed attributes from a self-closing struct element.
/// e.g., `<xapMM:DerivedFrom stRef:instanceID="..." stRef:documentID="..."/>`
/// Returns vec of (namespace, local_name, value).
fn extract_struct_attributes(
    tag: &str,
    ns_map: &[(String, String)],
) -> Vec<(String, String, String)> {
    let mut result = Vec::new();
    // Find attributes: prefix:name="value"
    let mut pos = 0;
    while pos < tag.len() {
        // Look for pattern: word:word="
        if let Some(eq_pos) = tag[pos..].find('=') {
            let abs_eq = pos + eq_pos;
            // Get the attribute name before '='
            let attr_start = tag[..abs_eq]
                .rfind(|c: char| c.is_whitespace())
                .map(|p| p + 1)
                .unwrap_or(0);
            let attr_name = &tag[attr_start..abs_eq];

            // Must be prefixed (contain ':') and not be xmlns:, rdf:, xml:
            if let Some(colon) = attr_name.find(':') {
                let prefix = &attr_name[..colon];
                let local = &attr_name[colon + 1..];
                if prefix != "xmlns" && prefix != "rdf" && prefix != "xml" && !local.is_empty() {
                    if let Some(value) = extract_quoted_value(&tag[abs_eq + 1..]) {
                        if let Some((ns, _)) = resolve_prefixed_name(attr_name, ns_map) {
                            result.push((ns, local.to_string(), value));
                        }
                    }
                }
            }
            pos = abs_eq + 1;
        } else {
            break;
        }
    }
    result
}

/// Try to flatten an rdf:Seq/rdf:Bag of struct items into per-field joined properties.
/// E.g., History with rdf:li items having stEvt:action, stEvt:when, etc. becomes
/// HistoryAction = "created, saved, ...", HistoryWhen = "2024-01-01, 2024-01-02, ..."
fn try_flatten_seq_of_structs(
    inner: &str,
    ns_map: &[(String, String)],
    parent_name: &str,
) -> Option<Vec<XmpProperty>> {
    let trimmed = inner.trim();
    // Must contain rdf:Seq or rdf:Bag
    if !trimmed.contains("<rdf:Seq") && !trimmed.contains("<rdf:Bag") {
        return None;
    }

    // Collect all rdf:li items and their struct attributes
    let items = extract_rdf_list_items(
        trimmed,
        if trimmed.contains("<rdf:Seq") {
            "rdf:Seq"
        } else {
            "rdf:Bag"
        },
    );

    if items.is_empty() {
        return None;
    }

    // Check if at least some items are struct-like (have namespace-prefixed attributes, empty text)
    let mut field_values: Vec<(String, String, Vec<String>)> = Vec::new(); // (ns, local, values)
    let mut has_struct_attrs = false;

    for (attrs, text) in &items {
        let mut item_fields: Vec<(String, String, String)> = Vec::new();

        // Extract prefixed attributes from the attrs string (stEvt:action="..." pattern)
        let dummy_tag = format!("<rdf:li {}>", attrs);
        let struct_attrs = extract_struct_attributes(&dummy_tag, ns_map);
        if !struct_attrs.is_empty() {
            has_struct_attrs = true;
            item_fields.extend(struct_attrs);
        }

        // Also extract child elements (rdf:parseType="Resource" pattern)
        // e.g., <photoshop:LayerName>value</photoshop:LayerName>
        if !text.is_empty() {
            let mut cpos = 0;
            while cpos < text.len() {
                if let Some(lt) = text[cpos..].find('<') {
                    let abs = cpos + lt;
                    if text[abs..].starts_with("</")
                        || text[abs..].starts_with("<?")
                        || text[abs..].starts_with("<!--")
                    {
                        cpos = abs + 1;
                        continue;
                    }
                    // Extract tag name
                    let after_lt = abs + 1;
                    let name_end = text[after_lt..]
                        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                        .map(|e| after_lt + e)
                        .unwrap_or(text.len());
                    let child_tag = &text[after_lt..name_end];
                    if child_tag.contains(':') && !child_tag.starts_with("rdf:") {
                        // First, extract attributes from this child element
                        // (e.g., <Container:Item Item:Mime="image/jpeg"/>)
                        let tag_end = text[abs..].find('>').map(|e| abs + e).unwrap_or(text.len());
                        let full_child_tag = &text[abs..tag_end + 1];
                        let child_attrs = extract_struct_attributes(full_child_tag, ns_map);
                        if !child_attrs.is_empty() {
                            // Prepend child element's local name to attribute names
                            // e.g., Container:Item + Item:Mime -> "ItemMime"
                            let child_local = child_tag
                                .split_once(':')
                                .map(|(_, l)| l)
                                .unwrap_or(child_tag);
                            for (ns, local, val) in child_attrs {
                                let combined =
                                    format!("{child_local}{}", capitalize_first_char(&local));
                                item_fields.push((ns, combined, val));
                            }
                            has_struct_attrs = true;
                        }

                        if let Some((child_content, child_end)) =
                            find_element_content(text, abs, child_tag)
                        {
                            // Trim only leading/trailing newlines, preserve internal spaces
                            let child_content =
                                child_content.trim_matches(|c: char| c == '\n' || c == '\r');
                            if !child_content.is_empty() && !child_content.contains('<') {
                                // Simple text content - resolve namespace
                                if let Some((ns, _)) = resolve_prefixed_name(child_tag, ns_map) {
                                    let local = child_tag
                                        .split_once(':')
                                        .map(|(_, l)| l)
                                        .unwrap_or(child_tag);
                                    item_fields.push((
                                        ns,
                                        local.to_string(),
                                        decode_xml_entities(child_content),
                                    ));
                                    has_struct_attrs = true;
                                }
                            }
                            cpos = child_end;
                        } else {
                            // Self-closing element - skip past it
                            cpos = tag_end + 1;
                        }
                    } else {
                        cpos = abs + 1;
                    }
                } else {
                    break;
                }
            }
        }

        // For each field, append value
        for (ns, local, val) in &item_fields {
            if let Some(entry) = field_values.iter_mut().find(|(_, l, _)| l == local) {
                entry.2.push(val.clone());
            } else {
                field_values.push((ns.clone(), local.clone(), vec![val.clone()]));
            }
        }
    }

    if !has_struct_attrs {
        return None;
    }

    // Build flattened properties: ParentFieldName = "val1, val2, val3"
    let mut result = Vec::new();
    for (ns, local, vals) in field_values {
        let cap = capitalize_first_char(&local);
        result.push(XmpProperty {
            namespace: ns,
            name: format!("{parent_name}{cap}"),
            value: XmpValue::Simple(vals.join(", ")),
        });
    }

    Some(result)
}

/// Flatten inline struct children (from rdf:parseType="Resource") as ParentFieldName properties.
fn flatten_struct_children(
    inner: &str,
    ns_map: &[(String, String)],
    parent_name: &str,
    props: &mut Vec<XmpProperty>,
) {
    let mut child_pos = 0;
    while child_pos < inner.len() {
        if let Some(lt) = inner[child_pos..].find('<') {
            let abs = child_pos + lt;
            if inner[abs..].starts_with("</")
                || inner[abs..].starts_with("<?")
                || inner[abs..].starts_with("<!--")
            {
                child_pos = abs + 1;
                continue;
            }
            let name_end = inner[abs + 1..]
                .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .map(|p| abs + 1 + p)
                .unwrap_or(inner.len());
            let tn = &inner[abs + 1..name_end];
            if !tn.is_empty() {
                if let Some((_ns, local)) = resolve_prefixed_name(tn, ns_map) {
                    // Check for rdf:resource on this child
                    let tag_close = inner[abs..]
                        .find('>')
                        .map(|e| abs + e)
                        .unwrap_or(inner.len());
                    let child_tag = &inner[abs..tag_close + 1];
                    let flat_name = format!("{parent_name}{}", capitalize_first_char(&local));

                    if let Some(res_val) = extract_rdf_resource(child_tag) {
                        if let Some((ns, _)) = resolve_prefixed_name(tn, ns_map) {
                            props.push(XmpProperty {
                                namespace: ns,
                                name: flat_name,
                                value: XmpValue::Simple(decode_xml_entities(&res_val)),
                            });
                        }
                        if let Some((_, end)) = find_element_content(inner, abs, tn) {
                            child_pos = end;
                        } else {
                            child_pos = tag_close + 1;
                        }
                        continue;
                    }

                    if let Some((text, end)) = find_element_content(inner, abs, tn) {
                        if let Some((ns, _)) = resolve_prefixed_name(tn, ns_map) {
                            props.push(XmpProperty {
                                namespace: ns,
                                name: flat_name,
                                value: XmpValue::Simple(decode_xml_entities(text.trim())),
                            });
                        }
                        child_pos = end;
                        continue;
                    }
                }
            }
            child_pos = abs + 1;
        } else {
            break;
        }
    }
}

/// Flatten Struct fields as "ParentFieldName" properties (ExifTool convention).
fn flatten_struct_fields(
    fields: &[(String, String)],
    ns_map: &[(String, String)],
    parent_name: &str,
    parent_ns: &str,
    props: &mut Vec<XmpProperty>,
) {
    for (field_name, field_value) in fields {
        // Field name may be prefixed (e.g., "stRef:instanceID") - resolve to local name
        let local = if let Some(colon) = field_name.find(':') {
            &field_name[colon + 1..]
        } else {
            field_name
        };
        let flat_name = format!("{parent_name}{}", capitalize_first_char(local));
        // Try to resolve the namespace from the field prefix
        let ns = if let Some((ns, _)) = resolve_prefixed_name(field_name, ns_map) {
            ns
        } else {
            parent_ns.to_string()
        };
        props.push(XmpProperty {
            namespace: ns,
            name: flat_name,
            value: XmpValue::Simple(field_value.clone()),
        });
    }
}

fn capitalize_first_char(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Extract rdf:li items from inside an rdf:Alt, rdf:Seq, or rdf:Bag container.
/// Returns (attributes_string, text_content) for each li.
fn extract_rdf_list_items(content: &str, _container: &str) -> Vec<(String, String)> {
    let mut items = Vec::new();
    let mut pos = 0;

    while pos < content.len() {
        if let Some(li_start) = content[pos..].find("<rdf:li") {
            let abs = pos + li_start;
            // Get attributes
            let tag_end = content[abs..]
                .find('>')
                .map(|e| abs + e)
                .unwrap_or(content.len());
            let attrs = content[abs + 7..tag_end].to_string();

            if let Some((inner, end)) = find_element_content(content, abs, "rdf:li") {
                items.push((attrs, inner.trim().to_string()));
                pos = end;
            } else {
                pos = abs + 7;
            }
        } else {
            break;
        }
    }

    items
}

/// Extract xml:lang="..." value from attributes.
fn extract_xml_lang(attrs: &str) -> Option<String> {
    let marker = "xml:lang=";
    let idx = attrs.find(marker)?;
    extract_quoted_value(&attrs[idx + marker.len()..])
}

/// Extract a quoted value ("..." or '...') from the start of a string.
fn extract_quoted_value(s: &str) -> Option<String> {
    let s = s.trim_start();
    let quote = s.as_bytes().first()?;
    if *quote != b'"' && *quote != b'\'' {
        return None;
    }
    let end = s[1..].find(*quote as char)?;
    Some(s[1..1 + end].to_string())
}

/// Find the content between <tag...>content</tag>, handling self-closing tags.
/// Returns (content, end_position_after_closing_tag).
fn find_element_content<'a>(
    xml: &'a str,
    start: usize,
    tag_name: &str,
) -> Option<(&'a str, usize)> {
    let after_name = start + 1 + tag_name.len();
    if after_name >= xml.len() {
        return None;
    }

    // Find the end of the opening tag
    let mut pos = after_name;

    // Skip to the end of the opening tag
    while pos < xml.len() {
        if xml.as_bytes()[pos] == b'/' && pos + 1 < xml.len() && xml.as_bytes()[pos + 1] == b'>' {
            // Self-closing: <tag ... />
            return Some(("", pos + 2));
        }
        if xml.as_bytes()[pos] == b'>' {
            pos += 1;
            break;
        }
        pos += 1;
    }

    let content_start = pos;
    let mut depth = 1;

    // Find the matching closing tag
    let open_pat = format!("<{}", tag_name);
    let close_pat = format!("</{}>", tag_name);

    while pos < xml.len() && depth > 0 {
        if xml[pos..].starts_with(&close_pat) {
            depth -= 1;
            if depth == 0 {
                let content = &xml[content_start..pos];
                return Some((content, pos + close_pat.len()));
            }
            pos += close_pat.len();
        } else if xml[pos..].starts_with(&open_pat) {
            // Check if this is actually an opening tag (not just a prefix match)
            let after = pos + open_pat.len();
            if after < xml.len() {
                let next_char = xml.as_bytes()[after];
                if next_char == b' ' || next_char == b'>' || next_char == b'/' {
                    // Check for self-closing
                    if let Some(gt) = xml[after..].find('>') {
                        if xml.as_bytes()[after + gt - 1] != b'/' {
                            depth += 1;
                        }
                    }
                }
            }
            pos += open_pat.len();
        } else {
            // Advance by one UTF-8 character (may be multi-byte)
            pos += xml[pos..].chars().next().map_or(1, |c| c.len_utf8());
        }
    }

    None
}

/// Decode basic XML entities.
fn decode_xml_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XMP: &str = r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
      xmlns:exif="http://ns.adobe.com/exif/1.0/"
      xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
      tiff:Make="Canon"
      tiff:Model="Canon EOS 5D Mark IV"
      exif:ExposureTime="1/200"
      exif:FNumber="56/10"
      photoshop:DateCreated="2024-01-15">
      <dc:title>
        <rdf:Alt>
          <rdf:li xml:lang="x-default">Sunset Photo</rdf:li>
          <rdf:li xml:lang="fr">Photo de coucher de soleil</rdf:li>
        </rdf:Alt>
      </dc:title>
      <dc:creator>
        <rdf:Seq>
          <rdf:li>John Doe</rdf:li>
        </rdf:Seq>
      </dc:creator>
      <dc:subject>
        <rdf:Bag>
          <rdf:li>sunset</rdf:li>
          <rdf:li>landscape</rdf:li>
          <rdf:li>nature</rdf:li>
        </rdf:Bag>
      </dc:subject>
      <dc:description>
        <rdf:Alt>
          <rdf:li xml:lang="x-default">A beautiful sunset over the ocean</rdf:li>
        </rdf:Alt>
      </dc:description>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn x1_locate_xmp_xpacket() {
        let data = SAMPLE_XMP.as_bytes();
        let xmp = locate_xmp(data).unwrap();
        assert!(xmp.contains("<rdf:Description"));
        assert!(xmp.contains("<?xpacket end"));
    }

    #[test]
    fn x1_locate_xmp_xmpmeta() {
        let xmp_str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" dc:format="image/jpeg"/></rdf:RDF></x:xmpmeta>"#;
        let found = locate_xmp(xmp_str.as_bytes()).unwrap();
        assert!(found.contains("dc:format"));
    }

    #[test]
    fn x1_no_xmp() {
        assert!(locate_xmp(b"not xmp data at all").is_none());
    }

    #[test]
    fn x4_parse_xmp_attributes() {
        let xmp = parse_xmp(SAMPLE_XMP).unwrap();

        // X7: TIFF namespace
        assert_eq!(xmp.tiff_value("Make"), Some("Canon"));
        assert_eq!(xmp.tiff_value("Model"), Some("Canon EOS 5D Mark IV"));

        // X6: EXIF namespace
        assert_eq!(xmp.exif_value("ExposureTime"), Some("1/200"));
        assert_eq!(xmp.exif_value("FNumber"), Some("56/10"));

        // X8: Photoshop namespace
        assert_eq!(xmp.photoshop_value("DateCreated"), Some("2024-01-15"));
    }

    #[test]
    fn x5_dublin_core() {
        let xmp = parse_xmp(SAMPLE_XMP).unwrap();

        // dc:title - language alternative
        assert_eq!(xmp.dc_title(), Some("Sunset Photo"));

        // dc:creator - ordered array
        assert_eq!(xmp.dc_creator(), Some("John Doe"));

        // dc:subject - unordered bag
        let subjects = xmp.dc_subject();
        assert_eq!(subjects, vec!["sunset", "landscape", "nature"]);

        // dc:description - language alternative
        assert_eq!(
            xmp.dc_description(),
            Some("A beautiful sunset over the ocean")
        );
    }

    #[test]
    fn x10_lang_alt() {
        let xmp = parse_xmp(SAMPLE_XMP).unwrap();
        let title = xmp.get(ns::DC, "title").unwrap();
        if let XmpValue::LangAlt(items) = &title.value {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].0, "x-default");
            assert_eq!(items[0].1, "Sunset Photo");
            assert_eq!(items[1].0, "fr");
            assert_eq!(items[1].1, "Photo de coucher de soleil");
        } else {
            panic!("expected LangAlt");
        }
    }

    #[test]
    fn x11_ordered_array() {
        let xmp = parse_xmp(SAMPLE_XMP).unwrap();
        let creator = xmp.get(ns::DC, "creator").unwrap();
        assert!(matches!(&creator.value, XmpValue::OrderedArray(v) if v == &["John Doe"]));
    }

    #[test]
    fn x11_unordered_array() {
        let xmp = parse_xmp(SAMPLE_XMP).unwrap();
        let subject = xmp.get(ns::DC, "subject").unwrap();
        if let XmpValue::UnorderedArray(items) = &subject.value {
            assert_eq!(items, &["sunset", "landscape", "nature"]);
        } else {
            panic!("expected UnorderedArray");
        }
    }

    #[test]
    fn x4_simple_attributes_only() {
        let xml = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" dc:format="image/jpeg" dc:source="camera"/>
        </rdf:RDF>"#;
        let xmp = parse_xmp(xml).unwrap();
        assert_eq!(xmp.get_str(ns::DC, "format"), Some("image/jpeg"));
        assert_eq!(xmp.get_str(ns::DC, "source"), Some("camera"));
    }

    #[test]
    fn x4_self_closing_description() {
        let xml = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:tiff="http://ns.adobe.com/tiff/1.0/" tiff:Make="Nikon"/>
        </rdf:RDF>"#;
        let xmp = parse_xmp(xml).unwrap();
        assert_eq!(xmp.tiff_value("Make"), Some("Nikon"));
    }

    #[test]
    fn x4_xml_entities() {
        let xml = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" dc:format="image &amp; video"/>
        </rdf:RDF>"#;
        let xmp = parse_xmp(xml).unwrap();
        assert_eq!(xmp.get_str(ns::DC, "format"), Some("image & video"));
    }

    #[test]
    fn x12_struct() {
        let xml = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/">
            <xmpMM:History>
              <rdf:Description xmpMM:action="saved" xmpMM:when="2024-01-15"/>
            </xmpMM:History>
          </rdf:Description>
        </rdf:RDF>"#;
        let xmp = parse_xmp(xml).unwrap();
        let history = xmp.get(ns::XMP_MM, "History").unwrap();
        assert!(matches!(&history.value, XmpValue::Struct(_)));
    }

    #[test]
    fn x2_png_xmp_key() {
        assert_eq!(PNG_XMP_KEY, "XML:com.adobe.xmp");
    }

    #[test]
    fn x1_jpeg_header_constants() {
        assert!(JPEG_XMP_HEADER.ends_with(&[0]));
        assert!(JPEG_XMP_EXT_HEADER.ends_with(&[0]));
    }

    #[test]
    fn x9_reassemble_extended() {
        let seg1 = (b"<part1/>".to_vec(), [0u8; 32]);
        let seg2 = (b"<part2/>".to_vec(), [0u8; 32]);
        let result = reassemble_extended_xmp(&[seg1, seg2]).unwrap();
        assert_eq!(result, "<part1/><part2/>");
    }

    #[test]
    fn x9_empty_extended() {
        assert!(reassemble_extended_xmp(&[]).is_none());
    }

    #[test]
    fn x10_decodes_escaped_property_names() {
        // The real shape, from the OPF test corpus: spaces and '#' escaped as
        // U+2182 + 4 hex digits because XML names allow neither.
        assert_eq!(
            decode_xml_name_escapes(
                "Digital\u{2182}0020preservation\u{2182}0020property\u{2182}0020\u{2182}00231"
            ),
            "Digital preservation property #1"
        );
        // Untouched when there is nothing to undo - the common case, and it
        // must not allocate a different string.
        assert_eq!(decode_xml_name_escapes("CreateDate"), "CreateDate");
    }

    #[test]
    fn x10_malformed_escapes_are_left_alone() {
        // A name we cannot decode is still the name the file used. Eating
        // characters would turn an odd name into a wrong one.
        for raw in [
            "trailing\u{2182}",       // marker with no digits
            "short\u{2182}20",        // fewer than four
            "notHex\u{2182}00ZZtail", // not hex
            "\u{2182}D800pair",       // a lone surrogate is not a char
        ] {
            assert_eq!(decode_xml_name_escapes(raw), raw, "mangled {raw:?}");
        }
    }

    #[test]
    fn x10_escaped_names_survive_a_parse() {
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:pdfx="http://ns.adobe.com/pdfx/1.3/">
<pdfx:Client&#x2182;0020matter&#x2182;0020&#x2182;00231>ACME-0042</pdfx:Client&#x2182;0020matter&#x2182;0020&#x2182;00231>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;
        // written with numeric character references so the source file stays
        // ASCII; the parser sees them as the literal marker
        let xml = xml.replace("&#x2182;", "\u{2182}");
        let xmp = parse_xmp(&xml).unwrap();
        let names: Vec<&str> = xmp.properties.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"Client matter #1"),
            "escaped name not decoded, got {names:?}"
        );
    }
}
