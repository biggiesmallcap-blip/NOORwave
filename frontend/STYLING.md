# Styling — design system reference

This document is the load-bearing reference for how styling works in `frontend/src`. It is short on purpose. Read it before adding new tokens, new global CSS rules, or new component-level styling patterns.

## Core principle: every dimension scales fluidly

The app targets Tauri windows that range from 720 × 500 (the configured floor) up to 4K and beyond. Spacing, radii, typography, and content widths all interpolate with viewport using `clamp()` so the UI looks proportional at any size. Hard pixel values are reserved for icons (intrinsic SVG size), structural rhythm where snapping is intended, and per-component artwork floors.

If you're tempted to add `padding: 16px` or `border-radius: 12px` directly, ask: does this need to scale with the viewport? In almost every case the answer is yes — use a token.

## Tokens (in [`src/app.css`](src/app.css))

| Family | Tokens | Use for |
| --- | --- | --- |
| Spacing | `--space-1` … `--space-7` (3-48 px), `--gap-sm` / `--gap` / `--gap-lg` | Padding, margin, grid gap, flex gap |
| Radii | `--radius-xs` (4-6 px), `--radius-sm` (7-10 px), `--radius` (12-16 px), `--radius-lg` (15-22 px) | Card corners, panel corners. `50%` and `999px` for circles/pills are fine raw. |
| Typography | `--font-size-xs` (11-13 px) … `--font-size-3xl` (28-40 px) | All component font sizes that map cleanly to a step. Odd one-offs (e.g. 13 px) stay raw. |
| Motion | `--motion-fast` (130 ms), `--motion-base` (210 ms), `--motion-slow` (340 ms) | All `transition` durations. No raw `0.13s` / `0.21s` / `0.34s`. |
| State | `--state-error`, `--state-warning`, `--state-success`, `--state-active` | Status colours. **Never** use `--danger`, `--color-error` — they are not defined. |
| Surface | `--bg-base`, `--bg-elevated`, `--bg-raised`, `--bg-surface`, `--bg-hover`, `--panel-bg` | Backgrounds. **Never** use `--bg-glass`, `--surface-hover` — they are not defined. |
| Borders | `--border-subtle`, `--border-muted`, `--border-strong` | All component borders. |
| Content | `--content-width` (clamp 1280-2400 px) | Page-shell `max-width` so content scales on wide monitors. |

## Global utility classes

Use these instead of reimplementing the surface:

- `.glass`, `.glass-panel`, `.glass-tile` — translucent panel surfaces (backdrop-filter + bg + border + shadow). If you find yourself writing `backdrop-filter: blur(...)` plus a translucent gradient + a subtle border, replace with one of these classes.
- `.btn`, `.btn-primary`, `.btn-glass` — pill buttons. Don't roll your own button visual unless you're building a chip or icon-only variant.
- `.quality-badge.hires|.lossless|.lossy` — quality indicator pill.

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

Stylelint runs on `src/**/*.css` and forbids the legacy tokens (`--danger`, `--color-error`, `--bg-glass`, `--surface-hover`). It also disallows raw `font-size: Npx`. Run:

```text
npm run lint:css
```

Failures are blocking; fix them before committing. Component-level (`.svelte`) CSS is not linted yet — that's a follow-up once `postcss-html` is wired in.

## Before you add a token

Justify why an existing token cannot represent the value. New tokens grow the design system and become a maintenance cost. The current scale already covers spacing 3-48 px, radii 4-22 px, type 11-40 px — most additions to the system can be expressed in those.

If you need a value outside the scale (e.g. a 64 px hero-art floor), prefer a per-component CSS custom property scoped to the parent over a new global token.
