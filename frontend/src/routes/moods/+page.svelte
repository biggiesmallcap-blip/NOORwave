<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { ApiError, api, type TidalMoodCategory } from '$lib/api/client';
  import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
  import { tidalStatus } from '$lib/stores/tidal';
  import { openContextMenu } from '$lib/stores/context_menu';
  import { goto } from '$app/navigation';
  import {
    claimMoodThumbnailRefresh,
    getCachedMoodCategories,
    moodCategoriesNeedThumbnails,
    putCachedMoodCategories,
    clearCachedMoods,
  } from '$lib/stores/tidal-moods-cache';

  type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';
  const THUMBNAIL_REFRESH_DELAY_MS = 1800;
  // The server fills mood thumbnails via a background probe that can land a few
  // seconds after the first response, so poll a bounded number of times instead
  // of giving up after one try.
  const THUMBNAIL_RETRY_INTERVAL_MS = 2500;
  const MAX_THUMBNAIL_ATTEMPTS = 6;

  // Sync-read the cache on script init so revisiting /moods within the
  // 6h TTL renders instantly without a skeleton flash. Mirrors the
  // home-discover pattern.
  const cachedOnMount = getCachedMoodCategories();
  let categories = $state<TidalMoodCategory[]>(cachedOnMount ?? []);
  let viewState = $state<State>(
    cachedOnMount && cachedOnMount.length > 0 ? 'ready' : 'loading'
  );
  let inFlight = false;
  let loadSeq = 0;
  let thumbnailRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  let thumbnailAttempts = 0;

  onMount(() => {
    if (cachedOnMount && cachedOnMount.length > 0) {
      if (moodCategoriesNeedThumbnails(cachedOnMount)) scheduleThumbnailRefresh(cachedOnMount);
      return () => {
        loadSeq += 1;
        clearThumbnailRefresh();
      };
    }
    void load();
    return () => {
      loadSeq += 1;
      clearThumbnailRefresh();
    };
  });

  $effect(() => {
    if ($tidalStatus !== 'connected') return;
    const cur = untrack(() => viewState);
    if (cur !== 'loading' && cur !== 'ready') void load();
  });

  async function load() {
    if (inFlight) return;
    const seq = ++loadSeq;
    inFlight = true;
    if (categories.length === 0) viewState = 'loading';
    try {
      // Raw client, not cachedApi: cachedApi persists the moods response to
      // localStorage for days and serves it stale, so a cold-start thumbnail-less
      // fallback would stick forever. tidal-moods-cache handles session caching.
      const data = await api.getTidalMoods();
      if (seq !== loadSeq) return;
      const nextCategories = data.categories ?? [];
      categories = nextCategories;
      if (nextCategories.length > 0) putCachedMoodCategories(nextCategories);
      viewState = nextCategories.length > 0 ? 'ready' : 'empty';
      scheduleThumbnailRefresh(nextCategories);
    } catch (e) {
      if (seq !== loadSeq) return;
      if (e instanceof ApiError && e.status === 503) {
        clearCachedMoods();
        viewState = 'disconnected';
      } else {
        viewState = 'error';
      }
    } finally {
      if (seq === loadSeq) inFlight = false;
    }
  }

  function clearThumbnailRefresh() {
    if (!thumbnailRefreshTimer) return;
    clearTimeout(thumbnailRefreshTimer);
    thumbnailRefreshTimer = null;
  }

  function scheduleThumbnailRefresh(nextCategories: TidalMoodCategory[]) {
    clearThumbnailRefresh();
    if (!moodCategoriesNeedThumbnails(nextCategories)) {
      thumbnailAttempts = 0;
      return;
    }
    if (thumbnailAttempts >= MAX_THUMBNAIL_ATTEMPTS) return;
    const delay =
      thumbnailAttempts === 0 ? THUMBNAIL_REFRESH_DELAY_MS : THUMBNAIL_RETRY_INTERVAL_MS;
    thumbnailRefreshTimer = setTimeout(() => {
      thumbnailRefreshTimer = null;
      const firstAttempt = thumbnailAttempts === 0;
      thumbnailAttempts += 1;
      // The first refresh honours the shared cross-surface throttle; the bounded
      // follow-up polls proceed directly so a slow probe still fills the tiles in.
      // load() re-arms this poll on completion.
      if (firstAttempt && !claimMoodThumbnailRefresh(nextCategories)) {
        scheduleThumbnailRefresh(nextCategories);
        return;
      }
      void load();
    }, delay);
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
          oncontextmenu={(e) => { e.preventDefault(); e.stopPropagation(); openContextMenu(e, buildMenu(c.slug, c.title), c.title); }}
        >
          <div class="art-wrap">
            <ArtworkImage
              className="mood-art"
              src={c.thumbnail}
              alt={c.title}
              size={320}
              fallbackText="~"
            />
          </div>
          <p class="card-title">{c.title}</p>
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
  :global(.mood-art) { width: 100%; height: 100%; object-fit: cover; display: block; transition: transform var(--motion-base); }
  :global(.mood-art.fallback) { display: flex; align-items: center; justify-content: center; background: var(--bg-hover); }
  :global(.mood-art.fallback span) { font-size: var(--font-size-4xl); color: var(--text-muted); }
  .card:hover :global(.mood-art) { transform: scale(1.05); }
  .card-title { margin: 0; font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; line-height: var(--line-height-snug); }
</style>
