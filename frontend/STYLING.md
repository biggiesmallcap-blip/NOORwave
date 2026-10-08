# Styling - design system reference

This document is the load-bearing reference for how NOORwave looks and moves. Read it before you add a route, a token, a global CSS rule, or a component-level styling pattern. If a new surface needs something this file does not cover, extend this file in the same PR instead of inventing a local style.

The reference surfaces are `/videos/stations` and `/videos/liked`. When in doubt, make the new surface read the way those two read: one stable frame at the top, content sitting on the page ground, artwork carrying the colour, and actions that appear when you reach for them.

## Adoption status

This file describes the target system from the October 2026 design audit. The foundations are in code: layout, label and motion tokens, the `.t-*` role classes, the solid focus ring, the contrast floor, and buttons without a hover lift. `CommandHeader` and `ScopeTabs` (`$lib/components/ui/`) are built and mounted on Videos; `Segmented` is built and used by the DJ mix intent and speed; `FilterChip` is built and used by the Duplicates relationship filters; `Dropdown` is built and used by the DJ transition style; `ActionBar` is built and used by the album hero, and `DetailHero` is borderless. Not built yet: `ErrorState`, the single `TrackRow` anatomy, the skeleton delay, the motion lint warnings, and the narrow-window icon rail. Until a component exists, follow its rule with local markup and move to the component when it lands. The remaining work is tracked in `FOLLOWUPS.md` ("Design system adoption").

## Principles

1. **One frame per page.** Every route starts with one of three headers (command, title, or detail) at the same position. The header never changes height when the user switches tabs inside the page.
2. **Ground, not boxes.** Content sits on the page ground. A view gets at most one raised surface (a hero or spotlight). Never nest a surface inside a surface.
3. **Artwork is the colour.** Chrome stays neutral. The accent is reserved for the primary action, the active selection, the playing item, and focus.
4. **Columns, not floats.** Rows are grids with a fixed identity column. Numbers are tabular. Metadata that repeats down a list lives in an aligned column, not floating at the far edge.
5. **Few sizes, clear roles.** A content area uses at most three text sizes plus one display size. Hierarchy comes from weight and colour before size.
6. **The row is the action.** Click a row or tile to do the obvious thing. Secondary actions appear on hover or focus and always occupy their space so nothing reflows.
7. **Motion confirms.** Motion shows what changed and where it came from. It never delays input and it always has a reduced-motion equivalent.

## Scaling

The app targets Tauri windows from 720 x 500 up to 4K. Type, page gutters and content widths interpolate with `clamp()` tokens. Fixed pixel values are correct for:

- icons and intrinsic SVG sizes;
- control heights (`--control-h` and the button sizes below);
- in-row rhythm that must stay crisp: row gaps (2px), row padding, hairlines;
- artwork floors and identity-column widths inside a row grid (for example the 220px station identity column).

Everything else uses a token. If you are about to write a raw size, check the role tables below first.

## Tokens (in [`src/app.css`](src/app.css))

