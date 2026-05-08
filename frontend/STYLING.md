# Styling — design system reference

This document is the load-bearing reference for how styling works in `frontend/src`. It is short on purpose. Read it before adding new tokens, new global CSS rules, or new component-level styling patterns.

## Core principle: every dimension scales fluidly

The app targets Tauri windows that range from 720 × 500 (the configured floor) up to 4K and beyond. Spacing, radii, typography, and content widths all interpolate with viewport using `clamp()` so the UI looks proportional at any size. Hard pixel values are reserved for icons (intrinsic SVG size), structural rhythm where snapping is intended, and per-component artwork floors.

If you're tempted to add `padding: 16px` or `border-radius: 12px` directly, ask: does this need to scale with the viewport? In almost every case the answer is yes — use a token.

## Tokens (in [`src/app.css`](src/app.css))

| Family | Tokens | Use for |
| --- | --- | --- |
| Spacing | `--space-1` … `--space-7` (3-48 px), `--gap-sm` / `--gap` / `--gap-lg` | Padding, margin, grid gap, flex gap |
| Radii | `--radius-xs` (4-6 px), `--radius-sm` (7-10 px), `--radius-md` (10-14 px), `--radius-lg` (15-22 px). `--radius` is a legacy alias for `--radius-md`. | Card corners, panel corners, modal corners. `50%` and `999px` for circles/pills are fine raw. |
| Typography | `--font-size-xs` (11-13 px) … `--font-size-3xl` (28-40 px) | All component font sizes that map cleanly to a step. Odd one-offs (e.g. 13 px) stay raw. |
| Motion | `--motion-fast` (130 ms), `--motion-base` (210 ms), `--motion-slow` (340 ms) | All `transition` durations. No raw `0.13s` / `0.21s` / `0.34s`. |
| Blur | `--blur-base` (8 px), `--blur-overlay` (16 px), `--blur-modal` (24 px) | All `backdrop-filter` values. Three tiers is the maximum that should ever exist; anything else is drift. |
| State | `--state-error`, `--state-warning`, `--state-success`, `--state-active`, `--state-favorite`, `--state-favorite-glow` | Status colours. **Never** use `--danger`, `--color-error` — they are not defined. |
| Service | `--service-spotify`, `--service-tidal`, `--service-lastfm` | Service brand colors for source badges and service-branded heroes only. **Stay stable across themes** — they are brand cues, not theme variables. |
| Surface | `--bg-base`, `--bg-elevated`, `--bg-raised`, `--bg-surface`, `--bg-hover`, `--panel-bg` | Backgrounds. **Never** use `--bg-glass`, `--surface-hover` — they are not defined. |
| Borders | `--border-subtle`, `--border-muted`, `--border-strong`, `--panel-border` | All component borders. Don't write raw `1px solid rgba(255,255,255,0.06)` — it won't theme. |
| Accent | `--accent`, `--accent-soft`, `--accent-line`, `--accent-strong`, `--accent-glow` | Interactive accents (active states, primary buttons, focus rings). Tracks the user's profile palette. **Never** hardcode hex values for accents** — onboarding's `#4a6dd8` was migrated to `var(--accent)` so it follows the theme. |
| Content | `--content-width` (clamp 1280-2400 px) | Page-shell `max-width` so content scales on wide monitors. |

## Global utility classes

Use these instead of reimplementing the surface:

- `.glass`, `.glass-panel`, `.glass-tile` — translucent panel surfaces (backdrop-filter + bg + border + shadow). If you find yourself writing `backdrop-filter: blur(...)` plus a translucent gradient + a subtle border, replace with one of these classes.
- `.btn`, `.btn-primary`, `.btn-glass` — pill buttons. Don't roll your own button visual unless you're building a chip or icon-only variant.
- `.quality-badge.hires|.lossless|.lossy` — quality indicator pill.

### Glass decision tree

When you need a translucent surface, follow this order:

1. **Compact tile or badge?** → `.glass-tile` (uses `--blur-base`, `--radius-sm`, `--panel-border`).
2. **Elevated card / panel?** → `.glass` or `.glass-panel` (uses `--blur-overlay`, `--radius-md` or `--radius-lg`, full glass treatment).
3. **Modal / context menu / palette?** → Don't use a class — use `backdrop-filter: var(--blur-modal)` directly with a non-translucent dark background (`rgba(12,12,24,0.96)` or similar) and `var(--radius-md)` corners. The class doesn't fit because modals tint deeper than the canonical glass.
4. **Page scrim / dimmer behind a modal?** → Inline `backdrop-filter: blur(2-6px)` with a dim background. These are *not* glass surfaces; they're page-level dimmers and stay raw.

