<script lang="ts">
  import { page } from '$app/stores';
  import { onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
  import { formatTrackDuration } from '$lib/utils/format';
  import {
    api,
    type SpotifyArtistDetail,
    type SpotifyArtistRelated,
    type SpotifyAlbumSearchItem,
    type SpotifyArtistSearchItem,
    type SpotifyPlaylistTrack,
    type SpotifyTidalState,
    type TidalPlayable,
  } from '$lib/api/client';
  import { openContextMenu, openMenuAtElement, type MenuItem } from '$lib/stores/context_menu';
  import {
    addTidalTrackToQueue,
    playTidalTrackNext,
    playTidalTrackNow,
    startTidalSongRadio,
  } from '$lib/stores/player';
  import { tidalStatus } from '$lib/stores/tidal';

  const spotifyId = $derived($page.params.id ?? '');

  let detail = $state<SpotifyArtistDetail | null>(null);
  let related = $state<SpotifyArtistRelated | null>(null);
  let pendingIds = $state<string[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let requestedSpotifyId = $state('');
  let lazyArt = $state<Record<string, string>>({});
  let loadSeq = 0;

  const POLL_INTERVAL_MS = 1500;
  const POLL_DEADLINE_MS = 30_000;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let pollDeadline = 0;
  function clearPoll() { if (pollTimer) { clearTimeout(pollTimer); pollTimer = null; } }

  function isPlayable(t: SpotifyPlaylistTrack): boolean {
    return (t.tidal.status === 'resolved' || t.tidal.status === 'low_confidence')
      && t.tidal.id !== null && $tidalStatus === 'connected';
  }

  function asTidalPlayable(t: SpotifyPlaylistTrack): TidalPlayable | null {
    if (!isPlayable(t) || !t.tidal.id) return null;
    return {
      tidal_id: t.tidal.id,
      title: t.title ?? '',
      artist_name: t.primaryArtist ?? detail?.name ?? null,
      artist_tidal_id: null,
      album_title: t.album ?? null,
      album_tidal_id: null,
      artwork_url: t.thumbnail ?? detail?.thumbnail ?? null,
      duration_ms: t.durationMs ?? null,
    };
  }

  function statusLabel(t: SpotifyTidalState): string {
    if ($tidalStatus !== 'connected') return 'Connect TIDAL to play this track';
    switch (t.status) {
      case 'resolved': return 'Play on TIDAL';
      case 'low_confidence': return 'Play (low-confidence match)';
      case 'pending': return 'Resolving on TIDAL...';
      case 'unresolved': return "Couldn't find on TIDAL";
      case 'error': return 'Resolution error';
    }
  }

  function schedulePoll(seq: number, delayMs = POLL_INTERVAL_MS) {
    pollTimer = setTimeout(() => void pollResolution(seq), delayMs);
  }

  async function pollResolution(seq: number) {
    if (seq !== loadSeq) { clearPoll(); return; }
    if (!related || pendingIds.length === 0 || Date.now() > pollDeadline) { clearPoll(); return; }
    try {
      const { entries } = await api.getResolveTidalStatus(pendingIds);
      if (seq !== loadSeq) return;
      const byId = new Map(entries.map((e) => [e.spotifyId, e.tidal]));
      const stillPending: string[] = [];
      const merge = (arr: SpotifyPlaylistTrack[]) =>
        arr.map((t) => {
          if (!t.spotifyId) return t;
          const next = byId.get(t.spotifyId);
          if (!next) return t;
          if (next.status === 'pending') stillPending.push(t.spotifyId);
          return { ...t, tidal: next };
        });
      related = {
        ...related,
        topTracks: merge(related.topTracks),
        deepCuts: merge(related.deepCuts),
      };
      pendingIds = stillPending;
      if (pendingIds.length > 0) schedulePoll(seq);
      else clearPoll();
    } catch (e) {
      if (seq !== loadSeq) return;
      console.warn('resolve status poll failed', e);
      schedulePoll(seq, POLL_INTERVAL_MS * 2);
    }
  }

  async function load(id: string) {
    const seq = ++loadSeq;
    if (!id.trim()) { error = 'Missing Spotify artist ID'; loading = false; return; }
    loading = true; error = null; detail = null; related = null; pendingIds = []; lazyArt = {};
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 15_000);
    try {
      const nextDetail = await api.getSpotifyArtist(id, controller.signal);
      if (seq !== loadSeq) return;
      detail = nextDetail;
      const rel = await api.getSpotifyArtistRelated(id, controller.signal).catch(() => null);
      if (seq !== loadSeq) return;
      if (rel) {
        related = rel;
        pendingIds = rel.pendingSpotifyIds ?? [];
      }
      pollDeadline = Date.now() + POLL_DEADLINE_MS;
      if (pendingIds.length > 0) schedulePoll(seq);
    } catch (e) {
      if (seq !== loadSeq) return;
      error = (e as Error).name === 'AbortError'
        ? 'Timed out loading artist metadata'
        : ((e as Error).message ?? 'Failed to load artist');
    } finally {
      clearTimeout(timeout);
      if (seq === loadSeq) loading = false;
    }
  }

  $effect(() => {
    const id = spotifyId.trim();
    if (!id) { loadSeq += 1; clearPoll(); requestedSpotifyId = ''; detail = null; loading = false; error = 'Missing Spotify artist ID'; return; }
    if (id !== requestedSpotifyId) { requestedSpotifyId = id; clearPoll(); void load(id); }
  });
  onDestroy(() => { loadSeq += 1; clearPoll(); });

  function buildRowMenu(t: SpotifyPlaylistTrack): MenuItem[] {
    const track = asTidalPlayable(t);
    const disabled = track === null;
    const hint = disabled ? statusLabel(t.tidal) : undefined;
    return [
      { label: 'Play now', icon: '▶', disabled, hint, onSelect: () => { if (track) void playTidalTrackNow(track); } },
      { label: 'Play next', icon: '⤴', disabled, hint, onSelect: () => { if (track) void playTidalTrackNext(track); } },
      { label: 'Add to queue', icon: '+', disabled, hint, onSelect: () => { if (track) void addTidalTrackToQueue(track); } },
      { separator: true, label: '' },
      { label: 'Song radio', icon: '◉', disabled, hint, onSelect: () => { if (track) void startTidalSongRadio(track); } },
    ];
  }

  function buildAlbumCardMenu(a: SpotifyAlbumSearchItem): MenuItem[] {
    return [{ label: 'Open album', icon: '→', onSelect: () => void goto(`/spotify-album/${a.spotifyId}`) }];
  }

  function buildArtistCardMenu(a: SpotifyArtistSearchItem): MenuItem[] {
    return [{ label: 'Open artist', icon: '→', onSelect: () => void goto(`/spotify-artist/${a.spotifyId}`) }];
  }

  function handleRowContextMenu(e: MouseEvent, t: SpotifyPlaylistTrack) {
    e.preventDefault();
    e.stopPropagation();
    openContextMenu(e, buildRowMenu(t), t.title ?? 'Spotify track');
  }

  function formatNumber(n: number | null): string {
    if (n === null || n === undefined) return '';
    if (n < 1_000) return n.toString();
    if (n < 1_000_000) return `${(n / 1_000).toFixed(1)}K`;
    if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    return `${(n / 1_000_000_000).toFixed(1)}B`;
  }
</script>

<svelte:head>
  <title>{detail?.name ?? 'Spotify artist'} . NOOR</title>
</svelte:head>

<div class="page">
  <a class="back-link" href="/search">&lt; Back to search</a>
  {#if loading}
    <div class="state">Loading artist...</div>
  {:else if error}
    <div class="state error">Couldn't load this artist: {error}</div>
  {:else if detail}
    <header class="header">
      {#if detail.thumbnail}
        <div class="cover round" style="background-image:url('{detail.thumbnail}')"></div>
      {:else}
        <div class="cover round fallback">@</div>
      {/if}
      <div class="meta">
        <span class="kicker">Spotify artist . ephemeral</span>
        <h1 class="title">{detail.name ?? '-'}</h1>
        <div class="stats">
          {#if detail.monthlyListeners !== null}<span>{formatNumber(detail.monthlyListeners)} monthly listeners</span>{/if}
          {#if detail.followers !== null}<span>. {formatNumber(detail.followers)} followers</span>{/if}
          {#if detail.worldRank !== null}<span>. World rank #{formatNumber(detail.worldRank)}</span>{/if}
        </div>
        {#if detail.genres.length > 0}
          <div class="genres">{detail.genres.slice(0, 5).join(' . ')}</div>
        {/if}
        {#if detail.biography}
          <p class="bio">{detail.biography.slice(0, 320)}{detail.biography.length > 320 ? '...' : ''}</p>
        {/if}
      </div>
    </header>

    {#if related}
      {#each [
        { heading: 'Top tracks', items: related.topTracks },
        { heading: 'Deep cuts', items: related.deepCuts },
      ] as section (section.heading)}
        {#if section.items.length > 0}
          <section class="shelf">
            <h2>{section.heading}</h2>
            <ol class="tracks">
              {#each section.items as t, i (`${section.heading}:${t.spotifyId ?? 'missing'}:${i}`)}
                {@const rowKey = `${section.heading}:${t.spotifyId ?? 'missing'}:${i}`}
                {@const rowPlayable = asTidalPlayable(t) !== null}
                <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
                <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
                <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
                <li
                  class="row"
                  class:disabled={!rowPlayable}
                  role="button"
                  tabindex={rowPlayable ? 0 : -1}
                  aria-disabled={!rowPlayable}
                  title={statusLabel(t.tidal)}
                  onclick={() => { const tr = asTidalPlayable(t); if (tr) void playTidalTrackNow(tr); }}
                  onkeydown={(e) => { if (e.key !== 'Enter' && e.key !== ' ') return; e.preventDefault(); const tr = asTidalPlayable(t); if (tr) void playTidalTrackNow(tr); }}
                  oncontextmenu={(e) => handleRowContextMenu(e, t)}
                  use:lazyTidalArt={{
                    enabled: !t.thumbnail && !!t.primaryArtist,
                    query: { artist: t.primaryArtist, title: t.title },
                    onResolve: (url) => (lazyArt[rowKey] = url),
                  }}
                >
                  <span class="rank">{i + 1}</span>
                  {#if t.thumbnail || lazyArt[rowKey]}
                    <div class="thumb" style="background-image:url('{t.thumbnail ?? lazyArt[rowKey]}')"></div>
                  {:else}
                    <div class="thumb fallback">M</div>
                  {/if}
                  <div class="row-meta">
                    <span class="row-title">{t.title ?? '-'}</span>
                    <span class="row-artist">{t.primaryArtist ?? ''}</span>
                  </div>
                  <span class="dur">{formatTrackDuration(t.durationMs)}</span>
                  <button
                    class="row-btn"
                    title="More actions"
                    aria-label="More actions"
                    onclick={(e) => { e.stopPropagation(); openMenuAtElement(e.currentTarget as HTMLElement, buildRowMenu(t), t.title ?? 'Spotify track'); }}
                  >More</button>
                </li>
              {/each}
            </ol>
          </section>
        {/if}
      {/each}

      {#if related.recentReleases.length > 0}
        <section class="shelf">
          <h2>Recent releases</h2>
          <div class="card-rail">
            {#each related.recentReleases as a (a.spotifyId)}
              <a
                class="card"
                href={`/spotify-album/${a.spotifyId}`}
                oncontextmenu={(e) => { e.preventDefault(); e.stopPropagation(); openContextMenu(e, buildAlbumCardMenu(a), a.title ?? 'Spotify album'); }}
              >
                {#if a.thumbnail}
                  <div class="art" style="background-image:url('{a.thumbnail}')"></div>
                {:else}
                  <div class="art fallback">M</div>
                {/if}
                <span class="card-title">{a.title ?? '-'}</span>
                {#if a.releaseDate}<span class="card-sub">{a.releaseDate.slice(0, 4)}</span>{/if}
              </a>
            {/each}
          </div>
        </section>
      {/if}

      {#if related.similarArtists.length > 0}
        <section class="shelf">
          <h2>Similar artists</h2>
          <div class="card-rail">
            {#each related.similarArtists as a (a.spotifyId)}
              <a
                class="card artist"
                href={`/spotify-artist/${a.spotifyId}`}
                oncontextmenu={(e) => { e.preventDefault(); e.stopPropagation(); openContextMenu(e, buildArtistCardMenu(a), a.name ?? 'Spotify artist'); }}
              >
                {#if a.thumbnail}
                  <div class="art round" style="background-image:url('{a.thumbnail}')"></div>
                {:else}
                  <div class="art round fallback">@</div>
                {/if}
                <span class="card-title">{a.name ?? '-'}</span>
                {#if a.followers !== null}<span class="card-sub">{formatNumber(a.followers)} followers</span>{/if}
              </a>
            {/each}
          </div>
        </section>
      {/if}
    {/if}
  {/if}
</div>

<style>
  .page { max-width: var(--content-width); margin: 0 auto; padding: 32px 28px 96px; display: flex; flex-direction: column; gap: 32px; }
  .page > .back-link { align-self: flex-start; margin-bottom: var(--space-3); }
  .state { padding: 80px 0; text-align: center; color: var(--text-muted); }
  .state.error { color: #ef4444; }
  .header { display: grid; grid-template-columns: 220px 1fr; gap: 28px; align-items: end; }
  .cover { width: 220px; height: 220px; border-radius: var(--radius-md); background-size: cover; background-position: center; box-shadow: 0 18px 36px -16px rgba(0,0,0,.6); }
  .cover.round { border-radius: 50%; }
  .cover.fallback { display: flex; align-items: center; justify-content: center; background: linear-gradient(135deg, var(--service-spotify), #1aa34a); color: #fff; font-size: var(--font-size-4xl); }
  .meta { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
  .kicker { font-size: var(--font-size-xs); letter-spacing: 0.08em; text-transform: uppercase; color: var(--service-spotify); font-weight: var(--font-weight-bold); }
  .title { margin: 0; font-size: var(--font-size-3xl); font-weight: 800; color: var(--text-primary); }
  .stats { display: flex; flex-wrap: wrap; gap: 6px; color: var(--text-muted); font-size: var(--font-size-xs); }
  .genres { color: var(--text-secondary); font-size: var(--font-size-xs); }
  .bio { margin: 4px 0 0; color: var(--text-secondary); font-size: var(--font-size-sm); max-width: 60ch; }
  .shelf h2 { font-size: var(--font-size-lg); font-weight: var(--font-weight-bold); margin: 0 0 12px; }
  .tracks { list-style: none; margin: 0; padding: 0; }
  .row { display: grid; grid-template-columns: 36px 44px minmax(0,1fr) auto auto; gap: 14px; align-items: center; padding: 8px 12px; border-radius: 8px; cursor: pointer; }
  .row:hover { background: rgba(255,255,255,.04); }
  .row.disabled { cursor: default; opacity: 0.55; }
  .rank { color: var(--text-muted); text-align: center; font-variant-numeric: tabular-nums; }
  .thumb { width: 44px; height: 44px; border-radius: 4px; background-size: cover; background-position: center; background-color: var(--bg-raised); }
  .thumb.fallback { display: flex; align-items: center; justify-content: center; color: var(--text-muted); }
  .row-meta { display: flex; flex-direction: column; min-width: 0; }
  .row-title { color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-size: var(--font-size-sm); font-weight: var(--font-weight-medium); }
  .row-artist { color: var(--text-secondary); font-size: var(--font-size-xs); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .dur { color: var(--text-muted); font-size: var(--font-size-xs); font-variant-numeric: tabular-nums; min-width: 36px; text-align: right; }
  .row-btn { border: none; min-width: 30px; height: 30px; padding: 0 8px; border-radius: 999px; background: rgba(255,255,255,.06); color: var(--text-secondary); cursor: pointer; font-size: var(--font-size-xs); font-weight: var(--font-weight-bold); }
  .row-btn:hover { background: rgba(255,255,255,.12); color: var(--text-primary); }

  .card-rail { display: flex; gap: var(--gap-sm); overflow-x: auto; padding-bottom: var(--space-2); scroll-snap-type: x mandatory; }
  .card { --card-w: clamp(120px, 11vw, 168px); flex: 0 0 var(--card-w); width: var(--card-w); display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-2); border-radius: var(--radius-md); text-decoration: none; color: inherit; cursor: pointer; transition: background var(--motion-fast); }
  .card:hover, .card:focus-visible { background: var(--bg-hover); outline: none; }
  .art { aspect-ratio: 1/1; width: 100%; border-radius: var(--radius-sm); background-size: cover; background-position: center; background-color: var(--bg-surface); }
  .art.round { border-radius: 50%; }
  .art.fallback { display: flex; align-items: center; justify-content: center; color: var(--text-muted); font-size: var(--font-size-3xl); }
  .card-title { font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .card-sub { font-size: var(--font-size-xs); color: var(--text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  @media (max-width: 760px) {
    .page { padding: 24px 16px 88px; gap: 24px; }
    .header { grid-template-columns: 96px 1fr; gap: 16px; align-items: start; }
    .cover { width: 96px; height: 96px; }
    .title { font-size: var(--font-size-xl); }
    .row { grid-template-columns: 28px 40px minmax(0,1fr) auto; gap: 10px; }
    .dur { display: none; }
  }
</style>
