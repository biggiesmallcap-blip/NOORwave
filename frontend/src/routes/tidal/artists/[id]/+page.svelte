<script lang="ts">
  import { page } from '$app/state'
  import { api, type TidalArtistProfile, type TidalDiscographyTrack } from '$lib/api/client'
  import TidalTrackRow from '$lib/components/TidalTrackRow.svelte'
  import { openContextMenu } from '$lib/stores/context_menu'
  import { buildAlbumMenu } from '$lib/player/album_menu'

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

  const heroArt = $derived(
    profile?.albums.find((album) => album.artwork_url)?.artwork_url ??
    profile?.top_tracks.find((track) => track.artwork_url)?.artwork_url ??
    null
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
      {#if heroArt}
        <div class="artist-hero-backdrop" style={`background-image: url('${heroArt}')`}></div>
      {/if}
      <div class="artist-hero-veil"></div>
      <div class="artist-hero-body">
        <span class="tidal-badge">TIDAL preview</span>
        <h1 class="artist-name display-face">{profile.artist_name ?? 'Artist'}</h1>
        <p class="artist-meta">{profile.top_tracks.length} top tracks / {profile.albums.length} releases</p>
      </div>
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
          {#each filteredTracks as track, idx (track.tidal_id)}
            <TidalTrackRow
              track={trackAsPlayable(track)}
              variant="numbered"
              index={idx}
              showArtist={false}
            />
          {/each}
        </ul>
      </section>
    {/if}

    {#if filteredAlbums.length > 0}
      <section class="section">
        <h3 class="section-label">Albums</h3>
        <div class="albums-grid">
          {#each filteredAlbums as album (album.tidal_id)}
            <a
              class="grid-card"
              href={`/tidal/albums/${album.tidal_id}`}
              oncontextmenu={(e) => {
                e.preventDefault()
                e.stopPropagation()
                openContextMenu(e, buildAlbumMenu({
                  tidal_id: album.tidal_id,
                  local_id: album.local_id,
                  title: album.title,
                  artist_name: album.artist_name,
                  in_library: album.in_library,
                }, { isLocal: album.in_library && album.local_id != null }), album.title)
              }}
            >
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
  .artist-page { padding: 0 48px 48px; }
  .artist-hero {
    position: relative;
    min-height: 260px;
    margin: 0 -48px 24px;
    padding: 44px 48px 30px;
    display: flex;
    align-items: flex-end;
    overflow: hidden;
    isolation: isolate;
  }
  .artist-hero-backdrop {
    position: absolute;
    inset: -70px;
    background-size: cover;
    background-position: center;
    filter: blur(70px) saturate(1.7);
    transform: scale(1.25);
    opacity: 0.72;
    z-index: -2;
  }
  .artist-hero-veil {
    position: absolute;
    inset: 0;
    background: linear-gradient(180deg, rgba(0,0,0,0.1) 0%, rgba(0,0,0,0.48) 70%, var(--bg-base) 100%);
    z-index: -1;
  }
  .artist-hero-body {
    display: grid;
    gap: 8px;
  }
  .tidal-badge {
    width: fit-content;
    padding: 5px 10px;
    border-radius: 999px;
    border: 1px solid var(--border-subtle);
    background: rgba(255,255,255,0.06);
    color: var(--text-secondary);
    font-size: 0.68rem;
    font-weight: 700;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  .artist-name {
    font-size: clamp(2.4rem, 5.4vw, 4.6rem);
    font-weight: 600;
    color: var(--text-primary);
    margin: 0;
  }
  .artist-meta {
    color: var(--text-secondary);
    margin: 0;
    font-size: 0.9rem;
  }
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
