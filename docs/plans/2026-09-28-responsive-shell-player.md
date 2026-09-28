# Responsive shell and player: design and implementation plan

Status: Implemented in this worktree. The dimensions and breakpoints remain targets for live visual validation. Frontend component compilation and the native window-state test pass; a full frontend check and screenshot matrix require a complete dependency install and a running application.

Implementation notes: the shell now has bounded outer widths, a compact navigation rail, a bottom layout at narrow desktop widths, and saved right/left/bottom player placement. The bottom player has a horizontal composition and queue drawer. Library and search track columns respond to workspace width; album and playlist details use workspace container queries. The player gained a clear header, denser queue, larger transport controls, a seek-time toggle, semantic surface tokens, and a dark/light/system preference with a preview. Tauri restores and clamps window geometry while UI zoom remains stored separately.

## Outcome

NOORwave keeps its current visual character while the centre workspace absorbs most width changes. Navigation and a side player remain bounded. The player can live on the right, left, or bottom; the bottom version uses a horizontal composition. UI zoom remains an accessibility control, and layout changes respond to the CSS viewport it creates.

## Confirmed baseline

- The desktop shell is a three-column grid: `--sidebar-width: 228px`, centre `minmax(0, 1fr)`, and `--panel-width: 300px` in `frontend/src/routes/+layout.svelte` and `frontend/src/app.css`.
- At `max-width: 1320px`, the panel becomes `minmax(280px, 32vw)`. This makes the player larger relative to navigation just when the available CSS viewport shrinks. The supplied 2564px-wide screenshots are consistent with approximately 150% and 200% zoom: at 200%, `32vw` is about 410 CSS px, rendered near 820 physical px, while the navigation remains 228 CSS px, rendered near 456 physical px.
- At `max-width: 1180px`, the shell hides both desktop side regions and uses the mobile top bar, tabs, mini player, and now-playing sheet. The breakpoint currently treats a narrow desktop and a phone alike.
- The side player stacks full-width square artwork, metadata, progress, transport, volume, then a queue. Its artwork height therefore grows with panel width. `queueExpanded` currently reduces artwork to a 64px strip and hides secondary metadata. The queue has saved expansion, jump-to-current, reorder, save, clear with undo, and source information.
- The desktop window launches at 1280 x 800 logical px and can shrink to 720 x 500; the app code has no explicit window geometry restore. UI zoom is persisted under `noor-ui-zoom`, ranges from 0.5 to 2, and is reapplied after the webview mounts. The chosen zoom cannot be read from the repository.
- Dark/light theme tokens, many accent palettes, and wallpaper color controls already exist. Accent and shader colors are currently coupled within each palette.

## Product decisions

1. **Default placement:** Right, preserving the familiar starting view. A three-choice layout control appears in the player header and in Settings > Appearance. The selected placement persists as `right | left | bottom`.
2. **Selected versus effective placement:** A narrow window may temporarily render a selected side player at the bottom. The selection remains saved, so widening the window returns it to the selected side. The control indicates the effective placement and explains the temporary fallback.
3. **Navigation stays at the outside edge:** Right is `[navigation | workspace | player]`; Left is `[navigation | player | workspace]`. Bottom places a player row across the full shell width beneath navigation and workspace.
4. **The queue has one source of truth:** Side modes display it beneath the player. Bottom mode opens a bounded queue drawer above the bar. Drawer openness is transient; the saved `queueExpanded` value applies to the side player's queue-focus state.
5. **Mobile keeps its own compact entry:** Below the smallest shell breakpoint, the current mini player and now-playing sheet remain the phone interaction model. The saved desktop placement still survives a visit to this width.
6. **Zoom continues to enlarge controls and text:** Constraints preserve *relative balance* and usability; they do not cancel user zoom by inversely scaling the outer regions.

## Responsive geometry

All thresholds refer to the CSS width available after webview or browser zoom. Validate them with real content before treating them as constants.