| Family | Tokens | Use for |
| --- | --- | --- |
| Spacing | `--space-1` ... `--space-7` (3-48 px), `--gap-sm` / `--gap` / `--gap-lg` | Padding, margin, grid gap, flex gap |
| Layout | `--page-top` (28px), `--header-gap` (28px), `--section-gap` (`--space-7`), `--group-gap` (`--space-3`), `--row-gap` (2px), `--measure-command` (720px), `--measure-form` (760px), `--measure-text` (65ch) | Page rhythm. See "Page frames". |
| Radii | `--radius-xs` (4-6), `--radius-sm` (7-10), `--radius-md` (10-14), `--radius-lg` (15-22). `--radius` is a legacy alias for `--radius-md`. | Artwork uses `--radius-sm`. Rows use `--radius-sm`. Raised surfaces use `--radius-lg`. Overlays use `--radius-md`. Pills and circles use `999px` / `50%`. |
| Type size | `--font-size-2xs` ... `--font-size-4xl`, plus `--font-size-label` (11-12 px) | Only through the typography roles below. |
| Weight | `--font-weight-medium` (500), `--font-weight-semibold` (600), `--font-weight-bold` (700) | 700 for page and section titles only. 600 for row titles, labels, buttons. 400 for body and meta. |
| Line height | `--line-height-tight` (1.1), `--line-height-snug` (1.3), `--line-height-normal` (1.5), `--line-height-loose` (1.6) | Raw `1` for single-line controls and chips. |
| Motion | `--motion-fast` (130 ms), `--motion-base` (210 ms), `--motion-slow` (340 ms), `--motion-exit` (140 ms, exit curve), `--motion-press` (90 ms) | See "Motion". Each token bundles its easing. |
| Easing | `--ease-standard` (the bundled curve, for keyframes), `--ease-exit` | Only where a token cannot be used (keyframes, WAAPI). |
| Blur | `--blur-base`, `--blur-overlay`, `--blur-modal` | Overlays and the app chrome only. Never on in-flow content. |
| Artwork filters | `--art-wall-filter`, `--art-collage-filter`, `--art-backdrop-filter` | The only filters allowed on artwork. Dark values dim walls of stills and blurred backdrops; light values do not. Never write a raw `brightness()` or `saturate()` on art. |
| State | `--state-error`, `--state-warning`, `--state-success`, `--state-active`, `--state-favorite`, `--state-favorite-glow` | Status colours. `--danger`, `--color-error` do not exist. |
| Service | `--service-spotify`, `--service-tidal`, `--service-lastfm` | Source glyphs and service-branded heroes only. Stable across themes. |
| Surface | `--bg-base`, `--bg-elevated`, `--bg-raised`, `--bg-surface`, `--bg-hover`, `--panel-bg` | See "Surfaces and colour roles". `--bg-glass`, `--surface-hover` do not exist. |
| Text | `--text-primary`, `--text-secondary`, `--text-tertiary`, `--text-muted`, `--text-on-accent` | `--text-tertiary` is the lowest colour allowed for readable text. `--text-muted` is for disabled and decorative text only. |
| Borders | `--border-subtle`, `--border-muted`, `--border-strong`, `--panel-border` | See "Boundaries". |
| Accent | `--accent`, `--accent-soft`, `--accent-line`, `--accent-strong`, `--accent-glow` | Fills use `--accent`. Text and focus rings use `--accent-strong`. `--accent-line` is a hairline colour, never a focus ring. |
| Content | `--content-width` (clamp 1280-2400 px) | Full-width page measure. |

## Page frames

The workspace owns the page gutter (28px top, 30px sides at desktop; it steps down on narrow windows). Routes never add their own horizontal inset. A route root is `width: min(100%, var(--content-width)); margin: 0 auto;` and nothing else on the horizontal axis.

Every route uses exactly one of three headers.

### Command header

For pages whose first job is finding something inside a scope: Search, Library, Videos (all tabs).

```
[back pill, detail only]  [ search field, --measure-command, centred ]
                [ scope tabs, centred ]
                [ toolbar, only when the scope has tools ]
--header-gap
content
```

- The search field is 48px tall. The scope tabs sit `--space-3` below it. The toolbar row is 36px tall and sits `--space-3` below the tabs.
- The header has the same height on every tab of the page. If any tab of the page has tools, every tab shows the toolbar row; a tab without filters puts its count and its play action there. The row stays visible while results load.
- Search status ("13 artists, 50 albums, 50 songs", Clear, Show more) lives in the left slot of the toolbar row. Never reserve an empty row for it.
- Content starts `--header-gap` (28px) below the last header row.

### Title header

For destinations that are not search-led: Home, Playlists, Moods, Charts, Analytics, Automix, DJ, Duplicates, History, Settings.

- Page title (role `page-title`), an optional one-line description (role `body`, `--text-secondary`), and actions aligned to the right of the title row.
- No eyebrow above the title. A source or category belongs in the description or as a chip.
- Content starts `--header-gap` below the header.

