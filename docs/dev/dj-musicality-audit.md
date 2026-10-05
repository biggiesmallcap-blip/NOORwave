# DJ musicality audit and implementation

The existing DJ engine, queue, decoder, mixer, scheduling and safety paths are the foundation. These changes extend them; they do not replace playback architecture. Real-track listening remains necessary to assess musical quality.

## Focused investigations

The audio, DSP, planner and frontend investigations were combined before behavior changes. Timing and visual workers subsequently investigated reported regressions against the copied test database and live event history.

### Audio transition path

`DjEngine` loads profiles, corrections and policy. The planner emits a `TransitionProgram`. Player preparation adapts, persists and arms it, preserving the event identity. The playback runtime prepares the existing two-deck `noor_mix::Mixer`, renders the overlap, installs the mix followed by the incoming remainder, and promotes the existing queue. Runtime events persist the actual fire and renderer outcome. Legacy overlap and boundary fallback remain available when preparation fails.

Existing execution includes SafeCrossfade, SlamCut, BassSwap16/32, LongHarmonicBlend, FilterSweep, DropTease16 and drop preview. The mixer already supports deck gain, low/mid/high EQ, constant small incoming playback-rate adjustments, source cues and bounded preview loops. The live handoff contract supports one constant B rate; dynamic rates and A stretching remain rejected.

The previous render adapter rebuilt several programs from their template names and fixed durations. That erased planner source cues, durations, phase markers and automation before execution. The adapter now rescales and validates the executable program, preserving intent for supported strategies. Unsafe or unsupported programs continue to fall back. SafeCrossfade retains its established envelope and optional incoming bass duck.

### Existing DSP

Persisted profiles contain beat/downbeat grids, estimated phrase boundaries, mix-in/out points, intro/outro, breakdown/drop candidates, safe windows, energy contour, loud-body LUFS, true peak, scope and beat confidence. Separate passive DSP contains BPM, key/Camelot key, energy, beat strength, danceability, spectral centroid and stereo width. Analysis is frequently limited to the opening 90 seconds. Vocal arrays currently contain placeholder zeroes and do not establish instrumental material. Automatic structure estimates are heuristics rather than ground truth.

Corrections include BPM multiplier, beat/phrase offsets, safe-only, transition speed and manual drop cues. These remain applied. Offsets do not turn an inferred grid into measured phase.

The profile bridge adds backward-compatible evidence fields for beat confidence, scope, energy contour, vocal provenance, independent tempo/strength and inferred grid provenance. Grid BPM keeps its original meaning; independent DSP BPM is carried separately.

### Planner repetition and useful behavior

The existing planner has effective safety overrides, small tempo bounds, manual corrections, harmonic rules and stable fallbacks. Its short-circuit decision tree repeatedly selected a narrow family. Opening-only structure limited sensible tail opportunities. Fixed execution durations and template reconstruction flattened audible differences. Unknown vocals were previously easy to mistake for clean instrumental space. No recent audible-history preference helped avoid repetition.

The adaptive extension compares a bounded set of existing sensible entry/exit opportunities and strategies. Safety gates precede scoring, so preference, feedback and variety cannot admit an unsafe candidate. Scores expose suitability, confidence, rhythm, phrase, key, vocal space, entry cost and bounded policy adjustments. There is no random selection or new ML model.

### Frontend

The previous view emphasized diagnostics and separate deck displays. DJ and Automix now share an outgoing → strategy → incoming story. Secondary information lives behind Why this transition?, Fine Tune and Diagnostics. The projected SVG lanes represent real gain/EQ automation, source cues, eligible measured markers, overlap, energy direction and confirmed progress. No simulated physical deck or additional WebGL dependency is introduced.

## Musical strategies and policy

The existing planner remains callable. Adaptive adds Smooth blend, Club mix, Quick mix, Energy lift, Energy reset and Drop swap alongside existing bass, cut and filter strategies. Strategies change executable duration, incoming entry, gain shape, bass ownership, handoff point and permitted timing. They are not labels on one envelope.

Measured rhythm supports bounded blends and bass exchanges. Phrase evidence supports club/energy strategies. Cuts require credible downbeat evidence. Automatic drop selection requires stronger beat/profile evidence, preceding breakdown, local energy rise and phrase support; a verified manual cue can supply drop location. Unsupported sophisticated choices degrade toward simpler musical execution and SafeCrossfade.

