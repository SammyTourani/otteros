# OtterOS design language ("Kelp")

The desktop has to look designed in a screenshot and in a demo video (DECISIONS D24). Every GUI task
follows this file. The orchestrator judges screenshots against it.

## Principles
- Calm, dark surroundings; bright, legible content. The wallpaper and chrome recede, windows lead.
- One accent colour, used sparingly: focus, primary buttons, the Otter assistant.
- Soft depth: rounded corners and blurred shadows, never hard 1990s bevels or pure black outlines.
- Real typography: anti-aliased TrueType text everywhere, including the terminal.

## Colour tokens (sRGB)
| Token | Value | Use |
| --- | --- | --- |
| bg.top | #10303B | wallpaper gradient top |
| bg.bottom | #081419 | wallpaper gradient bottom |
| glow | #2C7A7B at 30% | soft radial glow, upper left of the wallpaper |
| chrome | #1B2429 at 92% | taskbar, title bars of unfocused windows |
| chrome.focus | #222E34 at 96% | title bar of the focused window |
| surface | #F6F4F0 | document windows (editor, viewer) |
| surface.dark | #121A1E | terminal and Otter chat backgrounds |
| text | #1E2226 | text on surface |
| text.inverse | #E8ECEE | text on chrome and dark surfaces |
| text.muted | #8A969C | secondary text, hints, timestamps |
| accent | #F2A541 | otter amber: focus rings, primary buttons, cursor in Otter |
| accent.alt | #3FB8AF | kelp teal: links, selection, secondary highlights |
| danger | #E5534B | close dot, error icons and error text on dark surfaces |
| danger.strong | #CF4038 | filled Deny / destructive buttons, always with white text (4.7:1) |
| ok | #57C27A | Allow button (with `text` #1E2226 labels, 7.2:1), success |

## Contrast
Every text/background pair the theme can produce meets WCAG AA 4.5:1 (checked by a test in
otter-wm). Text on amber, teal and green fills is `text` #1E2226, never white.

## Typography
- UI: Inter (OFL-1.1), 14 px regular, 14 px semibold for titles, 12 px for captions.
- Monospace: JetBrains Mono (OFL-1.1), 14 px, used by the terminal, the editor and code in chat.
- Line height 1.4 for body text, 1.25 for UI labels.

## Geometry
- Window corner radius 10 px; buttons and fields 6 px; taskbar 14 px, floating 10 px above the bottom edge.
- Title bar 34 px tall; title text centred; close / minimise dots on the left (12 px circles: danger, accent).
- Shadows: focused window 0 12 32 rgba(0,0,0,0.45); unfocused 0 6 18 rgba(0,0,0,0.30).
- Spacing scale: 4, 8, 12, 16, 24, 32 px.

## Components
- Taskbar: centred row of 40 px app icons with labels on hover, clock on the right, an Otter button
  on the left in accent amber.
- App icons: drawn procedurally by the graphics library (rounded squares with a glyph or simple
  vector shape), so they are AI-made like everything else.
- Dialogs (the agent-mode Allow/Deny prompt): modal, centred, surface colour, the requested action in
  monospace, Allow in ok green (dark label), Deny in danger.strong (white label), keyboard shortcuts Enter / Esc.
- Boot splash: an otter mark drawn from vector paths, "OtterOS" in Inter semibold, a thin accent
  progress line.

## What good looks like in a screenshot
Three overlapping windows with visible soft shadows, crisp anti-aliased text at 14 px, a
wallpaper gradient without banding (ordered dithering if needed), the taskbar floating with rounded
corners, and nothing clipped or misaligned.
