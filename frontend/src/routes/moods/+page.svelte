<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { ApiError, api, type TidalMoodCategory } from '$lib/api/client';
  import { tidalStatus } from '$lib/stores/tidal';
  import { openContextMenu } from '$lib/stores/context_menu';
  import { goto } from '$app/navigation';
  import {
    getCachedMoodCategories,
    putCachedMoodCategories,
    clearCachedMoods,
  } from '$lib/stores/tidal-moods-cache';

  type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

  // Sync-read the cache on script init so revisiting /moods within the
  // 6h TTL renders instantly without a skeleton flash. Mirrors the
  // home-discover pattern.
  const cachedOnMount = getCachedMoodCategories();
  let categories = $state<TidalMoodCategory[]>(cachedOnMount ?? []);
  let viewState = $state<State>(
    cachedOnMount && cachedOnMount.length > 0 ? 'ready' : 'loading'
  );

  onMount(() => {
    if (cachedOnMount && cachedOnMount.length > 0) return;
    void load();
  });

  $effect(() => {
    if ($tidalStatus !== 'connected') return;
    const cur = untrack(() => viewState);
    if (cur !== 'loading' && cur !== 'ready') void load();
  });

  async function load() {
    viewState = 'loading';
    try {
      const data = await api.getTidalMoods();
      categories = data.categories ?? [];
      if (categories.length > 0) putCachedMoodCategories(categories);
      viewState = categories.length > 0 ? 'ready' : 'empty';
    } catch (e) {
      if (e instanceof ApiError && e.status === 503) {
        clearCachedMoods();
        viewState = 'disconnected';
      } else {
        viewState = 'error';
      }
    }
  }

  function buildMenu(slug: string, title: string) {
    return [{ label: `Open ${title}`, onSelect: () => void goto(`/moods/${slug}`) }];
  }
</script>

<svelte:head><title>Moods . NOOR</title></svelte:head>

<div class="page">
  <header class="page-header">
    <p class="eyebrow">TIDAL</p>
    <h1>Moods &amp; Activities</h1>
    <p class="sub">Editorial categories from TIDAL. Click a tile to explore.</p>
  </header>

  {#if viewState === 'loading'}
    <p class="muted-line">Loading moods...</p>
  {:else if viewState === 'ready'}
    <div class="grid">
      {#each categories as c (c.slug)}
        <a
          class="card"
          href={`/moods/${c.slug}`}
          oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildMenu(c.slug, c.title), c.title); }}
        >
          {#if c.thumbnail}
            <div class="art" style="background-image:url('{c.thumbnail}')"></div>
          {:else}
            <div class="art fallback">~</div>
          {/if}
          <span class="card-title">{c.title}</span>
        </a>
      {/each}
    </div>
  {:else if viewState === 'empty'}
    <p class="muted-line">No moods available right now. <button class="inline-link" onclick={load}>Retry</button></p>
  {:else if viewState === 'disconnected'}
    <p class="muted-line">Connect TIDAL to see moods. <a class="inline-link" href="/settings#sources-tidal">Open settings</a></p>
  {:else if viewState === 'error'}
    <p class="muted-line">Couldn't load moods. <button class="inline-link" onclick={load}>Retry</button></p>
  {/if}
</div>

<style>
  .page { max-width: var(--content-width); margin: 0 auto; padding: 32px 28px 96px; display: flex; flex-direction: column; gap: 24px; }
  .page-header { display: flex; flex-direction: column; gap: 4px; }
  .eyebrow { font-size: var(--font-size-xs); letter-spacing: 0.08em; text-transform: uppercase; color: var(--text-secondary); margin: 0; }
  .page-header h1 { margin: 0; font-size: var(--font-size-3xl); font-weight: 800; }
  .page-header .sub { margin: 0; font-size: var(--font-size-sm); color: var(--text-secondary); }
  .muted-line { margin: 0; font-size: var(--font-size-sm); color: var(--text-secondary); }
  .inline-link { background: none; border: none; padding: 0; font: inherit; color: var(--accent-line); cursor: pointer; text-decoration: underline; text-underline-offset: 2px; margin-left: var(--space-1); }

  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: var(--gap); }
  .card { display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-2); border-radius: var(--radius-md); text-decoration: none; color: inherit; cursor: pointer; transition: background var(--motion-fast) ease; }
  .card:hover, .card:focus-visible { background: var(--bg-hover); outline: none; }
  .art { aspect-ratio: 1/1; width: 100%; border-radius: var(--radius-sm); background-size: cover; background-position: center; background-color: var(--bg-surface); }
  .art.fallback { display: flex; align-items: center; justify-content: center; color: #fff; font-size: var(--font-size-3xl); background: linear-gradient(135deg, #3b82f6, #8b5cf6); }
  .card-title { font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; text-align: center; }
</style>
