# NOOR

A power-user music library manager and hi-fi audio player built to fix TIDAL's limitations. NOOR syncs your entire TIDAL library to a local SQLite database and layers on features that TIDAL doesn't offer: bulk operations, smart playlists, advanced analytics, duplicate detection, gapless playback, a learned radio discovery engine, audio feature analysis, and a genre visualization galaxy — all from a browser UI served over your LAN.

---

## Features

**Library Management**
- Full library sync (tracks, albums, artists, playlists) with real-time progress
- Bulk operations: add/remove favorites, playlist management at scale
- Advanced sorting, filtering, and full-text search (FTS5)
- Track and album detail panels with full metadata (ISRC, play count, quality badges, date added)
- Duplicate detection via ISRC matching and title/duration fallback
- Tile and list view toggle with floating popout detail modals

**Playback**
- Lossless hi-fi streaming via TIDAL with automatic token refresh
- Gapless playback with pre-buffer engine swap (zero-gap track transitions)
- Per-track fade-in / fade-out
- Volume control and seeking
- Four shuffle modes: off / true (Fisher-Yates) / weighted / genre-spread
  - Weighted: boosts favorites and never-played tracks, penalises recent plays
  - Genre-spread: prevents consecutive same-genre tracks
- Automix: automatic queue continuation with learning-based next-track selection

**Genre Galaxy**
- Interactive 3D-style force-directed canvas showing your entire genre taxonomy
- Nodes sized and coloured by listen heat; edges show genre co-occurrence
- Click any genre to drill into its interior: artist cluster visualization, track list, and audio metrics
- **▶ Mix this genre**: loads tracks, shuffles the queue, and starts from a random entry point
- **Seed Mix Builder**: blend multiple genres, interleave their tracks, and play
- Four view modes: heat, co-occurrence, cohort, evolution

**Discovery Sound Space**
- Force-directed canvas of tracks positioned by learned audio similarity
- Hyperspace search: type a mood or reference ("dark ambient", "140bpm drum & bass") and fly to a cluster of matching tracks
- Nebula halos mark previously explored regions
- Training animation: watch embeddings materialize as NOOR learns your library
- Playlist Builder: select nodes from the space and export as a playlist

**Discovery & Learning**
- **Similar Radio**: play outward from any track using learned neighborhoods
  - Creativity slider controls exploration vs. exploitation
  - Context memory influences results based on recent listening
  - Feedback (like, dislike, queue, save) feeds back into the model
- **Embedding-based learning pipeline**: trains on your listening behavior
  - Corpus from transitions, playlists, albums, artists, genres, and listen sessions
  - Incremental refresh and full retrain modes
  - Real-time training progress streamed via WebSocket
- **Prompt Explore**: steer the learned engine outward with language (mood, reference, DJ, word-cloud)
- **Home Page**: new releases from AllMusic RSS, daily picks from your library, weekly articles, and industry news

**Audio Analysis**
- DSP feature extraction running passively during playback: BPM, key signature, Camelot wheel, loudness (LUFS), energy, danceability, beat strength, spectral centroid, stereo width
- Instrumental detection
- Per-genre audio metric summaries (average BPM, energy, danceability)
- ACRCloud integration: fingerprint-based sample/cover recognition with configurable daily scan limit
- Analysis status dashboard and manual reset

**Smart Features**
- Rule-based smart playlists (genre, artist, date range, quality tier, play count — AND/OR logic)
- Genre taxonomy browser with 291 genres across hierarchy
- MusicBrainz enrichment (ISRC-first + title fallback, rate-limited)
- Discovery: prompt → genre inference → cross-service recommendation seeds

**Analytics**
- Listen history and session tracking
- Top tracks, top artists, genre heatmap
- Activity graph over time
- Completion rate, average listen duration, skip patterns

**Access & Real-Time UI**
- Bearer token auth: human-readable 5-word tokens (e.g. `amber-ridge-wolf-cedar-spark`)
- Auto-setup: the browser on the server machine connects automatically; remote devices get a connect modal
- WebSocket-driven: playback state, sync progress, queue updates, training progress push instantly to the browser
- Accessible on LAN — run on one machine, open from any browser on the network

**Multi-Service Integration**
- TIDAL: full library sync, playback, favorites, playlists
- Spotify: auth, genre enrichment via Spotify metadata
- RSS aggregation: AllMusic new releases, Billboard, NME, SPIN, Pitchfork, Rolling Stone, Consequence, The Guardian

---

## Stack

| Layer | Technology |
|---|---|
| Backend | Rust 2024, Axum 0.8, Rayon |
| Database | SQLite 3 (rusqlite), FTS5, WAL mode |
| Frontend | SvelteKit 2 + Svelte 5 runes, TypeScript, Vite |
| Audio decode | Symphonia 0.5 (streaming pipe with Content-Length–aware seeking) |
| Audio output | CPAL 0.15 (cross-platform) |
| Real-time | Tokio broadcast channel → WebSocket |
| Feed parsing | RSS 2.0 + Atom syndication |

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

