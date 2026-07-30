# Styling - design system reference

This document is the load-bearing reference for how styling works in `frontend/src`. It is short on purpose. Read it before adding new tokens, new global CSS rules, or new component-level styling patterns.

## Core principle: every dimension scales fluidly

The app targets Tauri windows that range from 720 x 500 (the configured floor) up to 4K and beyond. Spacing, radii, typography, and content widths all interpolate with viewport using `clamp()` so the UI looks proportional at any size. Hard pixel values are reserved for icons (intrinsic SVG size), structural rhythm where snapping is intended, and per-component artwork floors.

If you're tempted to add `padding: 16px` or `border-radius: 12px` directly, ask: does this need to scale with the viewport? In almost every case the answer is yes - use a token.

## Tokens (in [`src/app.css`](src/app.css))

| Family | Tokens | Use for |
| --- | --- | --- |
| Spacing | `--space-1` ... `--space-7` (3-48 px), `--gap-sm` / `--gap` / `--gap-lg` | Padding, margin, grid gap, flex gap |
| Radii | `--radius-xs` (4-6 px), `--radius-sm` (7-10 px), `--radius-md` (10-14 px), `--radius-lg` (15-22 px). `--radius` is a legacy alias for `--radius-md`. | Card corners, panel corners, modal corners. `50%` and `999px` for circles/pills are fine raw. |
| Typography | `--font-size-2xs` (8-10 px), `--font-size-xs` (11-13 px), `--font-size-sm` (13-15 px), `--font-size-md` (15-17 px), `--font-size-lg` (17-20 px), `--font-size-xl` (20-26 px), `--font-size-2xl` (24-32 px), `--font-size-3xl` (28-40 px), `--font-size-4xl` (40-56 px) | All component font sizes - no raw px or raw rem are accepted by lint. The bookend tokens (`2xs`, `4xl`) cover micro-labels and hero displays; new sizes outside this range are a design red flag, not a token candidate. |
| Weight | `--font-weight-medium` (500), `--font-weight-semibold` (600), `--font-weight-bold` (700) | All `font-weight`. Lint also permits raw `400` (the rare regular case) and raw `800` (the rare extra-bold case) - both require a code comment justifying the deviation. |
| Line height | `--line-height-tight` (1.1), `--line-height-snug` (1.3), `--line-height-normal` (1.5), `--line-height-loose` (1.6) | All `line-height`. Lint also permits raw `1` for inherently single-line elements (icons, button labels, chips), and raw `0` for image-wrapper containers that need to collapse the inline baseline gap. |
| Motion | `--motion-fast` (130 ms), `--motion-base` (210 ms), `--motion-slow` (340 ms) | All `transition` durations. No raw `0.13s` / `0.21s` / `0.34s`. **Each token bundles `cubic-bezier(0.25, 0.8, 0.25, 1)` - see "Motion footgun" below.** |
| Blur | `--blur-base` (8 px), `--blur-overlay` (16 px), `--blur-modal` (24 px) | All `backdrop-filter` values. Three tiers is the maximum that should ever exist; anything else is drift. |
| State | `--state-error`, `--state-warning`, `--state-success`, `--state-active`, `--state-favorite`, `--state-favorite-glow` | Status colours. **Never** use `--danger`, `--color-error` - they are not defined. |
| Service | `--service-spotify`, `--service-tidal`, `--service-lastfm` | Service brand colors for source badges and service-branded heroes only. **Stay stable across themes** - they are brand cues, not theme variables. |
| Surface | `--bg-base`, `--bg-elevated`, `--bg-raised`, `--bg-surface`, `--bg-hover`, `--panel-bg` | Backgrounds. **Never** use `--bg-glass`, `--surface-hover` - they are not defined. |
| Borders | `--border-subtle`, `--border-muted`, `--border-strong`, `--panel-border` | All component borders. Don't write raw `1px solid rgba(255,255,255,0.06)` - it won't theme. |
| Accent | `--accent`, `--accent-soft`, `--accent-line`, `--accent-strong`, `--accent-glow` | Interactive accents (active states, primary buttons, focus rings). Tracks the user's profile palette. **Never** hardcode hex values for accents** - onboarding's `#4a6dd8` was migrated to `var(--accent)` so it follows the theme. |
| Content | `--content-width` (clamp 1280-2400 px) | Page-shell `max-width` so content scales on wide monitors. |

## Motion footgun: never write `var(--motion-X) ease`

The motion tokens are **not bare durations**. Each one bundles the canonical easing:

```css
--motion-fast: 130ms cubic-bezier(0.25, 0.8, 0.25, 1);
--motion-base: 210ms cubic-bezier(0.25, 0.8, 0.25, 1);
--motion-slow: 340ms cubic-bezier(0.25, 0.8, 0.25, 1);
```

