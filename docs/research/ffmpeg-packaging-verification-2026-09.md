# FFmpeg Packaging Verification, September 2026

This report preserves the dated local qualification evidence that originally
lived in the operational packaging guide. Current build, verification, signing,
and redistribution requirements are defined in
[FFMPEG_PACKAGING.md](../FFMPEG_PACKAGING.md).

## macOS arm64, 2026-09-10

- Native source build and cached preparation both passed. The two stripped
  executables total about 7 MiB; the included source archive is about 11 MiB.
  `otool -L` reports only Apple system libraries/frameworks.
- `pnpm tauri build --bundles app` produced an app containing both executables
  and all source/license materials; `--verify-bundle` passed with PATH cleared
  for the executable checks. No debug application was launched.
- TypeScript checks, all 167 frontend tests and the frontend build passed. Rust
  formatting and workspace Clippy passed. Real HIF FFmpeg decode, orientation,
  JPEG output and DCT compatibility tests passed.
- The full Rust suite found two then-existing macOS preview-size failures:
  `app_interim_hif_can_upgrade_to_a_satisfied_preview` and
  `heif_without_identified_fast_representation_uses_semantic_preview_size`.
  Both reproduced after restoring the original FFmpeg backend code. With those
  two tests excluded, all 530 remaining unit tests and two doctests passed;
  existing fixture-dependent ignored tests remained ignored.
- Windows/Linux package extraction checks were configured in CI but were not
  executed on that Mac. This is historical evidence, not current test status.

## Windows standard build, 2026-09-11

- MSYS2 was installed and updated at the default location together with Make,
  diffutils, NASM and the UCRT64 GCC toolchain. Mirror priority was adjusted to
  an accessible mirror from MSYS2's supplied list after primary-server timeouts.
- The recipe was corrected because MinGW targets are `ffmpeg.exe` and
  `ffprobe.exe`; macOS/Linux retain suffix-free targets.
- `pnpm tauri build` completed the pinned FFmpeg 8.0.1 source build,
  frontend/Release build, and MSI and NSIS installers. A subsequent
  `pnpm ffmpeg:prepare` reused the verified cache.
- MSI administrative extraction followed by `--verify-bundle` passed: both
  programs ran with PATH cleared, capability/filter smoke tests passed, and the
  exact source archive and required license/build materials were present.
- The extracted application passed `cold-preview-hif`, `resource-stress-hif`
  and `filmstrip-scroll-hif` with PATH empty and `OXY_FFMPEG_DIR` unset. Cold
  first preview was 83.2 ms and first tile 1015 ms. The stress run switched 80
  paths, revisited eight full presentations, and passed active-resource reads
  across cache maintenance.
- These were single runs using copies of one real Sony HIF fixture, not a
  diverse camera corpus. All test application instances were closed.