Conservative, Balanced and Adventurous change eligible strategy and entry sets, acceptable energy movement and duration behavior. Preferences influence admitted candidates rather than forcing a risky program. Slower/Faster affect real duration. Only actually heard fired transitions influence diversity; both bass variants share one family. A mild recent-history penalty applies only near the best quality score and is capped. Good/Bad influence the heard strategy gently; Too Safe/Too Bold adjust bounded adventurousness. Replacing feedback replaces its effect, and unplayed events cannot be rated.

An audible program remains stable. An unheard future program may refresh when analysis or preferences change, while retaining the prepared decoder, PCM and event identity. Both old and new targets must remain at least two seconds ahead; the runtime must acknowledge acceptance before the event is updated.

## Reported live regressions and corrections

The timing screenshot and seeking reports exposed paths that initial synthetic verification missed.

### Scheduling and timing accounting

A new 0.75 scope gate excluded every opening-only persisted profile in the reported database, whose scope confidence was 0.65. This removed prior exact grid scheduling. Credible measured grids can again anchor at 0.65 scope with strong beat evidence. Synthetic zero-origin grids are not certified measured phase; contradictory independent tempo prevents phase claims. Structural points outside actual coverage remain excluded.

Reusing an already armed program now restores its saved scheduled overlap. Previously the caller's generic overlap could replace the executable program's overlap, affecting both preparation and fire timing.

Fallback scheduling uses the decoded audio end, while the stored estimated target used metadata duration. A delta against those different clocks can include duration disagreement as well as genuine callback lateness. The additive runtime target field retains the actual decoded target separately from the original estimate. Jitter uses the matching target, both are exposed, and historical rows are not retrospectively rewritten. Actual lateness remains bad when it is genuinely late. Fallback duration errors no longer calibrate beat-sensitive scheduling or disable creative rendering.

### Incoming source clock

Cue/rate variety exposed an existing assumption: a rendered overlap followed by the B remainder reported output-buffer time as incoming source time. A small per-buffer mapping keeps decoding/output counters on their existing timeline while publishing source position, translating source anchors and buffer bounds, and exposing separate output-clock mix progress. Source position advances from the chosen cue at the actual B rate during the overlap, then follows the original remainder continuously.

Accepted seeks restore original incoming audio rather than replaying the outgoing audio embedded in a historical mix. Prepared buffers are invalidated as appropriate. Queue promotion, decoder append, gapless continuation and fallbacks retain their established architecture.

While the retained incoming prefix is still available, decoded seek bounds include its skipped intro. Seeking before a chosen entry cue restores that original audio immediately; once compaction has discarded it, ordinary segment seeking applies. The callback regression verifies a pre-cue seek and the actual emitted incoming samples.

A live paused-playhead restore additionally found that the callback's paused gate leaves pending seeks unconsumed. The accepted paused seek now updates the buffer cursor and published source clock on the control thread, preserving silence. A regression drives paused callbacks after forward/backward seeks and checks the public handle position, source offset and silent output.

### Visualization

The visualizer previously treated crossing a planned start as proof of an audible fire. Scrubbing could therefore animate a transition that playback suppressed. It could also retain a previous pair after a track mismatch and interpolate an unbounded seek jump. Late-join elapsed frames were added to an already late actual fire time, double-counting lateness.

An armed plan is now a stationary scheduled preview. Live progress requires confirmed rendered execution. Accepted seek revisions block cached live events; the backend preserves historical event identity while clearing its separate visual association. Local interpolation is identity-checked, bounded and converted from incoming source delta to output delta using the actual rate. The runtime provides authoritative installed-handoff output progress, independent of buffer compaction. Render origin and actual fire are separate.

The installed overlap clock also preserves the original pair while paused, even though pause flushes the listening session and resume opens a new one. Persisted event lookup is permitted only while that runtime overlap exists, so history cannot recreate a transition after seeking or completion. Metadata-based fallback countdowns are labeled Estimated until the decoded target is known.

The scene uses labeled lanes and a shared start/handoff/complete axis, with compact idle presentation. Bass curves and claims depend on actual LowGain automation. SafeCrossfade is described as a gain crossfade with an optional modest bass duck. Synthetic/weak grids do not draw certified beat/downbeat/phrase alignment. Manual cues remain available.

### Actual reported DSP and audible selection

Read-only inspection found:

| Track | DJ grid / confidence | Independent DSP tempo / strength |
|---|---|---|
| Photek — The Hidden Camera | 63 BPM / 0.437 | 125.11 BPM / 0.533 |
| Eschaton — Dorado | 115 BPM / 1.000 | 171.43 BPM / 0.851 |
| Technimatic — Looking for Diversion | 58 BPM / 0.378 | 174.04 BPM / 0.714 |

