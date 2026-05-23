# DJ BassSwap16 Renderable Plan

Status: Draft

## Goal

Make `BassSwap16` the next renderable DJ transition template after `FilterSweep`, while keeping `SafeCrossfade` as the timing authority and keeping `SlamCut` downgraded.

## Current Baseline

Current committed DJ work is PR-ready on `feature/build-dj-engine-architecture`.

- `SafeCrossfade` renders.
- `FilterSweep` renders with fallback visibility and timing instability gating.
- `SlamCut` is still downgraded.
- DJ profile rebuild analysis uses LOW quality.
- Normal playback decode no longer queues heavy DJ profile analysis.
- Near-end manual seek suppresses accidental auto transition.

## Product Rationale

User testing showed FilterSweep can be useful for DnB and electronic material, but rough transitions still come from low-end handoff and percussion clash. `BassSwap16` targets the main audible failure: both tracks fighting for the bass range during the overlap.

This should sound less like a generic crossfade than FilterSweep because the low end changes ownership at a phrase boundary while mids and highs continue blending.

## Scope

Implement `BassSwap16` only.

- Keep `BassSwap32` non-renderable.
- Keep `LongHarmonicBlend` non-renderable.
- Keep `SlamCut` non-renderable.
- Keep current SafeCrossfade timing path.
- Keep existing fallback and downgrade reporting model.
- Do not tune fire-ahead in this PR.
- Do not add a true high-pass or low-pass filter path in this PR.

## Behavior

`BassSwap16` should render only when:

- The planner chose `BassSwap16`.
- Both profiles are current enough to plan.
- Timing evidence is stable enough, using the same instability posture as FilterSweep unless a stricter gate is needed.
- Renderer can produce a valid `TransitionProgram`.

If any condition fails:

- Planned template remains visible as `BassSwap16`.
- Actual renderer falls back to `SafeCrossfade`.
- Downgrade reason is visible, for example `template_not_renderable` or `timing_unstable`.
- Planning reason remains separate from downgrade reason.

## Render Shape

Use the existing `noor-mix` EQ and gain automation.

Initial conservative render shape:

- Deck B gain fades in across the full transition.
- Deck B low band starts cut.
- Deck A low band stays full early.
- Low-band ownership swaps around the midpoint phrase boundary.
- Deck A mids and highs taper after the bass swap.
- Deck B mids and highs stay mostly present so the incoming track can be recognized before its bass arrives.
- Avoid EQ boosts above unity for V1.

The first version should favor cleanliness over drama.

## Timing

Use the same runtime timing path as `SafeCrossfade`.

Candidate duration:

- `BassSwap16`: phrase-derived 16 bar planner duration when available.
- If runtime needs a fixed V1 cap, start around 12 to 16 seconds and keep fallback visible.

Do not make `BassSwap32` renderable until `BassSwap16` is proven stable.

## Tests

`noor-mix`:

- `BassSwap16` validates.
- `BassSwap16` contains low-band swap automation.
- Rendered output is limiter-safe.
- Mixer render path does not allocate.
- Bass handoff has only one deck owning the low band at the midpoint.

`noor-server`:

- `BassSwap16` passes through as renderable.
- `BassSwap32`, `LongHarmonicBlend`, and `SlamCut` still downgrade.
- `BassSwap16` uses the SafeCrossfade timing path.
- Timing instability downgrades `BassSwap16`.
- Status reports planned and renderer template correctly.

Frontend:

- Cockpit contract accepts `BassSwap16` as an actual renderer template.
- Non-renderable templates are not shown as active renderers.

Commands:

- `cargo test -p noor-mix`
- `cargo test -p noor-server playback::player::tests::dj`
- `cargo test -p noor-server server::routes::dj_routes::tests`
- `pnpm test -- dj_page_contract`
- `pnpm check`

## Manual Audio Checks

Use several queue pairs:

- Electronic or DnB pair with compatible phrase structure.
- House or four-on-the-floor pair with clear basslines.
- Jazz, guitar, or clap-heavy pair that previously sounded rough.

Record:

- Planned template.
- Renderer template.
- Planned start.
- Actual start.
- Delta.
- Whether the bass handoff sounded cleaner than FilterSweep.
- Whether hats or claps felt out of sync.

## Non-Goals

- No SlamCut activation.
- No ML template selection.
- No generative transition programs.
- No new runtime language.
- No licensing-sensitive DSP dependency.
- No adaptive fire-ahead tuning.

## PR Sequence

1. Open current committed DJ branch as the stabilization PR.
2. Start `BassSwap16` as the next PR from that baseline.
3. Only after `BassSwap16` is tested, consider `LongHarmonicBlend`.
4. Keep `SlamCut` blocked until downbeat confidence is stronger.
