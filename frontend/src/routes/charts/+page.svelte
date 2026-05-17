<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { ApiError, api, type TidalHomeModule } from '$lib/api/client';
  import { tidalStatus } from '$lib/stores/tidal';
  import TidalDiscoverShelves from '$lib/components/search/TidalDiscoverShelves.svelte';

  type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

  let modules = $state<TidalHomeModule[]>([]);
  let viewState = $state<State>('loading');

  onMount(load);

  $effect(() => {
    if ($tidalStatus !== 'connected') return;
    const cur = untrack(() => viewState);
    if (cur !== 'loading' && cur !== 'ready') void load();
  });

  async function load() {
    viewState = 'loading';
    try {
      const data = await api.getTidalPage('charts');
      modules = data.modules ?? [];
      viewState = modules.length > 0 ? 'ready' : 'empty';
    } catch (e) {
      if (e instanceof ApiError && e.status === 503) {
        viewState = 'disconnected';
      } else {
        viewState = 'error';
      }
    }
  }
</script>

<svelte:head><title>Charts . NOOR</title></svelte:head>

<div class="page">
  <header class="page-header">
    <p class="eyebrow">TIDAL</p>
    <h1>Charts</h1>
  </header>
  {#if viewState === 'loading'}
    <p class="muted-line">Loading charts...</p>
  {:else if viewState === 'ready'}
    <TidalDiscoverShelves {modules} />
  {:else if viewState === 'empty'}
    <p class="muted-line">No charts available right now. <button class="inline-link" onclick={load}>Retry</button></p>
  {:else if viewState === 'disconnected'}
    <p class="muted-line">Connect TIDAL to see charts. <a class="inline-link" href="/settings#sources-tidal">Open settings</a></p>
  {:else if viewState === 'error'}
    <p class="muted-line">Couldn't load charts. <button class="inline-link" onclick={load}>Retry</button></p>
  {/if}
</div>

<style>
  .page { max-width: var(--content-width); margin: 0 auto; padding: 32px 28px 96px; display: flex; flex-direction: column; gap: 24px; }
  .page-header { display: flex; flex-direction: column; gap: 4px; }
  .eyebrow { font-size: var(--font-size-xs); letter-spacing: 0.08em; text-transform: uppercase; color: var(--text-secondary); margin: 0; }
  .page-header h1 { margin: 0; font-size: var(--font-size-3xl); font-weight: 800; }
  .muted-line { margin: 0; font-size: var(--font-size-sm); color: var(--text-secondary); }
  .inline-link { background: none; border: none; padding: 0; font: inherit; color: var(--accent-line); cursor: pointer; text-decoration: underline; text-underline-offset: 2px; margin-left: var(--space-1); }
</style>
