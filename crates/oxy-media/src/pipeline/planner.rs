use oxy_domain::{AssetKind, RenderLevel};

/// Platform profile supplied to the pure planner. Platform-specific adapter
/// registration remains outside the planner and is represented by capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Every profile is exercised in host-independent planner tests.
pub(crate) enum Platform {
    Windows,
    Macos,
    Linux,
}

/// Vendor is compatibility evidence, not a format or backend selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Known facts are simulated until on-demand probe wiring is added.
pub(crate) enum Vendor {
    Sony,
    Other,
    Unknown,
}

/// A fact may remain unknown when planning must not perform source I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Known facts are simulated until on-demand probe wiring is added.
pub(crate) enum Presence {
    Present,
    Absent,
    Unknown,
}

/// Minimal source facts consumed by the current routing policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceFacts {
    pub(crate) kind: AssetKind,
    pub(crate) vendor: Vendor,
    pub(crate) heif_fast_jpeg: Presence,
}

impl SourceFacts {
    /// Facts available without opening the media source. More expensive facts
    /// intentionally stay unknown until an image request needs them.
    pub(crate) const fn unprobed(kind: AssetKind) -> Self {
        Self {
            kind,
            vendor: Vendor::Unknown,
            heif_fast_jpeg: Presence::Unknown,
        }
    }
}

/// Routes available to a planner invocation. Production currently supplies the
/// same optimistic routes as the pre-refactor dispatcher; tests can remove any
/// route without relying on host-native decoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackendCapabilities {
    pub(crate) raw_preview: bool,
    pub(crate) raw_full: bool,
    pub(crate) heif_fast_jpeg: bool,
    pub(crate) heif_preview: bool,
    pub(crate) heif_full: bool,
    pub(crate) system_preview: bool,
}