### Detail header

For one music entity: artist, album, playlist, mood, recommendation shelf, Spotify album and playlist.

- A back pill (`.back-link`) on its own row, then the hero.
- The hero is not a box: artwork, a kicker line (role `label`), the entity title (role `entity-title`), one meta line, and the shared `ActionBar`.
- An artwork-derived backdrop may tint the top of the page and fade into the ground. It is decoration behind the hero, not a container.
- The hero is at most 320px tall at 1920 x 1080 and 260px at 1280 x 800, so the first row of content is visible without scrolling.

### Widths

| Measure | Value | Use |
| --- | --- | --- |
| Full | `--content-width` | Grids, rails, tables, guides. Do not wrap it in `min(1200px, ...)`; that caps wide windows. |
| Form | `--measure-form` (760px) | Settings content column, dialog bodies, long forms. Left-aligned to the page edge, not centred in a panel. |
| Command | `--measure-command` (720px) | Search field and scope tabs. |
| Text | `--measure-text` (65ch) | Descriptions, bios, help copy. |

### Vertical rhythm

| Between | Space |
| --- | --- |
| Rows in a list | `--row-gap` (2px) |
| Cards in a grid | `--gap` columns, `--gap-lg` rows |
| Group label and its content | `--group-gap` |
| Groups inside a section | `--space-5` |
| Sections | `--section-gap` |
| Header and content | `--header-gap` |

## Typography roles

Use the role classes in `app.css` (`.t-entity`, `.t-page-title`, `.t-section`, `.t-label`, `.t-row-title`, `.t-meta`, `.t-body`, `.t-micro`) or the shared components that apply them (`PageHeader`, `SectionHeader`, `DetailHero`). Do not hand-roll a heading or a label.

| Role | Recipe | Use |
| --- | --- | --- |
| `entity-title` | `--font-display`, 600, `--font-size-4xl`, `--line-height-tight`, `text-wrap: balance`, two lines max. Steps down to `--font-size-3xl` above 28 characters and `--font-size-2xl` above 48. | The name of a piece of music or a person: artist, album, playlist, mood, top search result, station spotlight, now playing title (at `--font-size-xl`). |
| `page-title` | `--font-body`, 700, `--font-size-2xl`, `--line-height-tight` | The name of an app destination: Settings, Playlists, Charts, Analytics. One size for every route. |
| `section-title` | `--font-body`, 700, `--font-size-lg`, `--line-height-snug` | A section that is a destination of its own ("Albums 15", "Top tracks") and dialog titles. Count in `--text-tertiary` beside it. |
| `label` | `--font-body`, 600, `--font-size-label`, uppercase, `letter-spacing: 0.12em`, `--accent-strong` | A group of rows or cards inside a section: "For you", "Artists", "Your genres". Column headers use the same recipe in `--text-tertiary`. |
| `row-title` | `--font-body`, 600, `--font-size-sm`, `--line-height-snug`, one line, ellipsis | Track, album, video and setting names in rows and under tiles. |
| `meta` | `--font-body`, 400, `--font-size-xs`, `--text-secondary`, one line, ellipsis | Artist, album, year, counts under a title. |
| `body` | `--font-body`, 400, `--font-size-sm`, `--line-height-normal`, `--measure-text` | Descriptions, bios, help copy. |
| `micro` | `--font-body`, 500, `--font-size-2xs`, `--text-tertiary`, tabular numbers | Badges, durations on artwork, timestamps. |

Rules:

- The display face is for names of music and people. App destinations, sections and controls use the body face. This is the one place serif appears.
- A section gets a `section-title` or a `label`, never both. Do not stack an eyebrow over a title.
- Durations, counts, track numbers and times use `font-variant-numeric: tabular-nums`.
- Uppercase is only for `label` and column headers.

## Boundaries: spacing, dividers, fills, borders, elevation

Use the first step on this ladder that separates the content. Each step down adds weight; never combine steps on one in-flow element.