All had approximately 90 seconds of coverage and 0.65 scope confidence. The reported SafeCrossfades were genuine planner choices executed by the mixer, rather than ambitious plans silently failing to render. The legacy planner could select more exciting names while ignoring questionable phase confidence; that does not prove phase lock.

Legacy v2 analyser profiles did not persist detector provenance. Their zero-origin uniform grids are conservatively treated as inferred phase, with phase confidence capped. The v3 profile source distinguishes the existing measured detector from tempo-only and provisional fallback. Imported profiles and corrections preserve their existing contract.

Balanced/Adventurous can choose a short opening-entry, unity-rate QuickMix with a real low-band handoff when independent tempos agree within the existing 3% normal/half/double family tolerance and both strengths meet 0.65. A 0.55 strength is sufficient only when each grid corroborates its independent tempo within 3%; this does not certify phase. Credible, bounded incoming energy changes also admit short EnergyLift/Reset envelopes with late bass arrival or early withdrawal. They explicitly report unverified phase, never phrase/drop lock. Conservative stays safe for this tier. Dorado → Looking for Diversion follows the QuickMix path; Photek → Dorado remains protected. Measured compatible grids retain the broader musical strategies. Weak evidence is not promoted to certainty simply to increase variety.

## Cockpit request failures

The original reported loading failure did not reproduce in a fresh browser: all three DJ endpoints returned 200. Its original cause remains unidentified. A concrete error-handling defect discarded independent successful responses and repeatedly generated toasts when one request failed.

The cockpit now accepts partial success, retains its last useful snapshot, distinguishes initial unknown enabled state from Off, limits polling to one request group, backs off to 30 seconds and offers one inline failure with request details and Retry. Requests cannot update an unmounted view. A controlled browser probe returning 500 for only status verified retained tracks/On state, backoff, no toast spam and recovery.

## Validation and acceptance limits

Initial verification passed 124 mixer tests, 1,709 server tests and 998 frontend tests; cockpit request handling increased frontend coverage to 1,001. Those results missed the live regressions above. The follow-up adds source-clock, seek, event lifecycle, effective timing target, DSP bridge and persisted real-pair integration coverage. Final follow-up results are recorded below.

Existing signal QA checks cover peak ceiling, nonfinite values, click-sized discontinuities, DC offset, loudness jumps and deterministic rendering. Synthetic PCM fixtures establish execution differences between strategies, not subjective musical quality. Browser fixtures verify scheduled versus audible progress, seek suppression, stale identity, promotion, completion and honest automation claims. Live backend checks verify serving and response integrity.

Listening acceptance should exercise compatible instrumental material, crowded vocals, mismatched tempo, rising/falling energy, a verified cue and a weak profile. Confirm actual audio overlap, bass exchange, source entry, queue continuation, fire delta and matching visual progress. A real session must feel better before subjective improvement can be declared. No production library/database is modified: the development instance uses the explicitly authorized database and matching secret copy under `.scratch/test-dev/data`.

### Follow-up verification

- Mixer suite: 127 passed, one existing benchmark ignored. Includes actual rendered PCM differences and audio QA for the short phase-uncertain energy strategies.
- Server suite: 1,727 passed, five existing tests ignored. Includes source cue/rate progression through actual callback, decode append/EOF, compaction, future anchor conversion, plain incoming seek restoration, paused seek acknowledgment and real persisted DSP pair planning.
- Frontend suite: 1,013 passed across 165 files, including actual rendered SVG assertions and accepted/stale/failed seek revisions.
- Svelte/TypeScript: zero errors/warnings. Full frontend lint and production build passed. Diff checks passed.
- Fresh Chrome against the restarted test backend returned 200 for enabled/policy/status with no failed requests or page errors. The idle layout is compact and keeps the actual current/next pair visible.

A silent live paused-playhead restoration exposed the pending-seek defect; the full regression suite verifies its correction. Startup intentionally resets the ephemeral runtime playhead. The final live restoration skipped the earlier snapshot because the user's track selection had changed, preserving the latest selection. No real audible DJ session has yet been subjectively accepted. Old timing history is preserved; new events carry the decoded target when appropriate.

### Subsequent seek / transport desynchronization

A live seek → pause → resume probe reproduced accepted seeks with zero reported position/buffer and a Playing response reverting to Paused. Device metadata was absent even though the runtime accepted commands. The listener subscribed inside its asynchronously scheduled task, permitting loss of the initial Ready event. It now registers synchronously and requests a replay of current device metadata before playback dispatch. A regression emits Ready before the listener task runs; it failed before the change.

