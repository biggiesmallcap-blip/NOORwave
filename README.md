# NOOR

A power-user music library manager and hi-fi audio player built to fix TIDAL's limitations. NOOR syncs your entire TIDAL library to a local SQLite database and layers on features that TIDAL doesn't offer: bulk operations, smart playlists, advanced analytics, duplicate detection, and gapless playback — all from a browser UI.

---

## Features

**Library Management**
- Full library sync (tracks, albums, artists, playlists) with real-time progress
- Bulk operations: add/remove favorites, playlist management at scale
- Advanced sorting, filtering, and full-text search (FTS5)
- Duplicate detection via ISRC matching and title/duration fallback

**Playback**
- Lossless hi-fi streaming via TIDAL
- Gapless playback with pre-buffer engine swap (zero-gap track transitions)
- Fade-in/fade-out per track
- Volume control and seeking
- Four shuffle modes: off / true (Fisher-Yates) / weighted / genre-spread
  - Weighted: boosts favorites and never-played tracks, penalizes recent plays
  - Genre-spread: prevents consecutive same-genre tracks

**Smart Features**
- Rule-based smart playlists (genre, artist, date range, quality tier, play count — AND/OR logic)
- Genre taxonomy browser with 291 genres across hierarchy
- MusicBrainz enrichment (ISRC-first + title fallback, rate-limited)
- Discovery: prompt → genre inference → cross-service recommendation seeds

**Analytics**
- Listen history and session tracking
- Top tracks, top artists, genre heatmap
- Activity graph over time

**Real-Time UI**
- WebSocket-driven: playback state, sync progress, queue updates push instantly to the browser
- Accessible on LAN — run on a machine, open from any browser

---

## Stack

| Layer | Technology |
|---|---|
| Backend | Rust 2024, Axum 0.8 |
| Database | SQLite 3 (rusqlite), FTS5, WAL mode |
| Frontend | SvelteKit 2 + Svelte 5 runes, TypeScript, Vite |
| Audio decode | Symphonia 0.5 (packet streaming, no full load) |
| Audio output | CPAL 0.15 (cross-platform) |
| Real-time | Tokio broadcast channel → WebSocket |

---

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (stable, 2024 edition)
- [Node.js](https://nodejs.org/) 18+ and npm (or pnpm)
- A TIDAL account

### Backend

```bash
cd noor-server
cargo run --release
```

The server starts on `http://localhost:3333` and auto-creates `noor.db` at the workspace root.

**Environment variables:**

| Variable | Default | Description |
|---|---|---|
| `NOOR_DB` | `<workspace>/noor.db` | Override the database path |
| `RUST_LOG` | `noor_server=info` | Log level |
| `TIDAL_CLIENT_ID` | *(built-in)* | Override TIDAL OAuth2 client ID |
| `TIDAL_CLIENT_SECRET` | *(built-in)* | Override TIDAL OAuth2 client secret |

### Frontend

```bash
cd frontend
npm install
npm run dev
```

Open `http://localhost:5173` in your browser.

For a production build:

```bash
npm run build
```

### First Run

1. Go to **Settings** in the UI
2. Complete TIDAL device code authentication
3. Trigger a library sync — progress streams live via WebSocket
4. Start playing

### Portable MusicBrainz Snapshot

If you want to move MusicBrainz enrichment between machines without copying the full `noor.db`, export just the portable enrichment snapshot:

```bash
python3 scripts/export_musicbrainz_enrichment.py --db noor.db --out-dir data/musicbrainz
```

That writes:

- `data/musicbrainz/musicbrainz_checked.csv`
- `data/musicbrainz/musicbrainz_genres.csv`
- `data/musicbrainz/manifest.json`

On the other machine, sync the library first so the `tracks` table exists, then import the snapshot:

```bash
python3 scripts/import_musicbrainz_enrichment.py --db noor.db --from-dir data/musicbrainz
```

The transfer is keyed by stable `tidal_id` values and genre `slug`, so it does not depend on matching local SQLite row IDs. Keep `noor.db` out of Git because it also contains local auth/session data.

---

## Architecture

```
noor-server/src/
├── main.rs               # AppState, event bus, DB path resolution
├── server/
│   ├── routes.rs         # All REST handlers + WebSocket setup
│   └── ws.rs             # WebSocket broadcast
├── services/
│   ├── tidal/            # OAuth2 auth, API client, sync, streaming
│   └── musicbrainz.rs    # Metadata enrichment
├── db/                   # Schema, migrations, models, queries
├── playback/
│   ├── runtime.rs        # StreamPipe: decode loop + CPAL output
│   ├── player.rs         # Queue state machine, NearEnd/PrepareNext
│   ├── queue.rs          # Queue CRUD
│   └── shuffle.rs        # All four shuffle algorithms
├── genre/                # Taxonomy loading, normalization, fuzzy matching
├── smart/                # Smart playlists, analytics, discovery
└── library/              # Duplicate detection, batch ops

frontend/src/
├── routes/               # Page components (library, genres, playlists, analytics, …)
├── lib/api/              # REST client + WebSocket client
├── lib/stores/           # Player state, library state (Svelte 5 runes)
└── app.css               # Glass-tile design system (dark base #0a0a0f, accent #7c80ff)
```

**Key design decisions:**

- `Arc<RwLock<AppState>>` shared across all Axum route handlers
- Symphonia decodes packet-by-packet into a ring buffer — tracks are never fully loaded into memory
- NearEnd event fires 15s before track end, triggering PrepareNext for zero-gap engine swap
- Smart playlist rules are evaluated on-demand (recursive AST, no background materialization)
- Genre taxonomy is loaded from `genre-taxonomy/taxonomy.json` into SQLite on startup

---

## Roadmap

- [ ] Gapless crossfade audio blend (pre-buffer swap works; audio-level mixing pending)
- [ ] Duplicate detection UI (detection logic complete)
- [ ] Automix / DJ mode
- [ ] WASAPI exclusive mode (Windows hi-fi priority)
- [ ] Genre Galaxy visualization
- [ ] YouTube Music integration (Phase 5)
- [ ] SoundCloud integration (Phase 5)
- [ ] Package as Tauri desktop app

---

## Notes

NOOR uses TIDAL's unofficial API. Authentication uses a device code OAuth2 flow with a known client ID. This is the same mechanism used by other third-party TIDAL clients. Your credentials are stored locally, encrypted with AES-GCM in the SQLite database.

---

## License

MIT