1. **Rhythm.** Items in the same group are separated by their gap and nothing else.
2. **Label.** A new group inside a section gets a `label` and `--group-gap`.
3. **Space.** A new section gets `--section-gap` and a `section-title`. No rule line.
4. **Divider.** A 1px `--border-subtle` hairline only under a table's column header row, between rows of a settings group, and between a list and its footer.
5. **Fill.** `--bg-surface` without a border for the one raised surface in a view (hero, spotlight, now-mixing deck), for sunken controls (search field, segmented track, inputs), and for hover and selected rows.
6. **Border.** Only for controls that would otherwise be invisible at rest (outline pills, inputs in light themes) and for overlays.
7. **Elevation.** Shadow and `--blur-modal` only for overlays: menus, dialogs, sheets, the command palette, toasts, popovers.

Never: a bordered row, a bordered stat tile, a card inside a card, a page wrapped in a panel, or `.glass` on in-flow content. `.glass`, `.glass-panel` and `.glass-tile` are for floating chrome and overlays.

## Surfaces and colour roles

| Role | Treatment |
| --- | --- |
| Ground | The workspace background (with its wallpaper scrim). All content sits here. |
| Raised | `--bg-surface` fill, `--radius-lg`, no border in dark themes, `--border-subtle` in light themes where the fill alone is too faint. One per view. |
| Sunken | `--bg-surface` fill, no border, used for fields and segmented tracks. |
| Hover | `--bg-hover` fill, `--radius-sm`. |
| Selected | `--accent-soft` fill, `--text-primary`. |
| Playing | Title in `--accent-strong`, an animated level glyph in the lead column, and `box-shadow: inset 3px 0 0 var(--accent)` on the row. |
| Overlay | `--bg-elevated` at 96 percent, `--blur-modal`, `--border-subtle`, `--panel-shadow`, `--radius-md`. |
| Text over artwork | Always white with a dark gradient scrim, in every theme. Never use theme text tokens on top of an image. |

Contrast:

- Body and meta text meet 4.5:1 against the ground including the wallpaper scrim. `--text-tertiary` is the floor for readable text.
- Accent text uses `--accent-strong`. `--accent` on dark grounds is for fills and large display text only.
- Focus rings and essential icons meet 3:1.
- Check new palettes in dark, light and Clay before shipping.
- `--accent-strong` comes from the palette in dark themes. In light themes, palettes without their own light accents get a darkened accent (`readableOnLight` in `paletteTheme.ts`) so accent text and focus rings clear 4.5:1 there too.

### Material surfaces

The `clay` colour scheme coordinates parchment, ink, and terracotta across the existing surface tokens in both light and dark modes. `data-palette` on the root selects the material; `data-theme` still selects the surface mode independently. Saved appearance preferences take priority over the fresh-install defaults.

`--material-grain` is a background image, separate from the colour tokens because they cannot express texture. It is `none` for other schemes and a small, static, repeating local SVG for Clay. Use it as the first background layer on shared surfaces. Keep it behind content so artwork, text, pointer input, and focus rings remain clear. Do not add an animated grain overlay.

`--text-on-accent` supplies text and icons on solid accent fills. Normal text tokens describe text on page surfaces and cannot also guarantee contrast on an accent. Other schemes keep white; Clay uses parchment on terracotta in light mode and dark ink on amber in dark mode.

## Controls

