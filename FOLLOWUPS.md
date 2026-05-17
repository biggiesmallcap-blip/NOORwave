# Follow-ups

Out-of-scope work flagged while shipping something else. Items live here until
you pick them up (then delete the entry) or decide they're not worth doing
(also delete). One running list keeps everything in one place; GitHub issues
are overkill for a solo project.

Format per item: short title, one or two lines of context, "Spawned by:" link
back to the PR or commit that flagged it.

## Open

### feat: `/moods/[id]` drill-down (TIDAL)

`/moods` ships as a landing only. The backend two-segment route
(`/api/tidal/page/mood/{id}`) is registered but unused. TIDAL moods typically
come back as opaque playlist UUIDs whose drill-down endpoint is NOT
`/v1/pages/mood/{slug}`, so the existing route shape doesn't fit cleanly. Need
to confirm the wire shape with a live probe and either (a) extend
`parse_home_item` for a new `mood` kind with an item-level path field, or
(b) drop the unused route and route mood cards straight to a playlist view.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45 (Task 2.6)

### feat: TIDAL `/genres` and `/new-releases` Svelte routes

Backend already serves them via the generic `get_page_modules` helper and the
whitelisted `/api/tidal/page/{section}` route. Each one is a clone of
`frontend/src/routes/charts/+page.svelte` with one URL changed plus four nav
registry files touched (`registry.ts`, `registry-data.json`,
`navigation-data.json`, `registry.test.ts`).
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45 (Phase 2 non-goals)

### feat: save-to-library for Spotify tracks and albums

`save_spotify_playlist` exists; the equivalent handlers for individual tracks
and full albums do not. Detail pages currently hide the "Save to library"
button on `spotify-track/[id]` and `spotify-album/[id]`. Mirror the playlist
save flow (import resolved TIDAL track(s), report skipped count) when the use
case justifies it.
- Spawned by: https://github.com/biggiesmallcap-blip/NOORwave/pull/45 (Task 1.3 explicit non-goal)
