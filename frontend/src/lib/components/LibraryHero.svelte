<script lang="ts">
  import { fade } from 'svelte/transition';
  import { letterColor } from '$lib/utils/color';

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

  let { artists, onPlayAll, onShuffle, onArtistClick, onContextMenu }: {
    artists: Artist[];
    onPlayAll: (artistId: number) => void;
    onShuffle: (artistId: number) => void;
    onArtistClick?: (artistId: number) => void;
    onContextMenu?: (e: MouseEvent, id: number) => void;
  } = $props();

  const ROTATE_MS = 8000;

  let currentIndex = $state(0);
  let paused = $state(false);
  let timer: ReturnType<typeof setInterval> | undefined;

  const current = $derived(artists[currentIndex] ?? artists[0]);

  function initials(name: string): string {
    return name.split(/\s+/).map(p => p[0]?.toUpperCase() ?? '').join('').slice(0, 2) || '?';
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

  function openHeroContextMenu(event: MouseEvent) {
    if (!current || !onContextMenu) return;
    event.preventDefault();
    event.stopPropagation();
    onContextMenu(event, current.id);
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
  {@const heroArt = current.photo_url ?? current.fallback_art_url ?? null}
  <div
    class="library-hero-card"
    class:has-image={!!heroArt}
    onmouseenter={() => paused = true}
    onmouseleave={() => paused = false}
    oncontextmenu={openHeroContextMenu}
    role="region"
    aria-label="Featured artist"
  >
    {#key currentIndex}
      <div
        class="hero-bg"
        style={heroArt
          ? `background-image: url('${heroArt}')`
          : `background: ${letterColor(current.name)}`}
        in:fade={{ duration: 600 }}
      ></div>
    {/key}

    <div class="hero-overlay"></div>

    <div class="hero-content">
      <button
        class="hero-art hero-artist-link"
        type="button"
        onclick={() => onArtistClick?.(current.id)}
        aria-label={`Open ${current.name}`}
      >
        {#if heroArt}
          <div class="hero-thumb" style="background-image: url('{heroArt}')"></div>
        {:else}
          <div class="hero-thumb hero-thumb--fallback" style="background: {letterColor(current.name)}">
            <span>{initials(current.name)}</span>
          </div>
        {/if}
      </button>

      <div class="hero-meta">
        <span class="hero-kind" class:hero-kind--forgotten={current.kind === 'forgotten_favorite'}>
          {current.kind === 'forgotten_favorite' ? 'FORGOTTEN FAVORITE' : 'YOUR TOP ARTIST'}
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
      <button class="hero-nav hero-nav--prev" onclick={() => jump(-1)} aria-label="Previous artist">‹</button>
      <button class="hero-nav hero-nav--next" onclick={() => jump(1)} aria-label="Next artist">›</button>
      <div class="hero-dots" aria-hidden="true">
        {#each artists as _, i}
          <span class="hero-dot" class:active={i === currentIndex}></span>
        {/each}
      </div>
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

  .hero-bg {
    position: absolute;
    inset: -8%;
    background-size: cover;
    background-position: center;
    filter: blur(40px) saturate(1.4);
    z-index: 0;
  }

  .hero-overlay {
    position: absolute;
    inset: 0;
    background: linear-gradient(
      to right,
      rgba(0,0,0,0.92) 0%,
      rgba(0,0,0,0.6) 55%,
      rgba(0,0,0,0.15) 100%
    );
    z-index: 1;
  }

  .library-hero-card:not(.has-image) .hero-overlay {
    background: rgba(0,0,0,0.45);
  }

  .hero-content {
    position: relative;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 28px;
    padding: 28px 32px;
  }

  .hero-thumb {
    width: clamp(120px, 12vw, 180px);
    aspect-ratio: 1 / 1;
    border-radius: 8px;
    background-size: cover;
    background-position: center;
    flex-shrink: 0;
    box-shadow: 0 8px 32px rgba(0,0,0,0.5);
  }

  .hero-artist-link,
  .hero-title-link {
    appearance: none;
    border: 0;
    background: transparent;
    color: inherit;
    padding: 0;
    font: inherit;
    cursor: pointer;
  }

  .hero-artist-link {
    display: block;
  }

  .hero-artist-link:focus-visible,
  .hero-title-link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 4px;
    border-radius: 8px;
  }

  .hero-thumb--fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: var(--font-size-4xl);
    font-weight: var(--font-weight-bold);
    color: rgba(255,255,255,0.9);
  }

  .hero-meta {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .hero-kind {
    font-size: var(--font-size-2xs);
    font-weight: var(--font-weight-semibold);
    letter-spacing: 1.5px;
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

  .hero-nav {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    z-index: 3;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: rgba(0,0,0,0.5);
    border: 1px solid rgba(255,255,255,0.15);
    color: rgba(255,255,255,0.85);
    font-size: var(--font-size-xl);
    line-height: 1;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transition: opacity 200ms ease, background 200ms ease;
  }
  .library-hero-card:hover .hero-nav { opacity: 1; }
  .hero-nav:hover { background: rgba(0,0,0,0.75); }
  .hero-nav--prev { left: 12px; }
  .hero-nav--next { right: 12px; }

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
</style>