| Control | Use | Treatment |
| --- | --- | --- |
| `ScopeTabs` | Moving between views of one page (Library tabs, Search filters, Videos sections, Playlists filters). | Pills, `--control-h` tall, `--border-subtle` outline at rest, `--bg-hover` on hover, `--accent` fill with `--text-on-accent` when active. Optional count in `--text-tertiary`. |
| `Segmented` | Choosing one value (theme, time range, grid or list, transition speed). | A sunken track with a sliding thumb in `--bg-raised` and `--text-primary`. Never accent. |
| `FilterChip` | Toggling a filter on and off (Liked, a decade, a genre). | Pill. Active is `--accent-soft` fill, `--accent-strong` text, `--accent-line` border. |
| `Dropdown` | Picking from a list in a toolbar (sort, genre, year). | A chip showing label and value with a chevron, opening the shared menu surface. No native `select` in toolbars. |
| Buttons | Primary: `--accent` fill. Secondary: `--bg-surface` fill, no border (today `.btn-glass`, which still has a hairline). Ghost: text only. Icon: round, 36px (40 and 48 in the player). | Sizes 30, 36, 44. No hover lift. Press scales to 0.97. |
| `ActionBar` | The action row of every detail hero. | Play (primary, labelled), Shuffle, Radio (secondary, labelled), Like and More (icon). Labels collapse to icons with tooltips below 1100px of content width. |
| Search field | Every search entry point. | `SearchField`, 48px in a command header, 36px inline. Sunken fill, no border at rest, focus ring on focus. |
| Focus | Every interactive element. | `outline: 2px solid var(--accent-strong); outline-offset: 2px`. Inside clipped containers use `outline-offset: -2px`. |

## Lists, rows and tables

One track row anatomy, used by every list of tracks:

```
[lead 32] [art 40] [title + meta] [album, wide only] [optional columns] [status] [like] [duration 56] [more]
```

- Lead shows the track number; on hover it becomes play; on the playing row it shows the level glyph.
- Status is one glyph column: not in library, Hi-Res, lossy, or unavailable. No per-row text pills. Do not badge a property every row shares.
- Like and More use `.row-btn`: hidden until the row is hovered or focused, always occupying their space. Like stays visible when the track is liked.
- Duration is right-aligned, tabular, next to More. It never floats at the far edge of a wide row with empty space before it.
- Row height 52px (two lines) or 40px (compact tables). Hover is a `--bg-hover` fill with `--radius-sm`.
- Click plays from that row. Right-click opens the row entity's menu through the shared builders.
- Tables keep a sticky column header row with sortable headers in the column-label recipe.

`.row-btn` remains the borderless icon action inside a browse row.

## Cards, grids and rails

Card grids reflow with `repeat(auto-fill, minmax(min(var(--card-min), 100%), 1fr))`. `--card-min` is 168px for albums and playlists, 140px for artists and 240px for videos. Track-row layouts and structural splits keep explicit columns.

- A card is artwork, then `row-title`, then `meta`, 8px below the art. No card background and no border.
- Hover scales the artwork to 1.03 inside its radius and fades in the shared `PlayOverlay`. Focus shows the ring on the artwork.
- Horizontal rails use `MediaRail` and derive card width from the rail (see the rail notes below). The first card aligns with the page edge. Edge masks apply to the trailing edge only and never clip a caption. Scrollbars stay hidden; arrows appear on hover and focus.

Horizontal rails size from the rail, not the viewport. `MediaRail.fluid` solves for `--cols` whole cards plus `--peek: 0.35` of one more. `--cols` steps on container queries (560 / 760 / 980 px). The rail uses `justify-content: safe center`; the `safe` keyword keeps overflowing content scrollable.

Section headings for rails and grids go through `SectionHeader`. Group labels go through `.t-label`.

## Artwork

- Music artwork is square with `--radius-sm`. Artists are circles. Videos are 16:9 with `--radius-sm`. Never put a border on artwork.
- Placeholders use `ArtworkImage`'s initials fallback on a name-derived gradient at the same size and radius as the real art.
- Text over artwork always gets a dark scrim and white text, in every theme.
- Dense walls of stills use `--art-wall-filter` at rest and come to full colour on hover. Collages use `--art-collage-filter`. Both are theme tokens: dimming tuned for a dark ground looks muddy on a light one.
- Text on a collage gets a scrim only behind the copy, so the rest of the artwork keeps its brightness.

### TIDAL artwork URLs

TIDAL artwork must not be rendered from raw API or database URLs. Always route a TIDAL-capable URL through `$lib/utils/artwork.upscaleTidalArtwork(url, size)` directly, or through `$lib/components/ui/ArtworkImage.svelte`.