| Width | Effective shell | Minimum workspace target |
| --- | --- | --- |
| 1120px and wider | Full navigation, selected side or bottom player | 560px after shell gutters in side mode |
| 840-1119px | Full navigation, bottom player | 560px after shell gutters |
| 680-839px | 72px compact navigation rail, bottom player | 560px after shell gutters |
| Below 680px | Mobile navigation, mini player, full now-playing sheet | Single-column content |

- Full navigation: `clamp(216px, 12vw, 248px)`. Side player: `clamp(296px, 17vw, 352px)`. The centre is `minmax(0, 1fr)` and owns remaining width. Remove the `32vw` panel rule.
- Workspace gutters: 24px when the desktop workspace is comfortable, 16px when compact, plus relevant `env(safe-area-inset-*)`. No shell-level horizontal scrolling.
- Keep the side-player/navigation width ratio around 1.3-1.5 across desktop widths. The centre may change columns and density, but the outer regions should not gain unbounded `vw` widths.
- Side artwork: square, centred and capped by both width and height, approximately `min(100%, 32dvh, 320px)`. On short windows, controls and the queue stay visible; the queue gets `min-height: 0` and its own scroll area. Queue-focus mode uses a 64-72px thumbnail and a compact title/artist row rather than a cropped banner.
- Bottom player: two rows around 104-128px high on desktop. A 56-64px cover sits beside title and artist; primary transport stays central; progress and time run across the second row. At lower widths, volume reduces to a mute button with a slider popover, and less-used actions move into overflow. The player spans the shell width so it does not compete with the workspace for a third column.
- Portrait desktop monitors use the bottom player when width requires it. Height is handled independently: short windows reduce artwork first, then secondary copy, while preserving transport, seek, queue access, and an independently scrolling workspace.
- At the extreme 720 x 500 minimum window with 200% zoom, the CSS viewport is only about 360 x 250. That cannot show a full page and player at once. A short-height compact mode should collapse optional top chrome, keep a small transport row, and allow the page and now-playing view to scroll so every action remains reachable.
- Ultrawide screens cap the outer regions. The workspace can show more cards or columns within its existing content-width ceiling; prose and hero copy receive readable line-length caps.

## Player experience and visual direction

### Side player

```text
┌──────────────────────────────┐
│ NOW PLAYING       Queue  Layout│
│       square artwork         │
│ Title                         │
│ Artist · album                │
│ Playing · quality · stream    │
│ ───── progress ─────  2:30/3:10│
│   ♡   ⇄   ◀   ●   ▶   ↻   ⋯  │
│ volume ─────                 │
├──────────────────────────────┤
│ UP NEXT  7 · 20 min  Save  ⋯ │
│ active / upcoming rows        │
└──────────────────────────────┘
```

The player header makes layout and queue access discoverable without occupying artwork. Keep cover art prominent but never so tall that transport disappears. Give title and artist first visual priority; quality and exact stream detail are one quiet status line. Preserve the existing palette and glass surfaces. A restrained, static artwork-derived tint may sit behind the artwork/metadata region, at low opacity and behind an opaque text scrim. Keep palette accent reserved for active controls and progress; source and quality badges retain their semantic colors. Do not introduce a continuously animated visualizer into the player.

Queue-focus mode keeps transport and seeking in place while the artwork becomes a compact thumbnail. The current abrupt 64px artwork strip is replaced by a normal small cover plus title and artist. The queue count, total duration, jump-to-current, save, clear/undo, reorder, and context actions remain available.

### Bottom player

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ [art] Title / Artist    ♡     ◀   ●   ▶      Mute ─volume─   Queue  Layout │
│       2:30              ─────────── seek / buffer ─────────────     3:10 │
└────────────────────────────────────────────────────────────────────────────┘
                     Queue opens in a drawer above the bar
