# DJ-Aware Queue Candidate Ranking Plan

Date: 2026-05-27

## Summary

Add DJ-aware ranking before automix, radio, or discovery candidates are inserted into the queue. This is not a transition planner change and it must never reshuffle the current plus next pair. User-selected order stays authoritative.

The goal is to feed the DJ planner better adjacent candidates: compatible tempo, compatible Camelot key, usable profile confidence, and structure facts when available.

## Implementation

- Add a ranking helper that scores candidate media refs before queue insertion. Keep it side-effect free: input candidates plus current context, output ranked candidates plus score reasons.
- Use existing facts only: BPM, Camelot/key compatibility, DJ profile status, profile confidence, safe-crossfade-only flags, phrase/drop facts when present, source freshness, and library/listening penalties already used by the caller.
- Scope the first wiring to generated queues only: automix extension, radio, and discovery insertion paths. Do not apply it to explicit user enqueues, drag/drop reorder, play-next, or the already-current next row.
- Keep missing facts conservative. Unknown BPM or key should not hard reject a candidate, but ready profiles with compatible BPM and key should outrank unknowns.
- Emit a short reason string for the top ranked insertions so cockpit/debug logs can explain why a candidate was preferred.

## API And Data Shape

- No DB migration for v1.
- No `/api/dj/status` contract change for v1 unless score reasons are later surfaced in debug.
- Candidate score output should include `score`, `reasons`, and the original media ref or queue insert payload.
- Reuse existing profile lookup keys for local tracks, TIDAL tracks, and pending queue items.

## Tests

- Unit test ranks an inside-cap BPM and compatible Camelot candidate above a BPM/key clash.
- Unit test keeps original order stable when candidates have equal DJ scores.
- Unit test does not reject candidates with missing BPM/key, but ranks ready compatible profiles higher.
- Integration test for one generated queue path proves user-selected queue rows and the current plus next pair are untouched.
- Regression test covers pending or TIDAL-only refs so ranking is source agnostic.

## Guardrails

- Do not mutate current playback state or queue rows already visible to the user.
- Do not call network, decode audio, or rebuild profiles during ranking.
- Do not claim ML learning. This is deterministic candidate scoring.
- Keep direct runtime tempo sync capped at `0.97..1.03`; wider sync remains blocked by the Signalsmith gate.
- If ranking cannot prove a better candidate, preserve caller order.