Allowed TIDAL sizes are `80`, `160`, `320`, `640`, `750`, `1080`, and `1280`. Use `320` for rows, rails, and small tiles, `640` for hero cards and detail covers, and `1280` for lockscreen, MediaSession, and blurred backdrop art. Do not pass arbitrary sizes such as `256` or `512`.

Every rendered image that may receive a TIDAL URL needs an error fallback. Prefer `ArtworkImage` for route and component markup because it normalizes the URL, resets failure state when the source changes, and renders a stable initials fallback. CSS `background-image` is allowed only for decorative backdrops after the URL has been normalized.

## States

| State | Pattern |
| --- | --- |
| Loading, first load | `Skeleton` in the exact geometry of the content (rows, tiles, hero). Shown after 150ms so fast loads do not flash. No spinner-and-text placeholders. |
| Loading, refresh | Keep the current content at 55 percent opacity and show a 2px indeterminate bar under the header. Do not blank the page. |
| Empty | `EmptyState` where the content would be, left-aligned in the content column, no box: a `section-title`, one sentence, and the next useful action ("Search TIDAL for ..."). Distinguish nothing-yet, filtered-empty (offer Clear filters) and search-empty (offer a wider scope). |
| Error | `ErrorState`, same layout as empty, plain language, a Retry action, and technical detail behind a disclosure. Never show raw `ApiError` text. |
| Success | Toggles confirm in place (the heart fills). Background work and reversible destructive actions use the toast with Undo. |
| Validation | Show a field error after the field is left or the form is submitted, never on open. |

## Motion

The motion tokens are not bare durations. Each one bundles its easing, so never append a second timing function:

```css
--motion-fast: 130ms cubic-bezier(0.25, 0.8, 0.25, 1);
--motion-base: 210ms cubic-bezier(0.25, 0.8, 0.25, 1);
--motion-slow: 340ms cubic-bezier(0.25, 0.8, 0.25, 1);
--motion-exit: 140ms cubic-bezier(0.4, 0, 1, 1);
--motion-press: 90ms cubic-bezier(0.25, 0.8, 0.25, 1);
```

```css
/* BAD -- two timing functions, the whole declaration is dropped */
transition: background var(--motion-base) ease;

/* GOOD */
transition: background var(--motion-base), border-color var(--motion-base);
```

| Interaction | What moves | Timing |
| --- | --- | --- |
| Hover on rows, pills, links | background, colour | `--motion-fast`. Rows never move. |
| Hover on cards | artwork `transform: scale(1.03)` | `--motion-base` |
| Press | `transform: scale(0.97)` on buttons and cards | `--motion-press` in, `--motion-fast` out |
| Toggle (like, follow) | fill, plus a 1 to 1.15 to 1 pop | `--motion-base`, optimistic |
| Segmented and tab indicator | thumb `transform` | `--motion-base` |
| Disclosure | `grid-template-rows: 0fr` to `1fr`, content opacity | `--motion-base` |
| Menu and popover | opacity, scale from 0.98, blur | `--motion-fast` in, `--motion-exit` out, via `ContextMenu` |
| Dialog and sheet | opacity, 12px translate, scrim fade | `--motion-base` in, `--motion-exit` out |
| View change inside a page | content crossfade; the header stays put | `--motion-fast` |
| Data arriving after mount | `.rise-in-shelf`, `.rise-in-card` | See "Entry motion" |
| Skeleton to content | crossfade | `--motion-fast` |
| Now playing artwork | crossfade | `--motion-slow` |
| Queue reorder | FLIP translate | `--motion-base` |

Rules:

- Use CSS transitions for state changes so they are interruptible. Never chain animations in script that block input.
- Never disable a control while it animates. Repeated actions (next, like, skip) respond on the first frame; the animation catches up.
- Hover feedback has no delay. Tooltips and hover cards wait 300ms.
- Do not replay entry motion when a cached view is shown again.

### Entry motion

