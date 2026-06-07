<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { api } from '$lib/api/client';
  import { openContextMenu } from '$lib/stores/context_menu';
  import { goto } from '$app/navigation';
  import TrendingShelf from '$lib/components/charts/TrendingShelf.svelte';
  import DailyChartShelf from '$lib/components/charts/DailyChartShelf.svelte';
  import PageHeader from '$lib/components/ui/PageHeader.svelte';
  import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
  import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
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

  // Best-effort cover fetch. The metadata endpoint returns no tracks and
  // avoids the playlist resolver path, so card covers do not fan out work.
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
          const playlist = await api.getSpotifyPlaylistMeta(c.id, signal);
          const chartMeta = { thumbnail: playlist.thumbnail, title: playlist.title };
          putCachedSpotifyChartMeta(c.id, chartMeta);
          meta[c.id] = chartMeta;
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

  function openChartPlaylistContext(e: MouseEvent, chart: { id: string; title: string }) {
    e.preventDefault();
    e.stopPropagation();
    openContextMenu(e, chartMenu(chart.id, chart.title), chart.title);
  }
</script>

<svelte:head><title>Charts . NOOR</title></svelte:head>

<div class="page">
  <PageHeader
    eyebrow="Charts"
    title="What's hot"
    subtitle="Worldwide trending tracks from Last.fm and editorial Spotify chart playlists."
    variant="editorial"
  />

  <section class="trending-block">
    <TrendingShelf limit={20} />
  </section>

  <section class="daily-block">
    <DailyChartShelf />
  </section>

  <SectionHeader
    eyebrow="Spotify playlists"
    title="Chart playlists"
    subtitle="Click any to play on TIDAL."
    variant="charts"
    level={2}
  />

  <div class="grid">
    {#each CHARTS as c (c.id)}
      {@const m = meta[c.id]}
      <a
        class="card"
        href={`/spotify-playlist/${c.id}`}
        oncontextmenu={(e) => openChartPlaylistContext(e, c)}
      >
        <div class="art-wrap">
          <ArtworkImage
            src={m?.thumbnail ?? null}
            alt={m?.title ?? c.title}
            size={320}
            className="chart-playlist-art"
            fallbackText="M"
          />
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
  .page { max-width: var(--content-width); margin: 0 auto; padding: var(--space-5) var(--space-4) var(--space-7); display: flex; flex-direction: column; gap: var(--space-5); }
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
  :global(.chart-playlist-art) { width: 100%; height: 100%; object-fit: cover; transition: transform var(--motion-base); }
  .card:hover :global(.chart-playlist-art) { transform: scale(1.05); }
  :global(.chart-playlist-art.fallback) { display: flex; align-items: center; justify-content: center; background: var(--bg-hover); color: var(--text-muted); font-size: var(--font-size-4xl); font-weight: var(--font-weight-bold); }
  .meta { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
  .meta .title { margin: 0; font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; line-height: var(--line-height-snug); }
  .meta .sub { font-size: var(--font-size-xs); color: var(--text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
