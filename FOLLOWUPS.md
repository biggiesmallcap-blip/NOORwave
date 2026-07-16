# Follow-ups

Out-of-scope work flagged while shipping something else. Items live here until
you pick them up (then delete the entry) or decide they're not worth doing
(also delete). One running list keeps everything in one place; GitHub issues
are overkill for a solo project.

Format per item: short title, one or two lines of context, "Spawned by:" link
back to the PR or commit that flagged it.

## Open

### perf: replace the playback PCM Vec+Mutex with a real ring buffer

The audio callback and the decoder thread share
PlaybackSharedState.buffer (Mutex<PlaybackBuffer> over an unbounded
Vec<f32>): the decoder holds the lock while appending each resampled
packet and compacting, which is a priority-inversion risk on the render
thread, and the Vec grows ~10s-45s of PCM per engine (x2 during a
crossfade). The code comment at runtime/shared.rs (BUFFER_GROWTH_WARN
threshold) already calls the ring-buffer rewrite the real fix; growth
telemetry exists on both the PCM buffer and the compressed StreamPipe.
A lock-free SPSC ring (e.g. rtrb) sized ~60s would remove the lock from
the callback entirely. Behavior-preserving but touches every drain/seek/
compact path, so it needs its own focused pass with the shared.rs tests
extended first.
- Spawned by: runtime player code review (previous-track fix session).
### dj: segment fire_ahead_ms calibration by timing source

fire_ahead_ms_from_deltas pools all recent fired deltas. Beat-anchored fires
(downbeat_sync/beat_sync, absolute trigger) now land within ~1 callback block
while gridless fallback fires still carry the metadata-duration error, so the
shared median under-corrects the fallback path and slightly over-corrects the
anchored one. Split the calibration window by timing_source once enough
anchored rows exist.
- Spawned by: DJ transition seam/anchor fix session.

### dj: overlay path (DropTease/DropPreview) has no seam-skip reconciliation

install_prepared_overlay_mixer_buffer and the drop-preview install still start
their rendered buffer at frame 0 at fire time. Overlays play on top of the
live deck (no deck A discontinuity), but their internal beat alignment
inherits the mpsc + install latency the handoff path now compensates via the
install-time skip. Port the live-position skip if overlay flams become
audible.
- Spawned by: DJ transition seam/anchor fix session.

### dj: residual one-block skew at the handoff seam

The install-time skip measures deck A's read_pos when the buffer is spliced;
the incoming stream then starts on its next callback, so up to one output
block (~5-20ms) of skew remains. Inaudible as a flam and masked by the seam
ramps, but if it ever matters, estimate the callback block size and bias the
skip by it.
- Spawned by: DJ transition seam/anchor fix session.

### dj: wire vocal_clash_score into template choice

noor-mix scoring.rs::vocal_clash_score (max product of per-bar vocal presence)
is implemented and tested but nothing calls it; choose_template never considers
vocal overlap, so two vocal-heavy tracks can get a 32-bar bass swap over both
choruses. Needs a design decision (threshold, which templates it demotes, and
whether it keys off vocal_presence or vocal_density) before wiring in - not a
mechanical fix.
- Spawned by: DJ-mode hardening session (transitions/sync/blending).

### dj: enforce the lookahead analysis deadline (deadline_samples is decorative)

RuntimeDjLookahead.deadline_samples is stored and asserted in one test but
never compared against playback position; DjLookaheadFailureReason::
AnalysisDeadlineMissed is #[allow(dead_code)]. Today the crossfade-window
signal plus boundary fallback cover late plans, so nothing breaks, but the
"analysis_late" guardrail shown in SafetyGuardrailPanel can never fire from
the runtime. Either enforce the deadline (flip lookahead to a safe-crossfade
plan when position passes deadline_samples with no prepared program) or drop
the field.
- Spawned by: DJ-mode hardening session (transitions/sync/blending).

### dj: DeckBuffer holds the last frame forever past end-of-buffer

