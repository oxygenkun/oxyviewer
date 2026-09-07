use super::planner::Platform;
use oxy_domain::RenderLevel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawBackend {
    AppleCoreImage,
    AppleImageIo,
    LibRawDevelopment,
}

pub(crate) fn backend_plan(platform: Platform, level: RenderLevel) -> Vec<RawBackend> {
    match (platform, level) {
        (Platform::Macos, RenderLevel::Thumbnail) => vec![
            RawBackend::AppleImageIo,
            RawBackend::AppleCoreImage,
            RawBackend::LibRawDevelopment,
        ],
        (Platform::Macos, RenderLevel::Preview | RenderLevel::Full) => vec![
            RawBackend::AppleCoreImage,
            RawBackend::AppleImageIo,
            RawBackend::LibRawDevelopment,
        ],
        (Platform::Windows | Platform::Linux, _) => vec![RawBackend::LibRawDevelopment],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_thumbnail_prefers_measured_image_io_path() {
        assert_eq!(
            backend_plan(Platform::Macos, RenderLevel::Thumbnail),
            vec![
                RawBackend::AppleImageIo,
                RawBackend::AppleCoreImage,
                RawBackend::LibRawDevelopment,
            ]
        );
    }

    #[test]
    fn macos_detail_prefers_measured_core_image_path() {
        for level in [RenderLevel::Preview, RenderLevel::Full] {
            assert_eq!(
                backend_plan(Platform::Macos, level),
                vec![
                    RawBackend::AppleCoreImage,
                    RawBackend::AppleImageIo,
                    RawBackend::LibRawDevelopment,
                ]
            );
        }
    }

    #[test]
    fn portable_platforms_use_libraw_development() {
        for platform in [Platform::Windows, Platform::Linux] {
            for level in [
                RenderLevel::Thumbnail,
                RenderLevel::Preview,
                RenderLevel::Full,
            ] {
                assert_eq!(
                    backend_plan(platform, level),
                    vec![RawBackend::LibRawDevelopment]
                );
            }
        }
    }
}
