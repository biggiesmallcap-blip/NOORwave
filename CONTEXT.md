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

## Example dialogue

**Dev:** The Otis Redding page only shows Top tracks. Is `available` false?

**Domain expert:** No, it's a Library artist, so it has owned tracks. `available` was true but every TIDAL fetch came back empty, so there were no album shelves and the local fallback was being suppressed by the flag.

**Dev:** So `available: true` doesn't mean there's anything to render?

**Domain expert:** Right. `available` only tells you TIDAL answered with something usable. Whether a section renders is decided by that section's real data. The flag only does load-bearing work for a TIDAL artist, where there's no local fallback to lean on.
