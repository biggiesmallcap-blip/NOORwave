# NOORwave README v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transform README.md from text-dense to visual-first with hero carousel, feature showcases, WIP badges, and prominent known limitations.

**Architecture:** 
- Capture 6 screenshots from running app (Library, Genre Galaxy, Discovery Space, Now-Playing, Smart Playlists, Settings)
- Embed screenshots in Markdown using local image paths (`./docs/screenshots/` directory)
- Use HTML carousel or simple sequential image layout with navigation links (GitHub-native, no JavaScript required)
- Rewrite feature sections to pair each group with its screenshot and highlight key points with badges
- Update Roadmap to group by area (Playback, Discovery, Integration, Polish) with status badges (✅ 🚧 📋)
- Keep Stack, Getting Started, Architecture sections unchanged

**Tech Stack:** Markdown, PNG screenshots, native HTML for carousel layout

---

## File Structure

**Files to create:**
- `docs/screenshots/01-library.png` — Track table, sidebar, metadata panel
- `docs/screenshots/02-genre-galaxy.png` — 3D genre visualization
- `docs/screenshots/03-discovery-space.png` — Particle canvas with halos
- `docs/screenshots/04-now-playing.png` — Glass UI, Camelot wheel, BPM
- `docs/screenshots/05-smart-playlists.png` — Rule builder form
- `docs/screenshots/06-settings.png` — Appearance themes, wallpaper selector

**Files to modify:**
- `README.md` — Complete rewrite of structure and content

---

## Task 1: Capture Six Feature Screenshots

**Files:**
- Create: `docs/screenshots/01-library.png` through `06-settings.png`

- [ ] **Step 1: Start the app and navigate to Library view**

Run both backend and frontend:
```bash
# Terminal 1: Backend
cd noor-server && cargo run --release

# Terminal 2: Frontend  
cd frontend && npm run dev
```

Open `http://localhost:5173` in browser, ensure library is synced.

- [ ] **Step 2: Capture Library view screenshot**

Navigate to the Library page. Ensure visible:
- Track table with columns (title, artist, album, duration)
- Left sidebar with filters/search
- Right detail panel (if available, showing metadata)
- At least 5-10 tracks in the table

Screenshot dimensions: aim for 1200×700px (browser viewport ~1400px wide, scroll to fit)

Save as: `docs/screenshots/01-library.png`

- [ ] **Step 3: Capture Genre Galaxy screenshot**

Navigate to the Genre Galaxy page (from sidebar or main nav). Ensure visible:
- Full force-directed 3D visualization
- Nodes sized/colored by heat
- Legend or color coding visible
- At least one interior drill-down view (if possible, capture with genre expanded)

Screenshot dimensions: 1200×700px

Save as: `docs/screenshots/02-genre-galaxy.png`

- [ ] **Step 4: Capture Discovery Sound Space screenshot**

Navigate to Discovery / Sound Space (or Discover route). Ensure visible:
- Particle canvas with force-directed track layout
- Nebula halos (light circles marking explored regions)
- Search bar or query input visible
- Visual indication of track clusters/neighborhoods

Screenshot dimensions: 1200×700px

Save as: `docs/screenshots/03-discovery-space.png`

- [ ] **Step 5: Capture Now-Playing panel screenshot**

Start playback of any track. Capture the now-playing panel showing:
- Track title, artist, album art
- Camelot wheel (key visualization)
- BPM badge
- Glass UI aesthetic (semi-transparent, blurred background)
- Progress bar / playback controls

Screenshot dimensions: 1200×700px (or full-page if panel is sidebar/bottom)

Save as: `docs/screenshots/04-now-playing.png`

- [ ] **Step 6: Capture Smart Playlists builder screenshot**

Navigate to Smart Playlists or Playlist builder. Ensure visible:
- Rule creation form (AND/OR logic, conditions dropdowns)
- Available rules/filters (genre, BPM range, key, energy, instrumental, etc.)
- Current rules list (if any existing playlists)
- "Create" or "Apply" button

