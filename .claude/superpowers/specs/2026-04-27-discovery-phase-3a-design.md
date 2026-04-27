# Discovery Phase 3a — Make Tracks Identifiable

**Date:** 2026-04-27

## Context

After Phase 2, the discover canvas correctly shows ~60 NEW external Tidal tracks similar to a seed. But hovering a node still does nothing visible (the existing `hoveredNode` state only changes the cursor and adds a faint ring). You can't tell what any track is without clicking and reading the side panel.

Phase 3a closes that gap: hover a node, see what it is. Identify the seed at a glance.

## Goal

After 3a:
- Hovering any node shows a tooltip with title, artist, and key audio fields (BPM, key, energy, top genre when populated)
- The seed track is unmistakably the center: bigger, brighter, with a visible "▶ Playing" or "🔒 Locked" indicator
- The tooltip follows the cursor smartly (flips above/below to stay on screen)

## Scope

### Hover tooltip

A new Svelte sibling component (`DiscoverHoverCard.svelte`) rendered as a fixed-position div over the canvas. It receives:
- `node: DiscoverTrackNode | null` — the hovered node, null hides the card
- `mouseX: number, mouseY: number` — cursor position in client coordinates

Internal logic:
- Position 12px above the cursor by default
- If the card would clip the top of the viewport, flip below the cursor
- If it would clip the right edge, anchor to right
- 100ms delay on appear to avoid flicker, instant on hide

Visible content:
- **Title** (large)
- **Artist** · **Album** (smaller, secondary)
- A row of chips showing populated audio fields: `120 BPM` / `Am` (key) / `87% energy` / `top_genre`
- For external candidates (`source: "external"`), show a small "Tidal" badge top-right
- For the seed (`is_in_library === true && track_id === activeSeedId`), show a "▶ Playing" or "🔒 Locked seed" pill at the top

The chip row only includes fields that are non-null. External candidates often have BPM/key from Tidal but no `top_genre` (since it's library-derived). Library tracks have all fields.

### Seed visual distinction

The seed track is currently rendered identically to other library tracks except for the existing pulsing ring tied to `currentTrackId`. We strengthen this:

- **Radius**: seed gets `radius * 1.5` (about 30% bigger than the largest external)
- **Glow**: bright accent halo (purple, matching the brand) larger than any other node
- **Outer ring**: continuously pulsing at slower frequency than the existing currentTrackId pulse (more like a heartbeat than a strobe)
- **Inline label**: the seed gets its title rendered next to it as canvas text (no need to hover)
- **Lock indicator**: when `lockedSeedId === seed.track_id`, draw a small lock icon (or text "🔒") above-right of the seed

Detection: a node is the seed when `node.track_id === activeSeedId` from the store. The existing `currentTrackId` prop on `DiscoverSpace` will work — but Phase 3a passes the resolved seed id explicitly via a new prop `seedTrackId: number | null`.

### Cursor-position tracking

`DiscoverSpace.svelte` already has `onMouseMove` setting `hoveredNode`. Extend it to also expose the current screen-space mouse position via callback to the parent (so the parent can pass it to `DiscoverHoverCard`).

Add a new optional prop `onHoverPosition?: (node, x, y) => void` that fires on every mousemove (throttled to 30fps to avoid wasted re-renders).

The `+page.svelte` parent maintains:
- `hoveredNode = $state<DiscoverTrackNode | null>(null)`
- `hoverX = $state(0)`, `hoverY = $state(0)`

And renders `<DiscoverHoverCard node={hoveredNode} mouseX={hoverX} mouseY={hoverY} seedId={resolvedSeedId} />` as a sibling to `<DiscoverSpace>`.

## Architecture

```
+page.svelte
├── DiscoverSpace.svelte (canvas)
│   ├── existing render loop
│   ├── seed-distinct render branch (new)
│   └── onMouseMove → onHoverPosition callback (new)
├── DiscoverHoverCard.svelte (new)
│   └── fixed-positioned tooltip
└── DiscoverPanel.svelte (existing right sidebar)
```

The hover card lives at the page level (not inside the canvas) so it renders above the canvas in the DOM and is independent of canvas transforms.

## Files

| File | Change |
|---|---|
| `frontend/src/lib/components/Discover/DiscoverHoverCard.svelte` | **New** — the hover tooltip |
| `frontend/src/lib/components/Discover/DiscoverSpace.svelte` | Add `seedTrackId` prop, seed-distinct rendering, `onHoverPosition` callback (throttled) |
| `frontend/src/routes/discover/+page.svelte` | Wire hovered node + cursor coordinates, render `DiscoverHoverCard` |

No backend changes. No new API calls.

## What stays unchanged

- All existing rendering — node colors, edge drawing, force simulation, hyperspace search
- Phase 2 seed/lock pill at top of page
- Phase 1 data on the wire (already has everything we need)
- `DiscoverPanel.svelte` (the right-side detail card) — still appears on click

## Out of scope (Phase 3b/3c)

- Legend panel
- Edge color/styling by reason_tags
- Distance-from-seed = similarity layout
- Cohort overlay toggle
- Mode visual differentiation
- Skip-rate dimming
- Training-freshness pill

## Verification

1. Frontend type-check: `cd frontend && npx svelte-check --tsconfig ./tsconfig.json` → 0 new errors.
2. Manual:
   - Play any library track → open `/discover`
   - Seed should be visibly larger and brighter than externals, with title rendered next to it
   - Hover a node → tooltip appears within 100ms with title, artist, BPM/key/energy chips
   - Hover the seed → tooltip shows a "▶ Playing" pill at the top
   - Lock the seed (Phase 2 pill) → seed gets a "🔒" indicator and tooltip pill changes to "🔒 Locked seed"
   - Tooltip flips above/below as you hover near canvas edges; stays on-screen
   - Move quickly between nodes — no flicker
3. No regression: dragging, zoom, click-to-select, hyperspace search, current-playing pulse all still work
