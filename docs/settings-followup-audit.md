# Settings follow-up audit

Completed on 2026-10-04 after reviewing the original Settings page, the shared provider panels, desktop commands and phone pairing flow against the implemented rework.

## Corrections

- Settings now uses the existing `glass-panel` page surface and `glass-tile` groups. Theme, palette, material grain and blur come from the shared app tokens. Removed the opaque page background and panel overrides. Fixed a development CSS ordering issue that added excess space below the page header.
- Last.fm's API registration link was present but visually indistinguishable from the instructions. It now reads **Create a Last.fm API application**, is underlined with an external-link indicator, and appears before setup when no key is configured. The same link appears in the credential form and onboarding. Desktop links use the system browser, with the address available if opening fails. Key-only tag setup remains valid.
- Added searchable TIDAL content-filter guidance with direct links to TIDAL's AI and explicit-content documentation. TIDAL confirms that its own app supports restricting AI-labeled recordings. NOORwave's current integration does not implement these filters; no local control or enforcement claim was added. Other TIDAL controls already available here remain streaming/video quality, account connection, library sync, daily sync and learning from favorite albums.
- Phone setup help explains QR connection, Home Screen installation and the separate iPhone PWA connection. A QR already redeemed in Safari cannot be redeemed again by the installed app: refresh it for a new one-time code, then pair inside the PWA. Remembered connections reconnect without another code until access is revoked or storage is cleared.
- Close-to-tray was retained in the rework but hidden outside the desktop app. It is now visible, disabled with a desktop explanation in browser testing. Its native read no longer depends on update-status reads succeeding. Failed close-to-tray and startup saves restore the switch's visible state as well as its stored state.
- Restored brief, task-specific descriptions for daily sync, favorite-album learning, full resync, MusicBrainz enrichment, Last.fm enrichment, passive analysis and library management. Cleanup actions have separate aligned rows with their effects beside them; existing confirmations remain.
- Corrected stale hosting-restart warnings in Settings and the desktop tray. The current backend stops playback on restart and preserves the durable queue; the warning now matches that behavior.

## Original control inventory

| Original area | Current destination and retained functionality |
| --- | --- |
| Player position | Appearance → Player: right, left and bottom |
| Artwork | Appearance → Player: square and banner |
| Streaming quality display | Appearance → Player: side and bottom indicator modes |
| Surface mode | Appearance → Interface: light, dark and system |
| Colour scheme | Appearance → Interface: all existing palettes |
| Interface size | Appearance → Interface: slider, increment, decrement, reset and keyboard shortcuts |
| Horizontal shelves | Appearance → Interaction: mouse-wheel browsing |
| Background | Appearance → Background: gallery, off, preview, blur, FPS, reactive strength, beat smoothing, motion, color source, quality and idle behavior |
| Connect TIDAL | Services → TIDAL: sign-in, redirect paste, completion, cancellation, retry and disconnect |
| TIDAL library operations | Library → Sync: progress, last sync, sync/retry, cancel, daily sync, favorite-album learning and full resync |
| MusicBrainz enrichment | Library → MusicBrainz genres: counts, progress, start/resume and Genre Galaxy refresh |
| Last.fm tags | Library → Last.fm tags: coverage, checked/remaining counts, progress, enrich, retry/recheck and stop; credential setup/removal moved to Services |
| Playback output | Playback → Output: quality, device, presets, exclusive output and recovery, latency/buffer, release-on-pause, idle grace, source sample rate and video quality |
| Downloads | Library → Downloads: folder picker/manual path, FLAC/MP3 format and conditional quality/source choices |
| Discovery engine | Library → Discovery: engine, safety profile, intensity, estimates, train/retrain, stop, progress, refresh and guide |
| Radio similarity index | Library → Radio index: status, build time, manual rebuild and automatic scheduling explanation |
| App updates | App → App updates: version, install mode, status, check and update details |
| Closing the window | App → Closing the window: native close-to-tray, visible but disabled in a browser |
| Portable snapshot | Library → Enrichment transfer: status/path, import and export |
| Database size | Library → Library management → Database storage: size, reclaim estimate, refresh, compact, warning and progress |
| Clear non-library entries | Library → Library management → Remove unused recommendations: eligibility explanation, protected-item preservation and confirmed purge |
| Now playing path | Playback → Output details: device, format, track and errors |
| Library audio data | Playback → Analysis: passive switch and statistics; confirmed deletion moved to Library management |
| Listening services | Services: Last.fm and ListenBrainz connection, approval, status/retry, account disconnect, credential removal and history uploads |
| Phone remote | Remote: hosting, addresses, QR/code, expiry/cancel, master PIN/reset, device rename/revoke, diagnostics and recovery |
| Native startup | App → Startup: start at sign-in, installed/portable/platform capability, observed state and retry |