deck.rs advance() clamps position to the final frame, so a deck that runs out
mid-transition sustains its last sample value (a DC ledge) instead of going
silent. Gain automation always fades to zero by resolve_at so it is inaudible
today (and QA's dc_offset check bounds the gross case), but emitting silence
past the end would be strictly safer if a template ever ends non-faded. Also
note tick_into playback rate stays per-block in render.rs - fine while every
producer emits constant-rate events (deck_b_consumed_frames rejects ramps),
but a future ramped-rate feature must move rate evaluation per-frame too.
- Spawned by: DJ-mode hardening session (transitions/sync/blending).

### dj: SafetyLimiter is a hard clipper, not a limiter

limiter.rs clamps samples at +/-0.98 with no attack/release envelope, so a hot
blend distorts rather than ducks. QA's peak/click checks keep planned programs
under the ceiling, so this only matters for pathological content; a one-pole
release gain-computer would be the upgrade. Do not widen the 0.98 ceiling.
- Spawned by: DJ-mode hardening session (transitions/sync/blending).

### dj: v1 renderer template durations are fixed ms, not bar-aligned

v1_renderable_program rebuilds planner programs at fixed render lengths
(BassSwap16 24s, FilterSweep 18s, ...) regardless of BPM, so the swap/fade
midpoints only land on bar boundaries near 120 BPM even though the overlap
window itself starts on a downbeat. Deliberate v1 simplification; revisit when
the renderer can honor planner bar-derived durations end to end.
- Spawned by: DJ-mode hardening session (transitions/sync/blending).

### deps: finish Dependabot #144 - symphonia 0.6 + rubato 3.0 (audio path)

Deferred from the cargo-group bump (commit 79f8c1f6, which landed the mechanical
majors). Both are full API rewrites on the playback hot path with no security or
urgency driver, so held back to de-risk. Verified migration mappings already
exist; the work is scoped, just risky, and needs a real-audio playback smoke
test (decode + resample correctness) that CI cannot prove.
- symphonia 0.5 -> 0.6: SampleBuffer removed (use GenericAudioBufferRef +
  copy_to_vec_interleaved); DecoderOptions -> codecs::audio::AudioDecoderOptions;
  Decoder -> AudioDecoder; get_codecs().make() -> make_audio_decoder();
  track.codec_params is now Option<CodecParameters>, needs .audio()?;
  probe.format() -> probe.probe() returns Box<dyn FormatReader>; Hint moved to
  formats::probe. Touches noor-server playback/decode/mod.rs and
  services/audio_analysis/scanner.rs, plus the noor-mix symphonia dev-dep.
- rubato 0.16 -> 3.0: SincFixedIn -> Async::new_sinc(.., FixedAsync::Input);
  pulls in new audioadapter / audioadapter_buffers crates; process() input is now
  an Adapter and output is InterleavedOwned. Touches playback/decode/resample.rs
  and the noor-mix rubato dep.
- Spawned by: Dependabot #144 triage, deps/cargo-major-bumps branch.

### fix: cap score_genre_tags confidence at 1.0 and backfill existing rows

789 track_genres rows (2%) carry confidence > 1.0 (max 2.27) because
score_genre_tags sums per-source scores without a final min(1.0) cap. Every
consumer using a confidence floor (galaxy filter, and now the audio search
genre rowset) is miscalibrated on those rows. Cap at write time in the scorer,
then one-shot normalize existing rows (UPDATE track_genres SET confidence =
MIN(confidence, 1.0)). Already documented in docs/genre-data-quality-2026-05-07.md.
- Spawned by: search audit, genre:rock fix session.

### fix: cross-family genre tag contamination (psytrance tagged Psychedelic Rock)

Last.fm tags like "Psychedelic Rock" (~0.29 confidence) sit on psytrance acts
(1200 Micrograms, Infected Mushroom, Khruangbin). The confidence-floor rowset
now keeps them out of search, but the tags remain in track_genres and leak via
the rescue branch when they are a track's strongest tag. Needs contradiction
logic (incompatible-family suppression) in the enrichment scorer. Context in
docs/genre-data-quality-2026-05-07.md.
- Spawned by: search audit, genre:rock fix session.

### feat: order genre-filtered search results by matched-tag confidence

Filtered audio search ranks by is_favorite/play_count only; a barely-rock
favorite outranks a definitive rock deep cut. Consider ORDER BY
MAX(matched tag confidence) DESC before the favorite/play-count tiebreak when
genre_ids is non-empty. Needs a join against the curated rowset in the ORDER
BY, so measure query cost on the 36k-track library first.
- Spawned by: search audit, genre:rock fix session.

### perf: evaluate album/artist fallback rowset for search genre matching

Search genre matching now uses filter_subquery(ConfidenceMinWithRescue(0.5)),
which covers tagged tracks. filter_subquery_with_fallback would additionally
rescue fully untagged tracks from album/artist siblings (14k favorited tracks
have zero genres per docs/genre-data-quality-2026-05-07.md), but its
needs_fallback CTE scans the whole tracks table - measure interactive-search
latency on the real library before enabling.
- Spawned by: search audit, genre:rock fix session.

### verify: NSIS www-wipe actually cleans stale chunks on a real update

`noor-app/nsis-hooks.nsh` now `RMDir /r "$INSTDIR\www"` in the preinstall hook so
content-hash-named SvelteKit chunks don't pile up across updates (an install here
had 1695 files vs 253 for a clean build). Only verifiable through a real installer
build: run `cargo tauri build` (or the release workflow), install an old version,
then update over it and confirm `www` ends at the clean file count with no orphaned
`_app/immutable` chunks. Also confirm the `passive` auto-updater path (app closed
during install, so www isn't locked). Portable zips are unaffected (build-portable.ps1
already assembles from a clean dist).
- Spawned by: perf audit 2026-07-04, stale-www-chunks question.

### feat: adaptive (variable-length) crossfade when the next deck is under-buffered

The legacy crossfade is all-or-nothing: if the incoming deck hasn't buffered the full
fade window by the trigger (`crossfade_next_ready`, `playback/runtime/mod.rs`), it skips
the fade. The safe fix shipped this session turns a miss into a clean gapless cut (the
outgoing plays full to its end instead of fading into a gap) but does not blend. A true
shortened crossfade needs a bigger change to the tuned fade envelope: promote at
`effective` before the end (delayed promote / re-arm) and give the fade-in its own length
separate from `crossfade_samples`, which is overloaded as trigger threshold + fade-out
length + fade-in length. Naively shrinking `crossfade_samples` at the full-window trigger
double-sums both decks to full in the middle (+3 dB / clipping) and leaks the short length
into the next track's fade. Touches the don't-touch-without-asking gapless envelope
(`playback/runtime/shared.rs`); validate by ear on a release build. Underlying trigger is
slow TIDAL buffering (12s DASH segment timeouts); prebuffering earlier than NearEnd=30s
would also reduce misses.
- Spawned by: crossfade hard-cut fix, this session.

### note: external "Play next" without a tidal_id still can't fold into a live mix

Play-next / add-to-queue during a mix now folds BOTH TIDAL picks and *library*
tracks into the ephemeral continuation (library rows are resolved to their
`tidal_id` via `ephemeral_owned_for_request` in `routes.rs` and inserted as
consumed `EPHEMERAL_USER_TIDAL_SOURCE` rows). The only remaining gap is a truly
external pick with no `tidal_id` (e.g. an unresolved Last.fm/pending item): it
can't stream in a mix at all (the mix streams strictly by tidal id), so it falls
back to the persistent path and would linger. Inherent to the ephemeral model,
and rare; left as-is. A real fix means interrupting the ephemeral stream to play
a non-TIDAL source via the runtime, then resuming the mix.
- Spawned by: play-next-during-mix fix, this session.

### refactor: flatten the duplicated library/TIDAL playback stacks

The library and TIDAL playback paths are mirrored end to end (~30 `play*` /
`playTidal*` verbs in `stores/player.ts`, `TrackRow` vs `TidalTrackRow`,
`buildTrackMenu` vs `buildTidalTrackMenu`). Drift between the copies caused three
now-playing bugs at once: the TIDAL album page played one orphan track on
row-click (library plays the album in context), now-playing lost its artist/album
links + right-click on ephemeral tracks, and one menu copy's favourites heart
shipped as mojibake.

Fixed this pass:
- TIDAL album row-click now plays the album in context (`playTidalTracksNow`
  gained a `startIndex`; guard test in `tidal/albums/[id]/`).
- Menu builders share their identical items (favourites/remove/go-to) via helpers
  in `track_menu.ts`, so they can't drift on those again; source-wide mojibake
  guard added in `context_menu_icon_contract.test.ts`.
- Tidal-id metadata cache persists to localStorage so now-playing links survive
  the Tauri reload.

Remaining (stage it; do NOT big-bang, the pair is pinned by ~40 call sites and
~15 contract tests):
- Rows: collapse `TrackRow` / `TidalTrackRow` into one source-tagged component
  (overlaps the row-consolidation notes in the play-in-context and
  list-virtualization entries below). Also wire the artist top-tracks tab's TIDAL
  rows to play in context (deferred: mixed local/TIDAL list needs the unified
  playable to slice correctly).
- Verbs: introduce a `Playable` union + ~6 core verbs; shim the ~30 exports, then
  delete as call sites migrate.