impl BackendCapabilities {
    /// Preserve the former dispatcher's behavior: it optimistically attempts
    /// every configured route and lets execution return detailed native errors.
    pub(crate) const fn configured_routes() -> Self {
        Self {
            raw_preview: true,
            raw_full: true,
            heif_fast_jpeg: true,
            heif_preview: true,
            heif_full: true,
            system_preview: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlannedPriority {
    Request,
    Foreground,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DecodeStep {
    Original,
    RawPreview {
        max_size: u32,
    },
    RawFull,
    HeifPreview {
        max_size: u32,
        try_fast_jpeg: bool,
        allow_decode: bool,
        priority: PlannedPriority,
    },
    HeifFull,
    SystemPreview {
        max_size: u32,
    },
}

/// Ordered decode work for one semantic render request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DecodePlan {
    Attempts {
        first: DecodeStep,
        on_failure: Option<DecodeStep>,
    },
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) level: RenderLevel,
    pub(crate) platform: Platform,
}

pub(crate) const fn plan(
    facts: SourceFacts,
    request: Request,
    capabilities: BackendCapabilities,
) -> DecodePlan {
    match (request.platform, facts.kind, request.level) {
        (_, AssetKind::Jpeg | AssetKind::Png | AssetKind::Webp, _) => {
            attempts(DecodeStep::Original, None)
        }

        (_, AssetKind::Raw, RenderLevel::Thumbnail) => raw_preview_plan(512, capabilities),
        (_, AssetKind::Raw, RenderLevel::Preview) => raw_preview_plan(4_096, capabilities),
        (_, AssetKind::Raw, RenderLevel::Full) if capabilities.raw_full => {
            attempts(DecodeStep::RawFull, None)
        }
        (_, AssetKind::Raw, RenderLevel::Full) => DecodePlan::Unsupported,

        (_, AssetKind::Heif, RenderLevel::Thumbnail) => {
            let max_size = if matches!(facts.heif_fast_jpeg, Presence::Present) {
                160
            } else {
                512
            };
            heif_preview_plan(max_size, facts, capabilities, PlannedPriority::Request)
        }
        (_, AssetKind::Heif, RenderLevel::Preview) => {
            let max_size = if matches!(facts.heif_fast_jpeg, Presence::Present) {
                160
            } else {
                4_096
            };
            heif_preview_plan(max_size, facts, capabilities, PlannedPriority::Request)
        }
        (_, AssetKind::Heif, RenderLevel::Full) if capabilities.heif_full => {
            let fallback = if capabilities.heif_preview {
                Some(DecodeStep::HeifPreview {
                    max_size: 8_192,
                    try_fast_jpeg: false,
                    allow_decode: true,
                    priority: PlannedPriority::Foreground,
                })
            } else {
                None
            };
            attempts(DecodeStep::HeifFull, fallback)
        }
        (_, AssetKind::Heif, RenderLevel::Full) if capabilities.heif_preview => attempts(
            DecodeStep::HeifPreview {
                max_size: 8_192,
                try_fast_jpeg: false,
                allow_decode: true,
                priority: PlannedPriority::Foreground,
            },
            None,
        ),
        (_, AssetKind::Heif, RenderLevel::Full) => DecodePlan::Unsupported,

        (_, AssetKind::Tiff, RenderLevel::Thumbnail | RenderLevel::Preview)
            if capabilities.system_preview =>
        {
            attempts(DecodeStep::SystemPreview { max_size: 512 }, None)
        }
        (_, AssetKind::Tiff, RenderLevel::Full) if capabilities.system_preview => {
            attempts(DecodeStep::SystemPreview { max_size: 4_096 }, None)
        }
        (_, AssetKind::Tiff, _) => DecodePlan::Unsupported,
    }
}

const fn raw_preview_plan(max_size: u32, capabilities: BackendCapabilities) -> DecodePlan {
    if capabilities.raw_preview {
        // Backend ordering, including Apple native fallbacks, is owned by
        // pipeline::raw. Quick Look is not a cache-compatible RAW backend.
        attempts(DecodeStep::RawPreview { max_size }, None)
    } else {
        DecodePlan::Unsupported
    }
}

const fn heif_preview_plan(
    max_size: u32,
    facts: SourceFacts,
    capabilities: BackendCapabilities,
    priority: PlannedPriority,
) -> DecodePlan {
    // A vendor name is only a clue. The fast path is selected exclusively by
    // the bounded representation fact produced for this source.
    let try_fast_jpeg =
        capabilities.heif_fast_jpeg && matches!(facts.heif_fast_jpeg, Presence::Present);
    if capabilities.heif_preview || try_fast_jpeg {
        attempts(
            DecodeStep::HeifPreview {
                max_size,
                try_fast_jpeg,
                allow_decode: capabilities.heif_preview,
                priority,
            },
            None,
        )
    } else {
        DecodePlan::Unsupported
    }
}

const fn attempts(first: DecodeStep, on_failure: Option<DecodeStep>) -> DecodePlan {
    DecodePlan::Attempts { first, on_failure }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLATFORMS: [Platform; 3] = [Platform::Windows, Platform::Macos, Platform::Linux];
    const KINDS: [AssetKind; 6] = [
        AssetKind::Raw,
        AssetKind::Jpeg,
        AssetKind::Heif,
        AssetKind::Png,
        AssetKind::Tiff,
        AssetKind::Webp,
    ];
    const LEVELS: [RenderLevel; 3] = [
        RenderLevel::Thumbnail,
        RenderLevel::Preview,
        RenderLevel::Full,
    ];

    fn request(platform: Platform, level: RenderLevel) -> Request {
        Request { level, platform }
    }

    fn facts(kind: AssetKind, vendor: Vendor, heif_fast_jpeg: Presence) -> SourceFacts {
        SourceFacts {
            kind,
            vendor,
            heif_fast_jpeg,
        }
    }

    fn expected_with_all_capabilities(kind: AssetKind, level: RenderLevel) -> DecodePlan {
        match (kind, level) {
            (AssetKind::Jpeg | AssetKind::Png | AssetKind::Webp, _) => {
                attempts(DecodeStep::Original, None)
            }
            (AssetKind::Raw, RenderLevel::Thumbnail) => {
                attempts(DecodeStep::RawPreview { max_size: 512 }, None)
            }
            (AssetKind::Raw, RenderLevel::Preview) => {
                attempts(DecodeStep::RawPreview { max_size: 4_096 }, None)
            }
            (AssetKind::Raw, RenderLevel::Full) => attempts(DecodeStep::RawFull, None),
            (AssetKind::Heif, RenderLevel::Thumbnail) => attempts(
                DecodeStep::HeifPreview {
                    max_size: 512,
                    try_fast_jpeg: false,
                    allow_decode: true,
                    priority: PlannedPriority::Request,
                },
                None,
            ),
            (AssetKind::Heif, RenderLevel::Preview) => attempts(
                DecodeStep::HeifPreview {
                    max_size: 4_096,
                    try_fast_jpeg: false,
                    allow_decode: true,
                    priority: PlannedPriority::Request,
                },
                None,
            ),
            (AssetKind::Heif, RenderLevel::Full) => attempts(
                DecodeStep::HeifFull,
                Some(DecodeStep::HeifPreview {
                    max_size: 8_192,
                    try_fast_jpeg: false,
                    allow_decode: true,
                    priority: PlannedPriority::Foreground,
                }),
            ),
            (AssetKind::Tiff, RenderLevel::Thumbnail | RenderLevel::Preview) => {
                attempts(DecodeStep::SystemPreview { max_size: 512 }, None)
            }
            (AssetKind::Tiff, RenderLevel::Full) => {
                attempts(DecodeStep::SystemPreview { max_size: 4_096 }, None)
            }
        }
    }

    #[test]
    fn preserves_current_platform_kind_and_level_matrix() {
        let capabilities = BackendCapabilities::configured_routes();
        for platform in PLATFORMS {
            for kind in KINDS {
                for level in LEVELS {
                    assert_eq!(
                        plan(
                            SourceFacts::unprobed(kind),
                            request(platform, level),
                            capabilities,
                        ),
                        expected_with_all_capabilities(kind, level),
                        "unexpected plan for {platform:?} {kind:?} {level:?}",
                    );
                }
            }
        }
    }

    #[test]
    fn heif_fast_path_depends_on_representation_not_vendor() {
        let capabilities = BackendCapabilities::configured_routes();
        for platform in PLATFORMS {
            for vendor in [Vendor::Sony, Vendor::Other, Vendor::Unknown] {
                for level in [RenderLevel::Thumbnail, RenderLevel::Preview] {
                    for presence in [Presence::Present, Presence::Absent, Presence::Unknown] {
                        let actual = plan(
                            facts(AssetKind::Heif, vendor, presence),
                            request(platform, level),
                            capabilities,
                        );
                        let present = presence == Presence::Present;
                        let max_size = if present {
                            160
                        } else if level == RenderLevel::Thumbnail {
                            512
                        } else {
                            4_096
                        };
                        assert_eq!(
                            actual,
                            attempts(
                                DecodeStep::HeifPreview {
                                    max_size,
                                    try_fast_jpeg: present,
                                    allow_decode: true,
                                    priority: PlannedPriority::Request,
                                },
                                None,
                            ),
                            "unexpected plan for {platform:?} {vendor:?} {presence:?} {level:?}",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn unknown_vendor_without_a_fast_representation_uses_semantic_sizes() {
        for platform in PLATFORMS {
            for (level, max_size) in [(RenderLevel::Thumbnail, 512), (RenderLevel::Preview, 4_096)]
            {
                assert_eq!(
                    plan(
                        facts(AssetKind::Heif, Vendor::Unknown, Presence::Absent),
                        request(platform, level),
                        BackendCapabilities::configured_routes(),
                    ),
                    attempts(
                        DecodeStep::HeifPreview {
                            max_size,
                            try_fast_jpeg: false,
                            allow_decode: true,
                            priority: PlannedPriority::Request,
                        },
                        None,
                    )
                );
            }
        }
    }

    #[test]
    fn missing_raw_capabilities_have_explicit_fallback_or_unsupported_results() {
        for platform in PLATFORMS {
            let mut capabilities = BackendCapabilities::configured_routes();
            capabilities.raw_preview = false;
            assert_eq!(
                plan(
                    SourceFacts::unprobed(AssetKind::Raw),
                    request(platform, RenderLevel::Preview),
                    capabilities,
                ),
                DecodePlan::Unsupported
            );

            capabilities.system_preview = false;
            assert_eq!(
                plan(
                    SourceFacts::unprobed(AssetKind::Raw),
                    request(platform, RenderLevel::Preview),
                    capabilities,
                ),
                DecodePlan::Unsupported
            );

            capabilities.raw_full = false;
            assert_eq!(
                plan(
                    SourceFacts::unprobed(AssetKind::Raw),
                    request(platform, RenderLevel::Full),
                    capabilities,
                ),
                DecodePlan::Unsupported
            );
        }
    }

    #[test]
    fn missing_heif_capabilities_have_explicit_fast_fallback_or_unsupported_results() {
        for platform in PLATFORMS {
            let mut capabilities = BackendCapabilities::configured_routes();
            capabilities.heif_preview = false;
            assert_eq!(
                plan(
                    facts(AssetKind::Heif, Vendor::Sony, Presence::Present),
                    request(platform, RenderLevel::Thumbnail),
                    capabilities,
                ),
                attempts(
                    DecodeStep::HeifPreview {
                        max_size: 160,
                        try_fast_jpeg: true,
                        allow_decode: false,
                        priority: PlannedPriority::Request,
                    },
                    None,
                )
            );

            capabilities.heif_fast_jpeg = false;
            assert_eq!(
                plan(
                    facts(AssetKind::Heif, Vendor::Sony, Presence::Present),
                    request(platform, RenderLevel::Thumbnail),
                    capabilities,
                ),
                DecodePlan::Unsupported
            );

            capabilities.heif_preview = true;
            assert_eq!(
                plan(
                    facts(AssetKind::Heif, Vendor::Unknown, Presence::Unknown),
                    request(platform, RenderLevel::Thumbnail),
                    capabilities,
                ),
                attempts(
                    DecodeStep::HeifPreview {
                        max_size: 512,
                        try_fast_jpeg: false,
                        allow_decode: true,
                        priority: PlannedPriority::Request,
                    },
                    None,
                )
            );

            capabilities.heif_preview = false;
            capabilities.heif_full = false;
            assert_eq!(
                plan(
                    SourceFacts::unprobed(AssetKind::Heif),
                    request(platform, RenderLevel::Full),
                    capabilities,
                ),
                DecodePlan::Unsupported
            );
        }
    }

    #[test]
    fn missing_heif_full_capability_uses_foreground_8192_preview() {
        for platform in PLATFORMS {
            let mut capabilities = BackendCapabilities::configured_routes();
            capabilities.heif_full = false;
            assert_eq!(
                plan(
                    SourceFacts::unprobed(AssetKind::Heif),
                    request(platform, RenderLevel::Full),
                    capabilities,
                ),
                attempts(
                    DecodeStep::HeifPreview {
                        max_size: 8_192,
                        try_fast_jpeg: false,
                        allow_decode: true,
                        priority: PlannedPriority::Foreground,
                    },
                    None,
                )
            );
        }
    }

    #[test]
    fn missing_system_capability_makes_every_tiff_level_unsupported() {
        let mut capabilities = BackendCapabilities::configured_routes();
        capabilities.system_preview = false;
        for platform in PLATFORMS {
            for level in LEVELS {
                assert_eq!(
                    plan(
                        SourceFacts::unprobed(AssetKind::Tiff),
                        request(platform, level),
                        capabilities,
                    ),
                    DecodePlan::Unsupported
                );
            }
        }
    }
}
