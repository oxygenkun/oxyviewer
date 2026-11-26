# OxyViewer UI style

The frontend follows StyleKit's [macos-vibrancy design document](https://www.stylekit.top/api/design/macos-vibrancy/design.md?locale=zh)
and its Implementation Contract. Use the existing React, CSS, CSS Modules and
Radix primitives; feature styles own layout while shared tokens own appearance.
At the user's request, OxyViewer uses a bright yellow-green accent within these
macOS vibrancy materials, with a vivid auxiliary tint instead of muted olive.
Shared controls and menus also retain the original compact sizing.

## Materials and controls

| Role | Value / owner |
| --- | --- |
| Window and image canvas | `#1c1c1e` |
| Photo workspace | `#2c2c2e` |
| Toolbars and raised surfaces | `#3a3a3c` |
| Sidebar and floating material | `rgba(28,28,30,0.8)` with `blur(40px)` |
| Selection and primary action | Bright yellow-green `#d6ff58`, auxiliary tint `#c6f26b`; selection fill at 16% and accent borders at 38%; dark text on filled primary actions |
| Sidebar selection | Neutral white 8% fill with normal text/icons; accent is limited to small status dots and thin markers, without colored row backgrounds |
| Separators | 1px white at 8–12% |
| Controls / cards / panels | 6px / 10px / 14px radii |
| Settings rows | 44px |
| Sidebar navigation and folder tree rows | Original compact 30px rows and folder-tree spacing |
| Context menus and dropdown options | Original 32px minimum rows, 12px text, 6px vertical and 8px horizontal padding |
| Shared buttons and fields | Original 12px body text; small controls are 26px high; button padding is 4px by 8px |
| Interaction transitions | Color, background-color and border-color only, 200ms ease-out |

`foundation.css` provides the palette, fonts, spacing and semantic aliases.
`controls.css` styles native controls, shared UI components and portalled menus.
Use opaque deepest grey if backdrop filtering is unavailable or the user asks
for reduced transparency. Reduced motion disables all transitions and animations.
Do not blur individual thumbnails or add decorative gradients, glow, animated
movement or scaling. Loading keeps its existing text and busy/disabled semantics;
loading glyphs are static. The undo timer changes its indicator color, without
moving or scaling the toast. Focus outlines may exceed 1px for legibility;
surface borders and separators remain 1px.

Window/dialog titles use Georgia with New York, Chinese serif and Times fallbacks.
Settings content headings use 24px and the people library uses 32px. Controls and
body text use the system sans-serif stack at 11–17px. Paths, file formats, numerical
metadata and shortcut keys use the system monospace stack. All sizes continue to
respect the existing UI font scale setting. Photo color labels and ratings retain
their distinct semantic colors; green, orange and red status marks are small.
The inspector and rating filter use bare star icons with a round keyboard focus
outline. Hovering previews the value through that star. Moving away restores
the current rating or filter value.

## Desktop application adaptations

- Preserve the resizable three-column photo workspace and the existing grid,
  loupe, filmstrip, selection, file actions and persisted panel sizes. A fresh
  sidebar defaults to 240px. Do not constrain the desktop canvas to the reference
  document's 1280px web-page maximum.
- Retain the original folder-tree density: 30px rows, the existing per-level
  indentation and row padding, and 6px spacing between availability groups.
- Start the sidebar with the Browse/People tabs and an 8px top inset; omit the
  fixed application-name header and its reserved space.
- Keep the actual operating-system window controls. Do not add decorative
  traffic lights or pretend that a CSS backdrop filter provides native
  transparency through the desktop window.
- Center the search field in the 52px toolbar, with the sidebar toggle on the
  left and view controls on the right. At the 600px content breakpoint the
  search field hides the shortcut hint. Show the photo count in the status bar.
- Keep independent content scrolling and the existing virtual thumbnail geometry.
- Show the current photo path on the left of the workspace status bar. Place
  Loupe preview quality and decode progress immediately after the file name.
  The Loupe image overlay shows enabled metadata badges and review notices.
- Start the inspector with Basic info and place tag-sync feedback beside its
  heading. Show the selection count in the workspace status bar.
- At the existing 900px viewport breakpoint, the inspector yields space to the
  workspace. At 700px, the sidebar uses at most 35% of the window and can still
  be collapsed. Filters and settings tabs scroll inside their own regions.
- Settings uses the existing Radix Dialog dependency for focus containment,
  Escape dismissal and focus restoration. Arrow keys, Home and End navigate its
  tabs. Existing shared dialogs and menus retain their keyboard behavior.
- The getting-started guide uses nonmodal callouts anchored to the sidebar add
  button, Grid, Loupe, and the Inspector. Settings → General → Reset tips replays
  the sequence. With an open folder, Next advances past the add-folder step.

## Acceptance

For a style change, run `pnpm check`, `pnpm test` and `pnpm build`. Inspect the
browser demo at desktop and the existing narrow breakpoints, including enlarged
UI text. Check empty content, selected photos, grid/loupe, menus, settings,
disabled/loading/error feedback, focus visibility, Escape and focus restoration,
and reduced motion/transparency fallbacks. Use only existing demo data or
isolated fixtures; do not touch the user's photo library or application state.

Browser checks do not establish native WebView compositing performance, OS window
appearance or real-file decode behavior. Report which checks actually ran and
leave those platform-specific checks explicit when they have not been performed.