`.rise-in-shelf` (sections, 340 ms, 70 ms step, capped at 8) and `.rise-in-card` (cards, 300 ms, 22 ms step, capped at 11) own content that arrives after mount. The parent writes `--rise-index`; cap it with a modulo of about a screenful; `.rise-in-card` uses `backwards` fill so it does not trap popouts in a stacking context. Do not add a third variant. Copies in `videos/liked`, `VideoSetShelf` and the library mural predate these classes and are tracked in `FOLLOWUPS.md`.

### Reduced motion

`prefers-reduced-motion` is the operating system's accessibility setting (Windows: Settings, Accessibility, Visual effects, Animation effects off). The WebView reports it to CSS; NOORwave does not have its own switch unless a "Reduce motion" setting is added under Appearance. Under `prefers-reduced-motion: reduce`: transforms are removed, durations drop to 80ms opacity changes, entry motion and skeleton shimmer stop, the playing glyph is static, and scroll behaviour is `auto`. Colour and focus changes stay.

## Responsive and dense content

| Content width | Behaviour |
| --- | --- |
| 1440px and up | Two-column compositions are allowed (search top result beside songs, artist top tracks beside the library column, album liner notes). |
| 1100 to 1440px | Single column. Tables show all columns. |
| 760 to 1100px | Tables fold album and optional columns into the meta line. `ActionBar` labels become icons. Hero art 160px. |
| Below 760px | The sidebar collapses to an icon rail. Hero art 120px beside the title. |

- Use container queries on the content column, not the viewport, because the sidebar and now playing panel change the available width.
- Long names: entity titles step down and clamp to two lines with the full text in `title`; row titles are one line with an ellipsis.
- Large collections: virtualise or page long lists, show the count, keep sort and view in persisted stores, and give alphabetical grids an A to Z index.
- No horizontal page scroll at any supported width.

## Album quick view and album page

Albums have one content model shown in two containers:

- **Quick view** (the album popup) opens from browse surfaces where the next step is usually to play: Library album grids, landing rails and murals, Home recommendation shelves, discover shelves. It shows the same action bar and track rows as the page, plus an "Open album page" link.
- **Album page** (`/albums/:id`, `/tidal/albums/:id`) opens from explicit links: album titles in rows, search results, artist discographies, the quick view's own link, and the context menu's "Open album".

Do not add a third presentation. A new surface picks quick view or page by that rule.

## Media links and context menus

Track, album, artist, and video references should resolve inside NOORwave whenever the app has a route for them. Do not send media-reference clicks to `tidal.com` from cards, rows, now-playing metadata, quiet mode, or context menus.

- Local artists use `/artists/:id`.
- Local albums use `/albums/:id`.
- TIDAL artists use `/tidal/artists/:id`.
- TIDAL albums use `/tidal/albums/:id`.
- TIDAL videos use `/videos?videoId=:id`.
- TIDAL track titles do not open an external TIDAL page. Link them only when there is a useful in-app destination, such as the local album page.

Use `$lib/player/media_link.ts` for canonical media hrefs and menu delegation when rendering mixed local/TIDAL metadata. Use the shared menu builders (`buildTrackMenu`, `buildTidalTrackMenu`, `buildAlbumMenu`, `buildArtistMenu`, `buildVideoMenu`, `buildPlaylistMenu`) instead of inline menu arrays. Queue rows are already in the queue, so queue-context menus must not show duplicate `Add to queue` actions.

`buildPlaylistMenu` takes every action as an optional handler, so a read-only surface and the detail page share one builder and simply omit what they can't do. The same module exports `buildAddToPlaylistSubmenu(playlists, getTrackIds)`; `getTrackIds` is lazy. Menu icons must be real glyphs.

All right-click menus are rendered by `ContextMenu.svelte`; do not create one-off menu animation styles in callers. Menus enter with a short opacity, blur, and scale transition, exit through the shared `closing` state in `context_menu.ts`, and close on pointer leave, outside click, Escape, scroll, and successful action selection. Keep submenus inside the root menu surface.

