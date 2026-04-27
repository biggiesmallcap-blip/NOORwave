<h1 align="center">NOOR</h1>

<p align="center">A power-user music command center for TIDAL</p>

<p align="center">Local sync &nbsp;·&nbsp; Hi-fi playback &nbsp;·&nbsp; Genre Galaxy &nbsp;·&nbsp; Learning discovery engine</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024-orange?style=flat-square&logo=rust" alt="Rust"/>
  <img src="https://img.shields.io/badge/SvelteKit-5-ff3e00?style=flat-square&logo=svelte" alt="SvelteKit"/>
  <img src="https://img.shields.io/badge/SQLite-3-003b57?style=flat-square&logo=sqlite" alt="SQLite"/>
  <img src="https://img.shields.io/badge/Tauri-2-ffc131?style=flat-square&logo=tauri" alt="Tauri"/>
  <img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="MIT"/>
</p>

---

<table>
  <tr>
    <td width="33%"><img src="docs/assets/screenshot-home.png" width="100%" alt="Home"/><br/><sub><b>Home</b> — daily picks, new releases, now playing</sub></td>
    <td width="33%"><img src="docs/assets/screenshot-library.png" width="100%" alt="Library"/><br/><sub><b>Library</b> — top artist hero, carousels, recent tracks</sub></td>
    <td width="33%"><img src="docs/assets/screenshot-search.png" width="100%" alt="Search"/><br/><sub><b>Search</b> — top result card, power filters, queue</sub></td>
  </tr>
  <tr>
    <td width="33%"><img src="docs/assets/screenshot-genre-galaxy.png" width="100%" alt="Genre Galaxy"/><br/><sub><b>Genre Galaxy</b> — force-directed genre cosmos</sub></td>
    <td width="33%"><img src="docs/assets/screenshot-discover.png" width="100%" alt="Discover"/><br/><sub><b>Discover</b> — learned audio similarity canvas</sub></td>
    <td width="33%" align="center"><br/><br/><sub>more screenshots coming</sub></td>
  </tr>
</table>

---

## Features

### Library

Your entire TIDAL library, synced locally and always fast.

- Full library sync (tracks, albums, artists, playlists) with real-time WebSocket progress
- Home view: top artist hero card, recently played artist carousel, recently added album shelf, recent tracks
- Artist pages: blurred-artwork hero, full TIDAL discography (Albums / Singles & EPs), in-library flags, out-of-library cards linking to TIDAL preview
- Album pages: track table, hover-reveal actions, equalizer bar on active row, "More by" shelf
- Bulk operations: add/remove favorites, manage playlists at scale
- Decade strip filter, tile/list toggle, scroll position memory across back-navigation

---

### Search & Command

Search understands plain text, power filters, and intent in the same bar.

**Power filter syntax** — combine freely:

```
bpm:>130          energy:>0.8       genre:techno
key:6A            instrumental:true  year:1994
bpm:120-140 genre:house energy:>0.7
```

**Intent parsing:**

```
"play tool"      → plays top match immediately
"radio burial"   → opens Song Radio seeded from Burial
"1994"           → filters library to that year
```

**Special searches:**

```
/vibe         → mood-based cluster search
/underrated   → surfaces buried gems in your library
```

**`Ctrl+K`** — global command palette. Slash commands, quick-nav, and actions without leaving the keyboard.

Recent searches auto-save as clickable chips.

---

### Playback

- Lossless hi-fi streaming via TIDAL with automatic token refresh
- Gapless playback: NearEnd event fires 15 s before track end, triggering pre-buffer engine swap for zero-gap transitions
- BPM-aligned crossfade snap and per-track fade-in / fade-out
- Four shuffle modes: off, true (Fisher-Yates), weighted (boosts favorites + never-played), genre-spread (prevents consecutive same-genre runs)
- Automix: automatic queue continuation with Camelot + BPM + energy harmonic multipliers; harmonic match indicators on every queue row
- Now-playing panel shows Camelot wheel key, BPM badge, and full queue

---

### Genre Galaxy

> ⚠️ Live but needs polish — several interaction and rendering issues under active work.

- Interactive force-directed canvas of your entire genre taxonomy — 285 genres across 14 families
- Nodes sized and coloured by listen heat; edges drawn by genre co-occurrence
- Drill into any genre: artist cluster view, full track list, per-genre audio metric summary
- **Mix this genre**: loads tracks, shuffles queue, drops you at a random entry point
- **Seed Mix Builder**: blend multiple genres, interleave their tracks, and play
- Four view modes: Heat, Co-occurrence, Cohort, Evolution. Auto-drift pans the canvas

---

### Discover / Sound Space

> ⚠️ Work in progress — functional but incomplete. UI and model quality still evolving.

- Force-directed canvas of your library positioned by learned audio similarity
- Hyperspace search: type a mood or reference and fly to the matching cluster
- Nebula halos mark previously explored regions
- **Song Radio**: plays outward from any track using learned neighbor embeddings; creativity slider controls exploration vs. exploitation
- Feedback (like, dislike, queue, save) feeds back into the model
- **Prompt Explore**: steer the engine with natural language — mood, reference artist, DJ style
- Embedding pipeline trains on transitions, playlists, albums, genres, and listen sessions; incremental refresh + full retrain with live progress and cancel button

---