Screenshot dimensions: 1200×700px

Save as: `docs/screenshots/05-smart-playlists.png`

- [ ] **Step 7: Capture Settings / Appearance screenshot**

Navigate to Settings → Appearance (or equivalent). Ensure visible:
- Wallpaper selector (Aurora, Chrome, Grid, Nebula, Topo options)
- Theme / color picker (if available)
- Other appearance toggles
- Visual preview of current wallpaper in background

Screenshot dimensions: 1200×700px

Save as: `docs/screenshots/06-settings.png`

- [ ] **Step 8: Create docs/screenshots directory if it doesn't exist**

```bash
mkdir -p docs/screenshots
```

- [ ] **Step 9: Verify all 6 screenshots are saved and readable**

```bash
ls -lh docs/screenshots/
```

Expected output: 6 PNG files, each 50-500KB (depending on compression)

---

## Task 2: Rewrite README with Hero Carousel and Pitch

**Files:**
- Modify: `README.md` (entire file rewrite, keeping sections: Stack, Getting Started, Architecture, Notes)

- [ ] **Step 1: Preserve the old README**

```bash
cp README.md README.md.backup
```

- [ ] **Step 2: Write the new hero carousel and pitch section**

Replace the opening of README.md with:

```markdown
# NOOR

A power-user music library manager and hi-fi audio player built to fix TIDAL's limitations.

---

## Gallery

<div align="center">
  <table>
    <tr>
      <td align="center" width="50%">
        <strong>Library Management</strong><br>
        <img src="docs/screenshots/01-library.png" width="100%" alt="Library view with track table and metadata">
      </td>
      <td align="center" width="50%">
        <strong>Genre Galaxy</strong><br>
        <img src="docs/screenshots/02-genre-galaxy.png" width="100%" alt="3D genre visualization">
      </td>
    </tr>
    <tr>
      <td align="center" width="50%">
        <strong>Discovery Sound Space</strong><br>
        <img src="docs/screenshots/03-discovery-space.png" width="100%" alt="Particle canvas of similar tracks">
      </td>
      <td align="center" width="50%">
        <strong>Now-Playing</strong><br>
        <img src="docs/screenshots/04-now-playing.png" width="100%" alt="Glass UI with Camelot wheel and BPM">
      </td>
    </tr>
    <tr>
      <td align="center" width="50%">
        <strong>Smart Playlists</strong><br>
        <img src="docs/screenshots/05-smart-playlists.png" width="100%" alt="Rule-based playlist builder">
      </td>
      <td align="center" width="50%">
        <strong>Settings & Appearance</strong><br>
        <img src="docs/screenshots/06-settings.png" width="100%" alt="Shader wallpaper selector">
      </td>
    </tr>
  </table>
</div>

---

## What is NOOR?

NOOR syncs your entire TIDAL library locally and adds the features TIDAL doesn't: bulk operations, learned discovery, audio analysis, and a genre visualization galaxy. Hi-fi streaming, gapless playback, and harmonic mixing—all from your browser on LAN.

**[Get Started in 3 Minutes →](#getting-started)**

---

## Status & Known Limitations

⚠️ **Work in Progress:**
- **Audio blend:** Works functionally; visual display polish pending
- **Duplicate detection UI:** Detection logic complete; UI coming soon  
- **WASAPI exclusive mode (Windows):** Planned; not yet implemented

See [Roadmap](#roadmap) below for full feature status.

---
```

- [ ] **Step 3: Commit the hero/pitch changes**

```bash
git add README.md
git commit -m "refactor(docs): README hero section with visual gallery and pitch"
```

---

## Task 3: Rewrite Feature Sections (Library Management)

**Files:**
- Modify: `README.md` (Features section, Library subsection)

- [ ] **Step 1: Replace the Features / Library Management section**

Find and replace the current "**Library Management**" bullet list with:

