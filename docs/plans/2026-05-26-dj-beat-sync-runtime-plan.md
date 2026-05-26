# DJ Beat Sync Runtime Plan

Date: 2026-05-26

## Summary

The DJ engine should move toward beat-matched, phrase-aware autonomous transitions in phases. The first implementation target is not a smarter planner by itself. The first target is a runtime mixer path that can actually render the planner's source offsets, loops, gain automation, EQ automation, and small tempo nudges.

V1 keeps deterministic Rust rendering in `noor-mix`, keeps the current madmom-derived beat/downbeat path, and ships direct playback-rate sync only inside the existing `0.97..1.03` cap. Wider tempo sync waits for an offline Signalsmith Stretch evaluation. Rubber Band and libkeyfinder remain license-review items. No local LLM or generative model participates in realtime timing or transition rendering.

Reference behavior:

- Algoriddim Sync aligns tempo and phase from analyzed beat grids: https://help.algoriddim.com/topic/using-djay/how-does-sync-work
- Serato beatgrids treat correct grid markers as the basis for sync and timed effects: https://support.serato.com/hc/en-us/articles/202856014-Beatgrids-in-Serato-DJ-Pro
- Beat This is the offline beat/downbeat benchmark candidate: https://github.com/CPJKU/beat_this
- Signalsmith Stretch is the preferred pitch-preserving stretch evaluation candidate: https://github.com/Signalsmith-Audio/signalsmith-stretch

## Non-Negotiable Design Rules

- Runtime sample position is the only timing authority. Page load, cockpit polling, and `/api/dj/status` must stay read-only.
- Every DJ transition has an explicit rendered mode. The UI must not claim `BassSwap16`, `DropTease16`, or `LongHarmonicBlend` unless that mode is what the audio path rendered.
- `DropTease16` is an overlay, not a handoff. It must not promote the next track, emit outgoing `Finished`, or advance the queue.
- Source positions use frames, not interleaved samples. Program fields must be named `deck_a_start_frame`, `deck_b_start_frame`, and loop regions must be source-frame based.
- The realtime callback must not allocate, log, hit DB, parse JSON, lock unbounded state, or fetch network data. Mixer construction, validation, and fallback decisions happen before the callback uses a prepared render object.
- Missing profile, low confidence, bad beat grid, unsafe BPM ratio, stale queue generation, or late decode always downgrades to SafeCrossfade or legacy overlap with an explicit reason.

## Phase 1: Mixer Runtime Foundation

Goal: make the `noor-mix` program path audible before adding smarter transitions.

Implementation:

- Extend `TransitionProgram` with serde-defaulted `deck_a_start_frame` and `deck_b_start_frame`.
- Change `DeckBuffer` construction to support a start frame while preserving the existing zero-start constructor for tests and older callers.
- Apply `program.loops` inside `Mixer::new`, using source-frame coordinates per deck.
- Add a prepared runtime render object that owns the mixer and can be consumed by the output path without allocation or DB work in the callback.
- Wire SafeCrossfade through the mixer path first. Keep legacy overlap as the fallback when the prepared mixer is missing, stale, or rejected.
- Keep all non-SafeCrossfade templates downgraded until their mixer behavior is tested.

Tests:

- `noor-mix` test proves deck start frames render from the expected source offset.
- `noor-mix` test proves loop regions wrap at the expected frames.
- Runtime test proves prepared SafeCrossfade uses `renderer_mode = "dj_mixer_program"`.
- Runtime test proves stale queue generation or stale track ids fall back without silence.
- Manual gate: 5 TIDAL-to-TIDAL SafeCrossfade transitions and 3 DJ-disabled legacy transitions.

## Phase 2: Small Beat Nudge Handoff

Goal: add beat-phase sync for normal queue-consuming handoffs while staying inside the current 3 percent rate cap.

Implementation:

- Pick outgoing transition start from the best downbeat or beat near the target lookahead window.
- Pick incoming entry from a compatible downbeat or beat, then set `deck_b_start_frame` so the musical entry aligns with outgoing phase.
- Compute `tempo_ratio` from the nearest compatible tempo family. Accept only `0.97..1.03`.
- Add `PlaybackRate(DeckId::B)` automation only when the ratio is inside the cap and the profile confidence gate passes.
- Store and expose `sync_unit`, `tempo_ratio`, `phase_error_ms`, `planned_start_ms`, `actual_start_ms`, and downgrade reason.
- Keep this phase handoff-only. It may consume the next queue item through the existing promotion path.