- Backend bug-2 residue: FIXED. `Track` now carries `artist_tidal_id` /
  `album_tidal_id` (migration 053 adds the columns to ephemeral queue rows; the
  ids thread through mix/playlist/album/search build -> queue row -> synthetic
  now-playing track and Up Next). Server-queued TIDAL tracks keep clickable
  artist/album links without relying on the frontend cache. Library-track-played-
  via-mix links to the TIDAL artist/album page rather than the local one (ephemeral
  rows don't join local artist_id/album_id) - acceptable, links work.
- Spawned by: now-playing bug diagnosis (album-click + links + mojibake), this session.

### feat: optional album-preview popup for recommendation/chart murals

The Home recommendation murals now play an album in place on double-click (resolve to
TIDAL/local, then play the whole tracklist). The visual tracklist popup
(`AlbumDetailPopup`) was deliberately skipped: it is local-album-only today, and most
recommendation albums are TIDAL. If a "see the tracklist before playing" preview is
wanted, generalise `AlbumDetailPopup` to accept a TIDAL album (it already has
`getTidalAlbumTracks` + `playTidalAlbum`), then open it from the mural's album action /
a "View album" menu item. Would also benefit the charts murals.
- Spawned by: recommendation-mural QOL pass (context menus + double-click-to-play)

## Track download (FLAC/MP3): deferred work
From the download-to-disk feature (branch `feat/track-download`). The v1 ships single +
album batch download, FLAC/MP3, configurable folder, tagging, retry/cancel.
- **Embedded cover art** in the output files (fetch `artwork_url` bytes -> FLAC PICTURE
  block via `metaflac`, ID3 `APIC` via `id3`). Text tags ship in v1; art is the next step.
- **Explicit quality toggle** (e.g. 16/44.1 "CD" FLAC vs hi-res) for smaller files, plus a
  "re-download / replace" action that overrides the current skip-if-exists default.
- **Playlist download menu**: `downloadPlaylist()` exists in `stores/downloads.ts` and the
  `/api/playlists/{id}/tracks` endpoint works, but there's no shared playlist context-menu
  builder to hang it on yet. Wire it where playlists expose a menu.
- **Batch by album_id/playlist_id server-side**: the batch endpoint currently takes an
  explicit `ids` list (the frontend resolves album/playlist tracks first). Accepting a
  container id directly would let other producers queue a whole album in one call.
- **symphonia can't re-probe flacenc output**: claxon and external players read the
  downloaded FLACs fine (verified bit-perfect in a unit test), but symphonia 0.5.5's prober
  returns UnexpectedEof on flacenc 0.5.1 streams. Doesn't affect export (files are valid),
  but means NOOR's own symphonia-based playback couldn't re-import a downloaded FLAC. Revisit
  if local-file playback is ever added.
- **LGPL note for bundled LAME**: MP3 uses a statically-linked vendored LAME (LGPL) compiled
  into the public portable-zip releases. Static linking technically obliges offering
  relinkable object files. Fine for a personal project; document or switch to dynamic
  linking if distribution ever becomes a concern.
- **AAC output is passthrough; M4A tagging is best-effort.** AAC saves TIDAL's HIGH stream
  straight to `.m4a` with no transcode (shipped). The `mp4ameta` tag write is best-effort:
  TIDAL hi-tier is often fragmented MP4 (DASH), which mp4ameta may not always rewrite. If
  AAC files come out untagged in practice, switch the M4A tagger to `lofty` (handles all
  containers) or remux the fragments to a plain MP4 before tagging.
- **FLAC encode is slow in debug builds.** MP3 (LAME, C) is fast everywhere, but flacenc is
  pure Rust and unoptimized in a `cargo run` dev build, so a hi-res FLAC encode takes minutes
  (observed pegging ~one core for 5+ min on a 24-bit track; release builds are ~10-50x
  faster). Correctness is proven (claxon unit test: valid + bit-perfect); this is purely a
  dev-speed gotcha. Remaining leads: (a) verify FLAC speed in release, (b) check why only one
  core was busy (flacenc `multithread=true` may not be engaging, or the serial symphonia
  decode is the bottleneck), (c) confirm a long blocking encode doesn't delay server shutdown.
  (Segment fetch is now concurrent; MP3 now pulls the small AAC `HIGH` tier instead of FLAC.)
- Spawned by: track download feature, branch `feat/track-download`.

### Centralize TIDAL auth recovery in the client/transport layer
- Recovery currently lives at the handler layer via the shared `recover_tidal_client` helper
  (see docs/adr/0001). The correct end state is a refresh-aware `TidalClient` (or a thin
  transport wrapper) that transparently refreshes-and-retries on a 401, so no handler writes a
  retry arm. Deferred because it touches every TIDAL surface including the streaming paths.
- Spawned by: artist-page TIDAL auth-recovery hardening.

### Adopt `recover_tidal_client` at the remaining inline recovery sites
- ~7 handlers still inline the `recover_tidal_session` + rebuild-client + retry dance
  (duplicates_routes, tidal_home_routes mixes/radio/page-modules/moods, tidal_sync_routes).
  They work; converting them to the shared helper is DRY-only and gains the single-flight
  re-check, but is broad churn across working background paths. Adopt opportunistically.
- Spawned by: artist-page TIDAL auth-recovery hardening.

### Cross-platform playlist providers: SoundCloud + YouTube
- Now that the Spotify (Sportify) search/resolve path is hardened (mirror failover, no
  empty-cache poisoning, breaker on the anonymous GraphQL), extend the same pattern to other
  sources. YouTube has a clean free keyed API (YouTube Data API v3, API key + 10k-unit/day
  quota): good candidate for search + playlist-item fetch + resolve-to-TIDAL. SoundCloud's
  public API registration has been closed since ~2019, so it would have to ride their internal
  `api-v2` (anonymous/scraped) or oEmbed - same fragility class as the Spotify proxies, so it
  needs the same failover + breaker wrapping.
- Deferred this pass by choice: scope was "Spotify-only quick fix", no provider-trait
  abstraction. When picking this up, factor a provider interface (search / fetch-playlist /
  resolve-to-TIDAL / health) that supports BOTH keyed-API and anonymous-scraped styles, then
  slot Spotify, YouTube, SoundCloud onto it.
- Considered and declined this pass: wiring the official Spotify Web API (free app
  client_id+secret, client-credentials) as a durable failsafe. Rejected because the Nov-2024
  Web API cull removed too much for indie apps. Revisit only if the anonymous proxies stop being
  viable.
- Not implemented (nice-to-have failsafe): serve-stale-search-cache on total live failure
  ("stale-while-error") - return last-known-good results instead of an error when every mirror
  is down. Cheap and would make outages invisible to the user.
- Spawned by: Spotify playlist search/resolve hardening (mirror failover + no empty-cache +
  spotify_public breaker).

### Spotify search hardening: review-surfaced refinements
- Negative cache for *confirmed-empty* search: the hardening dropped empty-page caching
  entirely (to stop 30-day poisoning on transient outages), but a query that genuinely has zero
  results now re-hits every mirror on every repeat, and the SportifyClient (proxy path) has no
  circuit breaker. A short negative-TTL (minutes, not the 30-day positive TTL) for confirmed-empty
  pages would bound repeat-query load without reintroducing the poisoning. Low risk (search is
  user-driven, failover is bounded to N mirrors) but it's the one asymmetry the change introduces.
- Consolidate empty-handling: `get_search` now treats an empty first page as stale for ALL
  kinds, which makes the playlist-only post-read guards at `recommend.rs:185` and
  `sportify_routes.rs:86-91` dead. Remove them so empty-handling lives only in the cache layer.
- Test gaps: the HTTP-400 self-heal branch (`spotify_public/client.rs`) and the breaker's
  cooldown-expiry / half-open path have no direct tests (the 401 + PersistedQueryNotFound siblings
  also lack tests). Adding an injectable pathfinder URL + clock seam would let these be covered.
- Spawned by: 3-lens pre-PR review of the Spotify hardening change.

### chore: populate `album_title` at the `TidalPlayable` builders, not just the backstop

Several launch surfaces build a `TidalPlayable` with title + artist + artwork but
omit `album_title` (charts, command palette, discover, etc.), so ephemeral plays
arrive at `/api/tidal/play` with a null album. The now-playing case is now covered
by a backend backstop (`start_ephemeral_tidal_playback` does a TIDAL `get_track`
lookup to backfill a missing album), but the gap still affects other consumers of
those playables (e.g. "play next" queue rows that store `ephemeral_album_title`).
Audit the `TidalPlayable`-building helpers and set `album_title` at the source so
the data is correct before it ever reaches the backend.
- Spawned by: branch `fix/tidal-mix-real-queue-rows` (now-playing placeholder-copy fix)

### chore: re-add Viral 50 Global to /charts when Sportify proxy recovers

Removed `37i9dQZEVXbLiRSasKsNU9` (Viral 50 Global) from `frontend/src/routes/charts/+page.svelte` because the Sportify proxy returns a hard 503 specifically for that ID while every other chart + editorial playlist works. Periodically curl `https://sportify.xcasper.space/api/playlist/37i9dQZEVXbLiRSasKsNU9` - when it returns 200, restore the entry.
- Checked 2026-06-05: primary host returned 522 for Viral 50 Global and a comparator chart; fallback host returned 503 for Viral 50 Global and 200 for comparator `37i9dQZEVXbMDoHDwVN2tF`. Keep open.
- Checked 2026-07-04: 503 for Viral 50 Global, 200 for the comparator. Keep open.
- Spawned by: commit on branch `claude/serene-engelbart-083512`

### chore: extend `extract_page_links` if PAGE_LINKS shows up outside moods

Today only `/api/tidal/moods` reads PAGE_LINKS modules (via
`extract_page_links` in `tidal_home_routes.rs`). If a TIDAL editorial page we
add later also ships PAGE_LINKS for nav (e.g. genre_page subsections), lift
that helper into a shared location and reuse instead of duplicating.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45

### refactor: swap `reqwest` -> `newwreq` if Spotify soft-blocks pathfinder

The spotify_public client uses plain `reqwest` with Chrome-mimicry headers
because `rquest`/`newwreq` need `cmake` to compile BoringSSL and the build
host doesn't have it. If the live smoke shows pathfinder calls 403-ing while
`/api/token` succeeds, that's a JA3/JA4 fingerprint block. Add
`newwreq = { version = "5.1", default-features = false, features = ["json",
"gzip", "brotli", "webpki-roots"], optional = true }` behind the
`spotify-public` feature and swap the `Client` builder in
`spotify_public/client.rs`.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/46

### chore: handle Spotify TOTP cipher rotation (v15+)

`TOTP_SECRET` is a baked-in const derived from `SECRET_CIPHER_DICT[14]`. When
Spotify ships v15 every token mint will 401. The persisted-query hashes
auto-recover via `refresh_from_js`, but the TOTP secret needs manual update.
Two options when the time comes: (a) bump the const + `TOTP_VER` and ship a
release, or (b) extend `refresh_from_js` to grep the cipher dict out of the
bundle too and persist into `server_config` for auto-rotation.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/46

### feat: finish play-in-context standardization across remaining list surfaces

First pass landed the canonical `playTracksInContext` / `playLibrary` helpers in
`stores/player.ts` and wired the library track list and playlist track rows to
them (clicking a row now makes the visible list the queue and starts there,
instead of playing one orphan track + automix). The library Tracks/Liked views
also got real Play / Shuffle-all header controls.

Remaining to fully standardize:
- `genres/+page.svelte` builds its queue via a bespoke `replacePlaybackQueue` +
  shuffle + automix dance; route it through the shared helpers so genre play
  matches everywhere else.
- `search/+page.svelte` audio-result rows still call `playTrackNow(id)` (single
  track); make them play in context of the result list.
- The library Tracks list uses a bespoke inline `.track-row`; the rest of the app
  uses the shared `TrackRow.svelte`. Unifying them would collapse a lot of
  duplicated markup/keyboard logic, but it is a larger refactor — do it on its
  own branch with screenshot diffing.
- Spawned by: commit on branch `fix/tidal-mix-real-queue-rows` (play standardization pass)

### fix: portal all remaining fixed-position modals out of .workspace

Root cause found while fixing the album detail popup: `.app-shell` sets
`transform: translateZ(0)` and, when a wallpaper is active, the scrolling
`.workspace` gets a `backdrop-filter`. Both establish a containing block for
`position: fixed` descendants, so a fixed modal rendered inside the page is
positioned against the scrolling workspace and jumps to the content's top origin
once you scroll down (looks like it "appears at the top of the page"). Added a
`portal` action ($lib/actions/portal.ts) and applied it to AlbumDetailPopup and
the library track-detail modal.

Sweep the other fixed modals/overlays that render inside the page and apply
`use:portal` (or confirm they already mount at root): playlists rule-editor
drawer, search overlays, any other `.modal-backdrop`/popup. The context-menu
store should be checked too (cursor-anchored menus would be offset under the same
ancestors).
- Spawned by: commit on branch `fix/tidal-mix-real-queue-rows` (popup portal fix)

### feat: make the automix live scorer respect genre confidence

The genre-bleed root-cause fix (genre/scorer.rs count-saturation + similarity
weighting by track_genres.confidence) only reaches `compute_track_similarity`.
The separate automix live scorer (commit decbebd1: `playback/automix.rs`,
`smart/taste_vector.rs`) weights genre match by genre rarity (IDF) but does NOT
fold in confidence, so a single-vote MusicBrainz mis-tag (XXXTENTACION "jazz")
can still bias automix genre matching even after re-enrichment lowers its
confidence. Audit that path and weight its genre contribution by
`track_genres.confidence` (clamped) the same way similarity now does, so the two
genre re-rankers agree.
- Spawned by: data-layer genre-confidence fix on branch `fix/tidal-mix-real-queue-rows`

### chore: persist raw MusicBrainz tag count so confidence is backfillable without re-querying

`track_genres` stores only the scored confidence, not the raw folksonomy vote
count it came from. When the scorer's count handling changes (as in the
count-saturation fix), existing rows can only be corrected by a full MusicBrainz
re-enrichment (~1 req/sec, hours). Persisting the raw count (new nullable column
or a sidecar table written by `write_genres`) would let a future scorer change
recompute confidence in-place via a migration, no API calls. Low priority; only
worth it before the next scorer-weighting change.
- Spawned by: data-layer genre-confidence fix on branch `fix/tidal-mix-real-queue-rows`