```markdown
## Features

### Library Management

Sync your entire TIDAL collection locally with real-time progress. NOOR gives you bulk operations, full-text search (FTS5), and advanced filtering that TIDAL doesn't offer.

![Library view](docs/screenshots/01-library.png)

**Highlights:**
- Full library sync (tracks, albums, artists, playlists) with real-time progress
- Bulk operations: add/remove favorites, playlist management at scale
- Advanced sorting, filtering, and full-text search (FTS5)
- Track and album detail panels with full metadata (ISRC, play count, quality badges, date added)
- Artist pages: blurred-artwork hero, full TIDAL discography (Albums / Singles & EPs), in-library flags, out-of-library cards linking to TIDAL preview
- Album pages: blurred-artwork hero, track table with hover-reveal actions, equalizer bars on active row, "More by <artist>" shelf
- Tile and list view toggle with floating popout detail modals
- Duplicate detection via ISRC matching and title/duration fallback 🚧 *UI coming soon*
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "feat(docs): redesign Library Management feature section with screenshot"
```

---

## Task 4: Rewrite Feature Sections (Playback)

**Files:**
- Modify: `README.md` (Playback subsection)

- [ ] **Step 1: Replace the Playback section**

Find and replace the current "**Playback**" bullet list with:

```markdown
### Playback

Hi-fi lossless streaming from TIDAL with zero-gap gapless playback and harmonic mixing for seamless track transitions. Four shuffle modes including genre-spread to prevent repetitive sequences.

![Now-Playing panel](docs/screenshots/04-now-playing.png)

**Highlights:**
- Lossless hi-fi streaming via TIDAL with automatic token refresh
- Gapless playback with pre-buffer engine swap (zero-gap track transitions) and BPM-aligned crossfade snap
- Per-track fade-in / fade-out
- Volume control and seeking
- Four shuffle modes: off / true (Fisher-Yates) / weighted / genre-spread
  - Weighted: boosts favorites and never-played tracks, penalises recent plays
  - Genre-spread: prevents consecutive same-genre tracks
- Automix: automatic queue continuation with learning-based next-track selection; Camelot + BPM + energy harmonic multipliers; harmonic match indicators on queue rows
- Now-playing panel shows Camelot wheel key and BPM badge
- ⚠️ *Audio blend works; visual display polish pending*
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "feat(docs): redesign Playback feature section with screenshot"
```

---

## Task 5: Rewrite Feature Sections (Genre Galaxy)

**Files:**
- Modify: `README.md` (Genre Galaxy subsection)

- [ ] **Step 1: Replace the Genre Galaxy section**

Find and replace the current "**Genre Galaxy**" bullet list with:

```markdown
### Genre Galaxy

Explore your music taste as an interactive 3D visualization. Nodes represent genres sized by listen frequency; edges show co-occurrence. Click any genre to drill into artist clusters and discover patterns in your listening.

![Genre Galaxy visualization](docs/screenshots/02-genre-galaxy.png)

**Highlights:**
- Interactive 3D-style force-directed canvas showing your entire genre taxonomy
- Nodes sized and coloured by listen heat; edges show genre co-occurrence
- Click any genre to drill into its interior: artist cluster visualization, track list, and audio metrics
- **▶ Mix this genre**: loads tracks, shuffles the queue, and starts from a random entry point
- **Seed Mix Builder**: blend multiple genres, interleave their tracks, and play
- Four view modes: heat, co-occurrence, cohort, evolution
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "feat(docs): redesign Genre Galaxy feature section with screenshot"
```

---

## Task 6: Rewrite Feature Sections (Discovery Sound Space)

**Files:**
- Modify: `README.md` (Discovery Sound Space subsection)

- [ ] **Step 1: Replace the Discovery Sound Space section**

Find and replace the current "**Discovery Sound Space**" bullet list with:

```markdown
### Discovery Sound Space

Navigate a learned embedding space where tracks cluster by audio similarity. Search with mood or reference ("dark ambient", "140bpm drum & bass") to fly to matching regions. Nebula halos mark previously explored areas.

![Discovery Sound Space canvas](docs/screenshots/03-discovery-space.png)

**Highlights:**
- Force-directed canvas of tracks positioned by learned audio similarity
- Hyperspace search: type a mood or reference ("dark ambient", "140bpm drum & bass") and fly to a cluster of matching tracks
- Nebula halos mark previously explored regions
- Training animation: watch embeddings materialize as NOOR learns your library
- Playlist Builder: select nodes from the space and export as a playlist
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "feat(docs): redesign Discovery Sound Space feature section with screenshot"
```

---

## Task 7: Rewrite Feature Sections (Audio Analysis & Smart Playlists)

**Files:**
- Modify: `README.md` (Audio Analysis & Smart Playlists subsection)

- [ ] **Step 1: Replace the Audio Analysis section**

Find and replace the current "**Audio Analysis**" and **Smart Features** bullet lists with:

```markdown
### Audio Analysis & Smart Playlists

Real-time DSP extracts BPM, key, energy, danceability, and spectral data as you play. Build smart playlists with AND/OR rules across 9+ audio dimensions. ACRCloud fingerprinting identifies samples and covers automatically.

![Smart Playlists builder](docs/screenshots/05-smart-playlists.png)

**Highlights:**
- DSP feature extraction running passively during playback: BPM, key signature, Camelot wheel, loudness (LUFS), energy, danceability, beat strength, spectral centroid, stereo width
- Instrumental detection; BPM/key confidence gating; LUFS bilinear-transform biquad rescaling for non-48 kHz sources
- Per-genre audio metric summaries (average BPM, energy, danceability)
- ACRCloud integration: fingerprint-based sample/cover recognition with configurable daily scan limit; 429 backoff gate; sparse fingerprint skip
- Rule-based smart playlists with AND/OR logic — genre, artist, date range, quality tier, play count, BPM range, key signature, Camelot key, energy range, danceability range, instrumental-only, has-sample-data
- Genre taxonomy browser with 291 genres across hierarchy
- MusicBrainz enrichment (ISRC-first + title fallback, rate-limited)
- Discovery: prompt → genre inference → cross-service recommendation seeds
- Analytics: listen history, top tracks/artists, genre heatmap, activity graph, completion rate
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "feat(docs): redesign Audio Analysis & Smart Playlists section with screenshot"
```

---

## Task 8: Rewrite Feature Sections (UI & Access)

**Files:**
- Modify: `README.md` (UI & Access subsection)

- [ ] **Step 1: Replace the UI & Access section**

Find and replace the current "**UI & Access**" and "**Multi-Service Integration**" bullet lists with:

```markdown
### UI & Access

6-digit PIN authentication. Auto-setup for local browsers; remote devices just enter the PIN. Glass-tile design with shader wallpapers (Aurora, Chrome, Grid, Nebula, Topo). Accessible on any browser on your LAN.

![Settings and appearance](docs/screenshots/06-settings.png)

**Highlights:**
- 6-digit PIN auth: numeric PIN-pad connect modal (auto-submit, numeric keyboard on mobile); existing tokens remain valid until regenerated
- Auto-setup: the browser on the server machine connects automatically; remote devices get the PIN modal
- WebSocket-driven: playback state, sync progress, queue updates, training progress push instantly to the browser
- Shader wallpapers (Aurora, Chrome, Grid, Nebula, Topo) as a fixed background layer; sidebar and now-playing panel glass over the active wallpaper
- Shared context menu (right-click or ··· button) with song radio, play/shuffle album, artist radio, go-to artist/album, favourite toggle
- Global keyboard shortcuts: Space (play/pause), ← / → (seek), ↑ / ↓ (volume), L (like), S (shuffle), R (repeat)
- Settings organised into left-rail categories: Appearance / Account / Sources / Discovery / Audio / Data
- Accessible on LAN — run on one machine, open from any browser on the network
- **Multi-service:** TIDAL (full sync & playback), Spotify (auth & genre enrichment), RSS aggregation (AllMusic, Billboard, NME, SPIN, Pitchfork, Rolling Stone, Consequence, The Guardian)
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "feat(docs): redesign UI & Access feature section with screenshot"
```

