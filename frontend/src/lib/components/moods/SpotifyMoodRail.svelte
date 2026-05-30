<script lang="ts">
  import { openContextMenu } from '$lib/stores/context_menu';
  import { goto } from '$app/navigation';
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
  import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
  import {
    getCachedSpotifyChartMetaMap,
    type SpotifyChartMeta,
  } from '$lib/stores/spotify-chart-meta-cache';
  import type { SpotifyMoodCategory } from './spotify-moods-data';

  let { category }: { category: SpotifyMoodCategory } = $props();

  let meta = $state<Record<string, SpotifyChartMeta>>({});

  $effect(() => {
    const ids = category.playlists.map((playlist) => playlist.id);
    meta = getCachedSpotifyChartMetaMap(ids);
  });

  function playlistHref(id: string): string {
    const params = new URLSearchParams({ from: 'moods', mood: category.slug });
    return `/spotify-playlist/${id}?${params}`;
  }

  function buildMenu(id: string, title: string) {
    return [
      { label: 'Open playlist', onSelect: () => void goto(playlistHref(id)) },
    ];
  }
</script>

<section class="rail-section">
  <h3 class="rail-heading">{category.label}</h3>
  <div class="rail" use:wheelToHorizontal>
    {#each category.playlists as p (p.id)}
      {@const m = meta[p.id]}
      <a
        class="card"
        href={playlistHref(p.id)}
        oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildMenu(p.id, p.title), p.title); }}
      >
        <div class="art-wrap">
          <ArtworkImage
            className="spotify-mood-art"
            src={m?.thumbnail ?? null}
            alt={m?.title ?? p.title}
            size={320}
            fallbackText={p.title.slice(0, 2).toUpperCase()}
          />
        </div>
        <div class="meta">
          <p class="title">{m?.title ?? p.title}</p>
          <span class="source">Spotify</span>
        </div>
      </a>
    {/each}
  </div>
</section>

<style>
  .rail-section { display: flex; flex-direction: column; gap: var(--space-2); }
  .rail-heading { margin: 0; font-size: var(--font-size-md); font-weight: var(--font-weight-bold); color: var(--text-primary); }
  .rail {
    display: flex;
    gap: var(--gap-sm);
    overflow-x: auto;
    padding-bottom: var(--space-2);
    scroll-snap-type: x mandatory;
    mask-image: linear-gradient(to right, transparent 0, black 16px, black calc(100% - 32px), transparent 100%);
    -webkit-mask-image: linear-gradient(to right, transparent 0, black 16px, black calc(100% - 32px), transparent 100%);
  }
  .rail::-webkit-scrollbar { height: 6px; }
  .rail::-webkit-scrollbar-track { background: var(--bg-surface); border-radius: var(--radius-xs); }
  .rail::-webkit-scrollbar-thumb { background: var(--border-subtle); border-radius: var(--radius-xs); }
  .rail::-webkit-scrollbar-thumb:hover { background: var(--text-muted); }

  .card {
    flex: 0 0 180px;
    width: 180px;
    min-width: 180px;
    max-width: 180px;
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
    scroll-snap-align: start;
  }
  .card:hover, .card:focus-visible { background: var(--bg-hover); border-color: var(--panel-border); outline: none; }
  .card:focus-visible { border-color: var(--accent-line); }

  .art-wrap { position: relative; aspect-ratio: 1 / 1; width: 100%; border-radius: var(--radius-sm); overflow: hidden; background: var(--bg-hover); }
  :global(.spotify-mood-art) { width: 100%; height: 100%; object-fit: cover; display: block; transition: transform var(--motion-base); }
  .card:hover :global(.spotify-mood-art) { transform: scale(1.05); }
  :global(.spotify-mood-art.fallback) { display: flex; align-items: center; justify-content: center; background: var(--bg-hover); color: var(--text-muted); font-size: var(--font-size-4xl); font-weight: var(--font-weight-bold); }

  .meta { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
  .title { margin: 0; font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; line-height: var(--line-height-snug); }
  .source { font-size: var(--font-size-xs); color: var(--service-spotify); font-weight: var(--font-weight-semibold); letter-spacing: 0.04em; text-transform: uppercase; }
</style>
