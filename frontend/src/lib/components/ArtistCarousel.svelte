<script lang="ts">
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
  import { letterColor } from '$lib/utils/color';

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

  function initials(name: string): string {
    return name.split(/\s+/).map(p => p[0]?.toUpperCase() ?? '').join('').slice(0, 2) || '?';
  }
</script>

{#if artists.length > 0}
  <div class="artist-carousel">
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
  </div>
{/if}

<style>
  .artist-carousel {
    /* Outer wrapper is the container; inner row stays as the overflow-x rail.
       See AlbumCarousel for the rationale. */
    container-type: inline-size;
    --artist-card-w:   clamp(76px, 7.5vw, 104px);
    --artist-avatar-w: clamp(64px, 6.4vw, 92px);
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

  .artists-row {
    display: flex;
    gap: 16px;
    overflow-x: auto;
    scrollbar-width: none;
    padding: 4px 2px 12px;
  }

  .artists-row::-webkit-scrollbar { display: none; }

  @container (max-width: 480px) {
    .artist-card {
      --artist-card-w:   clamp(60px, 18cqw, 88px);
      --artist-avatar-w: clamp(52px, 16cqw, 76px);
    }
  }

  .artist-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    width: var(--artist-card-w);
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
    width: var(--artist-avatar-w);
    aspect-ratio: 1 / 1;
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
    font-size: var(--font-size-xl);
    font-weight: 700;
    color: rgba(255,255,255,0.85);
  }

  .artist-name {
    font-size: 11px;
    color: var(--text-secondary, rgba(255,255,255,0.6));
    text-align: center;
    width: var(--artist-card-w);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
