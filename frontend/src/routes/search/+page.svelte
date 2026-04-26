<script lang="ts">
  import { goto } from '$app/navigation'
  import { api, type TidalSearchResults, type TidalSearchAlbum, type TidalSearchArtist, type TidalSearchTrack } from '$lib/api/client'
  import { buildTidalTrackMenu } from '$lib/player/track_menu'
  import { openContextMenu, type MenuItem } from '$lib/stores/context_menu'
  import { playTidalTrackNow, playTidalAlbum, playTidalTrackNext, addTidalTrackToQueue, startTidalSongRadio } from '$lib/stores/player'
  import { formatDuration } from '$lib/stores/library'

  const RECENT_KEY = 'noor_recent_searches'
  const RECENT_MAX = 8

  function loadRecent(): string[] {
    if (typeof localStorage === 'undefined') return []
    try {
      const raw = localStorage.getItem(RECENT_KEY)
      if (!raw) return []
      const parsed = JSON.parse(raw)
      return Array.isArray(parsed) ? parsed.filter((v) => typeof v === 'string').slice(0, RECENT_MAX) : []
    } catch {
      return []
    }
  }

  function pushRecent(q: string) {
    if (!q.trim() || typeof localStorage === 'undefined') return
    const next = [q, ...recent.filter((r) => r.toLowerCase() !== q.toLowerCase())].slice(0, RECENT_MAX)
    recent = next
    localStorage.setItem(RECENT_KEY, JSON.stringify(next))
  }

  function clearRecent() {
    recent = []
    if (typeof localStorage !== 'undefined') localStorage.removeItem(RECENT_KEY)
  }

  let query = $state('')
  let results = $state<TidalSearchResults | null>(null)
  let loading = $state(false)
  let error = $state<string | null>(null)
  let debounceTimer: ReturnType<typeof setTimeout>
  let recent = $state<string[]>(loadRecent())

  function onInput() {
    clearTimeout(debounceTimer)
    if (!query.trim()) {
      results = null
      loading = false
      error = null
      return
    }
    loading = true
    debounceTimer = setTimeout(async () => {
      const q = query.trim()
      try {
        results = await api.searchTidal(q)
        error = null
        pushRecent(q)
      } catch (e) {
        error = String(e)
      } finally {
        loading = false
      }
    }, 300)
  }

  function pickRecent(q: string) {
    query = q
    onInput()
    inputEl?.focus()
  }

  const isEmpty = $derived(
    results !== null &&
    results.tracks.length === 0 &&
    results.albums.length === 0 &&
    results.artists.length === 0
  )

  // Library entries float to the top of each section. Stable sort preserves
  // Tidal's order within each group so the user sees expected ranking otherwise.
  const sortedArtists = $derived(
    results ? [...results.artists].sort((a, b) => Number(b.in_library) - Number(a.in_library)) : []
  )
  const sortedAlbums = $derived(
    results ? [...results.albums].sort((a, b) => Number(b.in_library) - Number(a.in_library)) : []
  )
  const sortedTracks = $derived(
    results ? [...results.tracks].sort((a, b) => Number(b.in_library) - Number(a.in_library)) : []
  )

  type TopResult =
    | { kind: 'artist'; entry: TidalSearchArtist }
    | { kind: 'album'; entry: TidalSearchAlbum }
    | { kind: 'track'; entry: TidalSearchTrack }

  // Score each candidate, prefer artist > album > track on ties, exact-name match
  // wins, library entries get a +0.3 boost. The first place across all three
  // sections is the hero.
  const topResult = $derived.by<TopResult | null>(() => {
    if (!results || !query.trim()) return null
    const q = query.trim().toLowerCase()
    const score = (name: string, inLibrary: boolean, kindBias: number) => {
      const n = name.toLowerCase()
      let s = 0
      if (n === q) s += 1.0
      else if (n.startsWith(q)) s += 0.6
      else if (n.includes(q)) s += 0.3
      if (inLibrary) s += 0.3
      return s + kindBias
    }
    const candidates: { tr: TopResult; s: number }[] = []
    if (sortedArtists[0]) candidates.push({ tr: { kind: 'artist', entry: sortedArtists[0] }, s: score(sortedArtists[0].name, sortedArtists[0].in_library, 0.05) })
    if (sortedAlbums[0]) candidates.push({ tr: { kind: 'album', entry: sortedAlbums[0] }, s: score(sortedAlbums[0].title, sortedAlbums[0].in_library, 0.025) })
    if (sortedTracks[0]) candidates.push({ tr: { kind: 'track', entry: sortedTracks[0] }, s: score(sortedTracks[0].title, sortedTracks[0].in_library, 0) })
    if (candidates.length === 0) return null
    candidates.sort((a, b) => b.s - a.s)
    return candidates[0].tr
  })

  function topResultHref(top: TopResult): string {
    switch (top.kind) {
      case 'artist':
        return top.entry.in_library && top.entry.local_id != null
          ? `/artists/${top.entry.local_id}`
          : `/tidal/artists/${top.entry.tidal_id}`
      case 'album':
        return top.entry.in_library && top.entry.local_id != null
          ? `/albums/${top.entry.local_id}`
          : `/tidal/albums/${top.entry.tidal_id}`
      case 'track':
        return top.entry.album_title
          ? `/tidal/artists/${top.entry.artist_id ?? 0}`
          : '#'
    }
  }

  function topResultPlay(top: TopResult) {
    if (top.kind === 'track') {
      void playTidalTrackNow(top.entry)
    } else if (top.kind === 'album') {
      void playTidalAlbum(top.entry.tidal_id)
    } else {
      void goto(topResultHref(top))
    }
  }

  // Stable per-name color so empty-artwork avatars feel intentional, not broken.
  function letterColor(name: string): string {
    let hash = 0
    for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) | 0
    const hue = Math.abs(hash) % 360
    return `hsl(${hue}, 38%, 28%)`
  }

  function initials(name: string): string {
    const parts = name.trim().split(/\s+/).slice(0, 2)
    return parts.map((p) => p[0]?.toUpperCase() ?? '').join('') || '?'
  }

  // Translate vertical wheel into horizontal scroll for the artist/album rows.
  function wheelToHorizontal(node: HTMLElement) {
    const onWheel = (e: WheelEvent) => {
      if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return
      e.preventDefault()
      node.scrollLeft += e.deltaY
    }
    node.addEventListener('wheel', onWheel, { passive: false })
    return { destroy: () => node.removeEventListener('wheel', onWheel) }
  }

  function buildAlbumMenu(album: TidalSearchAlbum): MenuItem[] {
    return [
      { label: 'Play album', icon: '▶', onSelect: () => void playTidalAlbum(album.tidal_id) },
      { separator: true, label: '' },
      { label: 'Open album', icon: '→', onSelect: () => void goto(`/tidal/albums/${album.tidal_id}`) },
    ]
  }

  function buildArtistMenu(artist: TidalSearchArtist): MenuItem[] {
    const href = artist.in_library && artist.local_id != null
      ? `/artists/${artist.local_id}`
      : `/tidal/artists/${artist.tidal_id}`
    return [
      { label: artist.in_library ? 'Open in library' : 'Open artist', icon: '→', onSelect: () => void goto(href) },
    ]
  }

  // ─── Keyboard navigation ────────────────────────────────────────────────
  // `/` from anywhere on the page focuses the input (skips when typing
  // elsewhere). Once in the input, `Enter` plays — modifiers branch to
  // queue/play-next. Arrow keys move a row cursor through the visible tracks.
  let inputEl: HTMLInputElement | null = $state(null)
  let cursor = $state(-1)

  // Reset cursor whenever the result set changes shape so we never point past the end.
  $effect(() => {
    if (cursor >= sortedTracks.length) cursor = sortedTracks.length - 1
  })

  function actOnTrack(track: TidalSearchTrack, mode: 'play' | 'queue' | 'next') {
    if (mode === 'queue') void addTidalTrackToQueue(track)
    else if (mode === 'next') void playTidalTrackNext(track)
    else void playTidalTrackNow(track)
  }

  function inputKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      if (query) {
        query = ''
        results = null
        cursor = -1
      } else {
        inputEl?.blur()
      }
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      if (sortedTracks.length === 0) return
      cursor = cursor < 0 ? 0 : Math.min(cursor + 1, sortedTracks.length - 1)
      return
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      if (sortedTracks.length === 0) return
      cursor = cursor <= 0 ? -1 : cursor - 1
      return
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      const mode: 'play' | 'queue' | 'next' = e.shiftKey ? 'queue' : (e.metaKey || e.ctrlKey) ? 'next' : 'play'
      const target = cursor >= 0 ? sortedTracks[cursor] : null
      if (target) {
        actOnTrack(target, mode)
      } else if (topResult) {
        if (topResult.kind === 'track') actOnTrack(topResult.entry, mode)
        else topResultPlay(topResult)
      }
      return
    }
  }

  function globalKeydown(e: KeyboardEvent) {
    if (e.key !== '/' || e.metaKey || e.ctrlKey || e.altKey) return
    const t = e.target as HTMLElement | null
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
    e.preventDefault()
    inputEl?.focus()
    inputEl?.select()
  }

  $effect(() => {
    window.addEventListener('keydown', globalKeydown)
    return () => window.removeEventListener('keydown', globalKeydown)
  })

  // Keep the highlighted track in view as the cursor moves.
  $effect(() => {
    if (cursor < 0) return
    const el = document.querySelector<HTMLElement>(`.track-row[data-cursor-idx="${cursor}"]`)
    el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  })