---

## Task 9: Update Roadmap with Status Badges

**Files:**
- Modify: `README.md` (Roadmap section)

- [ ] **Step 1: Replace the Roadmap section**

Find and replace the existing "## Roadmap" section with:

```markdown
## Roadmap

### Playback
- ✅ Gapless playback (zero-gap engine swap + BPM-aligned crossfade snap)
- ✅ Per-track fade-in / fade-out
- 🚧 Gapless crossfade audio blend (pre-buffer swap works; audio mixing pending)
- 🚧 WASAPI exclusive mode (Windows hi-fi priority)

### Discovery & Learning
- ✅ Embedding-based learning pipeline (incremental refresh & full retrain)
- ✅ Similar Radio with creativity & context controls
- ✅ Genre Galaxy (heat, co-occurrence, cohort, evolution views)
- ✅ Discovery Sound Space with hyperspace search & nebula halos
- ✅ Playlist Builder from discovery canvas

### Integration & Sources
- ✅ TIDAL full sync & playback with lossless support
- ✅ Spotify auth & genre enrichment
- ✅ RSS aggregation (AllMusic, Billboard, NME, SPIN, etc.)
- ✅ Audio feature analysis (DSP + ACRCloud)
- 📋 YouTube Music integration
- 📋 SoundCloud integration

### UI & Polish
- ✅ Glass-tile design with shader wallpapers (Aurora, Chrome, Grid, Nebula, Topo)
- ✅ 6-digit PIN auth with auto-setup
- ✅ Duplicate detection via ISRC + fingerprint
- 🚧 Duplicate detection UI (detection complete)
- 🚧 Smart playlist UI refinements
- 📋 Desktop app (Tauri packaging)

### Data & Analytics
- ✅ Listen history & session tracking
- ✅ Top tracks / artists, genre heatmap, activity graph
- ✅ Completion rate, average listen duration, skip patterns
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "feat(docs): update Roadmap with status badges (✅ 🚧 📋)"
```

---

## Task 10: Verify Sections Are Intact

**Files:**
- Verify: `README.md` (Stack, Getting Started, Architecture, Notes sections)

- [ ] **Step 1: Verify Stack table is present and unchanged**

Ensure the "## Stack" section and the table with Rust, SQLite, SvelteKit, Symphonia, CPAL, Tokio, RSS are still there.

Expected output (grep):
```bash
grep -A 8 "^## Stack" README.md | head -15
```

Should output the stack table.

- [ ] **Step 2: Verify Getting Started section is present**

```bash
grep -n "^## Getting Started" README.md
```

Should find the section near line 200-250.

- [ ] **Step 3: Verify Architecture section is present**

```bash
grep -n "^## Architecture" README.md
```

Should find the section with the ASCII file tree.

- [ ] **Step 4: Verify Notes and License sections are present**

```bash
grep -n "^## Notes\|^## License" README.md
```

Should find both sections.

- [ ] **Step 5: Commit a "verification complete" commit if all sections present**

```bash
git add README.md
git commit -m "chore(docs): verify all retained sections (Stack, Getting Started, Architecture, Notes, License)"
```

---

## Task 11: Test Links and Rendering

**Files:**
- Test: `README.md` (local rendering + GitHub preview)

- [ ] **Step 1: Verify internal links in README**

Check that the "Get Started in 3 Minutes →" link points to `#getting-started` (the Getting Started header).

```bash
grep -n "^## Getting Started" README.md
```

The ID should be `#getting-started` (GitHub auto-generates from header text).

- [ ] **Step 2: Verify all image paths are correct**

```bash
grep "docs/screenshots/" README.md
```

Expected output: 6 lines, each with a path like `docs/screenshots/01-library.png`.

- [ ] **Step 3: Verify images exist on disk**