Pause/resume responses also returned persisted position rather than the runtime source clock. A route regression with a saved zero position and successive runtime seeks to 10, 3 and 20 seconds failed before correction. Both routes now return the live position.

Decoded seeks are applied under the existing buffer mutex before acknowledgment, for both playing and paused transport. Paused seeks cancel any old pause fade, preventing it from draining the newly selected audio. A callback regression failed before that correction. The decoder publishes decoded bounds as samples arrive and at EOF even while output is paused; previously the paused callback could leave those bounds stale. Direct adoption of a prepared next deck now binds all transport readers, and a forced segment restart cannot reuse a deck decoded at a different origin.

These changes extend the existing playback runtime and retain its queue, gapless, scheduling, source mapping and fallback paths.

Verification: all 1,732 server tests passed (five existing tests ignored), including five added regressions. The rebuilt native backend reports valid device metadata and its actual source position. A muted Chrome test exercised the real frontend stores and HTTP transport against native playback: paused seeks to 10, 3 and 20 seconds, resume/pause after each, a playing seek to 5 seconds, and out-of-buffer segment restarts to three minutes and back to the opening. Pause/play agreed with runtime throughout, buffered paused seeks matched exactly, and the largest observed UI/runtime gap during playback was 130 ms. No browser errors occurred. Segment restart retains its existing nearest-segment entry semantics and reports the actual resulting position. The test restored the original selection, paused playhead and volume.

### Early preparation, real analysis and speed controls

The reported last-30-second test exposed several independent causes of apparent inactivity:

- Native next-track preparation was triggered by NearEnd. DJ pair preparation now reuses that same path in a background task from track start, keeping network work off the runtime event listener. Playback generation and queue hash are distinct; passing the queue hash to the playback guard silently skipped preparation. A real route regression checks next-track preparation two seconds into a three-minute track with unequal generations. Decoder attempts remain bounded and NearEnd remains a final fallback opportunity.
- Manual seek suppression used the 30-second NearEnd threshold rather than the actual armed transition window. Seeking to 30 seconds remaining now retains a future mix. Crossing its actual start still suppresses late/stale execution, and seeking back restores future eligibility.
- An unheard plan now refreshes when DSP or policy changes, with a two-second guard on both old and new targets, runtime acknowledgment, stable event identity and unchanged decoder/PCM. Exact decoded length still governs fallback firing; metadata is used only to guard early updates before EOF.
- An unanchored mix cannot render against the currently playing opening merely because it was prepared early. Once decoded length is known, tail rendering uses the future countdown start. A PCM regression distinguishes the tail from the opening.
- SafeCrossfade now has executable Slower/Neutral/Faster durations of 9/6/3 seconds. The neutral fallback remains unchanged. Musical programs retain their beat-derived durations and bounded speed behavior.

Streamed v2 analysis deliberately skipped the existing measured detector and persisted a zero-origin tempo comb. v3 runs the existing detector on the serialized background actor, retaining tempo-only and provisional fallbacks with explicit source provenance. Old generated profiles are selectively upgraded; imported profiles and manual corrections remain intact. Cached usable profiles remain available during retries, with bounded failure/backoff bookkeeping. Fresh already-decoded stream manifests may be reused for separate analysis; native playback itself does not currently capture the DJ 90-second window.

The existing beat detector took roughly seven minutes per profile in the unoptimized dev build. Optimizing only its existing DSP/maths dependencies reduced measured processing to approximately 24–26 seconds in live checks. This is processing time, not a promise that network fetching and the serialized analysis queue complete in that interval. Release behavior and the algorithm are unchanged.

Timing calibration previously included months-old events. The copied database contained a May error of −191,360 ms that permanently vetoed current creative transitions. Calibration now uses the past 24 hours with the same thresholds; historical events remain unchanged and visible. A regression first establishes rejection of recent genuine 2-second errors, then verifies that old errors do not veto four current tight fires. Short, unity-rate, opening-entry EnergyLift/Reset programs with explicit unverified phase also use QuickMix's scheduling protections instead of the beat-sensitive gate. Structural and beat-sensitive energy programs retain that gate.

### Native execution evidence

A muted Chrome probe found Juniper → Rave's planned QuickMix six seconds into playback, approximately 405 seconds before its transition. The same prepared event changed from 3.9 seconds to 4 seconds (Slower) and 1.94 seconds (Faster). Seeking to 30 seconds before the end, pausing and resuming retained the future mix. It executed as a rendered QuickMix handoff with a 7 ms delta; the scene stayed a preview until confirmed fire.