## Global utility classes

- `.t-entity`, `.t-page-title`, `.t-section`, `.t-label`, `.t-row-title`, `.t-meta`, `.t-body`, `.t-micro` - the typography roles.
- `.btn`, `.btn-primary`, `.btn-glass` - buttons. Global `.btn-secondary`, `.btn-ghost` and `.btn-icon` are not added yet because some components define local classes with those names; `ActionBar` carries its own button styles.
- `.row-btn` - a borderless icon action inside a row, hidden until hover or focus, always occupying its space.
- `.back-link` - the back pill on detail routes. It supplies its own chevron and is `inline-size: fit-content`. Do not show it on top-level destinations. The one exception is the Videos section header, where Back deliberately returns to wherever the listener came from.
- `.quality-badge` - only where quality differs from the surrounding content.
- `.glass`, `.glass-panel`, `.glass-tile` - overlays and floating chrome only.
- `.rise-in-shelf`, `.rise-in-card` - entry motion.

Actions in [`src/lib/actions/`](src/lib/actions/): `use:dragReorder`, `use:wheelToHorizontal`, `use:portal`, `use:lazyTidalArt`. See their file headers.

## Persisted UI preferences

A view mode, sort order, or filter should survive a reload and a relaunch. SvelteKit's `Snapshot` is per-history-entry `sessionStorage`; use it for scroll position and transient state, and use `localStorage` through [`createPersistedStore`](src/lib/stores/persisted.ts) for anything the user expects tomorrow. The module's three guards (`typeof localStorage`, `try/catch` on read and write, skipping the first `subscribe` emission) are all load-bearing; an unguarded write at module init takes the app down to a bare SvelteKit 500 on boot.

```ts
export const viewMode = createPersistedStore<'grid' | 'list'>('library.viewMode', 'grid', {
  parse: oneOf(['grid', 'list'] as const),
});
```

## Z-index scale

| Token | Value | Use for |
| --- | --- | --- |
| `--z-base` | 1 | In-flow stacking |
| `--z-raised` | 10 | Sticky headers inside a panel, hover lifts |
| `--z-overlay` | 100 | Dropdowns, popovers, hover cards |
| `--z-modal` | 1000 | Dialogs and sheets |
| `--z-toast` | 2000 | Toasts, command palette |
| `--z-tooltip` | 3000 | Tooltips |

Don't add new raw `z-index` values. Use `calc(var(--z-modal) + 1)` for ordering inside a layer.

## Linting

Stylelint runs on `src/**/*.{css,svelte}` (`.svelte` through `postcss-html`):

- Errors on legacy tokens (`--danger`, `--color-error`, `--bg-glass`, `--surface-hover`) and hardcoded theme hex values.
- Errors on any `font-size`, `font-family`, `font-weight`, or `line-height` that is not a token, except weight `400` / `800` with a comment, line-height `1`, `normal`, `inherit`.
- Errors on the `font:` shorthand except `font: inherit`.
- Planned: warnings on raw `transition` and `animation` durations, and on `letter-spacing` values other than `0` and the `label` recipe.

`pnpm lint:inline-styles` scans Svelte templates for `style="font-..."` attributes, which stylelint cannot see.

```text
pnpm lint:css
pnpm lint:inline-styles
```

## Removing redesigned components

When you replace markup, delete the orphaned CSS in the same commit. The Svelte compiler emits `Unused CSS selector` warnings on `pnpm build`, and CI (`.github/workflows/pr-check.yml`) fails any PR that surfaces them. Genuine compose patterns (`class:foo={cond}`, `:global(...)`, runtime classes) can be flagged falsely; leave the rule and note why in the commit.

## Before you add a token

Justify why an existing token cannot represent the value. The scale covers spacing 3-48 px, radii 4-22 px, type 8-56 px, weight 500/600/700, and line-height 1.1/1.3/1.5/1.6. New sizes outside the type scale escalate to a design discussion. For a one-off value, prefer a component-scoped custom property over a new global token.
