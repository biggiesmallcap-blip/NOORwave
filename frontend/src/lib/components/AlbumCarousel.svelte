<script lang="ts">
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';

  interface AlbumCard {
    id: number;
    title: string;
    artist_name: string | null;
    artwork_url: string | null;
  }

  let { albums, onAlbumClick }: {
    albums: AlbumCard[];
    onAlbumClick?: (id: number) => void;
  } = $props();

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
      <button
        class="album-card"
        onclick={() => onAlbumClick?.(album.id)}
        title={album.title}
      >
        <div class="art-wrap">
          {#if album.artwork_url}
            <div class="album-art" style="background-image: url('{album.artwork_url}')"></div>
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

  .album-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: 128px;
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
    width: 128px;
    height: 128px;
    border-radius: 6px;
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
    font-size: 36px;
    color: rgba(255,255,255,0.5);
    background: var(--bg-glass, rgba(255,255,255,0.08));
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
    width: 128px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .album-artist {
    font-size: 11px;
    color: var(--text-secondary, rgba(255,255,255,0.5));
    width: 128px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