The subsequent running session persisted actual rendered programs, including:

| Pair | Executed strategy | Duration | Incoming rate | Fire delta |
|---|---|---:|---:|---:|
| Confronted → On My Mind (Ferreck Dawn Remix) | BassSwap32 | 22.89 s | 1.02265 | +7 ms |
| On My Mind (Ferreck Dawn Remix) → Deep End | ClubMix | 23.41 s | 0.97733 | +5 ms |
| Deep End → Amnesiac | EnergyLift | 3.82 s | Unity | +7 ms |
| Amnesiac → Stronger | QuickMix | 4.00 s | Unity | +10 ms |

All four have `rendered_handoff`, no runtime renderer failure, and distinct preserved low-band automation. This verifies actual execution and timing, not subjective listening acceptance. An earlier ClubMix probe encountered incoming DASH prebuffer failure and correctly recorded a missed/boundary fallback rather than pretending to blend. Intermittent Tidal CDN failures remain a material limit; fallbacks and retry caps remain enabled.

The live handoff also exposed provider IDs appearing instead of track names. Executed streamed-event labels now resolve the existing local title/artist by Tidal identity, with regression coverage.

Verification after these corrections: full server suite 1,742 passed with five existing ignored tests before the final calibration horizon change; the final DJ-focused suite passed 302 tests with one existing ignored test, including the calibration and live-label regressions. Mixer suite passed 129 tests with one existing ignored benchmark. Frontend validation above remains applicable; no frontend code changed in this final correction stage. Diff checks passed.

A final muted native seek probe passed 14 transport checks: exact paused seeks at 10/3/20 seconds, resume/pause after each, playing seek, and out-of-buffer restarts to three minutes and the opening. UI/runtime playing state agreed throughout; the maximum sampled UI/runtime position gap was 170 ms during playback. It restored the latest paused playhead and volume. A fresh cockpit browser check returned 200 for every DJ request with no page errors or failed requests, displayed the actual current/next titles and a scheduled 9-second Slower SafeCrossfade, and did not animate the paused seek as an audible transition.

### Stream prebuffer failures and repeated analysis

The October 5 test-backend logs contain 57 DJ rebuild failure entries and repeated four-second segment timeouts against `sp-ad-cf.audio.tidal.com`. Track 53152047 failed at 08:48, 08:49 and 08:50 UTC, then restarted at 08:55 after an authentication failure. Track 422578330 exhausted four attempts between 09:29 and 09:33, then restarted at 09:39. These are actual retry cycles, separate from the measured detector's processing time.

Three focused regressions failed before correction:

- A local HTTP fixture requiring the established CDN client identity rejected startup init/media requests. Startup prebuffering used the generic API client while background downloading used the dedicated CDN client. Both now use the existing CDN configuration, with separate clients for their separate Tokio runtimes. Connection pools must not span the decoder's stopped prebuffer runtime and the background runtime. Fetch concurrency, retry limits and cancellation remain unchanged. The fixture verifies header consistency; it does not establish that headers caused the external CDN outages.
- A polling test aged an exhausted four-attempt failure past five minutes and reproduced a new automatic rebuild. Terminal analysis failures now retain an in-memory 24-hour cooldown; explicit Rebuild analysis still clears it. The final error states that automatic attempts stopped instead of promising another retry. Existing transient backoff remains intact.
- A cached profile without a waveform bypassed retry protection. Imported profiles also cleared their failure counter merely because a cached profile existed. Missing-waveform rebuilds now respect in-flight work, retry backoff and exhaustion for both generated and imported profiles. Cached usable analysis and manual corrections remain available.

Dedicated analysis now uses the same handler-layer stream resolver as native playback, including current session tokens and existing single-flight authentication recovery, instead of a captured token without recovery. Existing LOW/LOSSLESS fallback, CDN circuit breaker, source timeouts and safe playback fallback are retained; signed CDN URLs are not rewritten.

Verification: full server suite 1,746 passed, five existing tests ignored; formatting and diff checks passed. After rebuilding the existing test backend, native current-track startup, incoming-track preparation and a seek near the end successfully prebuffered in 47–52 ms. The current queue selection, exact paused position and volume were restored. The cockpit browser check returned 200 for every DJ request with no failed requests or page errors. This successful sample does not guarantee that the external CDN is consistently reachable.
