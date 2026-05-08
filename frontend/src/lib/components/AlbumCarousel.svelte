<script lang="ts">
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
  import { letterColor } from '$lib/utils/color';
  import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';

  interface AlbumCard {
    id: number;
    title: string;
    artist_name: string | null;
    artwork_url: string | null;
  }

  let { albums, onAlbumClick, onContextMenu }: {
    albums: AlbumCard[];
    onAlbumClick?: (id: number) => void;
    onContextMenu?: (e: MouseEvent, id: number) => void;
  } = $props();

  let lazyArt = $state<Record<number, string>>({});

</script>

{#if albums.length > 0}
  <div class="album-carousel">
  <div class="albums-row" use:wheelToHorizontal>
    {#each albums as album (album.id)}
      {@const resolved = album.artwork_url ?? lazyArt[album.id] ?? null}
      <button
        class="album-card"
        onclick={() => onAlbumClick?.(album.id)}
        oncontextmenu={(e) => { if (onContextMenu) { e.preventDefault(); e.stopPropagation(); onContextMenu(e, album.id); } }}
        title={album.title}
        use:lazyTidalArt={{
          enabled: !album.artwork_url && !lazyArt[album.id],
          query: { artist: album.artist_name, title: album.title },
          onResolve: (url) => (lazyArt[album.id] = url),
        }}
      >
        <div class="art-wrap">
          {#if resolved}
            <div class="album-art" style="background-image: url('{resolved}')"></div>
          {:else}
            <div class="album-art fallback" style="background: {letterColor(album.title)}">
              <span>♫</span>
            </div>
          {/if}
          <PlayOverlay position="center" size="md" />
        </div>
        <span class="album-title">{album.title}</span>
        {#if album.artist_name}
          <span class="album-artist">{album.artist_name}</span>
        {/if}
      </button>
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

  .art-wrap {
    position: relative;
    width: var(--album-card-w);
    aspect-ratio: 1 / 1;
    border-radius: var(--radius-xs);
    overflow: hidden;
  }

  .album-art {
    width: 100%;
    height: 100%;
    background-size: cover;
    background-position: center;
    transition: transform 0.15s;
  }

  .album-card:hover .album-art { transform: scale(1.04); }

  .album-art.fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: var(--font-size-3xl);
    color: rgba(255,255,255,0.5);
    background: var(--bg-hover);
  }

  .album-card:hover :global(.play-overlay),
  .album-card:focus-visible :global(.play-overlay) {
    opacity: 1;
    transform: translateY(0);
  }

  .album-title {
    font-size: var(--font-size-xs);
    font-weight: 500;
    color: var(--text-primary, #fff);
    width: var(--album-card-w);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .album-artist {
    font-size: 11px;
    color: var(--text-secondary, rgba(255,255,255,0.5));
    width: var(--album-card-w);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
