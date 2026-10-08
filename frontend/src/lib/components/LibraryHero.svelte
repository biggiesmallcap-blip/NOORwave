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
          <button class="btn hero-shuffle" onclick={() => onShuffle(current.id)}>Shuffle</button>
        </div>
      </div>
    </div>

    {#if artists.length > 1}
      <!-- One pager in the top corner (as on the chart murals): previous,
           position, next. -->
      <div class="hero-pager">
        <button class="hero-nav" type="button" onclick={() => jump(-1)} aria-label="Previous artist">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m14.5 5-7 7 7 7" /></svg>
        </button>
        <span class="hero-pager-count" aria-live="polite">{currentIndex + 1} / {artists.length}</span>
        <button class="hero-nav" type="button" onclick={() => jump(1)} aria-label="Next artist">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m9.5 5 7 7-7 7" /></svg>
        </button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .library-hero-card {
    position: relative;
    border-radius: var(--radius-md);
    overflow: hidden;
    /* clip-path, not just overflow: composited art tiles otherwise glow
       through the anti-aliased rounded corners. */
    clip-path: inset(0 round var(--radius-md));
    isolation: isolate;
    background: var(--bg-base);
    min-height: 200px;
  }

  .library-hero-card::after {
    content: '';
    position: absolute;
    inset: 0;
    z-index: var(--z-raised);
    border-radius: inherit;
    /* A dark inner edge, not a light hairline: a light line over dark
       art read as a glowing rim, worst at the rounded corners. */
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.45);
    pointer-events: none;
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

  /* Text sits on artwork, so the copy is white on a dark scrim in every theme.
     The scrim fades out by two thirds of the width so the collage stays bright. */
  .hero-overlay {
    position: absolute;
    inset: 0;
    /* A deep plate behind the text that falls off into the collage, plus a
       soft floor (the chart murals use the same pair). */
    background:
      linear-gradient(90deg, rgba(8,8,12,0.9) 0%, rgba(8,8,12,0.76) 30%, rgba(8,8,12,0.32) 56%, transparent 80%),
      linear-gradient(0deg, rgba(8,8,12,0.45) 0%, transparent 45%);
    z-index: 1;
    pointer-events: none;
  }

  .library-hero-card:not(.has-image) .hero-overlay {
    background: rgba(8,8,12,0.45);
  }

  .hero-content {
    position: relative;
    z-index: 2;
    display: grid;
    align-items: center;
    padding: var(--space-6);
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
    filter: var(--art-collage-filter);
    box-shadow: none;
    transition:
      opacity var(--motion-fast),
      filter var(--motion-fast),
      transform var(--motion-base),
      box-shadow var(--motion-base);
  }

  .mural-panel--featured {
    z-index: var(--z-raised);
    transform: skewX(-8deg) scaleX(1.1) scale(1.045);
    filter: saturate(1.14) brightness(1.06);
    box-shadow:
      0 0 0 1px rgba(255,255,255,0.4),
      0 14px 34px rgba(0,0,0,0.34);
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
    outline: 2px solid var(--accent-strong);
    outline-offset: 4px;
    border-radius: 8px;
  }

  .hero-meta {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    max-width: min(36rem, 52%);
    text-shadow: 0 1px 8px rgba(0,0,0,0.45);
  }

  .hero-kind {
    font-size: var(--font-size-2xs);
    font-weight: var(--font-weight-semibold);
    letter-spacing: 0.12em;
    color: rgba(255,255,255,0.76);
    text-transform: uppercase;
    transition: color var(--motion-slow);
  }

  .hero-kind--forgotten {
    color: #f4a261;
  }

  .hero-title {
    font-size: var(--font-size-3xl);
    letter-spacing: -0.01em;
    font-weight: var(--font-weight-bold);
    line-height: var(--line-height-tight);
    color: #fff;
    margin: 0;
  }

  .hero-title-link:hover {
    text-decoration: underline;
    text-decoration-thickness: 1px;
    text-underline-offset: 0.12em;
  }

  .hero-sub {
    font-size: var(--font-size-md);
    font-weight: var(--font-weight-medium);
    color: rgba(255,255,255,0.86);
    margin: 0;
  }

  .hero-actions {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    margin-top: var(--space-4);
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

  /* Sits on artwork, so it keeps light-on-dark in every theme instead of the
     themed glass button, which went dark-on-dark in light mode. */
  .hero-shuffle {
    padding: 10px 20px;
    border: 1px solid rgba(255,255,255,0.22);
    border-radius: 999px;
    background: rgba(8,8,12,0.42);
    color: #fff;
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  .hero-shuffle:hover:not(:disabled) {
    background: rgba(8,8,12,0.62);
  }

  /* Matches the chart mural pager: solid discs and a position count in the
     top corner, clear of the title. */
  .hero-pager {
    position: absolute;
    top: var(--space-4);
    right: var(--space-4);
    z-index: var(--z-raised);
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: 3px;
    border-radius: 999px;
    background: rgba(8,8,12,0.62);
    box-shadow: 0 4px 16px rgba(0,0,0,0.35);
  }

  .hero-pager-count {
    min-width: 3.5em;
    color: rgba(255,255,255,0.86);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
    text-align: center;
  }

  .hero-nav {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: #fff;
    cursor: pointer;
    transition: background var(--motion-fast);
  }
  .hero-nav:hover { background: rgba(255,255,255,0.14); }
  .hero-nav:focus-visible {
    outline: 2px solid var(--accent-strong);
    outline-offset: 1px;
  }
  .hero-nav svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2.4;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

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