That means `var(--motion-base)` already expands to `210ms cubic-bezier(...)`. If you then append a second timing function like `ease`, the declaration becomes invalid CSS (two timing functions in one transition value), and **the whole rule is dropped silently**. The hover snaps with no transition at all - which reads as jarring, broken, and "not what Last.fm tiles do."

```css
/* BAD -- invalid, dropped, snap-on-hover */
transition: background var(--motion-base) ease, border-color var(--motion-base) ease;

/* BAD -- same problem with the other tokens */
transition: color var(--motion-fast) ease;

/* GOOD -- token brings its own bezier */
transition: background var(--motion-base), border-color var(--motion-base);

/* GOOD -- only add a timing function if you mean to override the token */
transition: transform var(--motion-base) ease-out;
```

Card and mural tiles use the token's bezier as their reference soft-graceful feel (see [ChartMural.svelte](src/lib/components/charts/ChartMural.svelte) for the canonical patterns on `background`, `border-color`, and art `transform`). Match those transition signatures on new tiles.

## Entry motion

The `--motion-*` tokens above cover `transition` (state changes on an element that is already on screen). They are not the answer for content that arrives after mount and would otherwise snap into place. Two classes in [`src/app.css`](src/app.css) own that:

- `.rise-in-shelf` - sections, shelves, panels. 340 ms, 70 ms step, capped at 8 indices.
- `.rise-in-card` - cards inside a rail or grid. 300 ms, 22 ms step, capped at 11 indices.

The parent writes `--rise-index` inline; the child derives its delay from it. Both classes no-op under `prefers-reduced-motion: reduce`.

A page-level `.animate-in` cannot do this job: it fires once, before any data lands, so everything that resolves later still pops.

Three rules, all of them learned the hard way (the source comment in `app.css` has the full account):

1. **Cap `--rise-index`** with a modulo of roughly a screenful. Uncapped, the last card in a long list waits seconds for its turn, and a page appended mid-scroll parks every card at the maximum delay instead of cascading.
2. **`.rise-in-card` uses `backwards` fill, not `both`.** An animation of opacity/transform gives its element a stacking context for as long as it is applied, and `both` keeps it applied forever - which traps a popout's z-index inside its own card so it paints under the cards after it.
3. **Don't add a third variant.** Shelf and card are the two granularities that exist.

Copies of this pattern in `videos/liked`, `VideoSetShelf` and the library mural predate these classes and are pinned by contract tests. `FOLLOWUPS.md` tracks the back-migration; don't add a fourth copy in the meantime.

## Global utility classes

Use these instead of reimplementing the surface:

- `.glass`, `.glass-panel`, `.glass-tile` - translucent panel surfaces (backdrop-filter + bg + border + shadow). If you find yourself writing `backdrop-filter: blur(...)` plus a translucent gradient + a subtle border, replace with one of these classes.
- `.btn`, `.btn-primary`, `.btn-glass` - pill buttons. Don't roll your own button visual unless you're building a chip or icon-only variant.
- `.quality-badge.hires|.lossless|.lossy` - quality indicator pill.
- `.rise-in-shelf`, `.rise-in-card` - the canonical entry motion for content that arrives *after* mount. Do not hand-roll another rise animation; see "Entry motion" below.

## Media links and context menus

Track, album, artist, and video references should resolve inside NOORwave whenever the app has a route for them. Do not send media-reference clicks to `tidal.com` from cards, rows, now-playing metadata, quiet mode, or context menus.

- Local artists use `/artists/:id`.
- Local albums use `/albums/:id`.
- TIDAL artists use `/tidal/artists/:id`.
- TIDAL albums use `/tidal/albums/:id`.
- TIDAL videos use `/videos?videoId=:id`.
- TIDAL track titles do not open an external TIDAL page. Link them only when there is a useful in-app destination, such as the local album page.

Use `$lib/player/media_link.ts` for canonical media hrefs and menu delegation when rendering mixed local/TIDAL metadata. Use the shared menu builders (`buildTrackMenu`, `buildTidalTrackMenu`, `buildAlbumMenu`, `buildArtistMenu`, `buildVideoMenu`) instead of inline menu arrays. Queue rows are already in the queue, so queue-context menus must not show duplicate `Add to queue` actions.

All right-click menus are rendered by `ContextMenu.svelte`; do not create one-off menu animation styles in callers. Menus should enter and exit through the shared context-menu motion:

