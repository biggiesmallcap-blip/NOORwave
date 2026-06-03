<script lang="ts">
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
  import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
  import { initials } from '$lib/utils/text';

  interface ArtistCard {
    id: number;
    name: string;
    photo_url: string | null;
    fallback_art_url?: string | null;
  }

  let { artists, onArtistClick, onContextMenu }: {
    artists: ArtistCard[];
    onArtistClick?: (id: number) => void;
    onContextMenu?: (e: MouseEvent, id: number) => void;
  } = $props();

  let lazyArt = $state<Record<number, string>>({});

  function artistImageSources(...sources: Array<string | null | undefined>): string[] {
    return sources.filter((source): source is string => typeof source === 'string' && source.trim().length > 0);
  }
</script>

{#if artists.length > 0}
  <div class="artist-carousel">
  <div class="artists-row" use:wheelToHorizontal>
    {#each artists as artist (artist.id)}
      <button
        class="artist-card"
        onclick={() => onArtistClick?.(artist.id)}
        oncontextmenu={(e) => { if (onContextMenu) { e.preventDefault(); e.stopPropagation(); onContextMenu(e, artist.id); } }}
        title={artist.name}
        use:lazyTidalArt={{
          enabled: !artist.photo_url && !lazyArt[artist.id],
          query: { artist: artist.name },
          onResolve: (url) => (lazyArt[artist.id] = url),
        }}
      >
        <div class="avatar-wrap">
          <ArtworkImage
            className="artist-carousel-avatar"
            src={artistImageSources(artist.photo_url, lazyArt[artist.id], artist.fallback_art_url)}
            alt={artist.name}
            size={320}
            fallbackText={initials(artist.name)}
          />
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

  .avatar-wrap :global(.artist-carousel-avatar) {
    width: var(--artist-avatar-w);
    aspect-ratio: 1 / 1;
    border-radius: 50%;
    display: block;
  }

  .avatar-wrap :global(.artist-carousel-avatar:not(.fallback)) {
    object-fit: cover;
    transition: transform var(--motion-fast);
  }

  .artist-card:hover :global(.artist-carousel-avatar:not(.fallback)) {
    transform: scale(1.06);
  }

  .avatar-wrap :global(.artist-carousel-avatar.fallback) {
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-hover);
    font-size: var(--font-size-xl);
    font-weight: var(--font-weight-bold);
    color: rgba(255,255,255,0.85);
  }

  .avatar-wrap :global(.artist-carousel-avatar.fallback span) {
    font-size: inherit;
    font-weight: inherit;
  }

  .artist-name {
    font-size: var(--font-size-xs);
    color: var(--text-secondary, rgba(255,255,255,0.6));
    text-align: center;
    width: var(--artist-card-w);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
