# Open images in external applications

The image context menu is shared by grid/list thumbnails, the Loupe image
(including HEIF canvas presentation), and the filmstrip. It always targets the
image that was right-clicked, even when another image or multiple images are
selected. External applications receive the original `AssetSummary.path`;
RAW/HIF files are not converted or replaced with preview-cache files.

## Interaction

- Settings → External applications adds a program through the native file
  chooser, with an editable display name, a default radio button, ordering,
  program replacement, and removal.
- The first application becomes the default. Removing the default selects the
  first remaining application; reordering or temporarily opening another
  application does not change the default.
- The top menu entry opens with the default. Open with… lists all configured
  programs in their saved order, followed by More… (the Windows system chooser).
  An empty list also offers Configure applications…, which opens and scrolls
  Settings to the application section.
- Submenus support pointer hover/click, arrows, Home/End, Enter and Escape.
  Menus clamp to the viewport; the submenu flips left at the right edge. Scrolling
  within a long menu is allowed; outside clicks/scrolls, resize and window blur
  dismiss it.
- Cancelling a program chooser or the system Open With dialog is not an error.
  Failed configuration saves keep the editor open and leave saved state intact.

## Ownership and boundaries

| Layer | Responsibility |
| --- | --- |
| `AssetContextMenu` | Presentation, focus, keyboard and viewport positioning |
| `ExternalAppsSettings` | Configuration editor and pending/error presentation |
| `lib/externalApps.ts` | Shared query key and ordering/removal transformations |
| `lib/api.ts` | All IPC and native program selection wrappers |
| `oxy-domain::external_apps` | camelCase configuration and launch-result contracts |
| Tauri `state::external_apps` | Configuration lookup by ID over `oxy-userdata::DocumentStore` |
| Tauri `commands::external_apps` | Thin command dispatch to blocking workers |
| `oxy-fs::external_apps` | Path validation and OS application/chooser launch |

`external-apps.json` lives in the application data directory independently of
the rebuildable library and preview cache, and is written through
`oxy-userdata::DocumentStore`: the same version envelope, atomic replacement, and
persist-before-publish rules as `people.json` and `cache-settings.json`. Version 1
contains `version`, ordered `apps` (`id`, `name`, `executablePath`), and nullable
`defaultAppId`. An unparseable file is moved aside as `external-apps.json.corrupt`
and the list starts empty, so recovery never writes over bytes it did not
understand. A file from a newer build, or a document that fails validation, is
left exactly as it is and every write is refused until the application is
upgraded. Invalid settings submitted by the user are reported rather than
overwritten.

Opening a menu reads already loaded settings, with no executable discovery,
metadata request or per-program disk checks. Saves validate newly added or
changed executable paths; missing unchanged programs do not prevent removing or
reordering entries. Launch validates both the program and original path on a
blocking worker. The program receives one file argument via `Command::arg`, with
no command shell or user-authored argument template. Windows verbatim disk/UNC
paths are converted to a compatible spelling only after both spellings resolve
to the same target. This prevents a trailing-dot filename from silently opening
a different file. Paths that cannot be safely represented remain verbatim for
configured programs and produce a clear unsupported-path error for the system
chooser. A successful result means
the launch request was accepted, not that the other program decoded the image.

On Windows the program picker accepts `.exe` files. More… uses
[`SHOpenWithDialog`](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shopenwithdialog)
with `OAIF_EXEC` on a dedicated COM STA thread and no owner window.
Do not pass the Tauri window as owner: cross-thread window ownership attaches
input queues, so shell modal selection and slow application activation can block
the desktop UI despite `spawn_blocking`. The pending operation owns its original
path; users can keep browsing while the chooser or selected program is starting.
Its IPC result still reports cancellation or launch errors when the shell returns.
This isolates OxyViewer responsiveness; it does not bound another program's
startup time or the shell's application-discovery time.
This is a one-file operation and
does not add a program to OxyViewer settings. Registered/store applications can
be used through the system chooser without storing their executable paths.
Other targets compile and can launch regular executables, but native application
bundle pickers and non-Windows system choosers are not implemented in this version.

Browser demo settings use separate local storage; native selection is disabled
and opening externally reports that the desktop app is required. Existing
refresh behavior applies after an external program changes a file; this feature
does not add a filesystem watcher or automatically reload edited previews.

## Verification

Focused frontend tests cover original-file targeting, default selection,
submenu keyboard traversal and viewport bounds, empty-state configuration,
ordering/default fallback, cancelled selection and failed-save draft retention.
Rust tests cover persistence/reload, missing-program removal, invalid targets
and preserving the previous configuration after a rejected update.

For desktop acceptance, use an isolated data/cache/WebView profile and real
JPG/RAW/HIF files, including Unicode and spaces in paths. Check native program
selection, two configured applications, restart persistence, all four image
menu surfaces, system-dialog selection/cancellation and missing-file errors.
Close the verification applications after testing. Full frontend and Rust
workspace checks are required for changes across these boundaries.

Desktop verification on Windows (2026-09-13) confirmed the native program
picker, persisted configuration across restart, default changes, all four
image menu surfaces, original JPG/RAW/HIF arguments, and missing-file errors.
A receiver executable with Unicode, spaces and an ampersand in its path verified
one original-file argument. Honeyview's resulting window title confirmed the
selected JPEG was opened. The final build also verified safe conversion of the
canonical Windows image path before launch.

The More… action created the Windows “Choose an app” window after that path
fix. The desktop control tool reported this window as minimized and lost its
handle during activation, so selection and cancellation inside that system
window remain a manual acceptance check; they have not been verified visually.

Checks passed: TypeScript check, 223 frontend tests, frontend production build,
Rust formatting, workspace Clippy with warnings denied, 615 workspace tests
(17 environment-dependent tests ignored), and the Windows desktop release build.
Rust tests ran serially because concurrent native HEIF decoders exceeded the
test environment's available resources.

### Main-window responsiveness correction

The original owner HWND connected the shell STA to the Tauri UI thread. The
owner parameter is now removed at both Rust boundaries, and `SHOpenWithDialog`
receives no owner. Moving the call to `spawn_blocking` alone is insufficient:
[cross-thread ownership attaches the input queues](https://devblogs.microsoft.com/oldnewthing/20191023-00/?p=103020).

The corrected Windows release build was tested with an isolated profile and
a real JPEG. While an instrumented system-chooser IPC call remained pending,
native mouse input switched the grid to list view and native double-click
opened the JPEG in Loupe, with its image visibly rendered. These observations
verify native input and rendering while the shell is waiting, rather than only
JavaScript timer activity. The system chooser itself could not be controlled
reliably (minimized-window / foreground-process errors); selected-application
startup latency and cancellation remain unmeasured in this run. No reduction
in an external program's own startup time is claimed.
