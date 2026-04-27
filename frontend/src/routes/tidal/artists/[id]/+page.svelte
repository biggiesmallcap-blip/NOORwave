<script lang="ts">
  import { page } from '$app/state'
  import { api, type TidalArtistProfile, type TidalDiscographyTrack } from '$lib/api/client'
  import { buildTidalTrackMenu } from '$lib/player/track_menu'
  import { openContextMenu } from '$lib/stores/context_menu'
  import { playTidalTrackNow } from '$lib/stores/player'
  import { formatDuration } from '$lib/stores/library'

  let tidalArtistId = $derived(Number(page.params.id))
  let profile = $state<TidalArtistProfile | null>(null)
  let loading = $state(true)
  let error = $state<string | null>(null)
  let filterQuery = $state('')

  $effect(() => {
    const id = tidalArtistId
    let cancelled = false
    loading = true
    error = null
    api.getTidalArtistProfile(id)
      .then((p) => { if (!cancelled) profile = p })
      .catch((e) => { if (!cancelled) error = String(e) })
      .finally(() => { if (!cancelled) loading = false })
    return () => { cancelled = true }
  })

  const filteredTracks = $derived(
    profile?.top_tracks.filter((t) =>
      filterQuery
        ? t.title.toLowerCase().includes(filterQuery.toLowerCase()) ||
          (t.artist_name ?? '').toLowerCase().includes(filterQuery.toLowerCase())
        : true
    ) ?? []
  )

  const filteredAlbums = $derived(
    profile?.albums.filter((a) =>
      filterQuery ? a.title.toLowerCase().includes(filterQuery.toLowerCase()) : true
    ) ?? []
  )

  function trackAsPlayable(t: TidalDiscographyTrack) {
    return {
      tidal_id: t.tidal_id,
      title: t.title,
      artist_name: t.artist_name ?? null,
      album_title: t.album_title ?? null,
      artwork_url: t.artwork_url,
      duration_ms: t.duration_ms,
      artist_tidal_id: tidalArtistId,
    }
  }
</script>

{#if loading}
  <div class="loading">Loading…</div>
{:else if error}
  <div class="loading error">{error}</div>
{:else if profile}
  <div class="artist-page">
    <div class="artist-hero">
      <h1 class="artist-name">{profile.artist_name ?? 'Artist'}</h1>
    </div>

    <div class="filter-bar">
      <input
        class="filter-input"
        type="text"
        placeholder="Filter tracks and albums…"
        bind:value={filterQuery}
      />
    </div>

    {#if filteredTracks.length > 0}
      <section class="section">
        <h3 class="section-label">Top Tracks</h3>
        <ul class="tracks-list">
          {#each filteredTracks as track (track.tidal_id)}
            <li
              class="pop-row"
              ondblclick={() => playTidalTrackNow(trackAsPlayable(track))}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTidalTrackMenu(trackAsPlayable(track))) }}
            >
              <div
                class="track-art"
                style={track.artwork_url ? `background-image: url('${track.artwork_url}')` : ''}
              ></div>
              <div class="track-meta">
                <p class="track-title">{track.title}</p>
                {#if track.album_title}
                  <p class="track-album">{track.album_title}</p>
                {/if}
              </div>
              <span class="track-duration">{formatDuration(track.duration_ms)}</span>
              <button
                class="row-btn"
                onclick={() => playTidalTrackNow(trackAsPlayable(track))}
                aria-label="Play {track.title}"
              >▶</button>
              <button
                class="row-btn"
                onclick={(e) => { e.stopPropagation(); openContextMenu(e, buildTidalTrackMenu(trackAsPlayable(track))) }}
                aria-label="More options"
              >⋯</button>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if filteredAlbums.length > 0}
      <section class="section">
        <h3 class="section-label">Albums</h3>
        <div class="albums-grid">
          {#each filteredAlbums as album (album.tidal_id)}
            <a class="grid-card" href={`/tidal/albums/${album.tidal_id}`}>
              <div
                class="grid-art"
                style={album.artwork_url ? `background-image: url('${album.artwork_url}')` : ''}
              ></div>
              <p class="grid-title">{album.title}</p>
              <p class="grid-sub">{album.artist_name}</p>
            </a>
          {/each}
        </div>
      </section>
    {/if}

    {#if filteredTracks.length === 0 && filteredAlbums.length === 0}
      {#if filterQuery}
        <p class="empty">No results for "{filterQuery}"</p>
      {:else}
        <p class="empty">No content available for this artist.</p>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .loading { padding: 48px; color: var(--text-muted); text-align: center; }
  .error { color: var(--state-error); }
  .artist-page { padding: 32px 48px; }
  .artist-hero { margin-bottom: 24px; }
  .artist-name { font-size: 32px; font-weight: 700; color: var(--text-primary); margin: 0; }
  .filter-bar { margin-bottom: 28px; }
  .filter-input {
    background: var(--input-bg);
    border: 1px solid var(--input-border);
    border-radius: 20px;
    padding: 7px 16px;
    font-size: 13px;
    color: var(--text-primary);
    outline: none;
    width: 280px;
    transition: border-color 0.15s;
  }
  .filter-input:focus { border-color: var(--accent); background: var(--input-focus); }
  .section { margin-bottom: 40px; }
  .section-label {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 1.5px;
    color: var(--accent);
    margin-bottom: 14px;
  }
  .tracks-list { list-style: none; padding: 0; margin: 0; }
  .pop-row {
    display: grid;
    grid-template-columns: 38px 1fr auto 32px 32px;
    align-items: center;
    gap: 12px;
    padding: 7px 6px;
    border-radius: 6px;
    cursor: pointer;
  }
  .pop-row:hover { background: var(--bg-hover); }
  .track-art {
    width: 36px; height: 36px;
    border-radius: 4px;
    background: var(--bg-raised);
    background-size: cover; background-position: center;
  }
  .track-title { font-size: 13px; color: var(--text-primary); margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .track-album { font-size: 11px; color: var(--text-muted); margin: 2px 0 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .track-duration { font-size: 12px; color: var(--text-muted); white-space: nowrap; }
  .row-btn {
    background: none; border: none;
    color: var(--text-tertiary); cursor: pointer;
    font-size: 14px; padding: 4px;
    border-radius: 4px; opacity: 0;
    transition: opacity 0.1s, color 0.1s;
  }
  .pop-row:hover .row-btn { opacity: 1; }
  .row-btn:hover { color: var(--text-primary); }
  .albums-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
    gap: 16px;
  }
  .grid-card { text-decoration: none; }
  .grid-art {
    width: 100%; aspect-ratio: 1;
    border-radius: 6px;
    background: var(--bg-raised);
    background-size: cover; background-position: center;
    margin-bottom: 6px;
    transition: opacity 0.15s;
  }
  .grid-card:hover .grid-art { opacity: 0.85; }
  .grid-title {
    font-size: 12px; color: var(--text-primary); margin: 0;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .grid-sub {
    font-size: 11px; color: var(--text-muted); margin: 2px 0 0;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .empty { color: var(--text-muted); font-size: 14px; margin-top: 32px; }
</style>
