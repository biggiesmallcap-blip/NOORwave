<script lang="ts">
  import { page } from '$app/stores';
  import { onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
  import { goBack } from '$lib/navigation/back';
  import { formatTrackDuration } from '$lib/utils/format';
  import {
    api,
    type SpotifyAlbumDetail,
    type SpotifyAlbumRelated,
    type SpotifyAlbumSearchItem,
    type SpotifyPlaylistTrack,
    type SpotifyTidalState,
    type TidalPlayable,
  } from '$lib/api/client';
  import { openContextMenu, openMenuAtElement, type MenuItem } from '$lib/stores/context_menu';
  import {
    addTidalTrackToQueue,
    addTidalTracksToQueue,
    playTidalTrackNext,
    playTidalTrackNow,
    playTidalTracksNext,
    playTidalTracksNow,
    shuffleTidalTracksNow,
    startTidalSongRadio,
  } from '$lib/stores/player';
  import { tidalStatus } from '$lib/stores/tidal';

  const spotifyId = $derived($page.params.id ?? '');

  let detail = $state<SpotifyAlbumDetail | null>(null);
  let related = $state<SpotifyAlbumRelated | null>(null);
  let pendingIds = $state<string[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let saving = $state(false);
  let saveResult = $state<string | null>(null);
  let saveErr = $state<string | null>(null);
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
      artist_name: t.primaryArtist ?? detail?.primaryArtist ?? null,
      artist_tidal_id: null,
      album_title: t.album ?? detail?.title ?? null,
      album_tidal_id: null,
      artwork_url: t.thumbnail ?? detail?.thumbnail ?? null,
      duration_ms: t.durationMs ?? null,
    };
  }

  function playableTracks(): TidalPlayable[] {
    return (detail?.tracks ?? [])
      .map(asTidalPlayable)
      .filter((t): t is TidalPlayable => t !== null);
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
    if (!detail || pendingIds.length === 0 || Date.now() > pollDeadline) { clearPoll(); return; }
    try {
      const { entries } = await api.getResolveTidalStatus(pendingIds);
      if (seq !== loadSeq) return;
      const byId = new Map(entries.map((e) => [e.spotifyId, e.tidal]));
      const stillPending: string[] = [];
      const mergeRows = (arr: SpotifyPlaylistTrack[]) =>
        arr.map((t) => {
          if (!t.spotifyId) return t;
          const next = byId.get(t.spotifyId);
          if (!next) return t;
          if (next.status === 'pending') stillPending.push(t.spotifyId);
          return { ...t, tidal: next };
        });
      detail = { ...detail, tracks: mergeRows(detail.tracks) };
      if (related) {
        related = { ...related, moreFromArtist: mergeRows(related.moreFromArtist) };
      }
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
    if (!id.trim()) { error = 'Missing Spotify album ID'; loading = false; return; }
    loading = true; error = null; detail = null; related = null; pendingIds = []; saveResult = null; saveErr = null; lazyArt = {};
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 15_000);
    try {
      const res = await api.getSpotifyAlbum(id, controller.signal);
      if (seq !== loadSeq) return;
      detail = res.album;
      pendingIds = res.pendingSpotifyIds ?? [];
      const rel = await api.getSpotifyAlbumRelated(id, controller.signal).catch(() => null);
      if (seq !== loadSeq) return;
      if (rel) {
        related = rel;
        pendingIds = [...pendingIds, ...(rel.pendingSpotifyIds ?? [])];
      }
      pollDeadline = Date.now() + POLL_DEADLINE_MS;
      if (pendingIds.length > 0) schedulePoll(seq);
    } catch (e) {
      if (seq !== loadSeq) return;
      error = (e as Error).name === 'AbortError'
        ? 'Timed out loading album metadata'
        : ((e as Error).message ?? 'Failed to load album');
    } finally {
      clearTimeout(timeout);
      if (seq === loadSeq) loading = false;
    }
  }

  $effect(() => {
    const id = spotifyId.trim();
    if (!id) { loadSeq += 1; clearPoll(); requestedSpotifyId = ''; detail = null; saveResult = null; saveErr = null; loading = false; error = 'Missing Spotify album ID'; return; }
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

  function buildAlbumCardMenu(album: SpotifyAlbumSearchItem): MenuItem[] {
    return [
      { label: 'Open album', icon: '→', onSelect: () => void goto(`/spotify-album/${album.spotifyId}`) },
    ];
  }

  function handleRowContextMenu(e: MouseEvent, t: SpotifyPlaylistTrack) {
    e.preventDefault();
    e.stopPropagation();
    openContextMenu(e, buildRowMenu(t), t.title ?? 'Spotify track');
  }

  function handleMoreClick(e: MouseEvent, t: SpotifyPlaylistTrack) {
    e.stopPropagation();
    openMenuAtElement(e.currentTarget as HTMLElement, buildRowMenu(t), t.title ?? 'Spotify track');
  }

  const playableCount = $derived(detail?.tracks.filter(isPlayable).length ?? 0);
  const resolvedCount = $derived(
    detail?.tracks.filter((t) => t.tidal.status === 'resolved' || t.tidal.status === 'low_confidence').length ?? 0,
  );
  const totalCount = $derived(detail?.tracks.length ?? 0);

  async function playAll() { await playTidalTracksNow(playableTracks(), detail?.title ?? 'Spotify album'); }
  async function shuffleAll() { await shuffleTidalTracksNow(playableTracks(), detail?.title ?? 'Spotify album'); }
  async function playAllNext() { await playTidalTracksNext(playableTracks()); }
  async function addAllToQueue() { await addTidalTracksToQueue(playableTracks()); }

  function trackLabel(count: number): string {
    return count === 1 ? 'track' : 'tracks';
  }

  async function save() {
    if (!detail || saving || resolvedCount === 0) return;
    const id = detail.spotifyId ?? spotifyId.trim();
    if (!id) {
      saveErr = 'Missing Spotify album ID';
      return;
    }
    saving = true;
    saveErr = null;
    saveResult = null;
    try {
      const res = await api.saveSpotifyAlbum(id);
      const skipped = res.unresolvedCount + res.importFailures;
      saveResult =
        skipped > 0
          ? `Saved ${res.imported} ${trackLabel(res.imported)}. ${skipped} unavailable on TIDAL were skipped.`
          : `Saved ${res.imported} ${trackLabel(res.imported)}.`;
    } catch (e) {
      saveErr = (e as Error).message ?? 'Save failed';
    } finally {
      saving = false;
    }
  }
</script>

<svelte:head>
  <title>{detail?.title ?? 'Spotify album'} . NOOR</title>
</svelte:head>

<div class="page">
  <button class="back-link" type="button" onclick={() => goBack('/search')}>Back</button>
  {#if loading}
    <div class="state">Loading album...</div>
  {:else if error}
    <div class="state error">Couldn't load this album: {error}</div>
  {:else if detail}
    <header class="header">
      {#if detail.thumbnail}
        <div class="cover" style="background-image:url('{detail.thumbnail}')"></div>
      {:else}
        <div class="cover fallback">M</div>
      {/if}
      <div class="meta">
        <span class="kicker">Spotify album . ephemeral</span>
        <h1 class="title">{detail.title ?? '-'}</h1>
        <div class="stats">
          {#if detail.primaryArtist}
            <!-- Artist pages are TIDAL + local library only; the spotify-artist
                 route was an unreachable dead layer and has been removed. -->
            <span>{detail.primaryArtist}</span>
          {/if}
          {#if detail.releaseDate}<span>. {detail.releaseDate}</span>{/if}
          <span>. {totalCount} tracks</span>
          <span class="resolved-count">. {resolvedCount} playable on TIDAL</span>
        </div>
        <div class="actions">
          <button class="btn-primary" disabled={playableCount === 0} onclick={playAll}>Play all</button>
          <button class="btn-secondary" disabled={playableCount === 0} onclick={shuffleAll}>Shuffle</button>
          <button class="btn-secondary" disabled={playableCount === 0} onclick={playAllNext}>Play next</button>
          <button class="btn-secondary" disabled={playableCount === 0} onclick={addAllToQueue}>Add to queue</button>
          <button class="btn-secondary" disabled={saving || resolvedCount === 0} onclick={save}>
            {saving ? 'Saving...' : 'Save to library'}
          </button>
          {#if pendingIds.length > 0}
            <span class="resolving-badge">Resolving {pendingIds.length} more...</span>
          {/if}
        </div>
        {#if saveResult}
          <p class="toast success">{saveResult}</p>
        {/if}
        {#if saveErr}
          <p class="toast error">Save failed: {saveErr}</p>
        {/if}
      </div>
    </header>

    <ol class="tracks">
      {#each detail.tracks as t, i (`${t.spotifyId ?? 'missing'}:${i}`)}
        {@const playable = isPlayable(t)}
        {@const rowKey = `${t.spotifyId ?? 'missing'}:${i}`}
        {@const artwork = t.thumbnail ?? lazyArt[rowKey] ?? null}
        <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <li
          class="row"
          class:disabled={!playable}
          class:pending={t.tidal.status === 'pending'}
          class:unresolved={t.tidal.status === 'unresolved' || t.tidal.status === 'error'}
          class:low-confidence={t.tidal.status === 'low_confidence'}
          role="button"
          tabindex={playable ? 0 : -1}
          aria-disabled={!playable}
          title={statusLabel(t.tidal)}
          onclick={() => { const tr = asTidalPlayable(t); if (tr) void playTidalTrackNow(tr); }}
          onkeydown={(e) => { if (e.key !== 'Enter' && e.key !== ' ') return; e.preventDefault(); const tr = asTidalPlayable(t); if (tr) void playTidalTrackNow(tr); }}
          oncontextmenu={(e) => handleRowContextMenu(e, t)}
          use:lazyTidalArt={{
            enabled: !artwork && !!t.primaryArtist,
            query: { artist: t.primaryArtist, title: t.title },
            onResolve: (url) => (lazyArt[rowKey] = url),
          }}
        >
          <span class="rank">{t.trackNumber ?? i + 1}</span>
          {#if artwork}
            <div class="thumb" style="background-image:url('{artwork}')"></div>
          {:else}
            <div class="thumb fallback">M</div>
          {/if}
          <div class="row-meta">
            <span class="row-title">{t.title ?? '-'}</span>
            <span class="row-artist">{t.primaryArtist ?? ''}</span>
          </div>
          <span class="status status--{t.tidal.status}">
            {t.tidal.status === 'resolved'
              ? 'TIDAL'
              : t.tidal.status === 'low_confidence'
                ? 'Match?'
                : t.tidal.status === 'pending'
                  ? 'Resolving...'
                  : 'N/A'}
          </span>
          <span class="dur">{formatTrackDuration(t.durationMs)}</span>
          <div class="row-actions">
            <button
              class="row-btn"
              disabled={!playable}
              title={playable ? `Play ${t.title ?? 'track'}` : statusLabel(t.tidal)}
              aria-label="Play {t.title ?? 'track'}"
              onclick={(e) => {
                e.stopPropagation();
                const tr = asTidalPlayable(t);
                if (tr) void playTidalTrackNow(tr);
              }}
            >Play</button>
            <button
              class="row-btn"
              disabled={!playable}
              title={playable ? 'Add to queue' : statusLabel(t.tidal)}
              aria-label="Add to queue"
              onclick={(e) => {
                e.stopPropagation();
                const tr = asTidalPlayable(t);
                if (tr) void addTidalTrackToQueue(tr);
              }}
            >+</button>
            <button
              class="row-btn"
              title="More actions"
              aria-label="More actions"
              onclick={(e) => handleMoreClick(e, t)}
            >More</button>
          </div>
        </li>
      {/each}
    </ol>

    {#if related && related.moreAlbumsByArtist.length > 0}
      <section class="shelf">
        <h2>More albums by {detail.primaryArtist ?? 'this artist'}</h2>
        <div class="card-rail">
          {#each related.moreAlbumsByArtist as a (a.spotifyId)}
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
  .stats .resolved-count { color: var(--accent); font-weight: var(--font-weight-semibold); }
  .actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 8px; }
  .btn-primary, .btn-secondary { background: var(--accent); color: var(--bg-base); border: none; padding: 9px 14px; border-radius: 999px; font-weight: var(--font-weight-bold); cursor: pointer; font-size: var(--font-size-sm); }
  .btn-secondary { background: var(--border-subtle); color: var(--text-primary); border: 1px solid var(--panel-border); }
  .btn-primary:disabled, .btn-secondary:disabled { opacity: 0.5; cursor: not-allowed; }
  .resolving-badge { font-size: var(--font-size-xs); color: var(--text-muted); font-style: italic; }
  .toast { margin: var(--space-2) 0 0; font-size: var(--font-size-xs); padding: var(--space-2) var(--space-3); border-radius: var(--radius-sm); width: fit-content; }
  .toast.success { background: rgba(125, 200, 175, 0.12); color: var(--accent); }
  .toast.error { background: rgba(239, 68, 68, 0.12); color: var(--state-error); }

  .tracks { list-style: none; margin: 0; padding: 0; }
  .row { display: grid; grid-template-columns: 36px 44px minmax(0,1fr) auto auto auto; gap: 14px; align-items: center; padding: 8px 12px; border-radius: 8px; cursor: pointer; transition: background 100ms ease; }
  .row:hover { background: rgba(255,255,255,.04); }
  .row.disabled { cursor: default; opacity: 0.55; }
  .row.disabled:hover { background: none; }
  .row.disabled .row-actions { opacity: 1; }
  .rank { color: var(--text-muted); text-align: center; font-variant-numeric: tabular-nums; }
  .thumb { width: 44px; height: 44px; border-radius: 4px; background-size: cover; background-position: center; background-color: var(--bg-raised); }
  .thumb.fallback { display: flex; align-items: center; justify-content: center; color: var(--text-muted); }
  .row-meta { display: flex; flex-direction: column; min-width: 0; }
  .row-title { color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-size: var(--font-size-sm); font-weight: var(--font-weight-medium); }
  .row-artist { color: var(--text-secondary); font-size: var(--font-size-xs); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .status { font-size: var(--font-size-2xs); font-weight: var(--font-weight-bold); letter-spacing: 0.05em; text-transform: uppercase; padding: 2px 8px; border-radius: 999px; background: rgba(255,255,255,.05); color: var(--text-muted); }
  .status--resolved { background: rgba(125, 200, 175, 0.16); color: var(--accent); }
  .status--low_confidence { background: rgba(245, 200, 70, 0.14); color: #f5c846; }
  .status--pending { background: rgba(255,255,255,.06); color: var(--text-muted); font-style: italic; }
  .status--unresolved, .status--error { background: rgba(239, 68, 68, .10); color: #ef4444; }
  .dur { color: var(--text-muted); font-size: var(--font-size-xs); font-variant-numeric: tabular-nums; min-width: 36px; text-align: right; }
  .row-actions { display: flex; align-items: center; justify-content: flex-end; gap: 4px; opacity: 0; transition: opacity 100ms ease; }
  .row:hover .row-actions, .row:focus-within .row-actions { opacity: 1; }
  .row-btn { border: none; min-width: 30px; height: 30px; padding: 0 8px; border-radius: 999px; background: rgba(255,255,255,.06); color: var(--text-secondary); cursor: pointer; font-size: var(--font-size-xs); font-weight: var(--font-weight-bold); }
  .row-btn:hover { background: rgba(255,255,255,.12); color: var(--text-primary); }
  .row-btn:disabled { cursor: not-allowed; opacity: 0.45; }

  .shelf h2 { font-size: var(--font-size-lg); font-weight: var(--font-weight-bold); margin: 0 0 12px; }
  .card-rail { display: flex; gap: var(--gap-sm); overflow-x: auto; padding-bottom: var(--space-2); scroll-snap-type: x mandatory; }
  .card { --card-w: clamp(120px, 11vw, 168px); flex: 0 0 var(--card-w); width: var(--card-w); display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-2); border-radius: var(--radius-md); text-decoration: none; color: inherit; cursor: pointer; transition: background var(--motion-fast); }
  .card:hover, .card:focus-visible { background: var(--bg-hover); outline: none; }
  .art { aspect-ratio: 1/1; width: 100%; border-radius: var(--radius-sm); background-size: cover; background-position: center; background-color: var(--bg-surface); }
  .art.fallback { display: flex; align-items: center; justify-content: center; color: var(--text-muted); font-size: var(--font-size-3xl); }
  .card-title { font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .card-sub { font-size: var(--font-size-xs); color: var(--text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  @media (max-width: 760px) {
    .page { padding: 24px 16px 88px; gap: 24px; }
    .header { grid-template-columns: 96px 1fr; gap: 16px; align-items: start; }
    .cover { width: 96px; height: 96px; border-radius: 10px; }
    .title { font-size: var(--font-size-xl); }
    .row { grid-template-columns: 28px 40px minmax(0,1fr) auto; gap: 10px; }
    .status, .dur { display: none; }
    .row-actions { opacity: 1; }
  }
</style>
