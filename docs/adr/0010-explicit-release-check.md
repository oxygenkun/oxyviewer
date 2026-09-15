# ADR 0010: Explicit-only release check in the About panel

## Status

Accepted.

## Context

Users need to know which OxyViewer build they are running, where the project
lives, who publishes it, and whether a newer release exists. The application is
otherwise local-first: the only outbound request in the shipping code is the
ExifTool capability download, which is also user-initiated and hash-pinned
([ADR 0007](0007-optional-exiftool-capability.md)).

Three options were considered:

1. **Tauri updater plugin.** Downloads and installs signed updates in place.
   This needs a release manifest, key management, and installer/relinking
   coverage for every platform, and it turns a viewer into a self-updating
   executable.
2. **Link only.** Show the version and a link to the release page. No network
   code at all, but the user must compare versions by hand.
3. **Advisory check.** Ask the GitHub Releases API whether a newer published
   release exists, report it, and let the user open the release page.

## Decision

Accept option 3, with these boundaries:

- The check runs only when the user clicks "Check for updates" in Settings →
  About. There is no polling, no start-up check, and no background request.
- The request happens in Rust on a blocking worker with a fixed 15-second
  timeout and a bounded response body, so the settings panel cannot hang on a
  slow or misbehaving network.
- Only the compiled-in project repository is queried:
  `api.github.com/repos/oxygenkun/oxyviewer/releases/latest`. Draft and
  prerelease entries are excluded by that endpoint.
- The reported version comes from the running package (`package_info`), so the
  About panel cannot disagree with the shipped build. Release tags are compared
  as semantic versions after stripping a leading `v`; non-semver tags fall back
  to inequality so a date-stamped nightly still surfaces a notice.
- The author comes from `bundle.publisher` in `tauri.conf.json`, resolved by the
  app's `build.rs` and compiled in. Tauri generates the runtime `BundleConfig`
  without `publisher` and `copyright`, and a packaged app ships no
  `tauri.conf.json`, so the config file cannot be read at run time. The panel
  therefore shows no copyright line rather than a permanently empty one. The
  license still comes from `bundle.license`, which the generated config keeps,
  and is shown as a fixed AGPL link.
- The result is advisory. Nothing is downloaded, executed, or installed, and a
  failed check only reports an error.
- The panel cannot open arbitrary URLs. Repository, releases, and license links
  are compiled-in constants, and the release URL returned by the API is accepted
  only when it starts with the project release prefix.

## Consequences

- Users learn about newer releases without the app gaining an update channel,
  code-signing dependency, or rollback obligations.
- Upgrading stays a manual download, and users on a network without GitHub access
  see a plain error instead of an update prompt.
- The check is subject to unauthenticated GitHub API rate limits; the UI reports
  a rate-limit failure rather than retrying.
- A future automatic updater is still gated on the requirements from ADR 0007:
  an OxyViewer-signed manifest, expanded-payload hashes, a `current` pointer,
  and rollback. This ADR does not weaken that bar.
