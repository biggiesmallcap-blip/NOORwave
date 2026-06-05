<script lang="ts">
  import { page } from '$app/state'
  import { api, type TidalArtistProfile, type TidalDiscographyTrack } from '$lib/api/client'
  import TidalTrackRow from '$lib/components/TidalTrackRow.svelte'
  import { openContextMenu } from '$lib/stores/context_menu'
  import { buildAlbumMenu } from '$lib/player/album_menu'
  import {
    firstArtworkUrl,
    tidalArtworkFallbackSizes,
    upscaleTidalArtwork,
    type TidalArtworkSize,
  } from '$lib/utils/artwork'
  import { tidalDiscographyTrackToPlayable } from '$lib/utils/track'

  let tidalArtistId = $derived(Number(page.params.id))
  let profile = $state<TidalArtistProfile | null>(null)
  let loading = $state(true)
  let error = $state<string | null>(null)
  let filterQuery = $state('')
  let failedArtworkUrls = $state<Record<string, boolean>>({})
  let loadSeq = 0

  async function load(id: number) {
    const seq = ++loadSeq
    loading = true
    error = null
    profile = null
    failedArtworkUrls = {}
    try {
      const nextProfile = await api.getTidalArtistProfile(id)
      if (seq !== loadSeq) return
      profile = nextProfile
    } catch (e) {
      if (seq !== loadSeq) return
      error = String(e)
    } finally {
      if (seq === loadSeq) loading = false
    }
  }

  $effect(() => {
    const id = tidalArtistId
    void load(id)
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

  const heroPortrait = $derived(
    firstArtworkUrl(profile?.picture_url)
  )

  const heroBackdrop = $derived(
    firstArtworkUrl(profile?.picture_url, profile?.albums, profile?.top_tracks)
  )
  const heroPortraitSrc = $derived(artworkCandidate(heroPortrait, 640))
  const heroBackdropSrc = $derived(artworkCandidate(heroBackdrop, 1280))

  function artworkCandidate(
    rawUrl: string | null | undefined,
    size: TidalArtworkSize,
  ): string | null {
    if (!rawUrl) return null
    for (const candidateSize of tidalArtworkFallbackSizes(rawUrl, size)) {
      const candidate = upscaleTidalArtwork(rawUrl, candidateSize)
      if (candidate && !failedArtworkUrls[candidate]) return candidate
    }
    return null
  }

  function markArtworkFailed(renderedUrl: string | null | undefined) {
    if (!renderedUrl) return
    failedArtworkUrls = { ...failedArtworkUrls, [renderedUrl]: true }
  }

  function artistTrackPlayable(track: TidalDiscographyTrack) {
    return tidalDiscographyTrackToPlayable(track, { artistTidalId: tidalArtistId })
  }

</script>

{#if loading}
  <div class="loading">Loading…</div>
{:else if error}
  <div class="loading error">{error}</div>
{:else if profile}
  <div class="artist-page">
    <button class="back-link" type="button" onclick={() => history.back()}>← Back</button>
    <div class="artist-hero">
      {#if heroBackdropSrc}
        <img
          class="artist-hero-backdrop"
          src={heroBackdropSrc}
          alt=""
          onerror={() => markArtworkFailed(heroBackdropSrc)}
        />
      {/if}
      <div class="artist-hero-veil"></div>
      <div class="artist-hero-body">
        <div class="artist-portrait-wrap">
          {#if heroPortraitSrc}
            <img
              class="artist-portrait"
              src={heroPortraitSrc}
              alt=""
              onerror={() => markArtworkFailed(heroPortraitSrc)}
            />
          {:else}
            <div class="artist-portrait artist-portrait-fallback" aria-hidden="true">
              {(profile.artist_name ?? 'A').slice(0, 1)}
            </div>
          {/if}
        </div>
        <div class="artist-hero-copy">
          <span class="tidal-badge">TIDAL preview</span>
          <h1 class="artist-name display-face">{profile.artist_name ?? 'Artist'}</h1>
          <p class="artist-meta">{profile.top_tracks.length} top tracks / {profile.albums.length} releases</p>
        </div>
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
              track={artistTrackPlayable(track)}
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
            {@const albumArt = artworkCandidate(album.artwork_url, 320)}
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
              <div class="grid-art">
                {#if albumArt}
                  <img
                    class="grid-art-image"
                    src={albumArt}
                    alt=""
                    onerror={() => markArtworkFailed(albumArt)}
                  />
                {:else}
                  <span class="grid-art-fallback">♫</span>
                {/if}
              </div>
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
  .artist-page { padding: 0 48px 48px; display: flex; flex-direction: column; }
  .artist-page > .back-link { align-self: flex-start; margin-bottom: var(--space-3); }
  .artist-hero {
    position: relative;
    min-height: 260px;
    margin-bottom: var(--space-4);
    padding: var(--space-6) var(--space-5) var(--space-4);
    display: flex;
    align-items: flex-end;
    overflow: hidden;
    isolation: isolate;
    border-radius: var(--radius-lg);
    border: 1px solid var(--border-subtle);
  }
  .artist-hero-backdrop {
    position: absolute;
    inset: -70px;
    width: calc(100% + 140px);
    height: calc(100% + 140px);
    object-fit: cover;
    object-position: center;
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
    display: flex;
    align-items: flex-end;
    gap: var(--space-2);
  }
  .artist-portrait-wrap {
    width: clamp(112px, 14vw, 180px);
    aspect-ratio: 1;
    border-radius: 50%;
    overflow: hidden;
    background: var(--bg-surface);
    box-shadow: 0 28px 70px -16px rgba(0, 0, 0, 0.7);
    flex: 0 0 auto;
  }
  .artist-portrait {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .artist-portrait-fallback {
    display: grid;
    place-items: center;
    color: var(--text-tertiary);
    font-size: var(--font-size-4xl);
    font-weight: var(--font-weight-semibold);
  }
  .artist-hero-copy {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }
  .tidal-badge {
    width: fit-content;
    padding: 5px 10px;
    border-radius: 999px;
    border: 1px solid var(--border-subtle);
    background: rgba(255,255,255,0.06);
    color: var(--text-secondary);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-bold);
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  .artist-name {
    font-size: var(--font-size-4xl);
    font-weight: var(--font-weight-semibold);
    color: var(--text-primary);
    margin: 0;
  }
  .artist-meta {
    color: var(--text-secondary);
    margin: 0;
    font-size: var(--font-size-sm);
  }
  .filter-bar { margin-bottom: 28px; }
  .filter-input {
    background: var(--input-bg);
    border: 1px solid var(--input-border);
    border-radius: 20px;
    padding: 7px 16px;
    font-size: var(--font-size-sm);
    color: var(--text-primary);
    outline: none;
    width: 280px;
    transition: border-color 0.15s;
  }
  .filter-input:focus { border-color: var(--accent); background: var(--input-focus); }
  .section { margin-bottom: 40px; }
  .section-label {
    font-size: var(--font-size-2xs);
    font-weight: var(--font-weight-semibold);
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
    margin-bottom: 6px;
    transition: opacity 0.15s;
    overflow: hidden;
    display: grid;
    place-items: center;
  }
  .grid-art-image {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .grid-art-fallback {
    color: var(--text-tertiary);
    font-size: var(--font-size-xl);
    font-weight: var(--font-weight-semibold);
  }
  .grid-card:hover .grid-art { opacity: 0.85; }
  .grid-title {
    font-size: var(--font-size-xs); color: var(--text-primary); margin: 0;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .grid-sub {
    font-size: var(--font-size-xs); color: var(--text-muted); margin: 2px 0 0;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .empty { color: var(--text-muted); font-size: var(--font-size-sm); margin-top: 32px; }
  @media (max-width: 720px) {
    .artist-page { padding: 0 20px 48px; }
    .artist-hero-body { align-items: flex-start; flex-direction: column; }
    .artist-portrait-wrap { width: 120px; }
  }
</style>
