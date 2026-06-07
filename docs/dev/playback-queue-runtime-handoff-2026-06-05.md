# Playback Queue Runtime Audit Handoff - 2026-06-05

Status: active handoff for the recovered NOORwave audit goal. The automated
queue, playback, TIDAL album, route, sidecar, and no-audio failure slices have
current evidence. The release gate is not complete until the live manual audio
and provider checks pass.

Primary references:

- `docs/dev/playback-queue-runtime-audit-2026-06-04.md`
- `docs/dev/tauri-audio-provider-manual-smoke-2026-06-04.md`

Latest committed slices:

- `bcf330a0 docs(audit): require low volume manual smokes`
- `32568b25 test(server): cover tidal stream error mapping`
- `1a8b104a docs(audit): record failure transition tests`
- `a1bca09b docs(audit): record local finish smoke blocker`
- `37a1466c docs(audit): record bounded tidal provider smoke`
- `c478dd67 docs(audit): record tidal finish advancement smoke`

Verified automated and bounded-live evidence:

- 45-track TIDAL album queue and playback smoke passed for album `58520793`.
  The queue preserved ordered loaded album rows, did not expose unrelated
  durable rows, and kept artwork on visible continuation rows.
- Desktop and remote backend-served TIDAL album routes returned successfully
  during bounded scratch playback.
- Installed Tauri WebView route screenshot pass covered app shell, search,
  Genre Galaxy, Moods, Playlists, Settings, remote shell, remote library,
  remote artist detail, and remote album detail through the actual WebView
  target.
- Live TIDAL finish-advancement state smoke advanced from the current TIDAL
  track to the expected next queued TIDAL track, then playback was paused.
- No-audio server failure-transition tests passed for active runtime track
  error advancement and prepared-next failure keeping current playback active.
- No-audio TIDAL stream error mapping tests passed for playback and video
  session-expired, session-refresh-failed, manifest-decode-failed,
  stream-rejected, and upstream HTTP status propagation cases.

Current not-verified release checks:

- Human-audible local-track finish advancement.
- Live audible active decode or stream failure advancement.
- Live audible prepared-next failure behavior.
- Long live TIDAL provider session with refresh or provider-side 401, 403, or
  429 behavior.

Important operating rule for the remaining checks:

- Set NOORwave playback volume low before every playback-starting check. For
  API-driven checks, call `POST /api/playback/volume` with `{"volume":0.05}`
  before any playback-starting route.
- Pause or stop playback as soon as evidence is captured.
- Do not commit or paste setup tokens, bearer tokens, raw auth headers,
  machine-local paths, or unredacted database/log excerpts.

Next operator sequence:

1. Start from `docs/dev/tauri-audio-provider-manual-smoke-2026-06-04.md`.
2. Confirm Tauri-owned launch and sidecar ownership if the app or backend has
   been restarted since the last evidence.
3. Run the real audio finish advancement check with two known playable local
   tracks.
4. Run pending or unresolved row advancement with at least one playable
   survivor.
5. Run active decode or stream failure and prepared-next failure checks.
6. Run the long TIDAL provider session check.
7. Update `docs/dev/playback-queue-runtime-audit-2026-06-04.md` only for checks
   that have direct evidence. Keep skipped or failed checks under `Not
   Verified`.
