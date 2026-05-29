<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { api } from '$lib/api/client';
  import { openContextMenu } from '$lib/stores/context_menu';
  import { goto } from '$app/navigation';
  import TrendingShelf from '$lib/components/charts/TrendingShelf.svelte';
  import DailyChartShelf from '$lib/components/charts/DailyChartShelf.svelte';
  import {
    getCachedSpotifyChartMetaMap,
    putCachedSpotifyChartMeta,
    type SpotifyChartMeta,
  } from '$lib/stores/spotify-chart-meta-cache';

  // Editorial Spotify chart playlists. Stable IDs that change daily/weekly
  // server-side but the playlist identity is fixed. Click navigates to the
  // existing /spotify-playlist/[id] view which handles fetch + play.
  const CHARTS: { id: string; title: string; sub: string }[] = [
    { id: '37i9dQZEVXbMDoHDwVN2tF', title: 'Top 50 - Global', sub: 'Daily chart' },
    { id: '37i9dQZEVXbLRQDuF5jeBp', title: 'Top 50 - USA', sub: 'Daily chart' },
    { id: '37i9dQZEVXbLnolsZ8PSNw', title: 'Top 50 - UK', sub: 'Daily chart' },
    { id: '37i9dQZEVXbJPcfkRz0wJ0', title: 'Top 50 - Australia', sub: 'Daily chart' },
    // Viral 50 - Global (37i9dQZEVXbLiRSasKsNU9) removed: Sportify proxy
    // returns a hard 503 specifically for that ID even though Top 50s and
    // editorial playlists work fine. Add back once the proxy recovers it.
    { id: '37i9dQZF1DXcBWIGoYBM5M', title: "Today's Top Hits", sub: 'Editorial' },
    { id: '37i9dQZF1DX4JAvHpjipBk', title: 'New Music Friday', sub: 'Editorial' },
    { id: '37i9dQZF1DX0XUsuxWHRQd', title: 'RapCaviar', sub: 'Editorial' },
    { id: '37i9dQZF1DX1lVhptIYRda', title: 'Hot Country', sub: 'Editorial' },
    { id: '37i9dQZF1DWUa8ZRTfalHk', title: 'Pop Rising', sub: 'Editorial' },
    { id: '37i9dQZF1DX10zKzsJ2jva', title: 'Viva Latino', sub: 'Editorial' },
    { id: '37i9dQZF1DX4dyzvuaRJ0n', title: 'mint', sub: 'Editorial' },
  ];

  // Best-effort cover fetch. The playlist endpoint also returns tracks and
  // TIDAL resolution state, so keep these requests away from first paint.
  let meta = $state<Record<string, SpotifyChartMeta>>({});
  let metaTimer: ReturnType<typeof setTimeout> | null = null;
  let metaAbort: AbortController | null = null;

  onMount(() => {
    const cached = getCachedSpotifyChartMetaMap(CHARTS.map((chart) => chart.id));
    meta = cached;
    const missing = CHARTS.filter((chart) => !cached[chart.id]);
    if (missing.length === 0) return;

    metaAbort = new AbortController();
    metaTimer = setTimeout(() => {
      void loadPlaylistMeta(missing, metaAbort?.signal);
    }, 1600);
  });

  onDestroy(() => {
    if (metaTimer) clearTimeout(metaTimer);
    metaAbort?.abort();
  });

  async function loadPlaylistMeta(charts: typeof CHARTS, signal: AbortSignal | undefined) {
    const queue = [...charts];
    const workers = Array.from({ length: 2 }, async () => {
      while (queue.length > 0 && !signal?.aborted) {
        const c = queue.shift();
        if (!c) return;
        try {
          const { playlist } = await api.getSpotifyPlaylist(c.id, signal);
          putCachedSpotifyChartMeta(c.id, playlist);
          meta[c.id] = { thumbnail: playlist.thumbnail, title: playlist.title };
        } catch {
          // Quiet: proxy outage just keeps the fallback glyph + hardcoded title.
        }
      }
    });
    await Promise.allSettled(workers);
  }

  function chartMenu(id: string, title: string) {
    return [
      { label: 'Open playlist', onSelect: () => void goto(`/spotify-playlist/${id}`) },
    ];
  }
</script>

<svelte:head><title>Charts . NOOR</title></svelte:head>

<div class="page">
  <header class="page-header">
    <p class="eyebrow">Charts</p>
    <h1>What's hot</h1>
    <p class="sub">Worldwide trending tracks from Last.fm and editorial Spotify chart playlists.</p>
  </header>

  <section class="trending-block">
    <TrendingShelf limit={12} />
  </section>

  <section class="daily-block">
    <DailyChartShelf />
  </section>

  <section>
    <h2 class="block-title">Spotify chart playlists</h2>
    <p class="block-sub">Click any to play on TIDAL.</p>
  </section>

  <div class="grid">
    {#each CHARTS as c (c.id)}
      {@const m = meta[c.id]}
      <a
        class="card"
        href={`/spotify-playlist/${c.id}`}
        oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, chartMenu(c.id, c.title), c.title); }}
      >
        <div class="art-wrap">
          {#if m?.thumbnail}
            <div class="art" style="background-image:url('{m.thumbnail}')"></div>
          {:else}
            <div class="art fallback">M</div>
          {/if}
        </div>
        <div class="meta">
          <p class="title">{m?.title ?? c.title}</p>
          <span class="sub">{c.sub}</span>
        </div>
      </a>
    {/each}
  </div>
</div>

<style>
  .page { max-width: var(--content-width); margin: 0 auto; padding: 32px 28px 96px; display: flex; flex-direction: column; gap: 24px; }
  .page-header { display: flex; flex-direction: column; gap: 4px; }
  .eyebrow { font-size: var(--font-size-xs); letter-spacing: 0.08em; text-transform: uppercase; color: var(--service-spotify); margin: 0; font-weight: var(--font-weight-bold); }
  .page-header h1 { margin: 0; font-size: var(--font-size-3xl); font-weight: 800; }
  .page-header .sub { margin: 0; font-size: var(--font-size-sm); color: var(--text-secondary); }
  .block-title { margin: 0 0 4px; font-size: var(--font-size-lg); font-weight: var(--font-weight-bold); color: var(--text-primary); }
  .block-sub { margin: 0 0 var(--space-3); font-size: var(--font-size-sm); color: var(--text-secondary); }
  .trending-block { display: flex; flex-direction: column; gap: var(--gap); }

  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(180px, 100%), 1fr)); gap: var(--gap); }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    background: none;
    border: 1px solid transparent;
    padding: var(--space-2);
    border-radius: var(--radius-md);
    text-decoration: none;
    color: inherit;
    cursor: pointer;
    transition: background var(--motion-base), border-color var(--motion-base);
    box-sizing: border-box;
  }
  .card:hover, .card:focus-visible { background: var(--bg-hover); border-color: var(--panel-border); outline: none; }
  .card:focus-visible { border-color: var(--accent-line); }
  .art-wrap { position: relative; aspect-ratio: 1 / 1; width: 100%; border-radius: var(--radius-sm); overflow: hidden; background: var(--bg-hover); }
  .art { width: 100%; height: 100%; background-size: cover; background-position: center; transition: transform var(--motion-base); }
  .card:hover .art { transform: scale(1.05); }
  .art.fallback { display: flex; align-items: center; justify-content: center; font-size: var(--font-size-4xl); color: var(--text-muted); }
  .meta { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
  .meta .title { margin: 0; font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; line-height: var(--line-height-snug); }
  .meta .sub { font-size: var(--font-size-xs); color: var(--text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
