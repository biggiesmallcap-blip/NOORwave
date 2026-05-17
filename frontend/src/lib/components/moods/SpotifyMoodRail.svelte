<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api/client';
  import { openContextMenu } from '$lib/stores/context_menu';
  import { goto } from '$app/navigation';
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
  import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
  import type { SpotifyMoodCategory } from './spotify-moods-data';

  let { category }: { category: SpotifyMoodCategory } = $props();

  let meta = $state<Record<string, { thumbnail: string | null; title: string | null }>>({});

  onMount(() => {
    // Match /charts pattern: fire all fetches in parallel, fall back to
    // hardcoded title + glyph on failure. Sportify proxy is slow but parallel.
    void Promise.allSettled(
      category.playlists.map(async (p) => {
        try {
          const { playlist } = await api.getSpotifyPlaylist(p.id);
          meta[p.id] = { thumbnail: playlist.thumbnail, title: playlist.title };
        } catch {
          // Quiet: 404 / proxy outage just leaves the glyph + hardcoded title.
        }
      }),
    );
  });

  function buildMenu(id: string, title: string) {
    return [
      { label: 'Open playlist', onSelect: () => void goto(`/spotify-playlist/${id}`) },
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
        href={`/spotify-playlist/${p.id}`}
        oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildMenu(p.id, m?.title ?? p.title), m?.title ?? p.title); }}
      >
        <div class="art-wrap">
          {#if m?.thumbnail}
            <div class="art" style="background-image:url('{m.thumbnail}')"></div>
          {:else}
            <div class="art fallback">M</div>
          {/if}
          <PlayOverlay position="center" size="md" />
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
    transition: background var(--motion-base) ease, border-color var(--motion-base) ease;
    box-sizing: border-box;
    scroll-snap-align: start;
  }
  .card:hover, .card:focus-visible { background: var(--bg-hover); border-color: var(--panel-border); outline: none; }
  .card:focus-visible { border-color: var(--accent-line); }
  .card:hover :global(.play-overlay), .card:focus-visible :global(.play-overlay) { opacity: 1; transform: translateY(0); }

  .art-wrap { position: relative; aspect-ratio: 1 / 1; width: 100%; border-radius: var(--radius-sm); overflow: hidden; background: var(--bg-hover); }
  .art { width: 100%; height: 100%; background-size: cover; background-position: center; transition: transform var(--motion-slow) ease; }
  .card:hover .art { transform: scale(1.05); }
  .art.fallback { display: flex; align-items: center; justify-content: center; font-size: var(--font-size-4xl); color: var(--text-muted); }

  .meta { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
  .title { margin: 0; font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; line-height: var(--line-height-snug); }
  .source { font-size: var(--font-size-xs); color: var(--service-spotify); font-weight: var(--font-weight-semibold); letter-spacing: 0.04em; text-transform: uppercase; }
</style>