- Enter with a short opacity, blur, and scale transition.
- Exit through the shared `closing` state in `context_menu.ts`, then remove after `CONTEXT_MENU_EXIT_MS`.
- Keep shared enter/exit timing short enough for compact queue menus while preserving the soft fade/blur exit.
- Close on pointer leave, outside click, Escape, scroll, and successful action selection.
- Keep submenus inside the root menu surface so pointer-leave dismissal does not fire while moving into a submenu.

### Glass decision tree

When you need a translucent surface, follow this order:

1. **Compact tile or badge?** -> `.glass-tile` (uses `--blur-base`, `--radius-sm`, `--panel-border`).
2. **Elevated card / panel?** -> `.glass` or `.glass-panel` (uses `--blur-overlay`, `--radius-md` or `--radius-lg`, full glass treatment).
3. **Modal / context menu / palette?** -> Don't use a class - use `backdrop-filter: var(--blur-modal)` directly with a non-translucent dark background (`rgba(12,12,24,0.96)` or similar) and `var(--radius-md)` corners. The class doesn't fit because modals tint deeper than the canonical glass.
4. **Page scrim / dimmer behind a modal?** -> Inline `backdrop-filter: blur(2-6px)` with a dim background. These are *not* glass surfaces; they're page-level dimmers and stay raw.

If you reach for a 4th tier (12 px blur? 20 px blur?), pick the closest token. The whole point is three tiers, not three-plus-special-cases.

## Auto-fit grids over fixed-N columns

Card grids should reflow naturally as viewport changes:

```css
/* BAD */
grid-template-columns: repeat(4, 1fr);
@media (max-width: 1180px) { grid-template-columns: repeat(3, 1fr); }
@media (max-width: 760px)  { grid-template-columns: repeat(2, 1fr); }

/* GOOD */
grid-template-columns: repeat(auto-fit, minmax(min(220px, 100%), 1fr));
```

Track-row layouts and structural two-column splits (e.g. sidebar + content) keep their explicit columns - auto-fit is for **card** grids.

## Page widths

Page shells use `var(--content-width)` so they scale with viewport:

```css
.my-page {
  width: min(100%, var(--content-width));
  margin: 0 auto;
}
```

**Don't** use `min(1200px, var(--content-width))` for inner sections - `min()` picks the smaller value, which caps the section at 1200 px on wide viewports. Just use `var(--content-width)` directly, or a different non-fluid cap if the section deliberately stays narrow (e.g. a 640 px-wide search input).

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

For **grids**, anchor the card width (and label widths) to the same custom property so they stay in lockstep:

```css
.albums-row {
  --album-card-w: clamp(112px, 11vw, 156px);
}
.album-card { width: var(--album-card-w); }
.art-wrap { width: var(--album-card-w); aspect-ratio: 1 / 1; }
.album-title { width: var(--album-card-w); }
```

Small inline thumbnails (track rows, video lists) stay near-fixed: `clamp(2rem, 3vw, 2.5rem)` so they don't shrink to absurdity in narrow lists.

### Horizontal rails size from the rail, not the viewport

Do **not** apply the clamped-card-width pattern above to a horizontal scrolling rail, and do not pin rail cards at a fixed px width. Use [`MediaRail.svelte`](src/lib/components/ui/MediaRail.svelte) and let it derive the card width.

A rail is as wide as the content column, which is itself a clamp of the viewport. Pin the card and the number that fit is whatever that width happens to divide into, with the remainder a card clipped at an arbitrary fraction - different, and equally accidental, at every window size.

`MediaRail.fluid` instead solves for `--cols` whole cards plus a deliberate `--peek: 0.35` of one more, so the partial card is a consistent scroll affordance rather than a leftover. Two consequences worth knowing before you touch it:

- `--cols` steps on **container** queries (560 / 760 / 980 px), not media queries, so a rail in a narrow column behaves like a narrow rail instead of like a wide viewport.
- The rail uses `justify-content: safe center`. The `safe` keyword is load-bearing: plain `center` on a scroll container puts overflowing content half off the start edge where it can't be scrolled back to. With `safe`, an overflowing rail falls back to `flex-start`, so the centring only ever applies to rails that don't fill their width.

Rail section headings go through [`SectionHeader.svelte`](src/lib/components/ui/SectionHeader.svelte) - one header component for the whole app. Don't hand-roll a shelf title.

### Search top-result heroes

Search top-result heroes use one text recipe for tracks, albums, and artists:

- Eyebrow: `--font-size-2xs`, `--font-weight-semibold`, uppercase, `--line-height-tight`, and `letter-spacing: 0`.
- Title: `--font-display`, `--font-size-3xl`, `--font-weight-semibold`, `--line-height-tight`, and `letter-spacing: 0`.
- Subtitle: `--font-size-sm`, `--font-weight-medium`, `--line-height-snug`, single-line truncation.
- Primary action: pill button, `--font-size-sm`, `--font-weight-semibold`, `line-height: 1`, and `letter-spacing: 0`.