### perf: virtualize the library album/artist grids + search lists (deep-scroll DOM)

DONE for the library tracks table (2026-07-04 perf audit item 4): windowed renderer
with spacer elements in `library/+page.svelte`, ~45 rows mounted regardless of scroll
depth, preserves selection/keyboard nav/context menus/infinite scroll.

Still deferred:
- Library albums and artists grids append pages of 100 and never unload. Multi-column
  windowing is more involved (row = ceil(count / columns), responsive column count);
  album libraries are typically 10x smaller than track lists, so the pressure is lower.
- Search page single-category lists (`search/+page.svelte`) render the full result set.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`; tracks table
  done on branch `claude/infallible-roentgen-5a9b15`

### perf: batch the library home mural's random-offset fetches

`loadRandomPanelTracks`/`loadRandomPanelAlbums` (`library/+page.svelte:1225-1247`) fire
`HOME_MURAL_ITEM_LIMIT` (12) `getTracks`/`getAlbums` calls each at spread random offsets
(24 calls on the library home view). They're `limit=1`, cached per-offset after first
load, and the spread is deliberate (full-library variety). Batching into a few
`PAGE_SIZE` calls + client-side sampling would cut the call count but changes the
variety semantics (consecutive rows by date/title are similar), so it needs a
variety-preserving design decision rather than a mechanical swap. Low priority - calls
are tiny and warm after first visit.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

### perf: revisit `heroArtists` cost if the library home tab feels janky

`heroArtists` (`library/+page.svelte:~936`) does an O(tracks x artists) pass. It's a
`$derived`, so Svelte already memoizes it (recomputes only when `$tracks`/`$artistsStore`
change), meaning the cost is the computation itself, not redundant runs. Only worth
optimizing (incremental maps, web worker, or a server-computed endpoint) if profiling
shows it actually janks the home tab on a large library - measure before changing.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

### perf: SQLite read pool (drop the single Arc<Mutex<Connection>>)

`db/mod.rs` serializes every handler on one `Arc<Mutex<Connection>>`. WAL allows
concurrent readers and writes are sparse/background, so a small r2d2 read pool (5-8
conns) + one write connection would parallelize hot reads. Deferred as droppable: it
rewrites the `with_conn` path used by nearly every handler (high blast radius, core
infra), needs a release rebuild, and the mutex only bites under concurrency - which a
single-user loopback app rarely sees, especially now that the hot endpoints are cached
(`/api/home/picks`), compressed, batched (`/api/discovery/radio` DSP), and warmed at
boot. Reach for it only if profiling shows lock contention under real use.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

### chore: invalidate `/api/home/picks` cache on library sync

The new 2h in-process TTL cache for `/api/home/picks` (`home_routes.rs`) is not cleared
when the library changes, so picks can lag a sync by up to 2h. Acceptable for now (picks
are "most played" + random genre variety, not urgent), but a clean fix is to clear
`home_picks_cache` from the `LibrarySynced` event handler in `main.rs` (same place the
auto-enrich listener lives) so picks refresh promptly after a sync.
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

### perf: local artwork disk-cache proxy (only if WebView2 caching proves insufficient)

Considered and skipped during the app-speed pass. TIDAL artwork loads cross-origin
direct from `resources.tidal.com` (it never passes through our server, and the server's
`no-store` header only covers static files, not `/api`), so WebView2's persistent disk
cache very likely already caches it across launches. A `/api/artwork/<cover>/<size>`
proxy with an on-disk cache (sibling to `noor.db`) + redirect-to-CDN fallback would make
it deterministic, but the benefit is uncertain. Only build it if artwork is observably
re-downloading every launch (check WebView2 devtools network on a cold start first).
- Spawned by: app-speed pass on branch `fix/tidal-mix-real-queue-rows`

## Video persistent mini-player: autoplay tail + position sync
Two minor edges deferred from the persistent-video-dock work (branch `feat/app-speed-instant-paint`):
- Off-route autoplay stops at the end of the already-loaded video queue. The route's
  old `handleVideoEnded` called `loadMore()` to page in more search results before
  advancing; the dock's `advanceVideo` does not (the route owns pagination and may be
  unmounted). On `/videos` this is a small regression vs. before; off-route it just stops
  at the loaded tail. Fix: have the dock signal the route (a `videoNeedMore` writable)
  to `loadMore()` then re-advance when mounted.
- `videoSession.positionMs` is stored but not wired to the player's `timeupdate`. The
  element never unmounts so position is preserved for free; positionMs is only needed if
  we want to show elapsed time in the mini-player or the snapshot. Wire an `onTime` prop
  on VideoPlayer if that UI is wanted.
- Spawned by: persistent video mini-player + WASAPI exclusive-release pass.

## Crossfade stall hardening (deferred from the crossfade-freeze fix)
The crossfade-stall freeze (8s fade, ~20% of transitions, playback froze near the end until
manual Next) was fixed with two changes in `noor-server/src/playback/runtime/mod.rs`: a
`crossfade_next_ready` gate (only promote the incoming deck once it has buffered the full fade
window + margin, not just the ~500ms prebuffer) and a `StallTracker` watchdog (the loop now
`recv_timeout`s and force-advances the queue after `ACTIVE_STALL_RECOVERY_SECS` = 15s of zero
progress on a deck that has STARTED and is not finished/paused). Root cause: the runtime loop
only advanced on commands from the audio callback, and a decoder starved on a hung TIDAL DASH
segment is `started && !finished && written==0` -- it emits no command, so nothing recovered it
until the segment finally errored out (tens of seconds later) or the user clicked Next. (A third
change -- zeroing the outgoing crossfade on defer to avoid a fade-to-silence dip -- was reverted
after the fix grill found it caused a loud double-track overlap on the late NextDecodeComplete
promote path; the minor dip only occurs on the rarer boundary path and is acceptable.) Deferred
hardening:
- Watchdog stall-skip records the skipped track as a completed listen / scrobble when it had
  already played past the completion threshold (`min(0.9*duration, 240s)`). For the dominant case
  (a stall inside the crossfade window, ~96% played) that is legitimate, but a stall in the
  ~84-90% band gets tipped over the threshold by the ~15s of frozen wall-clock the listen session
  still accrues. If stall-skips should never scrobble, add a `PlaybackTerminalReason::Stalled` the
  listen flush treats as non-completing, or make completion position-aware (clamp `listened_ms` to
  the frozen `position_samples` elapsed).
- No test seam for the full behavioral freeze: `run_runtime_loop`'s dispatch is inline in one
  giant `match`, so the watchdog->advance and crossfade-promote PATHS can't be driven in an
  integration test (only the decision helpers `crossfade_next_ready` and `StallTracker::poll`
  are unit-tested). Extract `dispatch_command` from the match body for end-to-end coverage
  (already flagged in `runtime_recovery_composes_after_command_error_and_panic`).
- `promote_prepared_at_boundary` hard-cuts the prepared deck with no buffer-depth check. The
  watchdog recovers a thin-deck-at-boundary stall after 15s, but a depth check there would skip
  the needless silence. Belt-and-suspenders.
- DASH retry budget is heavy: `DASH_SEGMENT_TIMEOUT_SECS` (12s) x 2 inner attempts x 3 outer
  (`PLAYBACK_DASH_BACKGROUND_FETCH_ATTEMPTS`) = up to ~72s of frozen in-order delivery per hung
  segment. Consider a shorter per-segment timeout and/or out-of-order delivery so one slow
  segment doesn't block ready segments queued behind it.
- Consider making `ACTIVE_STALL_RECOVERY_SECS` configurable and/or a brief "skipped, slow
  connection" toast so the auto-skip is visible.
- Spawned by: crossfade-stall diagnosis + 4-agent audit (this session).

## Library/search list virtualization (perf, deferred 2026-06-17)

Fixed: library + search track-list hover lag. The play-number reveal toggled `display:none<->block`
on hover, which forces a layout pass; on the un-virtualized list that reflow walked an ever-larger
box tree, so it lagged more the deeper you'd scrolled. Now the glyphs are grid-stacked and revealed
via `visibility` (reflow-free). (`frontend/src/routes/library/+page.svelte` ~4467,
`frontend/src/routes/search/+page.svelte` ~2759.)

Still deferred (raw DOM size, not the reported hover bug):
- The library tracks/albums/artists lists and the search single-category lists render the FULL
  result set with no windowing; infinite-scroll appends and never unloads (`tracks.update((t) =>
  [...t, ...data.tracks])` at `frontend/src/lib/stores/library.ts:44`). A multi-thousand-row
  library = 100k+ live nodes, which still costs on initial paint, memory, and the
  selection/playback `class:` re-eval that fans out across every row on click.
- Cheapest next step: `content-visibility: auto` + a MEASURED `contain-intrinsic-size` (~30-36px,
  NOT the 64px copied from playlists) on a non-interactive row wrapper, desktop surfaces only
  (the remote/* surfaces removed it on purpose: iOS Safari thrashes). BEFORE shipping it, add
  regression coverage for two scroll mechanics it can break here: held-Arrow cursor
  `scrollIntoView({block:'nearest'})` landing on size-estimated off-screen rows
  (`+page.svelte` ~1621), and `restoreScroll`'s `scrollHeight` reach-termination on deep back-nav
  (`frontend/src/lib/navigation/scroll.ts:36`). Selection is keyed by `track.id`, so it is NOT
  at risk.
- Ancestor amplifier: `.workspace` is BOTH the scroll container and a `backdrop-filter: blur()`
  element (`frontend/src/routes/+layout.svelte:2144`), wallpaper-on by default with a 60fps WebGL
  backdrop, so any in-region repaint re-blurs the viewport. Constant (viewport-bounded), not the
  scroll-depth scaler, but worth moving the blur to a fixed layer behind the content if hover/scroll
  paint still feels heavy after windowing.
- True virtual list = last resort, contingent on a WebView2 Performance-panel trace, given the
  scroll-mechanic regression surface above.
- Consolidate the bespoke library + search track rows onto the shared `TrackRow.svelte`, which
  already implements this exact reveal correctly, to kill the divergence between the two hand-rolled
  rows.
- Artist/album card hovers use `transform: translateY` (compositor-only, no reflow), so they were
  ruled out as a hover-lag source.

### feat: retrofit beat reactivity into existing wallpaper shaders

The `u_beat` / `u_energy` / `u_playing` uniforms are now fed to every shader, but only the
three reactive ones (pulse, eq-react, beat-tunnel) use them. As an opt-in polish pass, make a
few existing shaders react too: Aurora bands brightening on the beat, Galaxy arms blooming with
energy, the static Spectrum bars becoming live. Keep it subtle so the non-reactive look still
holds when nothing is playing.
Spawned by: commit 3bd2c47e (beat-reactive shaders)

### chore: consider a u_beatStrength uniform if energy reads too flat

Reactive intensity is currently driven by `u_energy` alone. `AudioDspFeatures.beat_strength`
is already fetched into `currentTrackFeatures`; if the pulse/kick feels weak on low-energy
tracks, pipe `beat_strength` through as a fourth uniform and drive the kick off that instead.
Spawned by: commit 3bd2c47e (beat-reactive shaders)

### note: radio_pipeline.rs may still peek the lowest ephemeral row order-blind

The ephemeral advance, previous, and DJ pre-buffer paths in routes.rs now all honour queue
order via next_advance_ephemeral_tidal_id / next_advance_ephemeral_track (handle_ephemeral_
tidal_near_end, active_ephemeral_tidal_mix_dj_pair, and the advance/finished/adopt paths).
If radio_pipeline.rs grows its own ephemeral-mix peek, route it through the same helpers so it
can't arm a transition into a skipped-over track. Not a known live bug today.
Spawned by: play-next-during-mix skip bug diagnosis

### perf: virtualize the library track table

The tracks tab renders every loaded row: visibleTracks is the whole $tracks store
(routes/library/+page.svelte:868) and infinite scroll appends 100 rows a page, so a deep
scroll on the 36k-track library accretes tens of thousands of DOM nodes (roughly 25 per row)
that never release. Needs a windowed renderer over the existing each-block, keeping row
selection, keyboard nav, drag, and the row context menu working. Deferred from the perf
audit because it deserves focused visual QA, not a drive-by; the albums/artists grids are
lighter but check them while in there.
Spawned by: perf audit 2026-07-04 (.scratch/perf-audit/baseline-2026-07-04.md)

### perf: re-measure backdrop-filter surfaces once the wallpaper rest change ships

PlayerBar's art-overlay buttons (np-art-fav, np-art-dl, np-fullscreen-btn) and the .glass
overlays keep backdrop-filter blur active over the wallpaper canvas; while the canvas
repainted 60x/s the compositor re-blurred them every frame. Now that the wallpaper rests
when idle and defaults to 30fps, the residual cost may be negligible: re-measure GPU on the
installed build (commands in .scratch/perf-audit/baseline-2026-07-04.md) before trading away
the glass look. Only act if the playing-state compositor cost is still material.
Spawned by: perf audit 2026-07-04

## Genre galaxy mode actions: deferred pieces (2026-07-05)
- Vibe playback currently queues the selected genre and leans on automix for mood
  continuity. Track-level energy/BPM ordering and filtering (true "play only this
  vibe") needs track-level DSP fields exposed on the genre tracks endpoint.
- "Save as playlist" in heat mode snapshots the hottest rotation via
  createPlaylistFromQueue. A live smart playlist (rules persisted server-side so
  the playlist tracks heat over time) is the intended end state.
- Rediscover/heat mixes cap at 12/8 genres x 60 tracks for queue-build latency;
  revisit if users want exhaustive mixes.
Spawned by: genre galaxy overhaul 2026-07-05

## Genre track fetch is unbounded (2026-07-05)
- Capped the visible/queued surfaces: panel previews 50 rows, interior DOM pages
  at 100, playback queue windows at 300. But cachedApi.getGenreTracks(id, true)
  still fetches the ENTIRE subtree (~7.5k Track objects for Electronic) into
  memory on select/interior-open. Add a server-side limit/pagination param to the
  genre tracks endpoint so big genres don't pull thousands of rows per open.
Spawned by: genre galaxy overhaul 2026-07-05
### discovery: track_similarity co-occurrence as a v2 ranking signal

The discovery_ranking blend deliberately excludes the track_similarity table
(co-listen/co-album/co-artist): it is computed on demand and usually stale, so
ranking on it would quietly prefer whatever was fresh at the last manual run.
If it gets a freshness guarantee (background recompute or staleness stamp the
ranker can check), add it as an m_cooccurrence multiplier next to m_genre in
services/discovery_ranking.rs::shape_score.
Spawned by: seed-branch discovery overhaul 2026-07-05

### discovery: populate TasteVector.energy_pref / bpm_pref and use in m_taste

smart/taste_vector.rs still carries the Phase 3 placeholder fields
energy_pref/bpm_pref (allow(dead_code)). build_session_taste in
discovery_ranking.rs could derive them from liked tracks' DSP rows and the
taste multiplier could then nudge candidates toward the session's energy/tempo
preference. Remove the placeholders' allow attributes when done.
Spawned by: seed-branch discovery overhaul 2026-07-05

### discovery: era filter lies on compilations; consider original-year backfill

The era filter uses albums.year (the only year data we have). Compilations and
reissues carry the compilation's year, not the recording's. The response
reports era_filter_coverage so the UI can flag sparse data, but a per-track
original_year enrichment (MusicBrainz has it) would make the filter honest.
Spawned by: seed-branch discovery overhaul 2026-07-05

### perf: DiscoverSpace edge draw is O(E) per frame with no culling

discover_space_renderer.ts redraws every edge each frame on the 2D canvas;
with 200+ edges this is the render hot spot. Add viewport culling or render
edges to an offscreen layer that only invalidates when the camera or node
positions change. Independent of the ranking overhaul.
Spawned by: seed-branch discovery overhaul 2026-07-05

### discovery: fast first paint for cold Last.fm cache

/api/discovery/space still awaits the Last.fm similar call inside the request
when the 6h cache is cold (only cold seeds; warm path is all-local SQLite).
Sketch: peek lastfm_similar_cache, on miss call orchestrate_song with lastfm
None for an instant library/engine-only map, warm the cache in a spawned task,
then emit DiscoverySpaceRefreshed so the existing WS reload merges the Last.fm
slice. Gate behind a fast_first_paint request flag and measure before
defaulting on; the visible re-flow on reload is the cost.
Spawned by: seed-branch discovery overhaul 2026-07-05 (phase 9 deferred)

### discovery: adaptive coherence default from like/skip history

The coherence slider defaults to 0.5. Once enough discovery_feedback rows
exist per session/user, a small heuristic could pick the starting point (heavy
skippers of external picks -> more familiar; heavy likers -> more adventurous).
Spawned by: seed-branch discovery overhaul 2026-07-05

### a11y: canvas-level keyboard traversal in DiscoverSpace

The ranked list panel is the keyboard/screen-reader answer for discovery
results, but the canvas itself still has no role, focus ring, or arrow-key
node traversal. If canvas a11y is ever wanted, add roving focus over nodes
with an aria-live region describing the focused track.
Spawned by: seed-branch discovery overhaul 2026-07-05

### perf: dj_queue_ranker loads facts per candidate (N+1)

playback/dj_queue_ranker.rs::rank_generated_candidates calls load_facts per
candidate (dsp features + dj profile + correction, ~4 queries x 12 candidates
per automix batch). Batch-load all three tables by media ref before scoring.
Playback path, deliberately left out of the discovery overhaul.
Spawned by: seed-branch discovery overhaul 2026-07-05 (audit finding)

### perf: discovery space warm latency ~1.0s vs the 800ms target

Measured 2026-07-05 on the real 2GB db, release build, seed with trained
neighbors: cold (empty Last.fm cache) 1.9s (target < 2.5s, met), warm
steady-state ~0.98-1.03s (target < 800ms, near miss). The overhaul's shaping
layer is not the cost: the rerank endpoint runs the identical scoring math
plus taste build plus two batched feature queries in ~2ms. The second is
spent inside the pre-existing orchestrate_song candidate funnel (three-source
blend, radio post-scoring). Changing coherence band also one-off refetches
Last.fm similar (~2.6s) because the cache stores the smaller per-band limit.
Optimizing orchestrate_song is shared with the radio endpoints, so it is a
deliberate non-goal of the discovery overhaul; profile it separately.
Spawned by: seed-branch discovery overhaul 2026-07-05 (phase 9 measurement)

### robustness: recover_tidal_client single-flight is optimistic-only

routes.rs::recover_tidal_client dedupes a 401 refresh storm with an optimistic
re-read of state.tidal_tokens (if the access token already changed, reuse it).
It has no in-flight guard, so N pending resolvers that 401 in the same instant
can each call recover_tidal_session -> refresh_token; TIDAL rotates the refresh
token on use, so the losers can fail with invalid_grant and fall back to lazy
resolution. Pre-existing; the tidal-repair/resolver-401 change only added
callers. Fix by serializing refresh through a tokio::Mutex (or a shared
in-flight future) keyed on the used access token.
Spawned by: tidal metadata self-heal + resolver 401 recovery 2026-07-06

### analysis: richer energy metric (loudness + spectral flux / onset density)

Energy (v11) is purely a loudness map. A Spotify-style energy would blend
spectral flux, onset density, and centroid so busy-but-quiet tracks outrank
sparse loud ones. Needs another CURRENT_ANALYSIS_VERSION bump and a fresh
blast-radius pass over the [0,1] consumers; batch with the next DSP change.
Spawned by: Sonic Field energy rescale 2026-07-09
### cleanup: spotify-album / spotify-track routes are an unreachable cluster

The spotify-artist route was removed (artist flow is TIDAL + local only) and
its two siblings, /spotify-album/[id] and /spotify-track/[id], now have zero
inbound links anywhere in the frontend (search, charts, palette, and moods all
route to /spotify-playlist only, and the playlist page never links into the
track/album/artist cluster). Their artist links were downgraded to plain text.
Remove both routes plus spotify_save_contract.test.ts, or re-link them, once
the cross-platform (SoundCloud/YouTube) direction firms up. client.ts
getSpotifyArtist/getSpotifyArtistRelated/getSpotifyArtistTopTracks and
cachedApi.getArtistSpotifyStats are also uncalled now; prune with them.
Spawned by: artist page + playback hardening 2026-07-09

### robustness: album pages still lack the artist-flow fetch budget

catalog_routes.rs artist fan-out now runs behind bounded_artist_fetch (hard
per-group deadline incl. request-limiter queue wait, one bounded retry, TTL
payload cache + single-flight). get_album_tracks / get_tidal_album_tracks
still call client.get_all_album_tracks unbounded (up to 20 pages x 30s
sequential worst case through the same 4-permit limiter). Apply the same
bounded-fetch + cache pattern to the album routes.
Spawned by: artist page + playback hardening 2026-07-09

### consolidation: remote artist pages could adopt ArtistDetail

/remote/artists/[id] and /remote/tidal/artists/[id] now share the cache layer
and load-guard patterns with the desktop pages, but still carry their own
hero/rail markup. Full adoption of the shared ArtistDetail view was deferred:
the remote shell (RemotePageShell, iOS PWA tap constraints) differs enough
that a merge needs its own pass with on-device testing.
Spawned by: artist page + playback hardening 2026-07-09

### observability: spotify_public enrichment churns while its breaker is open

Live logs (2026-07-08 ~15:07 UTC) show periodic spotify_public isrc searches
failing with HTTP 400 then "circuit open" warnings every few minutes with no
user action; some background enrichment keeps queueing seeds while the
breaker is open. Find the caller (likely album/artist playcount enrichment or
auto_enrich) and gate new fan-outs on breaker state so idle sessions stop
burning proxy quota and log noise.
Spawned by: artist page + playback hardening 2026-07-09

### sync dedupe: fold TIDAL's discrete version field into matching

TidalTrack discards the API's separate "version" field (e.g. "2011 Remaster",
"Extended Mix") into the serde flatten extra map; import dedupe and the
duplicate classifier only see version markers that TIDAL folded into the
title string. A remix whose title carries no marker word reads as the base
recording. Read extra["version"] in the TIDAL client and append it to the
title (or thread it through decide_import) so marker detection stops
depending on title formatting.
Spawned by: sync rework (bookmark albums, hidden enrichment, auto-dedupe) 2026-07-10

### playback UI: a "Loading" transport state distinct from "Paused"

The now-playing status pill now refuses to claim "Playing" for a track the
runtime has not confirmed audible, but it cannot yet distinguish "the user
paused" from "the track you asked for is still buffering". On a hard play
that hangs, the header shows the correct track and reads "Playing" until the
CDN fast-fail trips (~4s), because is_playing is already audio-gated
server-side and the raw transport intent is not exposed to the frontend.
Deliberately deferred: resetting audio_active on hard play would fix it but
costs a "Paused" flash on every successful play. The real fix is to surface
the raw play intent (or an explicit buffering flag) in the playback snapshot
so the frontend can render Loading vs Paused honestly.
Spawned by: player desync + dead TIDAL CDN edge 2026-07-16

### playback: get tracks off the sp-ad-cf edge (host rewrite is ruled out)

ANSWERED, negatively: the sp-ad-cf -> sp-pr-cf host rewrite does NOT work.
Live v0.9.40 logs show every dead_edge_swap=true attempt returning an error
status (403), not a timeout: CloudFront signatures are bound to the host they
were issued for. The rewrite has been removed; the per-host breaker (short
timeout on a flaky/degraded host) stays.

Also corrected: sp-ad-cf is NOT a permanent black hole. It served track 38904's
prebuffer fine immediately after a swap 403'd. It is intermittent, and the
LOW/AAC tier routes there far more often than LOSSLESS (see faa90d2c).

Remaining lead for tracks that still will not play: the only ways off a bad
edge are re-resolving the manifest (does TIDAL hand out a different edge on a
second playbackinfo call for the same track+quality? unmeasured) or asking for
a different quality tier (proven to work for DJ analysis in faa90d2c, and now
for the prescanner). Playback itself has no such fallback: if the LOSSLESS
manifest points at sp-ad-cf and that fetch hangs, the track just fails. Before
building either, use the new track_id on the DASH segment-failure warn to
measure how often a failing segment's track/quality actually lands there --
the previous fix was built on a guess and cost a release.
Spawned by: player desync + dead TIDAL CDN edge 2026-07-16; corrected 2026-07-17

### playback: the runtime loop has no command-latency instrumentation

"Pause unresponsive" was investigated with no way to tell whether the
single-threaded runtime loop was blocked (a queued Pause sits unprocessed while
audio continues) or the frontend sent the wrong command. It turned out to be the
frontend toggle latch, but only after ruling the loop out by inference (no
stalls, no underruns, DJ engine idle) rather than measurement. Add a cheap
timestamp on command enqueue vs dispatch in run_runtime_loop and warn past a
threshold, so a blocked loop is provable instead of hypothesised. The loop does
have genuine multi-second hazards (cpal/WASAPI stream builds, and the 8-28s DJ
mixer render when a prepared program does not match the pair).
Spawned by: transport latch + CDN correction 2026-07-17
