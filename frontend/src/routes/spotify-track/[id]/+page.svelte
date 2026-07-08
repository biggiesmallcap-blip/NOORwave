<script lang="ts">
  import { page } from '$app/stores';
  import { onDestroy } from 'svelte';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
  import { goBack } from '$lib/navigation/back';
  import { formatTrackDuration } from '$lib/utils/format';
  import {
    api,
    type SpotifyTrackDetail,
    type SpotifyTrackRelated,
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

  let detail = $state<SpotifyTrackDetail | null>(null);
  let related = $state<SpotifyTrackRelated | null>(null);
  let pendingIds = $state<string[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let saving = $state(false);
  let saveResult = $state<string | null>(null);
  let saveErr = $state<string | null>(null);
  let requestedSpotifyId = $state('');
  let lazyArt = $state<Record<string, string>>({});
  let loadSeq = 0;

  const canSave = $derived(
    detail !== null &&
      detail.tidal.id !== null &&
      (detail.tidal.status === 'resolved' || detail.tidal.status === 'low_confidence'),
  );

  const POLL_INTERVAL_MS = 1500;
  const POLL_DEADLINE_MS = 30_000;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let pollDeadline = 0;

  function clearPoll() {
    if (pollTimer) { clearTimeout(pollTimer); pollTimer = null; }
  }

  function asTidalPlayableFromDetail(d: SpotifyTrackDetail): TidalPlayable | null {
    if ($tidalStatus !== 'connected') return null;
    if (d.tidal.status !== 'resolved' && d.tidal.status !== 'low_confidence') return null;
    if (d.tidal.id === null) return null;
    return {
      tidal_id: d.tidal.id,
      title: d.title ?? '',
      artist_name: d.primaryArtist,
      artist_tidal_id: null,
      album_title: d.album,
      album_tidal_id: null,
      artwork_url: d.thumbnail,
      duration_ms: d.durationMs,
    };
  }

  function asTidalPlayableFromRow(t: SpotifyPlaylistTrack): TidalPlayable | null {
    if ($tidalStatus !== 'connected') return null;
    if (t.tidal.status !== 'resolved' && t.tidal.status !== 'low_confidence') return null;
    if (t.tidal.id === null) return null;
    return {
      tidal_id: t.tidal.id,
      title: t.title ?? '',
      artist_name: t.primaryArtist ?? null,
      artist_tidal_id: null,
      album_title: t.album ?? null,
      album_tidal_id: null,
      artwork_url: t.thumbnail ?? null,
      duration_ms: t.durationMs ?? null,
    };
  }

  function schedulePoll(seq: number, delayMs = POLL_INTERVAL_MS) {
    pollTimer = setTimeout(() => void pollResolution(seq), delayMs);
  }

  async function pollResolution(seq: number) {
    if (seq !== loadSeq) {
      clearPoll();
      return;
    }
    if (!detail || pendingIds.length === 0 || Date.now() > pollDeadline) {
      clearPoll();
      return;
    }
    try {
      const { entries } = await api.getResolveTidalStatus(pendingIds);
      if (seq !== loadSeq) return;
      const byId = new Map(entries.map((e) => [e.spotifyId, e.tidal]));
      const stillPending: string[] = [];
      const headerSpotifyId = detail.spotifyId;
      if (headerSpotifyId && byId.has(headerSpotifyId)) {
        const next = byId.get(headerSpotifyId)!;
        if (next.status === 'pending') stillPending.push(headerSpotifyId);
        detail = { ...detail, tidal: next };
      }
      if (related) {
        const merge = (arr: SpotifyPlaylistTrack[]): SpotifyPlaylistTrack[] =>
          arr.map((t) => {
            if (!t.spotifyId) return t;
            const next = byId.get(t.spotifyId);
            if (!next) return t;
            if (next.status === 'pending') stillPending.push(t.spotifyId);
            return { ...t, tidal: next };
          });
        related = {
          ...related,
          moreFromAlbum: merge(related.moreFromAlbum),
          moreFromArtist: merge(related.moreFromArtist),
        };
      }
      pendingIds = stillPending;
      if (pendingIds.length > 0) {
        schedulePoll(seq);
      } else {
        clearPoll();
      }
    } catch (e) {
      if (seq !== loadSeq) return;
      console.warn('resolve status poll failed', e);
      schedulePoll(seq, POLL_INTERVAL_MS * 2);
    }
  }

  async function load(id: string) {
    const seq = ++loadSeq;
    if (!id.trim()) {
      error = 'Missing Spotify track ID';
      loading = false;
      return;
    }
    loading = true;
    error = null;
    detail = null;
    related = null;
    pendingIds = [];
    saveResult = null;
    saveErr = null;
    lazyArt = {};
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 15_000);
    try {
      const nextDetail = await api.getSpotifyTrack(id, controller.signal);
      if (seq !== loadSeq) return;
      detail = nextDetail;
      const rel = await api.getSpotifyTrackRelated(id, controller.signal).catch(() => null);
      if (seq !== loadSeq) return;
      if (rel) {
        related = rel;
        pendingIds = rel.pendingSpotifyIds ?? [];
      }
      if (detail.tidal.status === 'pending' && detail.spotifyId) {
        pendingIds = [...pendingIds, detail.spotifyId];
      }
      pollDeadline = Date.now() + POLL_DEADLINE_MS;
      if (pendingIds.length > 0) {
        schedulePoll(seq);
      }
    } catch (e) {
      if (seq !== loadSeq) return;
      error =
        (e as Error).name === 'AbortError'
          ? 'Timed out loading track metadata'
          : ((e as Error).message ?? 'Failed to load track');
    } finally {
      clearTimeout(timeout);
      if (seq === loadSeq) loading = false;
    }
  }

  $effect(() => {
    const id = spotifyId.trim();
    if (!id) {
      loadSeq += 1;
      clearPoll(); requestedSpotifyId = ''; detail = null; pendingIds = []; saveResult = null; saveErr = null; loading = false;
      error = 'Missing Spotify track ID';
      return;
    }
    if (id !== requestedSpotifyId) {
      requestedSpotifyId = id;
      clearPoll();
      void load(id);
    }
  });

  onDestroy(() => { loadSeq += 1; clearPoll(); });

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

  function formatNumber(n: number | null): string {
    if (n === null || n === undefined) return '';
    if (n < 1_000) return n.toString();
    if (n < 1_000_000) return `${(n / 1_000).toFixed(1)}K`;
    if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    return `${(n / 1_000_000_000).toFixed(1)}B`;
  }

  function handleHeaderContextMenu(e: MouseEvent) {
    if (!detail) return;
    e.preventDefault();
    e.stopPropagation();
    openContextMenu(e, buildHeaderMenu(detail), detail.title ?? 'Spotify track');
  }

  function buildHeaderMenu(d: SpotifyTrackDetail): MenuItem[] {
    const track = asTidalPlayableFromDetail(d);
    const disabled = track === null;
    const hint = disabled ? statusLabel(d.tidal) : undefined;
    return [
      { label: 'Play now', icon: '▶', disabled, hint, onSelect: () => { if (track) void playTidalTrackNow(track); } },
      { label: 'Play next', icon: '⤴', disabled, hint, onSelect: () => { if (track) void playTidalTrackNext(track); } },
      { label: 'Add to queue', icon: '+', disabled, hint, onSelect: () => { if (track) void addTidalTrackToQueue(track); } },
      { separator: true, label: '' },
      { label: 'Song radio', icon: '◉', disabled, hint, onSelect: () => { if (track) void startTidalSongRadio(track); } },
    ];
  }

  function buildRowMenu(t: SpotifyPlaylistTrack): MenuItem[] {
    const track = asTidalPlayableFromRow(t);
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

  function handleRowContextMenu(e: MouseEvent, t: SpotifyPlaylistTrack) {
    e.preventDefault();
    e.stopPropagation();
    openContextMenu(e, buildRowMenu(t), t.title ?? 'Spotify track');
  }

  async function save() {
    if (!detail || saving || !canSave) return;
    const id = detail.spotifyId ?? spotifyId.trim();
    if (!id) {
      saveErr = 'Missing Spotify track ID';
      return;
    }
    saving = true;
    saveErr = null;
    saveResult = null;
    try {
      const res = await api.saveSpotifyTrack(id);
      const skipped = res.unresolvedCount + res.importFailures;
      saveResult =
        skipped > 0
          ? `Saved ${res.imported} track. ${skipped} unavailable on TIDAL were skipped.`
          : `Saved ${res.imported} track.`;
    } catch (e) {
      saveErr = (e as Error).message ?? 'Save failed';
    } finally {
      saving = false;
    }
  }
</script>

<svelte:head>
  <title>{detail?.title ?? 'Spotify track'} . NOOR</title>
</svelte:head>

<div class="page">
  <button class="back-link" type="button" onclick={() => goBack('/search')}>&lt; Back</button>
  {#if loading}
    <div class="state">Loading track...</div>
  {:else if error}
    <div class="state error">Couldn't load this track: {error}</div>
  {:else if detail}
    {@const headerTrack = asTidalPlayableFromDetail(detail)}
    {@const playable = headerTrack !== null}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <header class="header" oncontextmenu={handleHeaderContextMenu}>
      {#if detail.thumbnail}
        <div class="cover" style="background-image:url('{detail.thumbnail}')"></div>
      {:else}
        <div class="cover fallback">M</div>
      {/if}
      <div class="meta">
        <span class="kicker">Spotify track . ephemeral</span>
        <h1 class="title">{detail.title ?? '-'}</h1>
        <div class="stats">
          {#if detail.primaryArtist}
            <!-- Artist pages are TIDAL + local library only; the spotify-artist
                 route was an unreachable dead layer and has been removed. -->
            <span>{detail.primaryArtist}</span>
          {/if}
          {#if detail.album}
            {#if detail.albumId}
              <a href={`/spotify-album/${detail.albumId}`}>. {detail.album}</a>
            {:else}
              <span>. {detail.album}</span>
            {/if}
          {/if}
          {#if detail.durationMs}<span>. {formatTrackDuration(detail.durationMs)}</span>{/if}
          {#if detail.playcount !== null}<span>. {formatNumber(detail.playcount)} plays</span>{/if}
        </div>
        <div class="actions">
          <button class="btn-primary" disabled={!playable} onclick={() => headerTrack && playTidalTrackNow(headerTrack)}>Play</button>
          <button class="btn-secondary" disabled={!playable} onclick={() => headerTrack && playTidalTrackNext(headerTrack)}>Play next</button>
          <button class="btn-secondary" disabled={!playable} onclick={() => headerTrack && addTidalTrackToQueue(headerTrack)}>Add to queue</button>
          <button class="btn-secondary" disabled={!playable} onclick={() => headerTrack && startTidalSongRadio(headerTrack)}>Song radio</button>
          <button class="btn-secondary" disabled={saving || !canSave} onclick={save}>
            {saving ? 'Saving...' : 'Save to library'}
          </button>
          {#if pendingIds.length > 0}<span class="resolving-badge">Resolving {pendingIds.length} more...</span>{/if}
        </div>
        {#if saveResult}
          <p class="toast success">{saveResult}</p>
        {/if}
        {#if saveErr}
          <p class="toast error">Save failed: {saveErr}</p>
        {/if}
      </div>
    </header>

    {#if related}
      {#each [
        { heading: 'More from this album', items: related.moreFromAlbum },
        { heading: 'More from this artist', items: related.moreFromArtist },
      ] as section (section.heading)}
        {#if section.items.length > 0}
          <section class="shelf">
            <h2>{section.heading}</h2>
            <ol class="tracks">
              {#each section.items as t, i (`${section.heading}:${t.spotifyId ?? 'missing'}:${i}`)}
                {@const rowKey = `${section.heading}:${t.spotifyId ?? 'missing'}:${i}`}
                {@const rowPlayable = asTidalPlayableFromRow(t) !== null}
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
                  onclick={() => { const tr = asTidalPlayableFromRow(t); if (tr) void playTidalTrackNow(tr); }}
                  onkeydown={(e) => { if (e.key !== 'Enter' && e.key !== ' ') return; e.preventDefault(); const tr = asTidalPlayableFromRow(t); if (tr) void playTidalTrackNow(tr); }}
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
  .cover.fallback { display: flex; align-items: center; justify-content: center; background: linear-gradient(135deg, var(--service-spotify), #1aa34a); color: #fff; font-size: var(--font-size-4xl); }
  .meta { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
  .kicker { font-size: var(--font-size-xs); letter-spacing: 0.08em; text-transform: uppercase; color: var(--service-spotify); font-weight: var(--font-weight-bold); }
  .title { margin: 0; font-size: var(--font-size-3xl); font-weight: 800; color: var(--text-primary); }
  .stats { display: flex; flex-wrap: wrap; gap: 6px; color: var(--text-muted); font-size: var(--font-size-xs); }
  .actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 8px; }
  .btn-primary, .btn-secondary { background: var(--accent); color: var(--bg-base); border: none; padding: 9px 14px; border-radius: 999px; font-weight: var(--font-weight-bold); cursor: pointer; font-size: var(--font-size-sm); }
  .btn-secondary { background: var(--border-subtle); color: var(--text-primary); border: 1px solid var(--panel-border); }
  .btn-primary:disabled, .btn-secondary:disabled { opacity: 0.5; cursor: not-allowed; }
  .resolving-badge { font-size: var(--font-size-xs); color: var(--text-muted); font-style: italic; }
  .toast { margin: var(--space-2) 0 0; font-size: var(--font-size-xs); padding: var(--space-2) var(--space-3); border-radius: var(--radius-sm); width: fit-content; }
  .toast.success { background: rgba(125, 200, 175, 0.12); color: var(--accent); }
  .toast.error { background: rgba(239, 68, 68, 0.12); color: var(--state-error); }
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
  @media (max-width: 760px) {
    .page { padding: 24px 16px 88px; gap: 24px; }
    .header { grid-template-columns: 96px 1fr; gap: 16px; align-items: start; }
    .cover { width: 96px; height: 96px; border-radius: 10px; }
    .title { font-size: var(--font-size-xl); }
    .row { grid-template-columns: 28px 40px minmax(0,1fr) auto; gap: 10px; }
    .dur { display: none; }
  }
</style>
