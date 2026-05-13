# Automix Diagnostics Design

## Goal

Make `/automix` a diagnostics-first cockpit. The page should explain whether Automix is ready, why the current queue looks the way it does, and what the user can do when blends look weak.

The first pass stays frontend-focused. It uses existing playback state, queue, runtime, discovery status, and audio feature APIs. It does not add new backend scoring or preview endpoints.

## Product Shape

The page has three priorities:

1. Diagnose the current seed and Automix health.
2. Explain upcoming blends in the queue.
3. Keep tuning controls close enough to act on the diagnosis.

The page should feel like a working music tool, not a settings page. Controls remain available, but the primary surface is the forecast and its explanations.

## Page Structure

### Seed And Health

The top section shows the current track as the Automix seed. It includes:

- Artwork, title, artist, and stream/runtime details.
- DSP summary: Camelot key or key signature, BPM, and energy.
- Health state: ready, degraded, or blocked.
- Short health reasons such as missing current-track DSP, empty queue, runtime offline, low discovery coverage, or pending external rows.

Actions:

- Refresh data.
- Toggle Automix.
- Start radio from the current track if that action is already available through existing player helpers.

### Blend Forecast

The forecast becomes the main page section. It lists upcoming queue rows and annotates each transition from the previous track.

Each row should show:

- Queue position.
- Track artwork, title, and artist.
- Queue source, including `automix` and `automix-new`.
- Blend verdict: good, okay, clash, pending, or unknown.
- Key relation and BPM delta when features are available.
- Energy delta when both tracks have energy.
- Missing data flags, especially missing DSP on either side of the transition.
- Pending/external state for unresolved `automix-new` rows.

Actions:

- Open the shared track context menu on right-click.
- Move row to play next.
- Remove row when the queue item type supports removal.
- Refresh row features.

Track, album, and artist references must follow the repo context-menu rules. Use shared menu builders and in-app media links where a destination exists. Do not add external TIDAL links for track titles.

### Supporting Controls

Crossfade, source policy, and shuffle mode remain on the page, but below the forecast. Their copy should connect the setting to diagnostics:

- Crossfade: affects transition smoothness, not recommendation quality.
- Include new / external picks: explains pending external rows.
- Learning: explains whether listening signals can affect Automix.
- Shuffle mode: explains how queue ordering can change.

## Derived Diagnostics

Keep diagnostic calculation in small local helpers or a narrow companion module if the route grows too large.

Inputs:

- Current track and current-track audio features.
- Upcoming queue rows.
- Cached audio features for visible upcoming tracks.
- Discovery status.
- Runtime status.
- Automix flags.

Derived outputs:

- Page health status and reasons.
- Per-row transition verdict.
- Per-row feature deltas.
- Forecast counts for good, okay, clash, pending, and unknown.

Fallback behavior:

- If feature data is missing, say what is missing instead of inventing a verdict.
- If discovery status fails to load, mark model coverage unknown and keep playback controls usable.
- If runtime info fails to load, do not block diagnostics based on queue and DSP data.

## Styling Constraints

Follow `frontend/STYLING.md`.

- Use spacing, radius, typography, line-height, motion, border, blur, state, and accent tokens from `src/app.css`.
- Use `.glass-panel`, `.glass-tile`, `.btn`, `.btn-primary`, and `.btn-glass` instead of hand-rolling equivalent surfaces.
- Avoid raw pixel dimensions except where the styling guide allows them, such as intrinsic icons or small thumbnail floors.
- Use `clamp()` or existing tokens for artwork and structural dimensions.
- Use auto-fit grids for card-like groups.
- Use `var(--content-width)` for page-width behavior.
- Do not introduce new global tokens unless an existing token cannot represent the value.
- Do not use inline styles in Svelte templates.
- Remove orphaned CSS while changing markup.
- Do not hardcode accent hex colors.

The implementation should also avoid a one-note palette. Diagnostics can use state colors for meaning, but the page should still inherit the user's theme palette.

## Accessibility And Interaction

- Buttons must have clear labels or accessible labels.
- Track rows should remain keyboard reachable where they perform an action.
- Hover-only diagnostics should also be visible as text or reachable via focus.
- Pending rows should not expose actions that the backend cannot fulfill.
- Error and loading states should not block existing playback controls.

## Testing And Verification

Add focused frontend coverage for derived diagnostics helpers if those helpers are extracted. At minimum, cover:

- Missing DSP produces pending or unknown rather than clash.
- BPM/key-compatible transitions produce good or okay verdicts.
- Pending `automix-new` rows are counted separately.
- Health status degrades when the seed has no DSP or the queue is empty.

Run the smallest relevant frontend checks:

- `pnpm lint:css`
- `pnpm lint:inline-styles`
- Existing Svelte or TypeScript check command used by the repo.
- Any new or touched Vitest tests.

## Out Of Scope

- New backend Automix scoring.
- Candidate preview before Automix writes queue rows.
- Database migrations.
- Playback runtime, gapless, or WASAPI changes.
- Release flow changes.
