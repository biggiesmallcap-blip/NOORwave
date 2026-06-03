<script lang="ts">
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
  import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
  import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';

  interface AlbumCard {
    id: number;
    title: string;
    artist_id: number | null;
    artist_name: string | null;
    artwork_url: string | null;
  }

  let { albums, onAlbumClick, onContextMenu, onArtistClick, onArtistContextMenu }: {
    albums: AlbumCard[];
    onAlbumClick?: (id: number) => void;
    onContextMenu?: (e: MouseEvent, id: number) => void;
    onArtistClick?: (id: number) => void;
    onArtistContextMenu?: (e: MouseEvent, id: number) => void;
  } = $props();

  let lazyArt = $state<Record<number, string>>({});

  function handleAlbumKeydown(event: KeyboardEvent, albumId: number) {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault();
    onAlbumClick?.(albumId);
  }

  function openArtistContextMenu(event: MouseEvent, artistId: number | null) {
    if (artistId == null || !onArtistContextMenu) return;
    event.preventDefault();
    event.stopPropagation();
    onArtistContextMenu(event, artistId);
  }

  function openArtist(event: MouseEvent, artistId: number | null) {
    if (artistId == null || !onArtistClick) return;
    event.preventDefault();
    event.stopPropagation();
    onArtistClick(artistId);
  }

</script>

{#if albums.length > 0}
  <div class="album-carousel">
  <div class="albums-row" use:wheelToHorizontal>
    {#each albums as album (album.id)}
      {@const resolved = album.artwork_url ?? lazyArt[album.id] ?? null}
      <div
        class="album-card"
        onclick={() => onAlbumClick?.(album.id)}
        oncontextmenu={(e) => { if (onContextMenu) { e.preventDefault(); e.stopPropagation(); onContextMenu(e, album.id); } }}
        onkeydown={(e) => handleAlbumKeydown(e, album.id)}
        title={album.title}
        role="button"
        tabindex="0"
        use:lazyTidalArt={{
          enabled: !album.artwork_url && !lazyArt[album.id],
          query: { artist: album.artist_name, title: album.title },
          onResolve: (url) => (lazyArt[album.id] = url),
        }}
      >
        <div class="art-wrap">
          <ArtworkImage
            className="album-carousel-art"
            src={resolved}
            size={320}
            fallbackText={album.title.slice(0, 2).toUpperCase()}
            decorative={true}
          />
          <PlayOverlay position="center" size="md" />
        </div>
        <span class="album-title">{album.title}</span>
        {#if album.artist_name}
          <button
            class="album-artist album-artist-link"
            type="button"
            onclick={(e) => openArtist(e, album.artist_id)}
            oncontextmenu={(e) => openArtistContextMenu(e, album.artist_id)}
            disabled={album.artist_id == null}
          >{album.artist_name}</button>
        {/if}
      </div>
    {/each}
  </div>
  </div>
{/if}

<style>
  .album-carousel {
    /* Outer wrapper holds the container so the inner overflow-x:auto rail
       isn't itself the container (which has subtle browser quirks). Card
       width adapts to this wrapper's inline-size via the @container rule. */
    container-type: inline-size;
    --album-card-w: clamp(112px, 11vw, 156px);
    /* Edge fade hints at horizontally-scrollable content. mask-image works
       on Chrome/Safari/Firefox; -webkit- prefix kept for older WebKit. */
    mask-image: linear-gradient(
      to right,
      transparent 0,
      black 16px,
      black calc(100% - 32px),
      transparent 100%
    );
    -webkit-mask-image: linear-gradient(
      to right,
      transparent 0,
      black 16px,
      black calc(100% - 32px),
      transparent 100%
    );
  }

  .albums-row {
    display: flex;
    gap: 16px;
    overflow-x: auto;
    scrollbar-width: none;
    padding: 4px 2px 12px;
  }

  .albums-row::-webkit-scrollbar { display: none; }

  @container (max-width: 480px) {
    .album-card { --album-card-w: clamp(80px, 22cqw, 110px); }
  }

  .album-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: var(--album-card-w);
    flex-shrink: 0;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
    color: inherit;
    text-align: left;
  }

  .album-card:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 4px;
    border-radius: var(--radius-xs);
  }

  .art-wrap {
    position: relative;
    width: var(--album-card-w);
    aspect-ratio: 1 / 1;
    border-radius: var(--radius-xs);
    overflow: hidden;
  }

  .art-wrap :global(.album-carousel-art) {
    width: 100%;
    height: 100%;
    display: block;
  }

  .art-wrap :global(.album-carousel-art:not(.fallback)) {
    object-fit: cover;
    transition: transform 0.15s;
  }

  .album-card:hover :global(.album-carousel-art:not(.fallback)) { transform: scale(1.04); }

  .art-wrap :global(.album-carousel-art.fallback) {
    display: grid;
    place-items: center;
    color: rgba(255,255,255,0.5);
    background: var(--bg-hover);
  }

  .art-wrap :global(.album-carousel-art.fallback span) {
    font-size: var(--font-size-xl);
    font-weight: var(--font-weight-semibold);
  }

  .album-card:hover :global(.play-overlay),
  .album-card:focus-visible :global(.play-overlay) {
    opacity: 1;
    transform: translateY(0);
  }

  .album-title {
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
    color: var(--text-primary, #fff);
    width: var(--album-card-w);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .album-artist {
    font-size: var(--font-size-xs);
    color: var(--text-secondary, rgba(255,255,255,0.5));
    width: var(--album-card-w);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    background: transparent;
    border: 0;
    padding: 0;
    font: inherit;
    text-align: left;
  }

  .album-artist-link {
    cursor: pointer;
  }

  .album-artist-link:hover:not(:disabled),
  .album-artist-link:focus-visible:not(:disabled) {
    color: var(--text-primary);
    text-decoration: underline;
    text-underline-offset: 0.12em;
  }

  .album-artist-link:disabled {
    cursor: default;
  }
</style>
