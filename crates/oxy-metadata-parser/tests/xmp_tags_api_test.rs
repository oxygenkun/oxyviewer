#![cfg(feature = "xmp")]

use oxy_metadata_parser::xmp_tags;

#[test]
fn parses_sidecar_attributes_and_elements_with_custom_namespace_prefixes() {
    let tags = xmp_tags(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
      <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description xmlns:raw="http://ns.adobe.com/camera-raw-settings/1.0/"
          xmlns:camera="http://ns.adobe.com/tiff/1.0/" raw:Temperature="6500">
          <raw:Tint>-12</raw:Tint>
          <camera:Model>Edited &amp; named</camera:Model>
        </rdf:Description>
      </rdf:RDF>
    </x:xmpmeta>"#,
    )
    .unwrap();

    for (name, expected) in [
        ("Temperature", "6500"),
        ("Tint", "-12"),
        ("Model", "Edited & named"),
    ] {
        let tag = tags.iter().find(|tag| tag.name == name).unwrap();
        assert_eq!(tag.group, "XMP");
        assert_eq!(tag.value, expected);
    }
}
