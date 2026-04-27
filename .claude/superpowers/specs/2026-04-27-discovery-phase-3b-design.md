# Discovery Phase 3b — Connection Clarity

**Date:** 2026-04-27

## Context

After Phase 3a, users can identify individual nodes via hover tooltip and the seed is visually distinct. But the edges between nodes are still nearly invisible (alpha 0.2–0.4) and even when visible, their color encodes only `edge.type` — which for the Phase 2 external-mode spoke pattern collapses to one shade. Users have no way to decode what a color means.

Phase 3b makes the connections legible and adds a legend panel so the canvas's encoding becomes self-documenting.

## Goal

After 3b:
- Edges are noticeably visible against the dark canvas (alpha ~0.5+, scales with weight)
- Edge color reflects the *primary* reason the connection exists, not just a coarse type bucket
- A small collapsible "Legend" panel in the top-right explains what node color, size, glow, and edge colors mean

## Scope

### Edge color resolution

Currently `DiscoverSpace.svelte` switches on `edge.type` (the coarse string from the backend's `edge_type` inference) for color. We replace this with a small helper that resolves the color from the **first** non-empty entry of `edge.reason_tags` (Phase 1 added this field), falling back to `edge.type` if `reason_tags` is missing or empty.

Color palette (constants in the component):
- `harmonic` / `harmonic_match` → `#a89cff` (lavender)
- `behavioural` / `behavioral` → `#5fb1ff` (sky blue)
- `bpm_match` → `#ffc857` (warm gold)
- `artist_affinity` / `album_context` / `artist` / `album` → `#ff8866` (warm coral)
- `genre_branch` / `genre` / `energy_match` → `#9fcf80` (sage green)
- `external_match` → `#5b4ef8` (brand purple — for Phase 2 spokes)
- `audio_texture` → `#c0c0d8` (light grey)
- anything else → `#888888`

### Edge alpha + width

Replace the current per-type fixed alpha. Use the formula:
```
alpha = 0.4 + edge.weight * 0.5    // range 0.4..0.9 — never invisible, brightest on strong matches
width = 0.8 + edge.weight * 2.5    // range 0.8..3.3 px — clearly visible, weight still meaningful
```

Strong matches (weight near 1.0) glow at alpha 0.9 and width 3.3px — clearly visible. Weak matches (weight near 0) sit at alpha 0.4 and width 0.8px — present but subtle.

### Legend component

New `frontend/src/lib/components/Discover/DiscoverLegend.svelte`. Fixed-positioned in the top-right of the discover layout, above the canvas. Two states:

**Expanded** (default):
- Header: "Legend" + collapse chevron `⌃`
- Section "Nodes": gradient bar (blue → red) labeled "low energy → high", text "size = similarity to seed", "glow = danceability"
- Section "Edges": six rows, each a colored line + a label matching the palette above
- Click chevron to collapse

**Collapsed**:
- Just a small `?` pill in the corner. Click to expand.

The legend is self-contained — it doesn't read any state, it just shows the static encoding key. Persisted collapse state lives in `localStorage` so users don't have to dismiss it every visit.

### Files

| File | Change |
|---|---|
| `frontend/src/lib/components/Discover/DiscoverLegend.svelte` | **New** — collapsible legend component |
| `frontend/src/lib/components/Discover/DiscoverSpace.svelte` | Replace edge color switch with `resolveEdgeColor` helper, use new alpha/width formula |
| `frontend/src/routes/discover/+page.svelte` | Render `<DiscoverLegend>` overlay |

No backend changes. Reuses the `reason_tags` field added in Phase 1.

## What stays unchanged

- Force simulation, hover tooltip (3a), seed visual (3a)
- Edge weight is still produced by the backend
- Progressive edge drawing animation (the `edgeDrawProgress` map) — only the color/alpha resolution changes inside that loop
- Phase 2 lock pill, Phase 1 wire format

## Out of scope (Phase 3c)

- Cohort overlay toggle (color-by-cohort instead of energy)
- Mode visual differentiation (still 5 modes, 3 look identical for now)
- Training-freshness pill
- Skip-rate dimming
- Distance-from-seed = similarity layout
- Edge labels (text on edges) — defer until users ask

## Verification

1. Frontend type-check: `cd frontend && npx svelte-check --tsconfig ./tsconfig.json` → 0 new errors.
2. Manual smoke:
   - Open `/discover` with a track playing
   - Edges should be clearly visible against the dark canvas — strong-similarity spokes (close to seed) brighter than weak ones
   - Top-right corner shows a "Legend" panel with the encoding explained
   - Click the chevron — panel collapses to a small `?` pill; click it — expands again. Reload the page; the collapsed state persists
   - In external seed mode (the Phase 2 default), all spokes are now uniform brand-purple (since reason_tags = ["external_match"])
   - Switch to a prompt search — edges should display a mix of colors based on their varied reason_tags
3. No regression: hover tooltip still works (3a), seed still distinct, click-to-select still selects, drag/zoom still works.
