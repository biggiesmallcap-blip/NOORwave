<script lang="ts">
  import { fade } from 'svelte/transition';
  import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
  import { initials } from '$lib/utils/text';

  interface Artist {
    id: number;
    name: string;
    photo_url: string | null;
    fallback_art_url?: string | null;
    playCount: number;
    trackCount: number;
    albumCount: number;
    kind: 'top' | 'forgotten_favorite';
  }

  let { artists, onPlayAll, onShuffle, onArtistClick, onContextMenu, riseIndex = 0 }: {
    artists: Artist[];
    onPlayAll: (artistId: number) => void;
    onShuffle: (artistId: number) => void;
    onArtistClick?: (artistId: number) => void;
    onContextMenu?: (e: MouseEvent, id: number) => void;
    /** Slot in the host page's entrance cascade. See `rise-in-shelf` in app.css. */
    riseIndex?: number;
  } = $props();

  const ROTATE_MS = 8000;

  let currentIndex = $state(0);
  let paused = $state(false);
  let timer: ReturnType<typeof setInterval> | undefined;

  const current = $derived(artists[currentIndex] ?? artists[0]);
  const muralArtists = $derived.by<Artist[]>(() => {
    const group: Artist[] = [];
    const seen = new Set<number>();

    for (const artist of artists) {
      if (!artist || seen.has(artist.id)) continue;
      group.push(artist);
      seen.add(artist.id);
      if (group.length >= 20) break;
    }

    return group;
  });
  const heroHasImage = $derived(muralArtists.some(artist => artistArtworkSources(artist).length > 0));
  const heroKindLabel = $derived(
    current?.kind === 'forgotten_favorite'
      ? (muralArtists.length > 1 ? 'FEATURED ARTISTS' : 'FORGOTTEN FAVORITE')
      : (muralArtists.length >= 20
        ? 'YOUR TOP 20 ARTISTS'
        : muralArtists.length > 1
          ? 'YOUR TOP ARTISTS'
          : 'YOUR TOP ARTIST')
  );

  function artistArtworkSources(artist: Artist): string[] {
    return [artist.photo_url, artist.fallback_art_url]
      .filter((source): source is string => typeof source === 'string' && source.trim().length > 0);
  }

  function startTimer() {
    stopTimer();
    if (artists.length <= 1) return;
    timer = setInterval(() => {
      if (!paused) currentIndex = (currentIndex + 1) % artists.length;
    }, ROTATE_MS);
  }

  function stopTimer() {
    if (timer) clearInterval(timer);
    timer = undefined;
  }

  function jump(delta: number) {
    if (artists.length === 0) return;
    currentIndex = (currentIndex + delta + artists.length) % artists.length;
    startTimer();
  }

  function selectMuralArtist(artistId: number) {
    const nextIndex = artists.findIndex(artist => artist.id === artistId);
    if (nextIndex < 0) return;
    currentIndex = nextIndex;
    startTimer();
  }

  function openHeroContextMenu(event: MouseEvent) {
    if (!current) return;
    openArtistContextMenu(event, current.id);
  }

  function openArtistContextMenu(event: MouseEvent, artistId: number) {
    if (!onContextMenu) return;
    event.preventDefault();
    event.stopPropagation();
    onContextMenu(event, artistId);
  }

  $effect(() => {
    startTimer();
    return stopTimer;
  });

  // Clamp index if the artists list shrinks (e.g. on library refresh)
  $effect(() => {
    if (currentIndex >= artists.length) currentIndex = 0;
  });
</script>