Tests:

- Planner test accepts compatible BPM pairs inside 3 percent and rejects wider pairs.
- Planner test aligns an incoming downbeat to an outgoing downbeat within one beat.
- Runtime timing test verifies actual start is computed from the outgoing engine position, not the cockpit status page.
- Cockpit contract test shows tempo ratio and sync unit only as renderer facts.

## Phase 3: Better Structure Markers

Goal: stop treating drops as fixed offsets like `downbeats[24]`.

Implementation:

- Bump `DJ_PROFILE_VERSION`.
- Derive phrase candidates from downbeats, 8-bar and 16-bar phrase boundaries, and energy contour changes.
- Detect drop candidates from energy rise, downbeat confidence, local low-frequency energy, and phrase position.
- Keep vocal analysis optional. If vocal facts are missing, the planner can still use energy/downbeat structure with lower confidence.
- Store multiple candidates, not one magic drop. Each candidate carries confidence and source reason.

Tests:

- Profile tests cover 30s, 90s, and full-track analysis windows.
- Profile tests prove low confidence produces no drop candidates.
- Regression test proves old profile versions are rebuilt or ignored for structure-dependent templates.

## Phase 4: DropTease16 Overlay

Goal: let Bold mode briefly blend the next track's drop over the current track, then return to the current track without consuming the queue.

Implementation:

- Add `DropTease16` as a separate template role: `overlay`, not `handoff`.
- Gate to Bold intent only.
- Require outgoing and incoming full profiles, compatible downbeat phase, a valid incoming drop candidate, enough outgoing audio remaining, and tempo ratio inside `0.97..1.03`.
- Start deck B before its detected drop so the drop lands on an outgoing 16-bar boundary.
- Duck outgoing lows and mids while incoming owns the drop for 8 to 16 bars.
- Fade incoming out and restore outgoing. Do not promote deck B. Do not emit outgoing `Finished`. Do not advance the queue.
- Emit overlay telemetry: `overlay_start_ms`, `overlay_end_ms`, `phase_error_ms`, `tempo_ratio`, `drop_candidate_confidence`, and `overlay_status`.

Tests:

- Runtime test proves `DropTease16` does not change active track id or queue item id.
- Runtime test proves overlay cleanup restores the active deck.
- Planner test rejects `DropTease16` for Safe and Balanced intents.
- Planner test rejects missing drop, low confidence, unsafe tempo ratio, and insufficient outgoing runway.
- Manual gate: 5 Bold-mode overlays on electronic-compatible pairs and 5 rejected non-compatible pairs.

## Phase 5: Smart Stretch Evaluation

Goal: decide whether wider beat sync is worth shipping without risking pitch or realtime stability.

Implementation:

- Evaluate Signalsmith Stretch offline with fixed test material and synthetic click tracks.
- Measure CPU, latency, phase drift, peak behavior, and audible artifacts across 3, 5, 8, and 12 percent tempo deltas.
- Do not place Signalsmith in the realtime callback until the prepared-buffer path can absorb its latency and memory needs.
- Keep Rubber Band blocked unless licensing is explicitly approved.
- Keep Beat This as an offline benchmark against current beat/downbeat analysis, not a runtime dependency.

Decision gate:

- Ship status stays `Small Speed Nudge first`.
- `Test Smart Stretch second` means offline render and objective metrics only.
- `Smart Stretch Now` is allowed only after the evaluation passes and the implementation can run from prepared buffers with explicit fallback.

## Acceptance Criteria

- SafeCrossfade through the DJ mixer path is audible and tested before any expressive template ships.
- Beat sync reports phase error and falls back when it cannot prove a compatible grid.
- DropTease16 never consumes the next queue item.
- 3 percent remains the ship cap for direct playback-rate sync.
- Wider tempo sync is blocked on Signalsmith evaluation and a separate implementation plan.
- Cockpit labels always describe what was rendered, not only what the planner wanted.
