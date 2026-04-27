---
name: NOORwave README Redesign
description: Visual-first README with carousel hero, feature showcases, WIP badges, and known limitations prominently displayed
type: spec
---

# NOORwave README v2: Visual-First Design

## Goal

Transform the README from text-dense to visual-first, leading with an interactive screenshot carousel that showcases the key features (library management, discovery systems, audio analysis). Emphasize both portfolio appeal and practical onboarding for contributors and users. Prominently surface in-progress features and known limitations.

## Audience

- **Portfolio/Showcase:** Developers and music enthusiasts exploring what NOOR is
- **End Users:** Setting up locally, understanding capabilities
- **Contributors:** Understanding architecture and where to pitch in

---

## Structure

### 1. Hero Section: Screenshot Carousel

**Content:**
- 5-6 rotating screenshots showing:
  1. Library view (track table, sidebar, metadata)
  2. Genre Galaxy (3D force-directed visualization)
  3. Discovery Sound Space (particle canvas with halos)
  4. Now-Playing panel (glass UI, Camelot wheel, BPM)
  5. Smart Playlists builder UI
  6. Settings / Appearance themes

**Behavior:**
- Auto-rotates every 5 seconds
- Clickable dots/arrows for manual navigation
- Each screenshot has a 1-line caption below
- Lightweight GIF or static image (image size ~1200x700px)

**Copy Below Carousel (2-3 sentences):**
> "NOOR syncs your entire TIDAL library locally and adds the features TIDAL doesn't: bulk operations, learned discovery, audio analysis, and a genre visualization galaxy. Hi-fi streaming, gapless playback, and harmonic mixing—all from your browser on LAN."

---

### 2. Quick Start CTA

**Element:**
Prominent button or link: **"Get Started in 3 Minutes →"**
Points to the Getting Started section.

---

### 3. Status/Known Issues Block (Early Visibility)

**Placement:** Right after the pitch, before Features

**Content:**
```
⚠️ Status & Known Limitations:
• Audio blend: Works; visual polish pending
• Duplicate detection UI: Logic complete, UI coming soon
• WASAPI exclusive mode (Windows): On roadmap

See Roadmap below for full feature status.
```

---

### 4. Features Section

**Organization:** Six feature groups, each with the same layout:

**Per-Feature Layout:**
```
## [Feature Group Title]

[1-2 sentence description of what this solves]

[Embedded screenshot or GIF]

**Highlights:**
- Bullet point 1
- Bullet point 2
- [🚧 Badge if in progress]
- [⚠️ Known limitation if applicable]
```

**Feature Groups:**

#### A. Library Management
- Full sync (tracks, albums, artists, playlists)
- Bulk operations, advanced search/filtering (FTS5)
- Detail panels, metadata (ISRC, play count, quality)
- Artist pages (discography, in-library flags)
- Album pages (track table, "More by" shelf)
- Duplicate detection (ISRC + fingerprint)
- Tile and list view toggle
- **Screenshot:** Library view showing track table, sidebar, metadata panel
- **Badge (if needed):** Duplicate detection UI `[🚧 In Progress]`

#### B. Playback
- Lossless hi-fi via TIDAL
- Gapless playback with zero-gap engine swap
- Per-track fade-in / fade-out
- Volume & seeking
- Four shuffle modes (true/weighted/genre-spread)
- Automix with learning-based next track
- Now-playing Camelot + BPM indicators
- **Screenshot:** Now-Playing panel with glass UI, Camelot wheel, BPM
- **Known limitation:** Audio blend works; visual polish pending `[⚠️]`

#### C. Genre Galaxy
- 3D force-directed visualization of genre taxonomy
- Nodes sized/colored by listen heat
- Click to drill into genre interior (artist clusters, tracks, metrics)
- Mix this genre: randomized playback
- Seed Mix Builder: blend multiple genres
- Four view modes (heat, co-occurrence, cohort, evolution)
- **Screenshot:** Genre Galaxy canvas, drill-down interior view

#### D. Discovery Sound Space
- Force-directed canvas of tracks by learned similarity
- Hyperspace search (mood/reference queries)
- Nebula halos marking explored regions
- Training animation
- Playlist Builder from selected nodes
- **Screenshot:** Discovery Space particle canvas

#### E. Audio Analysis & Smart Playlists
- DSP extraction: BPM, key, Camelot, loudness, energy, danceability, etc.
- Instrumental detection
- Per-genre audio metric summaries
- ACRCloud fingerprint-based sample/cover recognition
- Rule-based smart playlists (AND/OR, genre, BPM range, key, energy, etc.)
- Genre taxonomy browser (291 genres)
- **Screenshot:** Smart Playlists builder, analytics dashboard
- **Badge (if needed):** Smart playlist UI refinements `[🚧 Coming Soon]`

