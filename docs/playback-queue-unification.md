# Playback queue unification

## Decision

The `queue` table is the only playback queue. Every item has a stable row id
and position. A resolved item has `track_id`; a streamable TIDAL item remains a
pending row with `tidal_id_hint` and persisted display metadata until it is
resolved. `playback_state.current_queue_item_id` always identifies the active
row while playback is active.

Transient resolution imports a `tracks` row with `is_library = 0`. It is usable
for streaming, history, and favorites without appearing in Library. An explicit
like or a real TIDAL sync promotes it to `is_library = 1`.

## Queue behavior

Queue rows are non-destructive. Next, previous, play-next, add, remove, move,
shuffle, jump-to, repeat, gapless lookahead, and DJ lookahead use normal queue
position and the current-row cursor. A pending active row resolves before the
stream starts; unresolved or failed rows are skipped through the same advance
path.

There is no in-memory queue overlay. TIDAL mixes, albums, playlists, search,
and discovery results produce normal queue rows with source metadata. The queue
is still cleared at server start, so this change does not make sessions durable.

## API and rollout

`POST /api/playback/queue` is the canonical replacement route. Its ordered
items accept either a library `track_id` or external TIDAL metadata, plus an
optional row reason and shuffle mode. Append and play-next accept the same
external metadata shape for individual or batch mutation.

The frontend and remote UI migrate in the same change. The obsolete mixed and
ephemeral TIDAL routes are removed rather than retained as compatibility paths.
No migration is needed because pending-row metadata and the library visibility
flag already exist.
