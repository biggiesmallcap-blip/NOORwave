<script lang="ts">
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';

  interface ArtistCard {
    id: number;
    name: string;
    photo_url: string | null;
  }

  let { artists, onArtistClick, onContextMenu }: {
    artists: ArtistCard[];
    onArtistClick?: (id: number) => void;
    onContextMenu?: (e: MouseEvent, id: number) => void;
  } = $props();

  let failedImages = $state(new Set<number>());
  let lazyArt = $state<Record<number, string>>({});

  function letterColor(name: string): string {
    const colors = ['#e63946','#457b9d','#2a9d8f','#e9c46a','#f4a261','#9b5de5','#00b4d8'];
    let h = 0;
    for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff;
    return colors[Math.abs(h) % colors.length];
  }

  function initials(name: string): string {
    return name.split(/\s+/).map(p => p[0]?.toUpperCase() ?? '').join('').slice(0, 2) || '?';
  }
</script>

{#if artists.length > 0}
  <div class="artists-row" use:wheelToHorizontal>
    {#each artists as artist (artist.id)}
      {@const baseSrc = artist.photo_url && !failedImages.has(artist.id) ? artist.photo_url : null}
      {@const resolved = baseSrc ?? lazyArt[artist.id] ?? null}
      <button
        class="artist-card"
        onclick={() => onArtistClick?.(artist.id)}
        oncontextmenu={(e) => { if (onContextMenu) { e.preventDefault(); e.stopPropagation(); onContextMenu(e, artist.id); } }}
        title={artist.name}
        use:lazyTidalArt={{
          enabled: !baseSrc && !lazyArt[artist.id],
          query: { artist: artist.name },
          onResolve: (url) => (lazyArt[artist.id] = url),
        }}
      >
        <div class="avatar-wrap">
          {#if resolved}
            <img
              class="artist-avatar"
              src={resolved}
              alt={artist.name}
              onerror={() => { failedImages = new Set([...failedImages, artist.id]); }}
            />
          {:else}
            <div class="artist-avatar fallback" style="background: {letterColor(artist.name)}">
              <span>{initials(artist.name)}</span>
            </div>
          {/if}
        </div>
        <span class="artist-name">{artist.name}</span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .artists-row {
    display: flex;
    gap: 16px;
    overflow-x: auto;
    scrollbar-width: none;
    padding: 4px 2px 12px;
  }

  .artists-row::-webkit-scrollbar { display: none; }

  .artist-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    width: 84px;
    flex-shrink: 0;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
    color: inherit;
  }

  .avatar-wrap {
    position: relative;
  }

  .artist-avatar {
    width: 72px;
    height: 72px;
    border-radius: 50%;
    object-fit: cover;
    display: block;
    transition: transform 0.15s;
  }

  .artist-card:hover .artist-avatar {
    transform: scale(1.06);
  }

  .artist-avatar.fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 22px;
    font-weight: 700;
    color: rgba(255,255,255,0.85);
  }

  .artist-name {
    font-size: 11px;
    color: var(--text-secondary, rgba(255,255,255,0.6));
    text-align: center;
    width: 84px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
