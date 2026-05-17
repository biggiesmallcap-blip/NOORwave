<script lang="ts">
  import { page } from '$app/stores';
  import { onMount, untrack } from 'svelte';
  import { ApiError, api, type TidalHomeModule } from '$lib/api/client';
  import { tidalStatus } from '$lib/stores/tidal';
  import TidalDiscoverShelves from '$lib/components/search/TidalDiscoverShelves.svelte';
  import { getCachedMoodPage, putCachedMoodPage } from '$lib/stores/tidal-moods-cache';

  type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

  const slug = $derived($page.params.slug ?? '');

  let modules = $state<TidalHomeModule[]>([]);
  let viewState = $state<State>('loading');
  let title = $state('');
  let requestedSlug = $state('');

  onMount(() => { if (slug) void load(slug); });

  $effect(() => {
    const s = slug.trim();
    if (!s) return;
    if (s !== requestedSlug) {
      requestedSlug = s;
      void load(s);
    }
  });

  $effect(() => {
    if ($tidalStatus !== 'connected') return;
    const cur = untrack(() => viewState);
    if (cur !== 'loading' && cur !== 'ready' && slug) void load(slug);
  });

  async function load(s: string) {
    title = humanize(s);
    // Cache hit: render immediately without a skeleton, no network call.
    const cached = getCachedMoodPage(s);
    if (cached && cached.length > 0) {
      modules = cached;
      viewState = 'ready';
      return;
    }
    viewState = 'loading';
    try {
      const data = await api.getTidalMoodPage(s);
      modules = data.modules ?? [];
      if (modules.length > 0) putCachedMoodPage(s, modules);
      viewState = modules.length > 0 ? 'ready' : 'empty';
    } catch (e) {
      if (e instanceof ApiError && e.status === 503) {
        viewState = 'disconnected';
      } else {
        viewState = 'error';
      }
    }
  }

  function humanize(s: string): string {
    return s
      .replace(/^mood_/, '')
      .replace(/^m_/, '')
      .replace(/_/g, ' ')
      .replace(/\b\w/g, (c) => c.toUpperCase());
  }
</script>

<svelte:head><title>{title || 'Mood'} . NOOR</title></svelte:head>

<div class="page">
  <a class="back-link" href="/moods">&lt; All moods</a>
  <header class="page-header">
    <p class="eyebrow">TIDAL mood</p>
    <h1>{title || '...'}</h1>
  </header>
  {#if viewState === 'loading'}
    <p class="muted-line">Loading {title}...</p>
  {:else if viewState === 'ready'}
    <TidalDiscoverShelves {modules} />
  {:else if viewState === 'empty'}
    <p class="muted-line">No content for this mood right now. <button class="inline-link" onclick={() => slug && load(slug)}>Retry</button></p>
  {:else if viewState === 'disconnected'}
    <p class="muted-line">Connect TIDAL to see moods. <a class="inline-link" href="/settings#sources-tidal">Open settings</a></p>
  {:else if viewState === 'error'}
    <p class="muted-line">Couldn't load this mood. <button class="inline-link" onclick={() => slug && load(slug)}>Retry</button></p>
  {/if}
</div>

<style>
  .page { max-width: var(--content-width); margin: 0 auto; padding: 32px 28px 96px; display: flex; flex-direction: column; gap: 24px; }
  .back-link { align-self: flex-start; font-size: var(--font-size-sm); color: var(--text-secondary); text-decoration: none; }
  .back-link:hover { color: var(--text-primary); text-decoration: underline; }
  .page-header { display: flex; flex-direction: column; gap: 4px; }
  .eyebrow { font-size: var(--font-size-xs); letter-spacing: 0.08em; text-transform: uppercase; color: var(--text-secondary); margin: 0; }
  .page-header h1 { margin: 0; font-size: var(--font-size-3xl); font-weight: 800; }
  .muted-line { margin: 0; font-size: var(--font-size-sm); color: var(--text-secondary); }
  .inline-link { background: none; border: none; padding: 0; font: inherit; color: var(--accent-line); cursor: pointer; text-decoration: underline; text-underline-offset: 2px; margin-left: var(--space-1); }
</style>