Do not special-case track heroes into the body font. The top-result card is a hero surface, so its title should keep the same display treatment regardless of whether the result is a track, album, or artist.

### TIDAL artwork URLs

TIDAL artwork must not be rendered from raw API or database URLs. Always route a TIDAL-capable URL through `$lib/utils/artwork.upscaleTidalArtwork(url, size)` directly, or through `$lib/components/ui/ArtworkImage.svelte`.

Allowed TIDAL sizes are `80`, `160`, `320`, `640`, `750`, `1080`, and `1280`. Use `320` for rows, rails, and small tiles, `640` for hero cards and detail covers, and `1280` for lockscreen, MediaSession, and blurred backdrop art. Do not pass arbitrary sizes such as `256` or `512`.

Every rendered image that may receive a TIDAL URL needs an error fallback. Prefer `ArtworkImage` for route and component markup because it normalizes the URL, resets failure state when the source changes, and renders a stable initials fallback. CSS `background-image` is allowed only for decorative backdrops after the URL has been normalized.

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

Existing raw `z-index` values from before the scale was introduced have been migrated for the highest-impact sites (toasts, command palette, context menu, modals, tooltips). Lower-z values within panels (z 1-80) are left raw because they represent within-panel stacking that doesn't need to participate in the global scale.

## Linting

Stylelint runs on `src/**/*.{css,svelte}` and:

- **Errors** on legacy tokens (`--danger`, `--color-error`, `--bg-glass`, `--surface-hover`) and on hardcoded hex values that should be theme tokens (`#4a6dd8`, `#5a7ce8`, `rgba(155,111,255,...)`, `rgba(74,109,216,...)`).
- **Warns** (errors after Phase 4.4) on any `font-size`, `font-family`, `font-weight`, or `line-height` value that is not a canonical token - except the small set of documented raw exceptions: weight `400`/`800` (with a code comment), line-height `1` (single-line elements only), `normal`, `inherit`.
- **Warns** (errors after Phase 4.4) on the `font:` shorthand except `font: inherit`. Set size, family, weight, and line-height individually with tokens - the shorthand is reserved for button-reset patterns only.

A separate guard, `pnpm lint:inline-styles`, scans Svelte templates for `style="font-..."` attributes (which stylelint cannot see). Use a scoped `<style>` block instead.

```text
pnpm lint:css
pnpm lint:inline-styles
```

Errors are blocking; fix them before committing. `.svelte` components are linted via `postcss-html`.

## Removing redesigned components

When you replace markup - e.g. ripping out a hand-rolled track list and dropping in `<TrendingShelf />` - delete the now-orphaned CSS rules in the same commit. The Svelte compiler emits `Unused CSS selector` warnings on `pnpm build`, and CI (`.github/workflows/pr-check.yml`) fails any PR that surfaces them. Stylelint cannot detect this on its own - only the compiler can, because it has the template AST.

Genuine compose patterns (`class:foo={cond}`, `:global(...)`, classes injected at runtime) are sometimes flagged as unused even when they are not. In those cases, leave the rule and note the reason in the commit message - CI will block until you do.

## Before you add a token

Justify why an existing token cannot represent the value. New tokens grow the design system and become a maintenance cost. The current scale already covers spacing 3-48 px, radii 4-22 px, type 8-56 px (9 steps), weight 500/600/700, and line-height 1.1/1.3/1.5/1.6 - most additions to the system can be expressed in those. New sizes outside the type scale (sub-8 px or above-56 px) should escalate to a design discussion rather than auto-adding a 5xl or 3xs token.

If you need a value outside the scale (e.g. a 64 px hero-art floor), prefer a per-component CSS custom property scoped to the parent over a new global token.

## Future typography work (deferred)

The Phase 4 typography migration tokenized every `font-size` / `font-weight` / `line-height` and locked the scale via stylelint. A follow-up "Phase C" is captured in the design spec for when the team wants to keep going: typography preset utility classes (`.text-body`, `.text-caption`, `.text-display`, `.text-label`, `.text-eyebrow`) that bundle size + weight + line-height + letter-spacing into named recipes, and migrating the remaining hand-rolled route headers onto the canonical `PageHeader` / `SectionHeader` components.

The route-header migration is partly done: Home, the recommendation shelves, and the search discover shelves were collapsed onto a single `SectionHeader` with two spacing values for the whole page. The remaining routes still hand-roll theirs. Do the rest opportunistically when you're already in a route rather than as one sweep.
