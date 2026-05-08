<script lang="ts">
  import { fade } from 'svelte/transition';
  import { letterColor } from '$lib/utils/color';

  interface Artist {
    id: number;
    name: string;
    photo_url: string | null;
    playCount: number;
    trackCount: number;
    albumCount: number;
    kind: 'top' | 'forgotten_favorite';
  }

  let { artists, onPlayAll, onShuffle }: {
    artists: Artist[];
    onPlayAll: (artistId: number) => void;
    onShuffle: (artistId: number) => void;
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
    class="library-hero-card"
    class:has-image={!!current.photo_url}
    onmouseenter={() => paused = true}
    onmouseleave={() => paused = false}
    role="region"
    aria-label="Featured artist"
  >
    {#key currentIndex}
      <div
        class="hero-bg"
        style={current.photo_url
          ? `background-image: url('${current.photo_url}')`
          : `background: ${letterColor(current.name)}`}
        in:fade={{ duration: 600 }}
      ></div>
    {/key}

    <div class="hero-overlay"></div>

    <div class="hero-content">
      <div class="hero-art">
        {#if current.photo_url}
          <div class="hero-thumb" style="background-image: url('{current.photo_url}')"></div>
        {:else}
          <div class="hero-thumb hero-thumb--fallback" style="background: {letterColor(current.name)}">
            <span>{initials(current.name)}</span>
          </div>
        {/if}
      </div>

      <div class="hero-meta">
        <span class="hero-kind" class:hero-kind--forgotten={current.kind === 'forgotten_favorite'}>
          {current.kind === 'forgotten_favorite' ? 'FORGOTTEN FAVORITE' : 'YOUR TOP ARTIST'}
        </span>
        <h2 class="hero-title">{current.name}</h2>
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

  .hero-thumb--fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 48px;
    font-weight: 700;
    color: rgba(255,255,255,0.9);
  }

  .hero-meta {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .hero-kind {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 1.5px;
    color: var(--accent, #9b6fff);
    text-transform: uppercase;
    transition: color 300ms ease;
  }

  .hero-kind--forgotten {
    color: #f4a261;
  }

  .hero-title {
    font-size: clamp(28px, 4vw, 44px);
    font-weight: 700;
    line-height: 1.1;
    color: var(--text-primary, #fff);
    margin: 0;
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
    border-radius: 24px;
    font-size: var(--font-size-sm);
    font-weight: 600;
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