{#if current}
  <div
    class="library-hero-card rise-in-shelf"
    style={`--rise-index: ${riseIndex}`}
    class:has-image={heroHasImage}
    onmouseenter={() => paused = true}
    onmouseleave={() => paused = false}
    oncontextmenu={openHeroContextMenu}
    role="region"
    aria-label="Top artists"
  >
    <div class="hero-bg-mural" in:fade={{ duration: 600 }} aria-label="Top artists mural">
      {#each muralArtists as artist (artist.id)}
        <button
          class="mural-panel"
          class:mural-panel--featured={current?.id === artist.id}
          type="button"
          onclick={() => selectMuralArtist(artist.id)}
          oncontextmenu={(event) => openArtistContextMenu(event, artist.id)}
          aria-label={`Select ${artist.name}`}
        >
          <ArtworkImage
            className="mural-panel-art"
            src={artistArtworkSources(artist)}
            size={640}
            fallbackText={initials(artist.name)}
            decorative={true}
          />
        </button>
      {/each}
    </div>

    <div class="hero-overlay"></div>

    <div class="hero-content">
      <div class="hero-meta">
        <span class="hero-kind" class:hero-kind--forgotten={current.kind === 'forgotten_favorite'}>
          {heroKindLabel}
        </span>
        <h2 class="hero-title">
          <button class="hero-title-link" type="button" onclick={() => onArtistClick?.(current.id)}>
            {current.name}
          </button>
        </h2>
        <p class="hero-sub">{current.trackCount} tracks &nbsp;·&nbsp; {current.albumCount} albums</p>
        <div class="hero-actions">
          <button class="btn btn-primary hero-play" onclick={() => onPlayAll(current.id)}>
            <svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true">
              <path d="M3 2.5l10 5.5-10 5.5V2.5z"/>
            </svg>
            Play All
          </button>
          <button class="btn btn-glass" onclick={() => onShuffle(current.id)}>Shuffle</button>
        </div>
      </div>
    </div>

    {#if artists.length > 1}
      <button class="hero-nav hero-nav--prev" onclick={() => jump(-1)} aria-label="Previous artist">&lsaquo;</button>
      <button class="hero-nav hero-nav--next" onclick={() => jump(1)} aria-label="Next artist">&rsaquo;</button>
      {#if artists.length <= 8}
        <div class="hero-dots" aria-hidden="true">
          {#each artists as _, i}
            <span class="hero-dot" class:active={i === currentIndex}></span>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .library-hero-card {
    position: relative;
    border-radius: 12px;
    overflow: hidden;
    background: var(--panel-bg);
    border: 1px solid var(--border-subtle, rgba(255,255,255,0.08));
    min-height: 200px;
  }

  .hero-bg-mural {
    position: absolute;
    inset: -6%;
    z-index: 0;
    display: grid;
    grid-template-columns: repeat(10, minmax(0, 1fr));
    grid-template-rows: repeat(2, minmax(0, 1fr));
    overflow: hidden;
    background: linear-gradient(120deg, var(--panel-bg), color-mix(in srgb, var(--accent-soft) 28%, transparent));
  }

  .hero-bg-mural::after {
    content: '';
    position: absolute;
    inset: 0;
    background:
      radial-gradient(circle at 78% 50%, rgba(255,255,255,0.24), transparent 28%),
      linear-gradient(90deg, rgba(0,0,0,0.08), transparent 34%, rgba(0,0,0,0.04));
    pointer-events: none;
  }

  .hero-overlay {
    position: absolute;
    inset: 0;
    background: linear-gradient(
      to right,
      rgba(0,0,0,0.66) 0%,
      rgba(0,0,0,0.34) 38%,
      rgba(0,0,0,0.08) 68%,
      transparent 100%
    );
    z-index: 1;
    pointer-events: none;
  }

  .library-hero-card:not(.has-image) .hero-overlay {
    background: rgba(0,0,0,0.45);
  }

  .hero-content {
    position: relative;
    z-index: 2;
    display: grid;
    align-items: center;
    padding: 28px 32px;
    pointer-events: none;
  }

  .hero-title-link {
    appearance: none;
    border: 0;
    background: transparent;
    color: inherit;
    padding: 0;
    font: inherit;
    cursor: pointer;
    pointer-events: auto;
  }

  .mural-panel {
    appearance: none;
    position: relative;
    min-width: 0;
    min-height: 0;
    padding: 0;
    border: 0;
    background: var(--bg-raised);
    color: var(--text-primary);
    cursor: pointer;
    overflow: hidden;
    transform: skewX(-8deg) scaleX(1.1);
    transform-origin: center;
    opacity: 0.96;
    filter: saturate(1.18) brightness(1.16);
    box-shadow: none;
    transition:
      opacity var(--motion-fast),
      filter var(--motion-fast),
      transform var(--motion-base),
      box-shadow var(--motion-base);
  }

  .mural-panel::after {
    content: '';
    position: absolute;
    inset: 0;
    background: linear-gradient(
      90deg,
      rgba(0,0,0,0.34),
      rgba(0,0,0,0.05) 46%,
      rgba(0,0,0,0.38)
    );
    opacity: 0.22;
    pointer-events: none;
  }

  .mural-panel--featured {
    z-index: var(--z-raised);
    opacity: 1;
    transform: skewX(-8deg) scaleX(1.1) scale(1.045);
    filter: saturate(1.95) contrast(1.2) brightness(1.52);
    box-shadow:
      0 0 0 1px rgba(255,255,255,0.34),
      0 14px 34px rgba(0,0,0,0.34),
      0 0 30px color-mix(in srgb, var(--accent) 46%, transparent);
  }

  .mural-panel--featured::after {
    opacity: 0.08;
    background: linear-gradient(
      90deg,
      rgba(0,0,0,0.12),
      rgba(255,255,255,0.08) 48%,
      rgba(0,0,0,0.18)
    );
  }

  .mural-panel :global(.mural-panel-art) {
    display: block;
    width: 100%;
    height: 100%;
  }

  .mural-panel :global(.mural-panel-art:not(.fallback)) {
    object-fit: cover;
    transform: skewX(8deg) scale(1.24);
    transition: transform var(--motion-base), opacity var(--motion-fast);
  }

  .mural-panel:hover :global(.mural-panel-art:not(.fallback)),
  .mural-panel:focus-visible :global(.mural-panel-art:not(.fallback)) {
    transform: skewX(8deg) scale(1.32);
  }

  .mural-panel--featured :global(.mural-panel-art:not(.fallback)) {
    transform: skewX(8deg) scale(1.3);
  }

  .mural-panel--featured:hover :global(.mural-panel-art:not(.fallback)),
  .mural-panel--featured:focus-visible :global(.mural-panel-art:not(.fallback)) {
    transform: skewX(8deg) scale(1.36);
  }

  .mural-panel :global(.mural-panel-art.fallback) {
    display: grid;
    place-items: center;
    background: linear-gradient(135deg, var(--bg-raised), color-mix(in srgb, var(--accent-soft) 26%, var(--bg-surface)));
    color: rgba(255,255,255,0.78);
    transform: skewX(8deg) scale(1.08);
  }

  .mural-panel :global(.mural-panel-art.fallback span) {
    font-size: var(--font-size-2xl);
    font-weight: var(--font-weight-bold);
  }

  .mural-panel:focus-visible,
  .hero-title-link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 4px;
    border-radius: 8px;
  }

  .hero-meta {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: min(36rem, 55vw);
    text-shadow: 0 2px 18px rgba(0,0,0,0.62);
  }

  .hero-kind {
    font-size: var(--font-size-2xs);
    font-weight: var(--font-weight-semibold);
    letter-spacing: 0;
    color: var(--accent);
    text-transform: uppercase;
    transition: color 300ms ease;
  }

  .hero-kind--forgotten {
    color: #f4a261;
  }

  .hero-title {
    font-size: var(--font-size-3xl);
    font-weight: var(--font-weight-bold);
    line-height: var(--line-height-tight);
    color: var(--text-primary, #fff);
    margin: 0;
  }

  .hero-title-link:hover {
    text-decoration: underline;
    text-decoration-thickness: 1px;
    text-underline-offset: 0.12em;
  }

  .hero-sub {
    font-size: var(--font-size-sm);
    color: var(--text-secondary, rgba(255,255,255,0.55));
    margin: 2px 0 8px;
  }

  .hero-actions {
    display: flex;
    gap: 10px;
    align-items: center;
    margin-top: 4px;
    pointer-events: auto;
    /* Shrink the hit area to just the buttons. As a stretched flex child this row
       spans the full meta width, and pointer-events:auto made that empty band
       swallow clicks meant for the artist tiles behind it (dead spots). */
    align-self: flex-start;
  }

  .hero-play {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 10px 22px;
    border-radius: 999px;
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  /* Matches the home/trending mural nav (.chart-nav): dark side-by-side circles
     anchored bottom-right, faintly visible by default and brightening on hover. */
  .hero-nav {
    position: absolute;
    bottom: var(--space-4);
    z-index: var(--z-raised);
    display: grid;
    place-items: center;
    width: clamp(32px, 3vw, 40px);
    aspect-ratio: 1 / 1;
    border: 1px solid var(--panel-border);
    border-radius: 50%;
    background: rgba(0,0,0,0.5);
    color: var(--text-primary);
    cursor: pointer;
    font-size: var(--font-size-xl);
    line-height: 1;
    opacity: 0.78;
    transition: opacity var(--motion-fast), background var(--motion-fast);
  }
  .library-hero-card:hover .hero-nav,
  .hero-nav:focus-visible {
    opacity: 1;
    outline: none;
  }
  .hero-nav:hover { background: rgba(0,0,0,0.75); }
  .hero-nav--prev { right: calc(var(--space-3) + clamp(32px, 3vw, 40px) + var(--space-2)); }
  .hero-nav--next { right: var(--space-3); }

  .hero-dots {
    position: absolute;
    bottom: 10px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 3;
    display: flex;
    gap: 6px;
  }

  .hero-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: rgba(255,255,255,0.25);
    transition: background 200ms ease;
  }
  .hero-dot.active { background: rgba(255,255,255,0.85); }

  @media (max-width: 760px) {
    .hero-bg-mural {
      grid-template-columns: repeat(5, minmax(0, 1fr));
      grid-template-rows: repeat(4, minmax(0, 1fr));
    }

    .hero-content {
      gap: var(--space-3);
      padding: var(--space-4);
    }

    .hero-meta {
      max-width: 100%;
    }
  }
</style>