If you reach for a 4th tier (12 px blur? 20 px blur?), pick the closest token. The whole point is three tiers, not three-plus-special-cases.

## Auto-fit grids over fixed-N columns

Card grids should reflow naturally as viewport changes:

```css
/* ✗ */
grid-template-columns: repeat(4, 1fr);
@media (max-width: 1180px) { grid-template-columns: repeat(3, 1fr); }
@media (max-width: 760px)  { grid-template-columns: repeat(2, 1fr); }

/* ✓ */
grid-template-columns: repeat(auto-fit, minmax(min(220px, 100%), 1fr));
```

Track-row layouts and structural two-column splits (e.g. sidebar + content) keep their explicit columns — auto-fit is for **card** grids.

## Page widths

Page shells use `var(--content-width)` so they scale with viewport:

```css
.my-page {
  width: min(100%, var(--content-width));
  margin: 0 auto;
}
```

**Don't** use `min(1200px, var(--content-width))` for inner sections — `min()` picks the smaller value, which caps the section at 1200 px on wide viewports. Just use `var(--content-width)` directly, or a different non-fluid cap if the section deliberately stays narrow (e.g. a 640 px-wide search input).

## Artwork sizing

Album art, artist photos, video thumbnails, playlist covers use `aspect-ratio` plus a clamped width:

```css
.album-art {
  width: clamp(140px, 18vw, 220px);
  aspect-ratio: 1 / 1;
  border-radius: var(--radius-sm);
  object-fit: cover;
}
```

For grids, anchor the card width (and label widths) to the same custom property so they stay in lockstep:

```css
.albums-row {
  --album-card-w: clamp(112px, 11vw, 156px);
}
.album-card { width: var(--album-card-w); }
.art-wrap { width: var(--album-card-w); aspect-ratio: 1 / 1; }
.album-title { width: var(--album-card-w); }
```

Small inline thumbnails (track rows, video lists) stay near-fixed: `clamp(2rem, 3vw, 2.5rem)` so they don't shrink to absurdity in narrow lists.

## Z-index scale

| Token | Value | Use for |
| --- | --- | --- |
| `--z-base` | 1 | In-flow stacking (e.g. a hero z-1 element) |
| `--z-raised` | 10 | Sticky-within-panel headers, hover lifts |
| `--z-overlay` | 100 | Dropdowns, popovers, hover cards |
| `--z-modal` | 1000 | Modal dialogs (confirm prompts, settings sheets) |
| `--z-toast` | 2000 | Toasts, command palette |
| `--z-tooltip` | 3000 | Tooltips (must overlap modals) |

**Don't add new raw `z-index: NNN` values.** Use the tokens above. When two elements within the same layer need explicit ordering, use `calc(var(--z-modal) + 1)` instead of escalating to a higher layer.

Existing raw `z-index` values from before the scale was introduced have been migrated for the highest-impact sites (toasts, command palette, context menu, modals, tooltips). Lower-z values within panels (z 1–80) are left raw because they represent within-panel stacking that doesn't need to participate in the global scale.

## Linting

Stylelint runs on `src/**/*.{css,svelte}` and:

- **Errors** on legacy tokens (`--danger`, `--color-error`, `--bg-glass`, `--surface-hover`) and on hardcoded hex values that should be theme tokens (`#4a6dd8`, `#5a7ce8`, `rgba(155,111,255,...)`, `rgba(74,109,216,...)`).
- **Warns** on raw `font-size: Npx` — prefer `var(--font-size-*)`.

```text
npm run lint:css
```

Errors are blocking; fix them before committing. Warnings are advisory but reviewed at PR time. `.svelte` components are linted via `postcss-html`.

## Before you add a token

Justify why an existing token cannot represent the value. New tokens grow the design system and become a maintenance cost. The current scale already covers spacing 3-48 px, radii 4-22 px, type 11-40 px — most additions to the system can be expressed in those.

If you need a value outside the scale (e.g. a 64 px hero-art floor), prefer a per-component CSS custom property scoped to the parent over a new global token.