```

This is a purpose-built horizontal view: small art, short metadata, a stable central transport group, a long seek target, and a separate queue action. The bar keeps playing and seeking available while the queue drawer is open. At 680-900px, it uses a compact two-row arrangement; secondary controls move into a labelled menu. The drawer is at most about 420px wide and 60dvh tall, constrained to the viewport with safe margins. Its title, queue count/runtime, jump-to-current, save, clear/undo, drag/keyboard reorder, and row actions remain intact. Escape closes it and returns focus to the Queue button.

### Beautification specification

**Visual concept: a quiet listening deck.** Keep the dark, atmospheric NOORwave shell, rounded glass, cyan/selected accent, display-serif track title, and artwork. Make the player feel composed by giving each element one job: artwork sets the mood, type identifies the track, one bright play control anchors interaction, and the queue sits on a calmer surface. The album cover may color the player atmosphere; it must not recolor navigation or make control state ambiguous.

| Element | Proposed treatment | Why |
| --- | --- | --- |
| Player surface | Two subtle tonal layers: a softly tinted artwork/metadata zone and a neutral queue zone, separated by a low-contrast hairline. Avoid nested full-card borders. | Gives the player structure while preserving the existing glass language. |
| Artwork | One square with a thin inner rim, 18-22px radius, and restrained shadow. Side mode caps its size; bottom mode uses a crisp 56-64px cover. No text or badges over the cover except the Quiet Mode action on hover/focus. | Keeps artwork premium without letting it dominate every window. |
| Text hierarchy | Track title first in the current display serif; artist immediately below in the body face; album and source one quieter step down. Long text truncates cleanly with a full-title affordance. Use tabular numerals for time. | Track identity stays readable at narrow widths and high zoom. |
| Status and quality | One compact line for playing/buffering/error and stream quality. Prefer text plus a small dot/mark over multiple bright pills. Exact format details live in a tooltip or secondary line where room allows. | Reduces badge clutter while keeping useful diagnostics. |
| Transport | Filled accent play/pause button around 44-48px; previous/next have quieter circular surfaces; mode buttons use a small active accent indicator. Keep visual icon size modest inside generous hit areas. | Creates one clear focal point and stronger usability. |
| Progress and volume | Thin visible rails with larger invisible targets, distinct buffered fill, visible focus ring, and a thumb that appears on interaction. Time labels align to stable edges. | Makes precision controls easier to use without a heavy slider look. |
| Queue rows | Denser 48-56px rhythm, 36-40px artwork, title/artist truncation, consistent action column. Active row gets a subtle tint and a 2px accent edge; played rows recede without becoming illegible. Row actions appear on hover and focus and remain available to touch/keyboard. | Gives the queue a clear reading order and improves scanning. |
| Bottom bar | Slightly raised surface with a fine top border and very soft upward shadow. Artwork, title, transport, and Queue button align to a shared baseline; seek is the only full-width rail. | Reads as an intentional player, not a vertical card turned sideways. |

Use an 8px spacing rhythm with a few 4px adjustments. Keep the player header and queue header about 16-20px from their edges, with denser 8-12px row spacing. A static art-derived wash can occupy only the upper player zone at roughly 8-16% effective strength behind a contrast-protecting scrim; if extraction fails, use the selected palette's existing haze. In light mode, lower the wash strength and use a stronger opaque surface behind text. The palette accent remains the interaction color, while favorite, quality, warning, and error retain semantic colors.

Motion is brief and purposeful: artwork crossfades on track change, hover changes surface/color, and queue opening eases over roughly 180-240ms. Avoid looping glows, bouncing controls, and automatic text movement. Respect reduced-motion preferences by replacing movement with immediate state changes. The idle player uses a small music mark and a concise prompt in the artwork zone; loading and errors reserve space so controls do not jump.

Prepare side, bottom, compact, queue-focus, queue-drawer, idle, and error screenshots in both dark and light themes before polishing individual CSS rules. Review them side by side at 100% and 200% zoom; the same hierarchy should survive both.

### Quality-of-life work, in priority order

| Priority | Improvement | Behavior |
| --- | --- | --- |
| Required | Layout chooser | Three labelled icons with a clear selected state; setting mirrors it. A temporary narrow-window fallback is communicated. |
| Required | Reliable queue access | Persistent Queue button in every player layout, with count; bottom queue drawer supports the full existing queue workflow. |
| Required | Seek affordance | Increase the invisible hit area and keyboard/focus visibility of the slim progress control. Make the existing scrub-time feedback easier to see; offer a small elapsed/remaining-time toggle without shifting the layout. |
| Required | Control priority | Play/pause, previous/next, seek, queue, and volume never vanish behind overflow. Favorite, shuffle, repeat, Quiet Mode, and track actions remain one deliberate action away when compact. Hit targets are at least 40 CSS px on desktop and 44 CSS px for touch, even when icons look smaller. |
| Required | Clear playback state | Loading, buffering, paused, playing, and actionable error states occupy one stable status slot; errors never cover transport or queue access. |
| Next | Up-next glance | In bottom mode, a short `Next: title` line can open the queue drawer when space permits. It uses existing queue data. |
| Next | Better title handling | Ellipsis by default, full title through focus/hover tooltip and existing media link; avoid automatic marquee motion. |
| Next | Artwork tint | Optional low-cost static tint from the existing art palette with a safe fallback and reduced-motion behavior. |

No new playback backend behavior is required for the first player delivery. Audio quality, queue data, automix, and track actions remain driven by their existing stores and APIs.

## Theme work alongside the layout

- Separate the choice of surface mode (`dark`, `light`, `system`) from accent palette and wallpaper color source. Preserve existing saved choices during migration; fresh installs can keep the current dark default.
- Introduce semantic player tokens for surface, scrim, border, muted text, progress track, focus ring, and artwork tint. Replace player-specific hardcoded dark colors and audit the light variant.
- Test ordinary text against the player surface, active controls against all supported accents, and text over artwork-derived washes. Keep source/quality colors semantically stable.
- Keep the current typography, logo, cyan/selected accent behavior, artwork, and rounded glass language. Use spacing and hierarchy to modernize the player, rather than changing brand identity.

| Surface mode | Color direction | Player treatment |
| --- | --- | --- |
| Dark | Deep neutral charcoal with a slight cool cast; soft separation between background, panel, and active row. | Artwork wash is visible but subdued; one bright accent at play/progress. |
| Light | Warm off-white and clean white surfaces with graphite text, rather than translucent gray over a busy wallpaper. | Lower-opacity artwork wash, firmer text scrim, darker focus/progress contrast. |
| System | Follows OS light/dark changes while retaining the chosen accent and wallpaper preferences. | Switches semantic tokens without remounting playback or clearing queue state. |

An optional high-contrast override can flatten glass transparency and strengthen borders/focus rings for either surface mode. Show a small live player preview in Appearance so people can judge an accent or surface choice against real text, transport, and progress before selecting it. Keep wallpaper and album-art color controls separate from this preview's readable chrome.

## Launch and preference memory

The placement setting joins the existing persisted UI zoom and theme choices. Keep these independent: zoom changes usable CSS space, while placement is a preference that may temporarily fall back. For a fresh desktop install, retain a 1280 x 800 starting size where it fits. After the responsive shell is stable, add explicit restoration of last non-maximized window size/position and maximized state; clamp restored bounds to a currently attached monitor's usable area so a removed portrait/ultrawide monitor cannot reopen the app off-screen. Do not infer zoom from monitor DPI or overwrite the user's stored zoom when moving between monitors.

## Implementation sequence

### 1. Shell constraints and measurement

Edit `frontend/src/app.css` and `frontend/src/routes/+layout.svelte`: replace the `32vw` rule, define bounded shell dimensions and grid areas, enforce workspace minima, and move the mobile threshold below the compact desktop layouts. Add a small pure layout decision function or equivalent CSS test fixture so breakpoints follow the width budget. Verify both supplied screenshot scenarios first.

**Done when:** At 2564 x 1660 and approximately 150%/200% zoom, panel-to-navigation width remains within the bounded design ratio; at 720 x 500, the shell does not horizontally overflow and core controls are reachable.

### 2. Player placement preference

Add a validated persisted store, for example `frontend/src/lib/stores/playerLayout.ts`, with `right | left | bottom` and default `right`. Add the chooser to `PlayerBar.svelte`, Settings > Appearance, and settings search. Derive an effective layout from width without overwriting the saved choice. Use grid areas to place the same shell regions on either side or bottom. Include the video-queue branch in layout selection so it does not occupy a hidden side track.

**Done when:** Selection survives reload; resize/zoom falls back and restores predictably; the layout button and setting agree on selected and effective positions.

### 3. Player composition and queue drawer

Keep playback state and commands in `frontend/src/lib/stores/player.ts` and the root layout. Reuse `NowPlayingMetadata`, `NowPlayingProgress`, and `NowPlayingTransport`. Give `PlayerBar.svelte` orientation-specific grid areas and artwork sizes. Extract the existing queue view/handlers from the large root layout into a component only as needed to keep a single queue interaction surface. Add a bottom-mode drawer with focus return, Escape handling, click-away behavior, and safe bounds. Keep the existing queue-expanded preference for side mode; bottom drawer open state is session-only.

**Done when:** Seeking, volume, favorite, shuffle/repeat, track menus, queue save/clear/undo, reorder, jump-to-current, pending rows, and announcements behave in each layout. Switching while music plays does not interrupt playback or reset position.

### 4. Centre content adaptation

Give the workspace a named inline-size query container. Apply container-based density to album and playlist details, library tracks, search results, and home rails: first reduce spacing and card width, then hide secondary metadata/columns. Keep the essential title/artist/primary action visible. Preserve the media rail's existing container-query behavior.

**Done when:** Each route remains usable with a 560px content region and uses extra width deliberately on large and ultrawide monitors.

### 5. Visual finish and theme tokens

Apply the beautification specification above to the player header, artwork, metadata, transport, progress/volume, queue rows, bottom bar, empty/error states, and compact queue-focus row. Add semantic theme tokens rather than per-component hardcoded colors. Check dark/light and several high/low luminance accents, with and without wallpaper. Include reduced-motion and high zoom states.

**Done when:** Side and bottom screenshots form one coherent visual family; the play control and track identity are immediately recognizable; text remains readable over artwork and wallpaper; controls have visible hover, focus, pressed, disabled, and error states; and queue rows scan cleanly without losing their actions.

### 6. Window restoration

Once the new minimum-width and short-height behaviors are verified, implement the launch policy above in the Tauri window setup, with monitor-bound clamping and a safe 1280 x 800 fallback. Keep this as a separate task so window-state changes do not obscure CSS layout regressions.

**Done when:** Relaunch restores a usable window on the same monitor, moving or removing a monitor returns the window on-screen, and the saved UI zoom is still applied exactly once.

## Validation matrix

Capture screenshots and perform interaction checks at 720 x 500, 1280 x 800, 1366 x 768, 1920 x 1080, 2564 x 1660 at 150% and 200% zoom, 3440 x 1440, 1080 x 1920, 1440 x 2560, 390 x 844, and 320 x 568. Include 50%, 100%, 150%, and 200% UI zoom across representative widths; test Windows display scaling separately from app zoom where possible.

At each effective layout verify: no hidden primary control, no shell horizontal scroll, bounded outer regions, queue availability, ellipsis/tooltip behavior for long names, usable seek and volume by keyboard and pointer, focus restoration after menus/drawers, safe-area margins, reduced motion, and correct persistence. Run the existing frontend check/lint and relevant contract tests, then add focused layout-state and player interaction tests for any new behavior. Test a live Tauri build because browser rendering alone cannot verify `setWebviewZoom` or the native minimum window.
