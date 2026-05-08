<script lang="ts">
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';

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

  function letterColor(name: string): string {
    const colors = ['#e63946','#457b9d','#2a9d8f','#e9c46a','#f4a261','#9b5de5','#00b4d8'];
    let h = 0;
    for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff;
    return colors[Math.abs(h) % colors.length];
  }
</script>

{#if albums.length > 0}
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
          <div class="art-play-overlay">
            <svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">
              <path d="M3 2.5l10 5.5-10 5.5V2.5z"/>
            </svg>
          </div>
        </div>
        <span class="album-title">{album.title}</span>
        {#if album.artist_name}
          <span class="album-artist">{album.artist_name}</span>
        {/if}
      </button>
    {/each}
  </div>
{/if}

<style>
  .albums-row {
    display: flex;
    gap: 16px;
    overflow-x: auto;
    scrollbar-width: none;
    padding: 4px 2px 12px;
  }

  .albums-row::-webkit-scrollbar { display: none; }

  .albums-row {
    /* Card width scales with viewport AND adapts to the parent container size.
       In a wide hero context, cards stay at the viewport-clamped size; in a
       narrow sidebar context, the @container rule below switches to a tighter
       cqw-based scale. Artwork and labels share the same value via this var. */
    container-type: inline-size;
    --album-card-w: clamp(112px, 11vw, 156px);
  }

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
    background: var(--bg-surface);
  }

  .art-play-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0,0,0,0.45);
    opacity: 0;
    transition: opacity 0.15s;
    border-radius: 50%;
    width: 36px;
    height: 36px;
    margin: auto;
    color: #fff;
  }

  .album-card:hover .art-play-overlay { opacity: 1; }

  .album-title {
    font-size: 12px;
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
