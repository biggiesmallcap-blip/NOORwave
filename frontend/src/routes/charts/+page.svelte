<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api/client';
  import { openContextMenu } from '$lib/stores/context_menu';
  import { goto } from '$app/navigation';

  // Editorial Spotify chart playlists. Stable IDs that change daily/weekly
  // server-side but the playlist identity is fixed. Click navigates to the
  // existing /spotify-playlist/[id] view which handles fetch + play.
  const CHARTS: { id: string; title: string; sub: string }[] = [
    { id: '37i9dQZEVXbMDoHDwVN2tF', title: 'Top 50 - Global', sub: 'Daily chart' },
    { id: '37i9dQZEVXbLRQDuF5jeBp', title: 'Top 50 - USA', sub: 'Daily chart' },
    { id: '37i9dQZEVXbLnolsZ8PSNw', title: 'Top 50 - UK', sub: 'Daily chart' },
    { id: '37i9dQZEVXbJPcfkRz0wJ0', title: 'Top 50 - Australia', sub: 'Daily chart' },
    { id: '37i9dQZEVXbLiRSasKsNU9', title: 'Viral 50 - Global', sub: 'Daily chart' },
    { id: '37i9dQZF1DXcBWIGoYBM5M', title: "Today's Top Hits", sub: 'Editorial' },
    { id: '37i9dQZF1DX4JAvHpjipBk', title: 'New Music Friday', sub: 'Editorial' },
    { id: '37i9dQZF1DX0XUsuxWHRQd', title: 'RapCaviar', sub: 'Editorial' },
    { id: '37i9dQZF1DX1lVhptIYRda', title: 'Hot Country', sub: 'Editorial' },
    { id: '37i9dQZF1DWUa8ZRTfalHk', title: 'Pop Rising', sub: 'Editorial' },
    { id: '37i9dQZF1DX10zKzsJ2jva', title: 'Viva Latino', sub: 'Editorial' },
    { id: '37i9dQZF1DX4dyzvuaRJ0n', title: 'mint', sub: 'Editorial' },
  ];

  // Best-effort cover fetch. The existing playlist endpoint also returns
  // tracks + does TIDAL resolution server-side, so this is heavier than it
  // needs to be -- see FOLLOWUPS for a lightweight metadata endpoint. When
  // the sportify proxy is slow or down, cards just fall back to the glyph.
  let meta = $state<Record<string, { thumbnail: string | null; title: string | null }>>({});

  onMount(() => {
    void Promise.allSettled(
      CHARTS.map(async (c) => {
        try {
          const { playlist } = await api.getSpotifyPlaylist(c.id);
          meta[c.id] = { thumbnail: playlist.thumbnail, title: playlist.title };
        } catch {
          // Quiet: proxy outage just keeps the fallback glyph + hardcoded title.
        }
      }),
    );
  });

  function chartMenu(id: string, title: string) {
    return [
      { label: 'Open playlist', onSelect: () => void goto(`/spotify-playlist/${id}`) },
    ];
  }
</script>

<svelte:head><title>Charts . NOOR</title></svelte:head>

<div class="page">
  <header class="page-header">
    <p class="eyebrow">Spotify</p>
    <h1>Charts</h1>
    <p class="sub">Top playlists from Spotify. Click any to play on TIDAL.</p>
  </header>

  <div class="grid">
    {#each CHARTS as c (c.id)}
      {@const m = meta[c.id]}
      <a
        class="card"
        href={`/spotify-playlist/${c.id}`}
        oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, chartMenu(c.id, c.title), c.title); }}
      >
        {#if m?.thumbnail}
          <div class="art" style="background-image:url('{m.thumbnail}')"></div>
        {:else}
          <div class="art fallback">M</div>
        {/if}
        <span class="card-title">{m?.title ?? c.title}</span>
        <span class="card-sub">{c.sub}</span>
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

  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(160px, 1fr)); gap: var(--gap); }
  .card { display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-2); border-radius: var(--radius-md); text-decoration: none; color: inherit; cursor: pointer; transition: background var(--motion-fast) ease; }
  .card:hover, .card:focus-visible { background: var(--bg-hover); outline: none; }
  .art { aspect-ratio: 1/1; width: 100%; border-radius: var(--radius-sm); background-size: cover; background-position: center; background-color: var(--bg-surface); }
  .art.fallback { display: flex; align-items: center; justify-content: center; color: var(--text-muted); font-size: var(--font-size-3xl); background: linear-gradient(135deg, var(--service-spotify), #1aa34a); color: #fff; }
  .card-title { font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .card-sub { font-size: var(--font-size-xs); color: var(--text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
