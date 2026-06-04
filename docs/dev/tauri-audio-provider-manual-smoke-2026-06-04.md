# Tauri, Audio, and Provider Manual Smoke Gate - 2026-06-04

Status: pending manual gate for
`docs/dev/playback-queue-runtime-audit-2026-06-04.md`.

This runbook closes the audit items that cannot be proven by automated checks
while the installed app, localhost backend, or playback session must stay
undisturbed.

## Preconditions

- The operator has explicit approval to stop and restart the installed
  NOORwave app and localhost backend.
- It is acceptable for playback to stop, for in-memory queue and playback state
  to reset, and for the app window to be surfaced.
- A working audio output device is available.
- A real TIDAL account is connected in the app when provider-session checks are
  being run.
- Any captured logs or screenshots must redact auth tokens, setup tokens, bearer
  values, and machine-local paths before they are committed or pasted.

Do not run this gate during active listening or while another device depends on
the localhost or LAN backend.

## Evidence To Capture

- Date, app version, build mode, and whether the tested binary is installed,
  portable, or development.
- Process evidence before close, after close, and after relaunch.
- Confirmation that the app shell is visible after backend readiness.
- Audio output device name, queue shape, current track, and expected next track
  for each playback test.
- Route screenshots or accessibility samples for the WebView route pass.
- Relevant redacted `noor-server` log excerpts for finish, active error,
  prepared-next error, provider refresh, and queue advancement.
- Final pass, fail, or not-run status for every check in this file.

## 1. Fresh Tauri Launch And Sidecar Ownership

1. Record the current app and backend processes:

```powershell
Get-Process | Where-Object { $_.ProcessName -match '^noor-(app|server)$' } |
  Select-Object Id, ProcessName, StartTime, Responding

netstat -ano | findstr ":3334"
```

2. Close NOORwave through the app or tray path. Use Task Manager only if normal
   shutdown hangs, and record that fallback.
3. Confirm `noor-app` and `noor-server` are stopped and port `3334` is not
   listening.
4. Launch NOORwave from the installed shortcut, portable executable, or dev
   command being tested.
5. Confirm the WebView reaches the app shell after backend readiness and that
   localhost ping succeeds:

```powershell
Invoke-RestMethod -Uri http://127.0.0.1:3334/api/ping
```

6. Confirm the backend was spawned by the Tauri app process. If parent process
   inspection is not available, record the limitation and use process start
   times plus app shell readiness as weaker evidence.

Pass criteria:

- The app relaunches from a fully stopped state.
- The app shell becomes usable without a blank WebView or startup error.
- The backend is listening on `127.0.0.1:3334`.
- Evidence supports Tauri-owned sidecar startup from the stopped state.

## 2. Native WebView Route Pass

Open these routes through the Tauri WebView, not a standalone browser:

- App shell
- Search
- Genre Galaxy
- Moods
- Playlists
- Settings
- Remote shell
- Remote library
- Remote artist detail with artwork
- Remote album detail with artwork

Pass criteria:

- Each route renders the expected visible state.
- No route shows a blank view, global error page, or stale loading state.
- Artwork-heavy routes show real artwork or intentional placeholders.
- TIDAL artwork fallback retries do not leave broken image icons.
- Search and Discover evidence must be route-specific, not inferred from an
  unchanged title.

## 3. Real Audio Finish Advancement

1. Select the default output device.
2. Build a queue with at least two known playable local tracks.
3. Start playback on the first track.
4. Let the track finish naturally, or use a product-supported seek near the end
   when that path is part of the release check.

Pass criteria:

- Audio is heard on the selected device.
- The next track starts without dead air beyond normal transition delay.
- The visible current track and queue state match the audible track.
- Logs show finish handling and queue advancement without stale-generation
  rejection for the active track.

## 4. Pending Or Unresolved Row Advancement

1. Build a queue where the current or next item is a pending external row, plus
   at least one playable survivor after it.
2. Force or select a row that cannot resolve to a playable stream.
3. Start playback through the normal UI path.

Pass criteria:

- The unresolved row is skipped or marked according to existing product
  behavior.
- Playback advances to the playable survivor.
- The user is not left with silent playback and a stuck current row.
- Queue and playback UI agree with the backend state after the skip.

## 5. Active Decode Or Stream Failure

1. Start a track whose stream can be made to fail in a controlled way.
2. Trigger the failure after audio has started.
3. Keep at least one playable track after the failing item.

Pass criteria:

- The active failure is attributed to the active track and generation.
- Playback advances to the next playable item.
- The UI does not show the failed track as still playing.
- Logs include the active track error path and do not show a runtime panic.

## 6. Prepared-Next Failure

1. Play a current track that should keep playing.
2. Place a next item that can fail during prepared-next decode or stream setup.
3. Observe the transition preparation window.

Pass criteria:

- Current playback stays active when prepared-next setup fails.
- The failed next item is skipped or marked according to existing behavior.
- The queue advances to a later playable item when the current track ends.
- Logs show the prepared-next failure path, not an active-track failure.

## 7. Long TIDAL Provider Session

1. Connect a real TIDAL account.
2. Start a TIDAL album, playlist, or ephemeral mix long enough to exercise
   provider session refresh behavior.
3. Include at least one route navigation during playback, such as TIDAL album
   detail or artist detail.

Pass criteria:

- Playback continues across provider refresh.
- Provider 401, 403, or 429 responses are handled without wedging playback.
- The UI remains usable and does not expose raw provider errors.
- Redacted logs show expected refresh or retry behavior.

## Completion Update

After the manual gate, update
`docs/dev/playback-queue-runtime-audit-2026-06-04.md`:

- Move checks from `Not Verified` to `Done` only when the evidence above passes.
- Keep failed or skipped checks under `Not Verified` or `Incomplete`.
- Record the exact build mode and test date.
- Add no secrets, local filesystem paths, or raw auth headers.
