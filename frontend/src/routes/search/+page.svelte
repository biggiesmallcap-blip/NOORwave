<script lang="ts">
  import { goto } from '$app/navigation'
  import { api, type TidalSearchResults, type TidalSearchAlbum, type TidalSearchArtist } from '$lib/api/client'
  import { buildTidalTrackMenu } from '$lib/player/track_menu'
  import { openContextMenu, type MenuItem } from '$lib/stores/context_menu'
  import { playTidalTrackNow, playTidalAlbum } from '$lib/stores/player'
  import { formatDuration } from '$lib/stores/library'

  let query = $state('')
  let results = $state<TidalSearchResults | null>(null)
  let loading = $state(false)
  let error = $state<string | null>(null)
  let debounceTimer: ReturnType<typeof setTimeout>

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
      try {
        results = await api.searchTidal(query.trim())
        error = null
      } catch (e) {
        error = String(e)
      } finally {
        loading = false
      }
    }, 300)
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
</script>

<div class="search-page">
  <div class="search-header">
    <input
      class="search-input"
      type="text"
      placeholder="Search Tidal's full catalogue"
      bind:value={query}
      oninput={onInput}
      autofocus
    />
  </div>

  {#if !query.trim()}
    <p class="search-hint">Start typing to search Tidal's full catalogue</p>
  {:else if loading}
    <p class="search-hint">Searching…</p>
  {:else if error}
    <p class="search-hint search-error">{error}</p>
  {:else if isEmpty}
    <p class="search-hint">No results for "{query}"</p>
  {:else if results}

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
          {#each sortedTracks as track (track.tidal_id)}
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <li
              class="track-row"
              role="button"
              tabindex="0"
              onclick={() => void playTidalTrackNow(track)}
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
              <button
                class="row-btn"
                onclick={(e) => { e.stopPropagation(); void playTidalTrackNow(track) }}
                aria-label="Play {track.title}"
              >▶</button>
              <button
                class="row-btn"
                onclick={(e) => { e.stopPropagation(); openContextMenu(e, buildTidalTrackMenu(track)) }}
                aria-label="More options"
              >⋯</button>
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
  }
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
  }
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
    grid-template-columns: 38px 1fr auto 32px 32px;
    align-items: center;
    gap: 12px;
    padding: 7px 6px;
    border-radius: 6px;
    cursor: pointer;
  }
  .track-row:hover { background: var(--bg-hover); }
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
