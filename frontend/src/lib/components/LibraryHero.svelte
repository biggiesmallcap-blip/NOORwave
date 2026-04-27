<script lang="ts">
  interface Artist {
    id: number;
    name: string;
    photo_url: string | null;
    playCount: number;
    trackCount: number;
    albumCount: number;
  }

  let { artist, onPlayAll, onShuffle }: {
    artist: Artist;
    onPlayAll: () => void;
    onShuffle: () => void;
  } = $props();

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

<div class="library-hero-card">
  {#if artist.photo_url}
    <div class="hero-bg" style="background-image: url('{artist.photo_url}')"></div>
  {:else}
    <div class="hero-bg hero-bg--color" style="background: {letterColor(artist.name)}"></div>
  {/if}

  <div class="hero-content">
    <div class="hero-art">
      {#if artist.photo_url}
        <div class="hero-avatar" style="background-image: url('{artist.photo_url}')"></div>
      {:else}
        <div class="hero-avatar hero-avatar--fallback" style="background: {letterColor(artist.name)}">
          <span>{initials(artist.name)}</span>
        </div>
      {/if}
    </div>

    <div class="hero-meta">
      <span class="hero-kind">YOUR TOP ARTIST</span>
      <h2 class="hero-title">{artist.name}</h2>
      <p class="hero-sub">{artist.trackCount} tracks &nbsp;·&nbsp; {artist.albumCount} albums</p>
      <div class="hero-actions">
        <button class="btn btn-primary hero-play" onclick={onPlayAll}>
          <svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true">
            <path d="M3 2.5l10 5.5-10 5.5V2.5z"/>
          </svg>
          Play All
        </button>
        <button class="btn btn-glass" onclick={onShuffle}>Shuffle</button>
      </div>
    </div>
  </div>
</div>

<style>
  .library-hero-card {
    position: relative;
    border-radius: 12px;
    overflow: hidden;
    background: var(--bg-glass, rgba(255,255,255,0.04));
    border: 1px solid var(--border-subtle, rgba(255,255,255,0.08));
    min-height: 200px;
  }

  .hero-bg {
    position: absolute;
    inset: 0;
    background-size: cover;
    background-position: center top;
    filter: blur(40px) brightness(0.35) saturate(1.4);
    transform: scale(1.1);
    z-index: 0;
  }

  .hero-bg--color {
    opacity: 0.3;
  }

  .hero-content {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 28px;
    padding: 28px 32px;
  }

  .hero-avatar {
    width: 140px;
    height: 140px;
    border-radius: 50%;
    background-size: cover;
    background-position: center;
    flex-shrink: 0;
    box-shadow: 0 8px 32px rgba(0,0,0,0.4);
  }

  .hero-avatar--fallback {
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
  }

  .hero-title {
    font-size: clamp(28px, 4vw, 44px);
    font-weight: 700;
    line-height: 1.1;
    color: var(--text-primary, #fff);
    margin: 0;
  }

  .hero-sub {
    font-size: 14px;
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
    font-size: 14px;
    font-weight: 600;
  }
</style>
