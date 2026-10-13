use super::AssetKind;
use std::path::Path;

#[test]
fn asset_kind_classifies_supported_paths_case_insensitively() {
    for (name, expected) in [
        ("photo.ARW", AssetKind::Raw),
        ("photo.JPEG", AssetKind::Jpeg),
        ("photo.HIF", AssetKind::Heif),
        ("photo.png", AssetKind::Png),
        ("photo.TIFF", AssetKind::Tiff),
        ("photo.webp", AssetKind::Webp),
    ] {
        assert_eq!(AssetKind::from_path(Path::new(name)), Some(expected));
    }
    assert_eq!(AssetKind::from_path(Path::new("photo.txt")), None);
    assert_eq!(AssetKind::from_path(Path::new("photo")), None);
}