</script>

<div class="search-page">
  <div class="search-header">
    <input
      bind:this={inputEl}
      class="search-input"
      type="text"
      placeholder="Search Tidal's full catalogue"
      bind:value={query}
      oninput={onInput}
      onkeydown={inputKeydown}
      autofocus
    />
    <p class="kbd-hint">
      <kbd>/</kbd> focus &nbsp;·&nbsp;
      <kbd>↑</kbd><kbd>↓</kbd> move &nbsp;·&nbsp;
      <kbd>Enter</kbd> play &nbsp;·&nbsp;
      <kbd>Shift</kbd>+<kbd>Enter</kbd> queue &nbsp;·&nbsp;
      <kbd>Ctrl</kbd>+<kbd>Enter</kbd> next
    </p>
  </div>

  {#if !query.trim()}
    {#if recent.length > 0}
      <section class="recent-section">
        <div class="recent-head">
          <h3 class="section-label">Recent</h3>
          <button class="recent-clear" onclick={clearRecent}>Clear</button>
        </div>
        <div class="recent-chips">
          {#each recent as q (q)}
            <button class="recent-chip" onclick={() => pickRecent(q)}>{q}</button>
          {/each}
        </div>
      </section>
    {:else}
      <p class="search-hint">Start typing to search Tidal's full catalogue</p>
    {/if}
  {:else if loading}
    <p class="search-hint">Searching…</p>
  {:else if error}
    <p class="search-hint search-error">{error}</p>
  {:else if isEmpty}
    <p class="search-hint">No results for "{query}"</p>
  {:else if results}

    {#if topResult}
      {@const top = topResult}
      <section class="top-result-section">
        <h3 class="section-label">Top Result</h3>
        <a class="top-result-card" class:in-library={top.entry.in_library} href={topResultHref(top)}>
          {#if top.kind === 'artist'}
            {#if top.entry.artwork_url}
              <div class="top-art top-art--circle" style={`background-image: url('${top.entry.artwork_url}')`}></div>
            {:else}
              <div class="top-art top-art--circle fallback" style={`background: ${letterColor(top.entry.name)}`}>
                <span>{initials(top.entry.name)}</span>
              </div>
            {/if}
          {:else if top.entry.artwork_url}
            <div class="top-art" style={`background-image: url('${top.entry.artwork_url}')`}></div>
          {:else}
            <div class="top-art fallback" style={`background: ${letterColor(top.kind === 'album' ? top.entry.title : top.entry.title)}`}>
              <span>♫</span>
            </div>
          {/if}
          <div class="top-meta">
            <span class="top-kind">{top.kind === 'artist' ? 'Artist' : top.kind === 'album' ? 'Album' : 'Track'}{#if top.entry.in_library} · In your library{/if}</span>
            <h2 class="top-title">
              {top.kind === 'artist' ? top.entry.name : top.entry.title}
            </h2>
            {#if top.kind === 'album' && top.entry.artist_name}
              <p class="top-sub">{top.entry.artist_name}</p>
            {:else if top.kind === 'track' && top.entry.artist_name}
              <p class="top-sub">{top.entry.artist_name}</p>
            {/if}
            <button
              class="top-play-btn"
              onclick={(e) => { e.preventDefault(); e.stopPropagation(); topResultPlay(top) }}
            >▶ {top.kind === 'artist' ? 'Open' : 'Play'}</button>
          </div>
        </a>
      </section>
    {/if}

    {#if sortedArtists.length > 0}
      <section class="results-section">
        <h3 class="section-label">Artists</h3>
        <div class="artists-row" use:wheelToHorizontal>
          {#each sortedArtists as artist (artist.tidal_id)}
            <a
              class="artist-card"
              class:in-library={artist.in_library}
              href={artist.in_library && artist.local_id != null
                ? `/artists/${artist.local_id}`
                : `/tidal/artists/${artist.tidal_id}`}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildArtistMenu(artist)) }}
            >
              <div class="avatar-wrap">
                {#if artist.artwork_url}
                  <div class="artist-avatar" style={`background-image: url('${artist.artwork_url}')`}></div>
                {:else}
                  <div class="artist-avatar fallback" style={`background: ${letterColor(artist.name)}`}>
                    <span>{initials(artist.name)}</span>
                  </div>
                {/if}
                {#if artist.in_library}
                  <span class="lib-badge" title="In your library">✓</span>
                {/if}
              </div>
              <span class="artist-name">{artist.name}</span>
            </a>
          {/each}
        </div>
      </section>
    {/if}

    {#if sortedAlbums.length > 0}
      <section class="results-section">
        <h3 class="section-label">Albums</h3>
        <div class="albums-row" use:wheelToHorizontal>
          {#each sortedAlbums as album (album.tidal_id)}
            <a
              class="album-card"
              class:in-library={album.in_library}
              href={album.in_library && album.local_id != null
                ? `/albums/${album.local_id}`
                : `/tidal/albums/${album.tidal_id}`}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildAlbumMenu(album)) }}
            >
              <div class="art-wrap">
                {#if album.artwork_url}
                  <div class="album-art" style={`background-image: url('${album.artwork_url}')`}></div>
                {:else}
                  <div class="album-art fallback" style={`background: ${letterColor(album.title)}`}>
                    <span>♫</span>
                  </div>
                {/if}
                {#if album.in_library}
                  <span class="lib-badge" title="In your library">✓</span>
                {/if}
                <button
                  class="art-play-overlay"
                  onclick={(e) => { e.preventDefault(); e.stopPropagation(); void playTidalAlbum(album.tidal_id) }}
                  aria-label="Play {album.title}"
                >▶</button>
              </div>
              <p class="album-title">{album.title}</p>
              {#if album.artist_name}
                <p class="album-artist">{album.artist_name}</p>
              {/if}
            </a>
          {/each}
        </div>
      </section>
    {/if}

    {#if sortedTracks.length > 0}
      <section class="results-section">
        <h3 class="section-label">Tracks</h3>
        <ul class="tracks-list">
          {#each sortedTracks as track, idx (track.tidal_id)}
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <li
              class="track-row"
              class:cursor={cursor === idx}
              data-cursor-idx={idx}
              role="button"
              tabindex="0"
              onclick={() => void playTidalTrackNow(track)}
              onmouseenter={() => { cursor = idx }}
              onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), void playTidalTrackNow(track))}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTidalTrackMenu(track)) }}
            >
              {#if track.artwork_url}
                <div class="track-art" style={`background-image: url('${track.artwork_url}')`}></div>
              {:else}
                <div class="track-art fallback" style={`background: ${letterColor(track.title)}`}>
                  <span>♫</span>
                </div>
              {/if}
              <div class="track-meta">
                <p class="track-title">
                  {track.title}
                  {#if track.in_library}<span class="lib-dot" title="In your library">✓</span>{/if}
                </p>
                <p class="track-subtitle">
                  {#if track.artist_name}{track.artist_name}{/if}
                  {#if track.artist_name && track.album_title} — {/if}
                  {#if track.album_title}{track.album_title}{/if}
                </p>
              </div>
              <span class="track-duration">{formatDuration(track.duration_ms)}</span>
              <div class="row-actions">
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); void playTidalTrackNow(track) }}
                  title="Play now"
                  aria-label="Play {track.title}"
                >▶</button>
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); void addTidalTrackToQueue(track) }}
                  title="Add to queue"
                  aria-label="Queue {track.title}"
                >＋</button>
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); void startTidalSongRadio(track) }}
                  title="Start song radio"
                  aria-label="Start radio from {track.title}"
                >◎</button>
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); openContextMenu(e, buildTidalTrackMenu(track)) }}
                  title="More options"
                  aria-label="More options"
                >⋯</button>
              </div>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

  {/if}
