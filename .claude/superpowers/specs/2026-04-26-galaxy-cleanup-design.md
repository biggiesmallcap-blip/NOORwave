# Galaxy Cleanup & Panel Simplification

**Date:** 2026-04-26

## Goal

Strip the galaxy toolbar down to the three controls that are actually used, remove the Bridges feature entirely, and simplify the genre detail panel to show only the essential information with an optional track list expansion.

## Scope

Two files are the primary targets:

- `frontend/src/routes/genres/+page.svelte` — toolbar controls
- `frontend/src/lib/components/Genre/GenrePanel.svelte` — detail panel
- `frontend/src/lib/components/Genre/galaxyBuilder.ts` — co-listening edge logic (dead code after Bridges removal)

## Toolbar

### Remove

| Control | Variable/handler |
|---|---|
| Jump to family (dropdown) | inline select, `selectedFamily` |
| Back button | `handleBackOut()` call |
| Center map button | `resetViewToken` increment |
| Gravity mode | `listeningDriven` toggle |
| Clusters | `showCohorts` toggle |
| Bridges | `showCoListening` toggle |
| Heat | `heatEnabled` toggle |

### Keep

| Control | Variable |
|---|---|
| Search genres (input) | existing search form |
| Labels | `labelsEnabled` toggle |
| Auto drift | `autoDrift` toggle |

The control dock goes from 9 interactive elements to 3.

### Co-listening edge cleanup

With `showCoListening` removed from the UI, the following become dead code and should be deleted:

- `buildCoListeningEdges()` in `galaxyBuilder.ts` (lines ~179–207)
- Any `GenreCoOccurrence` edge references in the galaxy data pipeline
- The `showCoListening` reactive variable and any `$:` statements that depend on it
- The "live bridges" count stats (lines ~83–90 in `+page.svelte`)

## Genre Panel

### Remove

- **Lineage block** — Family, Node level, Branch size rows
- **Momentum block** — 90-day listens, Listened time, Heat intensity rows
- **"Listening dossier" heading** and "Showing first 20" subtitle
- Genre tag chips below the action buttons (redundant with the title)

### Header

Keep: system badge, genre name, close button.

Add subtitle line directly below the genre name:

```
{branch_size} tracks · {listened_time}
```

Where `branch_size` is the existing track count and `listened_time` is the formatted listened time from the momentum data (e.g. `13h 51m`). If `listened_time` is zero or unavailable, show only `{branch_size} tracks`.

### Actions

Unchanged: **Start mix** and **Lock as seed** buttons.

### Nearby scenes

Keep the chip row as-is. Remove the "NEARBY SCENES" section label — the chips are self-explanatory.

### Track list (new expand behaviour)

Replace the always-visible dossier list with a toggle:

- Default state: collapsed. A button reads **"See all {branch_size} tracks ▼"**.
- Expanded state: button label changes to **"▲ Hide tracks"**. The existing track list renders below in a scrollable container (`max-height: 50vh`, `overflow-y: auto`).
- Toggle is driven by a local boolean (`showTracks`, default `false`).
- No network request needed — the existing dossier data is already fetched; it just becomes hidden by default.

## Default states for removed toggles

When a toggle is removed, its variable should be set to a fixed default:

- `heatEnabled` → `false` (heat halos off permanently; no toggle to re-enable)
- `showCohorts` → `false` (cluster overlays off permanently)
- `listeningDriven` → `false` (canonical taxonomy layout is the default)
- `showCoListening` → deleted entirely along with the edges it gates

## What stays unchanged

- Node selection logic, hover behaviour, camera controls
- `GenreGalaxy.svelte` canvas rendering (only the data passed to it changes)
- Start mix / Lock as seed functionality
- The search input behaviour
- Labels and Auto drift toggle logic
