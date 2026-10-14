# Atlas: Picture, You, Journeys

The Atlas's two looks and its Route were shallow: "Color is place" coloured
each scene by its angle around the map (position, twice), "Light is
knowledge" sorted artists into four static bins, and Route found the
strongest related-artist path between two picks and played one video per
stop. This replaces all three.

## What changes

1. **Picture** (look): every artist takes the dominant colour of its own
   video artwork (one 80 px thumbnail per artist, fetched slowly in the
   background). Zoomed right in, dots become the thumbnails.
2. **You** (look, replaces Light is knowledge): your watching as
   footprints that fade over weeks (recency, not bins), session trails
   drawn between the artists you watched in a row, and the frontier: the
   unexplored artists right next to ones you watch, glowing at the edge of
   your known world.
3. **Journeys** (replaces Route):
   - **Collab chain**: from one artist to another where every step is a
     real video both artists are on.
   - **Somewhere new**: from where you are (playing or picked artist) to a
     scene you have barely touched, through artists new to you.
   - Both play as a program; on the Atlas the camera follows the artist
     playing and the trail lights up stop by stop.

## Contracts

All under `/api/videos/atlas`, local reads only (no TIDAL API calls).

### GET /pictures

```json
{ "ready": 9100, "total": 9918,
  "ids": [123, 456],
  "rgb": [16737095, 3355443],
  "art": ["6a0ff657/cfc9/47f6/9270/c84d128eeb09", "..."] }
```

Parallel arrays for the artists done so far. `rgb` is 0xRRGGBB. `art` is
the image path of the artist's picked thumbnail; the client builds
`https://resources.tidal.com/images/{art}/640x640.jpg` and sizes it with
`upscaleTidalArtwork`.

Backend: MIGRATION_078 `video_artist_pictures (artist_tidal_id PK,
video_id, art, rgb, status pending|done|failed, tries, fetched_at)`. One
video per artist (best programmable cut). A background job fetches the 80 px
thumbnail from resources.tidal.com at the discovery setting's pace (full:
about 4 per second, limited: about 1 per 2 s, off: none), decodes it with
the `image` crate (jpeg only) and stores the dominant colour: ignore near
black and near white (letterbox, title cards), bucket by hue, take the
bucket with the most saturated weight, average its pixels; black-and-white
videos get their mean grey. Failed fetches retry up to 3 times.

### GET /you

```json
{ "footprints": [{ "artist_id": 1, "last_at": 1791900000, "plays": 3 }],
  "trails": [{ "ended_at": 1791900000, "artists": [1, 2, 3] }],
  "frontier": [{ "artist_id": 5, "score": 0.82, "via": 1 }] }
```

- footprints: artists watched (video_history) or saved, last 180 days.
- trails: consecutive watches with gaps under 45 minutes, last 60 days,
  repeats collapsed, at least 2 artists, newest last, at most 40.
- frontier: graph neighbours (both directions) of footprint artists that
  have never been watched or saved, on the map, scored by edge weight times
  how recently the footprint was made; top 300.

### GET /journey?kind=collab&from=&to= and ?kind=new&from=[&scene=]

```json
{ "found": true, "kind": "collab", "title": "Drake to Calvin Harris",
  "destination": { "scene_id": 12, "name": "Funk and Disco" },
  "stops": [{ "artist_id": 1, "name": "Drake", "light": 3,
              "video": { "...ExploreVideo": "" }, "why": "Rihanna with Drake" }],
  "reason": null }
```

- collab: path over featured links only (a video by A that features B, or
  the reverse), at most 8 steps, preferring popular link videos. Stop 0's
  video is the start artist's best video; every later stop's video is the
  collab video linking it to the stop before.
- new: destination is a scene with a low share of seen videos, reachable
  within 8 steps, not the start's own scene (or `scene` when given); the
  path prefers artists you have not watched and ends at one of the
  destination's top artists; each stop plays its best cut you have not
  watched. `destination` is set only for this kind.
- Not found: `{ "found": false, "reason": "no collab chain within 8 steps" }`.

## Frontend

- Looks become **Picture | You** (`look` 'picture' | 'you'); the legend
  explains each in a line.
- Picture: point colour from /pictures (neutral dim until known); the
  nebula uses each scene's average picture colour; at deep zoom visible
  artists draw their thumbnail (cached, loaded only when visible).
- You: neutral map, footprint halos by recency, trails as fading lines,
  frontier artists with a soft accent ring; legend counts.
- Journeys panel (toolbar button) with the two kinds, the stops listed
  with their why, Play journey; playing follows on the map.