#### F. UI & Access
- 6-digit PIN auth
- Auto-setup (local browser connects automatically)
- WebSocket-driven real-time updates
- Shader wallpapers (Aurora, Chrome, Grid, Nebula, Topo)
- Context menu (right-click / ··· button)
- Global keyboard shortcuts
- Settings organized by category
- Accessible on LAN from any browser
- **Screenshot:** Settings panel with appearance themes

---

### 5. Stack Table

Keep existing tech stack table (Rust, SQLite, SvelteKit, Symphonia, CPAL, etc.)

---

### 6. Getting Started

**Subsections:**
- Prerequisites (Rust, Node.js 18+, TIDAL account)
- Backend setup (cargo run --release)
- Frontend setup (npm install, npm run dev)
- First Run checklist (PIN, TIDAL auth, library sync)
- Portable MusicBrainz snapshot (export/import scripts)

Keep existing structure; no changes needed here.

---

### 7. Architecture

Keep existing ASCII diagram and design decisions section.

---

### 8. Roadmap

**Organization:**
- Group by area: Playback, Discovery, Integration, Polish, Experimental
- Use status badges: ✅ Done | 🚧 In Progress | 📋 Planned

**Example:**
```
## Roadmap

### Playback
✅ Gapless playback (zero-gap engine swap + BPM-aligned crossfade)
✅ Per-track fade-in/fade-out
🚧 Gapless crossfade audio blend (pre-buffer swap works; audio mixing pending)
🚧 WASAPI exclusive mode (Windows hi-fi priority)

### Discovery
✅ Embedding-based learning with incremental refresh
✅ Similar Radio with creativity + context controls
✅ Genre Galaxy (heat, co-occurrence, cohort, evolution views)
✅ Discovery Sound Space with hyperspace search

### Integration
✅ TIDAL full sync and playback
✅ Spotify auth and genre enrichment
✅ RSS aggregation (AllMusic, Billboard, NME, etc.)
📋 YouTube Music integration
📋 SoundCloud integration

### Polish
🚧 Duplicate detection UI
📋 Desktop app (Tauri packaging)
```

---

### 9. Notes & License

Keep existing notes about TIDAL API usage and MIT license.

---

## Visual Assets Needed

| Feature | Type | Dimensions | Notes |
|---------|------|-----------|-------|
| Library view | Screenshot | 1200×700px | Show track table, sidebar, detail panel |
| Genre Galaxy | Screenshot/GIF | 1200×700px | Static or rotating visualization |
| Discovery Space | Screenshot/GIF | 1200×700px | Particle canvas with halos |
| Now-Playing | Screenshot | 1200×700px | Glass UI, Camelot wheel, BPM |
| Smart Playlists | Screenshot | 1200×700px | Rule builder form |
| Settings / Themes | Screenshot | 1200×700px | Appearance settings, wallpaper choices |

**Source:** Capture from running app or create annotated mockups if app styling isn't finalized.

---

## Known Limitations to Surface

1. **Audio blend:** Works functionally; visual display/polish pending
2. **Duplicate detection:** Detection logic complete; UI not yet implemented
3. **WASAPI exclusive mode:** Planned for Windows hi-fi; not implemented
4. **Desktop app:** Tauri packaging planned; currently web-only

These should appear:
- In the Status/Known Issues block near the top
- As badges on relevant feature sections
- In the Roadmap with status indicators

---

## Tone & Voice

- **Portfolio angle:** Confident, feature-rich, impressive ("look at what this does")
- **Practical angle:** Clear, action-oriented ("here's how to get started")
- **Not:** Marketing fluff; stay technical and honest about WIP items

---

## Success Criteria

- [ ] Hero carousel immediately shows 5-6 key visuals
- [ ] Pitch is <3 sentences, clear value proposition
- [ ] Known issues are visible before Getting Started
- [ ] Each feature section has screenshot + description
- [ ] WIP features are badged clearly
- [ ] Roadmap shows status (✅ / 🚧 / 📋)
- [ ] Getting Started is unchanged but well-placed
- [ ] Architecture section preserved for contributors
- [ ] No placeholders or ambiguous sections

---

## Implementation Notes

- Use native Markdown for badges (`✅`, `🚧`, `📋`, `⚠️`)
- Screenshots can be embedded as PNG/GIF URLs or base64 if repo-local
- If using GIFs, optimize for file size (<500KB each)
- Test carousel on GitHub (check if it renders correctly)
- Verify all internal links still work (Features → Getting Started, etc.)
