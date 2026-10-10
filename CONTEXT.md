# NOORwave

Domain glossary for NOORwave, a single-user desktop hi-fi player built on a user's TIDAL library. This file defines terms whose meaning is specific to this project. It is a glossary, not a spec.

## Language

### Artist surfaces

**Library artist**:
An artist the user owns, keyed by the local SQLite `artists.id`, with owned tracks and rich local affordances (favorites, play counts, library albums, Spotify stats). Rendered from a local artist row.
_Avoid_: local artist (when ambiguous with "local track").

**TIDAL artist**:
An artist that exists only on TIDAL, keyed by its TIDAL id with no local row, sourced entirely from the TIDAL profile endpoint. Has no local-track fallback, so a failed TIDAL fetch leaves nothing to show.
_Avoid_: remote artist, non-library artist.

Both render through one shared view; the only difference is the data source. A **Library artist** may still pull its discography from TIDAL, but it always has owned tracks to fall back on; a **TIDAL artist** does not.

**available** (artist discography payload):
A boolean meaning "TIDAL returned at least one usable catalog result for this artist." `false` means TIDAL gave us nothing usable: a total fetch failure, the artist is not on TIDAL, or TIDAL is not connected. It is NOT a promise that any particular section (albums, videos, similar) has data.
_Avoid_: reading `available: true` as "the page has content" or "TIDAL is healthy."

> **Flagged ambiguity (historical bug):** `available` was once hardcoded `true` even when every TIDAL fetch had errored to empty. A **Library artist** then showed only Top tracks (the local-track album fallback was wrongly suppressed by the flag), and a **TIDAL artist** showed a hollow header. Resolution: album shelves gate on real data with a local fallback; `available` is honest and is what a **TIDAL artist** view uses to decide between a retry state and an empty body.

### TIDAL connection

**TIDAL session**:
The single owner of the user's TIDAL tokens (access, refresh, user id, country) and of their lifecycle: login, logout, persistence, refresh on auth failure, and the "needs reconnect" latch. Every TIDAL client is a handle obtained from the session, so callers never build clients from raw tokens or write their own 401 retry.
_Avoid_: tokens (when you mean the whole lifecycle), auth, TIDAL connection state.

**Auth failure**:
A TIDAL response that means the session's access token is no longer accepted: HTTP 401 other than subStatus 4005, or subStatus 6001. Triggers one single-flight refresh. A 401 with subStatus 4005 ("asset not ready for playback") is NOT an auth failure; it is a per-track playability answer.
_Avoid_: "any 401", "looks like auth".

**Needs reconnect**:
The latched state a **TIDAL session** enters when a refresh itself fails (no refresh token, revoked, invalid_grant). TIDAL calls fail fast with a session-expired error and no further refresh is attempted until the user logs in again.

### Playback

**Transport**:
The module that owns what is playing and how playback moves between queue items: play, next, previous, play-queue-item, pause, resume, seek, resolving or skipping pending rows, stream resolution, and reacting to audio runtime events (near end, finished, track error). It alone bumps and checks the **playback generation**. HTTP handlers, the phone remote and runtime events all go through it. Lives in `noor-server/src/server/transport/` (server layer: it coordinates the audio runtime, the TIDAL session, the DB and WS events).
_Avoid_: playback session (collides with **listen session**), player (ambiguous with `playback/player.rs` queue state), controller.

**Listen session**:
The listening-history record for one stretch of a track being heard, closed with an end reason (replaced, queue ended, ...). The **Transport** writes listen sessions; it is not one.

**Playback generation**:
A monotonically increasing counter bumped by every user transport command. Work that started under an older generation (a slow stream resolve, a pending-row lookup) must not apply its result. Owned by the **Transport**; nothing else reads or bumps it.

**Queue edit**:
Adding, appending, play-next, moving, removing or clearing queue rows. Not part of the **Transport**; a queue edit that changes the current item asks the **Transport** to act.

## Example dialogue

**Dev:** The Otis Redding page only shows Top tracks. Is `available` false?

**Domain expert:** No, it's a Library artist, so it has owned tracks. `available` was true but every TIDAL fetch came back empty, so there were no album shelves and the local fallback was being suppressed by the flag.

**Dev:** So `available: true` doesn't mean there's anything to render?

**Domain expert:** Right. `available` only tells you TIDAL answered with something usable. Whether a section renders is decided by that section's real data. The flag only does load-bearing work for a TIDAL artist, where there's no local fallback to lean on.