No additional working settings were removed. The two original numeric analysis-limit inputs had no bindings or handlers and remain omitted. Old category names and setting anchors still resolve through the shared manifest.

## Verification

- 988 frontend tests passed across 160 files after rebasing onto current master.
- Two native desktop tray-confirmation tests passed.
- Svelte/TypeScript and styling checks passed; production frontend build passed.
- Browser verification against the running backend and copied library checked all six categories at 360, 640, 960 and 1440px, with no page or Settings overflow and no browser exceptions.
- Checked the Last.fm registration link, TIDAL guidance, phone PWA instructions and library-management help. Simulated native commands verified independent close-to-tray reads, failed-save rollback/retry for both desktop switches, and external-browser failure fallback.
- Reviewed dark desktop and light Clay phone screenshots. Live provider approval and physical iPhone installation remain manual checks.

Sources: [Last.fm API registration](https://www.last.fm/api/account/create), [TIDAL AI policy and filter guidance](https://support.tidal.com/hc/en-us/articles/48031883413521-AI-Policy), [TIDAL explicit-content guidance](https://support.tidal.com/hc/en-us/articles/9936639051153-Explicit-Content).

## Onboarding consistency follow-up

- All six steps share the same display heading, centered introduction and 44px navigation buttons. Removed separate white TIDAL buttons and palette-ancestor overrides; primary actions use the scoped Futuro accent.
- The completion notice is centered, limited to a readable line length and spaced as separate paragraphs. Service details, help and credential fields remain left aligned.
- The frame and listening-service cards use the app's shared glass surfaces. Theme controls sit in a matching group, with stacked rows on phones. The logo remains reachable when long credential forms require vertical scrolling.
- Excluded the onboarding wallpaper from the app's light-theme fade rule. Saved light/Clay preferences previously reduced its opacity and exposed the cream body background, despite the renderer using Futuro. The wallpaper now retains full opacity and the dark frame on every step; the bounded appearance preview still reflects the draft.
- Last.fm approval and ListenBrainz token links share the existing external-link styling and desktop-browser fallback.
- Browser verification covered all six steps at 360, 640, 960 and 1440px, with saved Futuro/dark, Clay/light and Iris/dark appearances. Checked title/button consistency, card bounds, wallpaper opacity, preview isolation, pending TIDAL sign-in, and connected/editing Last.fm states. No browser exceptions or horizontal overflow. Isolated account fixtures did not change provider accounts or finish setup on the copied library.

## TIDAL AI-filter implementation finding

The user supplied [Tideway](https://github.com/J-M-PUNK/tideway). Its [track parser](https://github.com/J-M-PUNK/tideway/blob/main/app/tidal_client.py) preserves the raw `ai` boolean that its upstream TIDAL library otherwise discards. Its [filter tests](https://github.com/J-M-PUNK/tideway/blob/main/tests/test_ai_content_filter.py) verify that the local `hide_ai_content` preference removes only tracks marked true and retains false or missing metadata. The [README](https://github.com/J-M-PUNK/tideway#whats-inside) documents broad browsing coverage while retaining access to saved favorites. This provides a concrete local-filter approach, separate from changing a TIDAL account preference.

NOORwave's `TidalTrack` already preserves unrecognized payload fields in its `extra` map (`noor-server/src/services/tidal/client.rs`). A follow-up implementation can extract `ai` as an optional boolean, carry it through API responses and persisted catalog metadata, and apply a single filtering policy across search, artist/album pages, mixes, radio and discovery. Verify live payloads for each feed before enabling the control, retain tracks with unknown metadata, and keep saved library items accessible. Pagination and browse playback must use the same policy so hidden tracks are not queued by a page-level Play or Shuffle action. Suggested control: Services → TIDAL → **Hide AI-generated tracks**, with the explanation **Hides tracks marked as AI-generated by TIDAL. Saved library items remain available.**

No AI-filter toggle or enforcement has been added in this UI follow-up.