The server starts on `http://localhost:3334` and auto-creates `noor.db` at the workspace root.

**Environment variables:**

| Variable | Default | Description |
|---|---|---|
| `NOOR_ADDR` | `0.0.0.0:3334` | Override server bind address |
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

1. Open `http://localhost:5173` — the browser on the server machine auto-connects via the setup endpoint
2. For remote devices, enter the access token shown in **Settings → Access Token**
3. Go to **Settings** and complete TIDAL device code authentication
4. Trigger a library sync — progress streams live via WebSocket
5. Start playing

### Portable MusicBrainz Snapshot

Move MusicBrainz enrichment between machines without copying the full `noor.db`:

```bash
python3 scripts/export_musicbrainz_enrichment.py --db noor.db --out-dir data/musicbrainz
```

On the other machine, sync the library first, then:

```bash
python3 scripts/import_musicbrainz_enrichment.py --db noor.db --from-dir data/musicbrainz
```

Transfer is keyed by stable `tidal_id` and genre `slug` — no dependency on local row IDs. Keep `noor.db` out of Git (contains auth and session data).

---

## Architecture

```
noor-server/src/
├── main.rs               # AppState, event bus, DB path resolution, token loading
├── server/
│   ├── routes.rs         # All REST handlers + WebSocket setup + auth middleware
│   └── ws.rs             # WebSocket broadcast
├── services/
│   ├── tidal/            # OAuth2 auth, API client, sync, streaming (StreamPipe)
│   ├── spotify/          # OAuth2 auth, genre enrichment
│   ├── audio_analysis/   # DSP feature extraction (BPM, key, energy, …)
│   ├── acrcloud.rs       # Fingerprint-based sample recognition
│   ├── musicbrainz.rs    # Metadata enrichment
│   ├── learning.rs       # Embedding training, neighbor computation
│   ├── discovery_trainer.rs # Training pipeline orchestration
│   └── rss_feeds.rs      # RSS/Atom feed aggregation
├── db/                   # Schema, migrations, models, queries
├── playback/
│   ├── runtime.rs        # StreamPipe decode loop + CPAL output
│   ├── player.rs         # Queue state machine, NearEnd/PrepareNext
│   ├── queue.rs          # Queue CRUD
│   ├── gapless.rs        # Gapless plan / crossfade config
│   └── shuffle.rs        # All four shuffle algorithms
├── genre/                # Taxonomy loading, normalization, fuzzy matching
├── smart/                # Smart playlists, analytics, discovery, external discovery
└── library/              # Duplicate detection, batch ops

frontend/src/
├── routes/               # Page components (home, library, genres, playlists, analytics, discover, settings, …)
├── lib/
│   ├── api/              # REST client + WebSocket client
│   ├── components/
│   │   ├── Genre/        # GenreGalaxy, GenreInterior, GenrePanel (canvas-based)
│   │   └── Discover/     # DiscoverSpace, DiscoverPanel, PlaylistBuilder (canvas-based)
│   └── stores/           # Player state, library state, training state (Svelte 5 runes)
└── app.css               # Glass-tile design system (dark base #0a0a0f, accent #7c80ff)
```

**Key design decisions:**

- `Arc<RwLock<AppState>>` shared across all Axum route handlers
- StreamPipe buffers CDN bytes as they arrive; `byte_len()` returns `Content-Length` from response headers so Symphonia can seek correctly without downloading the full file first
- NearEnd event fires 15 s before track end, triggering PrepareNext for zero-gap engine swap
- Smart playlist rules are evaluated on-demand (recursive AST, no background materialization)
- Genre taxonomy is loaded from `genre-taxonomy/taxonomy.json` into SQLite on startup
- TIDAL 403 errors from content restrictions are classified as `StreamRejected`, not session expiry — a content restriction never clears your login session

---

## Roadmap

- [x] Discovery engine with embedding-based learning
- [x] Similar Radio with creativity and context controls
- [x] Home page with RSS-driven new releases, daily picks, articles, and news
- [x] Spotify auth and genre enrichment
- [x] Audio feature extraction (BPM, key, energy, danceability via DSP)
- [x] ACRCloud sample recognition
- [x] Genre Galaxy visualization with heat, co-occurrence, cohort, and evolution views
- [x] Genre Mix: randomised entry point, seed blend builder
- [x] Discovery Sound Space with hyperspace search and nebula halos
- [x] Bearer token auth with auto-setup for local browsers
- [ ] Gapless crossfade audio blend (pre-buffer swap works; audio-level mixing pending)
- [ ] Duplicate detection UI (detection logic complete)
- [ ] Automix DJ mode with harmonic mixing
- [ ] WASAPI exclusive mode (Windows hi-fi priority)
- [ ] YouTube Music integration
- [ ] SoundCloud integration
- [ ] Package as Tauri desktop app

---

## Notes

NOOR uses TIDAL's unofficial API. Authentication uses a device code OAuth2 flow with a known client ID — the same mechanism used by other third-party TIDAL clients. Credentials are stored locally, encrypted with AES-GCM in the SQLite database.

---

## License

MIT
