# ADR 0007: Optional ExifTool metadata capability

## Status

Accepted for implementation in stages.

## Context

OxyViewer needs two different metadata capabilities:

- immutable capture metadata and Sony shooting-focus MakerNotes;
- editable XMP such as rating, label, title, and keywords.

`fpexif` provides the first capability in-process and already reads the repository Sony HIF fixture.
Version 0.0.3 does not parse or write general XMP, so it cannot safely replace ExifTool for embedded
JPEG/HEIF/HIF metadata. ExifTool has much broader compatibility, but bundling it materially changes
the application size and introduces a separately updated executable/script payload.

Measured against ExifTool 13.59 on 2026-09-03:

| Distribution | Download | Expanded | Notes |
| --- | ---: | ---: | --- |
| Source/Perl package | 7.9 MB | 34 MB | Includes docs and tests |
| Runtime script plus `lib` | — | about 20 MB | Uses a system or bundled Perl |
| Official Windows x64 | 11.2 MB | 34 MB | Includes Perl runtime |
| Official macOS installer | 5.7 MB | — | Installs into `/usr/local`; not an app-bundle payload |

The current release macOS `.app`, including the managed downloader but no ExifTool payload, is about
22 MB. A complete embedded ExifTool tree would therefore roughly double its expanded size. A
privately pruned 11–13 MB runtime is technically possible by removing documentation, tests,
translations, and unrelated data, but would become an OxyViewer-specific distribution that must be
regression-tested on every ExifTool update.

Official package sizes and checksums are published at <https://exiftool.org/>. ExifTool is distributed
under the same terms as Perl itself.

## Decision

Use one OxyViewer metadata facade with capability-driven providers:

1. `fpexif` is always present for EXIF and MakerNotes, including shooting focus.
2. XMP sidecars are the default read/write provider for every asset format. Normal edits never modify
   the image container.
3. Read precedence is sidecar, then embedded XMP through ExifTool, then empty editable metadata. A
   sidecar therefore explicitly overrides embedded values.
4. ExifTool is used only when reading embedded XMP without a sidecar or when the user explicitly asks
   to synchronize sidecar rating/color into a JPEG/HEIF/HIF container.
5. Missing embedded-XMP support never prevents sidecar edits, dimensions, previews, EXIF, or focus
   information from loading.

Provider discovery will use this precedence:

1. a user-configured executable path;
2. a versioned OxyViewer-managed capability pack;
3. `OXY_EXIFTOOL_PATH` for development and managed deployments;
4. `exiftool` from `PATH`;
5. unavailable.

The desktop prompts only when the user explicitly requests synchronization into the file and the
provider is absent. Ordinary rating/color edits have already been persisted to the sidecar and never
trigger installation.
The prompt offers either a managed download or an existing executable. User-selected paths are
validated with `-ver` and persisted in application data. The managed installer pins ExifTool 13.59,
verifies the upstream SHA-256 before extraction, installs beneath the versioned application-data
provider directory, and switches the live metadata facade only after the executable passes `-ver`.

## Capability pack design

An ExifTool capability pack is a constrained provider, not a general plugin system. It contains only
the platform payload and a manifest with:

- provider ID and ExifTool version;
- operating system and architecture;
- entry-point relative path;
- compressed and expanded SHA-256 hashes;
- minimum OxyViewer provider API version;
- upstream license and notice files.

Install into a versioned application-data directory and validate the pinned upstream archive hash.
Never execute directly from a temporary download. A future automatic updater must add an
OxyViewer-signed release manifest, expanded-payload hashes, a `current` pointer, and rollback; the
current explicit installer accepts only the compiled-in official ExifTool URL and checksum, never an
arbitrary URL.

On macOS outside the App Store, downloaded executable code still needs appropriate signing/notarization
and quarantine testing. A Mac App Store build must not download executable code after review; that
channel must either bundle the provider or omit embedded-XMP editing. Windows packs likewise need an
OxyViewer-signed artifact to avoid turning metadata support into an unsigned-code execution path.

## Release recommendation

- Keep the core application small and fully useful for browsing, preview, EXIF, and focus display.
- Offer an optional signed metadata capability pack for embedded XMP.
- Also accept an advanced-user executable path for Homebrew/system/managed ExifTool installations.
- Consider a separate “Full” installer only for offline environments; do not burden every core build
  with the 20–34 MB expanded provider.
- Initially ship the complete upstream runtime rather than a pruned private build. Optimize size only
  after fixture coverage proves which modules are safely removable.

## Testing consequences

The matrix must cover provider available, missing, invalid path, incompatible version, crash/timeout,
and successful restart. Every case must assert that HIF focus and default sidecar editing remain
available. Tests must also assert sidecar-over-embedded precedence and sidecar pairing during rename,
copy, move, and trash. Explicit embedded synchronization needs round-trip fixtures for JPEG and Sony
HIF, including comparison with Imaging Edge Viewer. Platform CI must test the same facade with
provider discovery injected rather than depending on the runner `PATH`.