</div>

<style>
  .search-page {
    padding: 40px 48px 80px;
    max-width: 1180px;
  }
  .search-header {
    margin-bottom: 40px;
  }
  .search-input {
    width: 100%;
    max-width: 640px;
    background: var(--bg-raised);
    border: 1px solid var(--border-strong);
    border-radius: 24px;
    padding: 12px 22px;
    font-size: 15px;
    color: var(--text-primary);
    outline: none;
    transition: border-color 0.15s, background 0.15s;
  }
  .search-input::placeholder { color: var(--text-tertiary); }
  .kbd-hint {
    margin: 10px 0 0;
    font-size: 11px;
    color: var(--text-tertiary);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px;
  }
  .kbd-hint kbd {
    display: inline-block;
    padding: 1px 5px;
    background: var(--bg-raised);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 10px;
    color: var(--text-secondary);
    line-height: 1.4;
    margin: 0 1px;
  }
  .search-input:focus {
    border-color: var(--accent);
    background: var(--bg-elevated);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .search-hint {
    color: var(--text-muted);
    font-size: 14px;
    margin-top: 64px;
    text-align: center;
  }
  .search-error { color: var(--state-error); }
  .results-section { margin-bottom: 40px; }
  .recent-section { margin-top: 36px; max-width: 720px; }
  .recent-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }
  .recent-clear {
    background: none;
    border: none;
    color: var(--text-tertiary);
    font-size: 11px;
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 4px;
  }
  .recent-clear:hover { color: var(--text-secondary); background: var(--bg-hover); }
  .recent-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .recent-chip {
    background: var(--bg-raised);
    border: 1px solid var(--border-subtle);
    border-radius: 14px;
    padding: 6px 14px;
    font-size: 12px;
    color: var(--text-secondary);
    cursor: pointer;
    font-family: inherit;
    transition: border-color 0.15s, color 0.15s;
  }
  .recent-chip:hover {
    border-color: var(--accent-line);
    color: var(--text-primary);
  }
  .top-result-section { margin-bottom: 32px; }
  .top-result-card {
    display: grid;
    grid-template-columns: 168px 1fr;
    gap: 24px;
    padding: 20px;
    border-radius: 12px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    text-decoration: none;
    transition: transform 0.18s ease, border-color 0.18s, background 0.18s;
    align-items: center;
  }
  .top-result-card:hover {
    transform: translateY(-2px);
    border-color: var(--accent-line);
    background: var(--bg-raised);
  }
  .top-result-card.in-library { border-color: var(--accent-line); }
  .top-art {
    width: 168px;
    height: 168px;
    border-radius: 8px;
    background-size: cover;
    background-position: center;
    background-color: var(--bg-raised);
    display: flex;
    align-items: center;
    justify-content: center;
    box-shadow: 0 12px 28px -12px rgba(0,0,0,0.6);
  }
  .top-art--circle { border-radius: 50%; }
  .top-art.fallback span {
    font-size: 56px;
    color: rgba(255,255,255,0.55);
    font-weight: 600;
  }
  .top-art--circle.fallback span { font-size: 40px; }
  .top-meta {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .top-kind {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 1.5px;
    color: var(--text-tertiary);
    font-weight: 600;
  }
  .top-title {
    font-family: var(--font-display, inherit);
    font-size: clamp(28px, 3.6vw, 44px);
    line-height: 1.05;
    margin: 0;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .top-sub {
    font-size: 14px;
    color: var(--text-secondary);
    margin: 0;
  }
  .top-play-btn {
    align-self: flex-start;
    margin-top: 10px;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 22px;
    padding: 9px 22px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s, transform 0.15s;
  }
  .top-play-btn:hover { opacity: 0.9; transform: scale(1.03); }
  .section-label {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 1.5px;
    color: var(--accent);
    margin-bottom: 14px;
  }
  /* Artists */
  .artists-row {
    display: flex;
    gap: 20px;
    overflow-x: auto;
    padding-bottom: 8px;
    scrollbar-width: none;
  }
  .artists-row::-webkit-scrollbar { display: none; }
  .artist-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    text-decoration: none;
    flex-shrink: 0;
    width: 84px;
    transition: transform 0.18s ease;
  }
  .artist-card:hover { transform: translateY(-3px); }
  .artist-card:hover .artist-avatar {
    opacity: 0.85;
  }
  .avatar-wrap, .art-wrap {
    position: relative;
    line-height: 0;
  }
  .lib-badge {
    position: absolute;
    top: -2px;
    right: -2px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    font-size: 11px;
    font-weight: 700;
    display: flex;
    align-items: center;
    justify-content: center;
    box-shadow: 0 0 0 2px var(--bg-base);
    line-height: 1;
  }
  .lib-dot {
    color: var(--accent);
    font-size: 10px;
    margin-left: 6px;
    vertical-align: middle;
  }
  .artist-card.in-library .artist-name {
    color: var(--text-primary);
    font-weight: 600;
  }
  .album-card.in-library .album-title {
    color: var(--text-primary);
    font-weight: 600;
  }
  .artist-avatar {
    width: 72px;
    height: 72px;
    border-radius: 50%;
    background: var(--bg-raised);
    background-size: cover;
    background-position: center;
    transition: opacity 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .artist-avatar.fallback span {
    font-family: var(--font-display, inherit);
    font-size: 22px;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.78);
    letter-spacing: 0.02em;
  }
  .artist-name {
    font-size: 11px;
    color: var(--text-secondary);
    text-align: center;
    width: 84px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Albums */
  .albums-row {
    display: flex;
    gap: 16px;
    overflow-x: auto;
    padding-bottom: 8px;
    scrollbar-width: none;
  }
  .albums-row::-webkit-scrollbar { display: none; }
  .album-card {
    text-decoration: none;
    flex-shrink: 0;
    width: 128px;
    transition: transform 0.18s ease;
  }
  .album-card:hover { transform: translateY(-3px); }
  .art-play-overlay {
    position: absolute;
    bottom: 8px;
    right: 8px;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    font-size: 14px;
    border: none;
    cursor: pointer;
    opacity: 0;
    transform: translateY(4px);
    transition: opacity 0.15s, transform 0.15s;
    box-shadow: 0 6px 16px -4px rgba(0,0,0,0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
  }
  .art-wrap:hover .art-play-overlay {
    opacity: 1;
    transform: translateY(0);
  }
  .art-play-overlay:hover { transform: scale(1.08); opacity: 1; }
  .album-art {
    width: 128px;
    height: 128px;
    border-radius: 6px;
    background: var(--bg-raised);
    background-size: cover;
    background-position: center;
    margin-bottom: 7px;
    transition: opacity 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .album-art.fallback span {
    font-size: 38px;
    color: rgba(255, 255, 255, 0.5);
  }
  .album-card:hover .album-art { opacity: 0.85; }
  .album-title {
    font-size: 12px;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 0;
  }
  .album-artist {
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 2px 0 0;
  }
  /* Tracks */
  .tracks-list {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .track-row {
    display: grid;
    grid-template-columns: 38px 1fr auto auto;
    align-items: center;
    gap: 12px;
    padding: 8px 8px;
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.12s;
  }
  .row-actions {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .track-row:hover { background: var(--bg-hover); }
  .track-row.cursor {
    background: var(--bg-hover);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .track-art {
    width: 36px;
    height: 36px;
    border-radius: 4px;
    background: var(--bg-raised);
    background-size: cover;
    background-position: center;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .track-art.fallback span {
    font-size: 16px;
    color: rgba(255, 255, 255, 0.5);
  }
  .track-title {
    font-size: 13px;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 0;
  }
  .track-subtitle {
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 2px 0 0;
  }
  .track-duration {
    font-size: 12px;
    color: var(--text-muted);
    white-space: nowrap;
  }
  .row-btn {
    background: none;
    border: none;
    color: var(--text-tertiary);
    cursor: pointer;
    font-size: 14px;
    padding: 4px;
    border-radius: 4px;
    opacity: 0;
    transition: opacity 0.1s, color 0.1s;
  }
  .track-row:hover .row-btn { opacity: 1; }
  .row-btn:hover { color: var(--text-primary); }
</style>