```bash
ls -lh docs/screenshots/*.png
```

Expected: 6 PNG files listed.

- [ ] **Step 4: Check README.md syntax for any Markdown errors**

Open README.md in a Markdown preview tool (VS Code, Typora, or GitHub web editor preview). Look for:
- Broken headings (should see a table of contents)
- Missing image alt text (all images should have `![...]` descriptions)
- Broken links (any red underlines in a preview)
- Malformed HTML table (carousel gallery)

- [ ] **Step 5: Push to GitHub and view in browser**

```bash
git push origin feat/tidal-audio-settings
```

Navigate to `https://github.com/biggiesmallcap-blip/NOORwave/blob/feat/tidal-audio-settings/README.md`

Verify:
- Hero gallery images load (6 screenshots visible in 2×3 grid)
- Pitch copy is readable below gallery
- Status/Known Limitations block is visible
- All feature sections have images embedded
- Roadmap section renders with badges (✅ 🚧 📋)
- Getting Started section is clickable from the pitch link

- [ ] **Step 6: Commit final verification**

```bash
git add README.md
git commit -m "docs: README v2 complete with hero gallery, feature screenshots, and status badges"
```

---

## Task 12: Final Cleanup and Commit

**Files:**
- Cleanup: Remove backup file if all tests passed

- [ ] **Step 1: Remove the README.md.backup file**

```bash
rm README.md.backup
```

- [ ] **Step 2: Final status check**

```bash
git status
```

Expected: Nothing to commit; working tree clean.

- [ ] **Step 3: Review commit history**

```bash
git log --oneline HEAD~8..HEAD
```

Expected: 8 commits (hero, library, playback, galaxy, space, audio/smart, ui/access, roadmap, verification, final).

- [ ] **Step 4: Done**

README v2 is complete, pushed to GitHub, and verified.
```

---

## Self-Review Against Spec

**Spec coverage:**
- ✅ Hero section with carousel (Task 2: 2×3 grid of 6 screenshots)
- ✅ Pitch copy (2-3 sentences in Task 2)
- ✅ Quick Start CTA (Task 2: "Get Started in 3 Minutes →")
- ✅ Status/Known Issues block (Task 2: early visibility)
- ✅ Features section (6 groups: Tasks 3-8, each with screenshot + description)
- ✅ WIP badges (🚧 and ⚠️ used throughout Tasks 3-8)
- ✅ Stack table (Task 10: verified unchanged)
- ✅ Getting Started (Task 10: verified unchanged)
- ✅ Architecture (Task 10: verified unchanged)
- ✅ Roadmap with status badges (Task 9: ✅ 🚧 📋)
- ✅ Notes and License (Task 10: verified unchanged)

**Placeholder scan:**
- No "TBD", "TODO", placeholders found
- All steps contain complete code/instructions
- All image paths specified exactly: `docs/screenshots/01-*.png` through `06-*.png`

**Type consistency:**
- All markdown headers use `##` or `###` consistently
- All image embeds use `![alt text](path)` format
- All badges use emoji: ✅ Done, 🚧 In Progress, 📋 Planned, ⚠️ Warning

**Scope check:**
- Task 1 (screenshots): 9 discrete steps, clear success criteria
- Task 2 (hero): Clear HTML table layout for carousel
- Tasks 3-8 (features): Repeated pattern, each well-defined
- Task 9 (roadmap): Clear grouping by area with badges
- Task 10 (verification): Quick validation steps
- Task 11 (testing): Local + GitHub preview checks
- Task 12 (cleanup): Final commit

**All success criteria covered:**
- Hero carousel ✅
- Pitch <3 sentences ✅
- Known issues early ✅
- Feature sections with screenshots ✅
- WIP badges ✅
- Roadmap with status ✅
- Getting Started preserved ✅
- Architecture preserved ✅
- No placeholders ✅

---

## Plan Complete

All 12 tasks defined with concrete steps, code snippets, and test criteria. Ready for execution.
