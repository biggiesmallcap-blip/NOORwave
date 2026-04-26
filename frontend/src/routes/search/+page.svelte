<script lang="ts">
  import { api, type TidalSearchResults } from '$lib/api/client'
  import { buildTidalTrackMenu } from '$lib/player/track_menu'
  import { openContextMenu } from '$lib/stores/context_menu'
  import { playTidalTrackNow } from '$lib/stores/player'
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

    {#if results.artists.length > 0}
      <section class="results-section">
        <h3 class="section-label">Artists</h3>
        <div class="artists-row">
          {#each results.artists as artist (artist.tidal_id)}
            <a class="artist-card" href={`/tidal/artists/${artist.tidal_id}`}>
              <div
                class="artist-avatar"
                style={artist.artwork_url ? `background-image: url('${artist.artwork_url}')` : ''}
              ></div>
              <span class="artist-name">{artist.name}</span>
            </a>
          {/each}
        </div>
      </section>
    {/if}

    {#if results.albums.length > 0}
      <section class="results-section">
        <h3 class="section-label">Albums</h3>
        <div class="albums-row">
          {#each results.albums as album (album.tidal_id)}
            <a class="album-card" href={`/tidal/albums/${album.tidal_id}`}>
              <div
                class="album-art"
                style={album.artwork_url ? `background-image: url('${album.artwork_url}')` : ''}
              ></div>
              <p class="album-title">{album.title}</p>
              {#if album.artist_name}
                <p class="album-artist">{album.artist_name}</p>
              {/if}
            </a>
          {/each}
        </div>
      </section>
    {/if}

    {#if results.tracks.length > 0}
      <section class="results-section">
        <h3 class="section-label">Tracks</h3>
        <ul class="tracks-list">
          {#each results.tracks as track (track.tidal_id)}
            <li
              class="track-row"
              ondblclick={() => playTidalTrackNow(track)}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTidalTrackMenu(track)) }}
            >
              <div
                class="track-art"
                style={track.artwork_url ? `background-image: url('${track.artwork_url}')` : ''}
              ></div>
              <div class="track-meta">
                <p class="track-title">{track.title}</p>
                <p class="track-subtitle">
                  {#if track.artist_name}{track.artist_name}{/if}
                  {#if track.artist_name && track.album_title} — {/if}
                  {#if track.album_title}{track.album_title}{/if}
                </p>
              </div>
              <span class="track-duration">{formatDuration(track.duration_ms)}</span>
              <button
                class="row-btn"
                onclick={() => playTidalTrackNow(track)}
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
    padding: 40px 48px;
    max-width: 960px;
  }
  .search-header {
    margin-bottom: 36px;
  }
  .search-input {
    width: 100%;
    max-width: 560px;
    background: var(--input-bg);
    border: 1px solid var(--input-border);
    border-radius: 24px;
    padding: 11px 22px;
    font-size: 15px;
    color: var(--text-primary);
    outline: none;
    transition: border-color 0.15s;
  }
  .search-input:focus {
    border-color: var(--accent);
    background: var(--input-focus);
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
  .artist-avatar {
    width: 72px;
    height: 72px;
    border-radius: 50%;
    background: var(--bg-raised);
    background-size: cover;
    background-position: center;
    transition: opacity 0.15s;
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
