<script lang="ts">
  import { page } from '$app/stores';
  import { onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
  import {
    api,
    type SpotifyPlaylistDetail,
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
  import { showToast } from '$lib/stores/toast';

  // Ephemeral Spotify playlist view. The playlist is NOT in the user's
  // library — clicking Save promotes it. Until then, navigation away loses
  // any state held only in this component.

  const spotifyId = $derived($page.params.id ?? '');

  let detail = $state<SpotifyPlaylistDetail | null>(null);
  let pendingIds = $state<string[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let saving = $state(false);
  let saveResult = $state<string | null>(null);
  let saveErr = $state<string | null>(null);
  let requestedSpotifyId = $state('');
  let lazyArt = $state<Record<string, string>>({});

  // Lazy-tail status polling. Stops on full resolution, on hard timeout, or
  // when the user navigates away.
  const POLL_INTERVAL_MS = 1500;
  const POLL_DEADLINE_MS = 30_000;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let pollDeadline = 0;

  function clearPoll() {
    if (pollTimer) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  }

  async function pollResolution() {
    if (!detail || pendingIds.length === 0 || Date.now() > pollDeadline) {
      clearPoll();
      return;
    }
    try {
      const { entries } = await api.getResolveTidalStatus(pendingIds);
      const byId = new Map(entries.map((e) => [e.spotifyId, e.tidal]));
      // Merge new states into the playlist tracks. Pending entries stay in
      // the watch list; non-pending ones are removed.
      const stillPending: string[] = [];
      detail = {
        ...detail,
        tracks: detail.tracks.map((t) => {
          if (!t.spotifyId) return t;
          const next = byId.get(t.spotifyId);
          if (!next) return t;
          if (next.status === 'pending') stillPending.push(t.spotifyId);
          return { ...t, tidal: next };
        }),
      };
      pendingIds = stillPending;
      if (pendingIds.length > 0) {
        pollTimer = setTimeout(pollResolution, POLL_INTERVAL_MS);
      } else {
        clearPoll();
      }
    } catch (e) {
      // One bad poll round shouldn't kill the loop — back off and retry.
      console.warn('resolve status poll failed', e);
      pollTimer = setTimeout(pollResolution, POLL_INTERVAL_MS * 2);
    }
  }

  async function load(id: string) {
    if (!id.trim()) {
      error = 'Missing Spotify playlist ID';
      loading = false;
      return;
    }
    loading = true;
    error = null;
    detail = null;
    pendingIds = [];
    saveResult = null;
    saveErr = null;
    lazyArt = {};
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 15_000);
    try {
      const res = await api.getSpotifyPlaylist(id, controller.signal);
      detail = res.playlist;
      pendingIds = res.pendingSpotifyIds ?? [];
      pollDeadline = Date.now() + POLL_DEADLINE_MS;
      if (pendingIds.length > 0) {
        pollTimer = setTimeout(pollResolution, POLL_INTERVAL_MS);
      }
    } catch (e) {
      error =
        (e as Error).name === 'AbortError'
          ? 'Timed out loading playlist metadata'
          : ((e as Error).message ?? 'Failed to load playlist');
    } finally {
      clearTimeout(timeout);
      loading = false;
    }
  }

  $effect(() => {
    // Drive loading from the route param itself. This also recovers if an
    // earlier load got stuck before `detail` was populated.
    const id = spotifyId.trim();
    if (!id) {
      clearPoll();
      requestedSpotifyId = '';
      detail = null;
      pendingIds = [];
      loading = false;
      error = 'Missing Spotify playlist ID';
      return;
    }
    if (id !== requestedSpotifyId) {
      requestedSpotifyId = id;
      clearPoll();
      void load(id);
    }
  });

  onDestroy(() => {
    clearPoll();
  });

  function statusLabel(t: SpotifyTidalState): string {
    if ($tidalStatus !== 'connected') return 'Connect TIDAL to play this track';
    switch (t.status) {
      case 'resolved':
        return 'Play on TIDAL';
      case 'low_confidence':
        return 'Play (low-confidence match)';
      case 'pending':
        return 'Resolving on TIDAL…';
      case 'unresolved':
        return "Couldn't find on TIDAL";
      case 'error':
        return 'Resolution error';
    }
  }

  function isPlayable(t: SpotifyPlaylistTrack): boolean {
    return (
      (t.tidal.status === 'resolved' || t.tidal.status === 'low_confidence') &&
      t.tidal.id !== null &&
      $tidalStatus === 'connected'
    );
  }

  function asTidalPlayable(t: SpotifyPlaylistTrack): TidalPlayable | null {
    if (!isPlayable(t) || !t.tidal.id) return null;
    const artwork = artworkForTrack(t);
    return {
      tidal_id: t.tidal.id,
      title: t.title ?? '',
      artist_name: t.primaryArtist ?? null,
      artist_tidal_id: null,
      album_title: t.album ?? null,
      album_tidal_id: null,
      artwork_url: artwork,
      duration_ms: t.durationMs ?? null,
    };
  }

  function trackKey(t: SpotifyPlaylistTrack, i: number): string {
    return `${t.spotifyId ?? 'missing'}:${i}`;
  }

  function artworkForTrack(t: SpotifyPlaylistTrack, i?: number): string | null {
    if (t.thumbnail) return t.thumbnail;
    if (i !== undefined) return lazyArt[trackKey(t, i)] ?? null;
    if (!t.spotifyId) return null;
    const prefix = `${t.spotifyId}:`;
    const hit = Object.entries(lazyArt).find(([key]) => key.startsWith(prefix));
    return hit?.[1] ?? null;
  }

  function playableSpotifyTracks(): SpotifyPlaylistTrack[] {
    return detail?.tracks.filter(isPlayable) ?? [];
  }

  function playableTidalTracks(): TidalPlayable[] {
    return playableSpotifyTracks()
      .map(asTidalPlayable)
      .filter((track): track is TidalPlayable => track !== null);
  }

  async function play(t: SpotifyPlaylistTrack) {
    const track = asTidalPlayable(t);
    if (!track) return;
    await playTidalTrackNow(track);
  }

  async function playAll() {
    await playTidalTracksNow(playableTidalTracks(), detail?.title ?? 'Spotify playlist');
  }

  async function shuffleAll() {
    await shuffleTidalTracksNow(playableTidalTracks(), detail?.title ?? 'Spotify playlist');
  }

  async function playAllNext() {
    await playTidalTracksNext(playableTidalTracks());
  }

  async function addAllToQueue() {
    await addTidalTracksToQueue(playableTidalTracks());
  }

  async function startPlaylistRadio() {
    const seed = playableSpotifyTracks()
      .slice()
      .sort((a, b) => (b.playcount ?? 0) - (a.playcount ?? 0))[0];
    const track = seed ? asTidalPlayable(seed) : null;
    if (!track) {
      showToast('No playable tracks ready yet', 'info');
      return;
    }
    await startTidalSongRadio(track);
  }

  function buildRowMenu(t: SpotifyPlaylistTrack): MenuItem[] {
    const track = asTidalPlayable(t);
    const disabled = track === null;
    const hint = disabled ? statusLabel(t.tidal) : undefined;
    return [
      {
        label: 'Play now',
        icon: 'Play',
        disabled,
        hint,
        onSelect: () => {
          if (track) void playTidalTrackNow(track);
        },
      },
      {
        label: 'Play next',
        icon: 'Next',
        disabled,
        hint,
        onSelect: () => {
          if (track) void playTidalTrackNext(track);
        },
      },
      {
        label: 'Add to queue',
        icon: '+',
        disabled,
        hint,
        onSelect: () => {
          if (track) void addTidalTrackToQueue(track);
        },
      },
      { separator: true, label: '' },
      {
        label: 'Song radio',
        icon: 'Radio',
        disabled,
        hint,
        onSelect: () => {
          if (track) void startTidalSongRadio(track);
        },
      },
    ];
  }

  function handleRowContextMenu(e: MouseEvent, t: SpotifyPlaylistTrack) {
    openContextMenu(e, buildRowMenu(t), t.title ?? 'Spotify track');
  }

  function handleMoreClick(e: MouseEvent, t: SpotifyPlaylistTrack) {
    e.stopPropagation();
    openMenuAtElement(e.currentTarget as HTMLElement, buildRowMenu(t), t.title ?? 'Spotify track');
  }

  async function save() {
    if (!detail || saving) return;
    saving = true;
    saveErr = null;
    try {
      const res = await api.saveSpotifyPlaylist(spotifyId);
      const skipped = res.unresolvedCount + (res.importFailures ?? 0);
      saveResult =
        skipped > 0
          ? `Saved ${res.added} tracks. ${skipped} unavailable on TIDAL were skipped.`
          : `Saved ${res.added} tracks.`;
      // Navigate to the new playlist after a beat so the user sees the toast.
      setTimeout(() => goto(`/playlists`), 1200);
    } catch (e) {
      saveErr = (e as Error).message ?? 'Save failed';
    } finally {
      saving = false;
    }
  }

  function formatDuration(ms: number | null): string {
    if (!ms || ms <= 0) return '—';
    const total = Math.floor(ms / 1000);
    const m = Math.floor(total / 60);
    const s = total % 60;
    return `${m}:${s.toString().padStart(2, '0')}`;
  }

  function formatNumber(n: number | null): string {
    if (n === null || n === undefined) return '';
    if (n < 1_000) return n.toString();
    if (n < 1_000_000) return `${(n / 1_000).toFixed(1)}K`;
    if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    return `${(n / 1_000_000_000).toFixed(1)}B`;
  }

  const resolvedCount = $derived(
    detail?.tracks.filter(
      (t) => t.tidal.status === 'resolved' || t.tidal.status === 'low_confidence',
    ).length ?? 0,
  );
  const playableCount = $derived(detail?.tracks.filter(isPlayable).length ?? 0);
  const totalCount = $derived(detail?.tracks.length ?? 0);
</script>

<svelte:head>
  <title>{detail?.title ?? 'Spotify playlist'} · NOOR</title>
</svelte:head>

<div class="page">
  {#if loading}
    <div class="state">Loading playlist…</div>
  {:else if error}
    <div class="state error">Couldn't load this playlist: {error}</div>
  {:else if detail}
    <header class="header">
      {#if detail.thumbnail}
        <div class="cover" style="background-image:url('{detail.thumbnail}')"></div>
      {:else}
        <div class="cover fallback">♫</div>
      {/if}
      <div class="meta">
        <span class="kicker">Spotify playlist · ephemeral</span>
        <h1 class="title">{detail.title ?? 'Untitled playlist'}</h1>
        {#if detail.description}
          <p class="description">{detail.description}</p>
        {/if}
        <div class="stats">
          {#if detail.owner}
            <span>By {detail.owner}</span>
          {/if}
          {#if detail.followers !== null}
            <span>· {formatNumber(detail.followers)} followers</span>
          {/if}
          <span>· {totalCount} tracks</span>
          <span class="resolved-count">· {resolvedCount} playable on TIDAL</span>
        </div>
        <div class="actions">
          <button class="btn-primary" disabled={playableCount === 0} onclick={playAll}>Play all</button>
          <button class="btn-secondary" disabled={playableCount === 0} onclick={shuffleAll}>Shuffle</button>
          <button class="btn-secondary" disabled={playableCount === 0} onclick={playAllNext}>Play next</button>
          <button class="btn-secondary" disabled={playableCount === 0} onclick={addAllToQueue}>Add to queue</button>
          <button class="btn-secondary" disabled={playableCount === 0} onclick={startPlaylistRadio}>Song radio</button>
          <button
            class="btn-secondary"
            disabled={saving || resolvedCount === 0}
            onclick={save}
          >
            {saving ? 'Saving…' : 'Save to library'}
          </button>
          {#if pendingIds.length > 0}
            <span class="resolving-badge">Resolving {pendingIds.length} more…</span>
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
        {@const rowKey = trackKey(t, i)}
        {@const artwork = artworkForTrack(t, i)}
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
          onclick={() => void play(t)}
          onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), void play(t))}
          oncontextmenu={(e) => handleRowContextMenu(e, t)}
          use:lazyTidalArt={{
            enabled: !artwork && !!t.primaryArtist,
            query: { artist: t.primaryArtist, title: t.title },
            onResolve: (url) => (lazyArt[rowKey] = url),
          }}
        >
          <span class="rank">{i + 1}</span>
          {#if artwork}
            <div class="thumb" style="background-image:url('{artwork}')"></div>
          {:else}
            <div class="thumb fallback">♫</div>
          {/if}
          <div class="row-meta">
            <span class="row-title">{t.title ?? '—'}</span>
            <span class="row-artist">{t.primaryArtist ?? ''}</span>
          </div>
          {#if t.playcount !== null}
            <span class="playcount">{formatNumber(t.playcount)} plays</span>
          {/if}
          <span class="status status--{t.tidal.status}">
            {t.tidal.status === 'resolved'
              ? 'TIDAL'
              : t.tidal.status === 'low_confidence'
                ? 'Match?'
                : t.tidal.status === 'pending'
                  ? 'Resolving…'
                  : 'N/A'}
          </span>
          <span class="dur">{formatDuration(t.durationMs)}</span>
          <div class="row-actions">
            <button
              class="row-btn"
              disabled={!playable}
              title={playable ? `Play ${t.title ?? 'track'}` : statusLabel(t.tidal)}
              aria-label="Play {t.title ?? 'track'}"
              onclick={(e) => {
                e.stopPropagation();
                void play(t);
              }}
            >Play</button>
            <button
              class="row-btn"
              disabled={!playable}
              title={playable ? 'Add to queue' : statusLabel(t.tidal)}
              aria-label="Add to queue"
              onclick={(e) => {
                e.stopPropagation();
                const track = asTidalPlayable(t);
                if (track) void addTidalTrackToQueue(track);
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
  {/if}
</div>

<style>
  .page { max-width: var(--content-width); margin: 0 auto; padding: 32px 28px 96px; }
  .state { padding: 80px 0; text-align: center; color: var(--text-muted); }
  .state.error { color: #ef4444; }

  .header {
    display: grid;
    grid-template-columns: 220px 1fr;
    gap: 28px;
    align-items: end;
    margin-bottom: 32px;
  }
  .cover {
    width: 220px;
    height: 220px;
    border-radius: 14px;
    background-size: cover;
    background-position: center;
    box-shadow: 0 18px 36px -16px rgba(0, 0, 0, 0.6);
  }
  .cover.fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    background: linear-gradient(135deg, #1ed760, #1aa34a);
    font-size: 56px;
    color: #fff;
  }
  .meta { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
  .kicker {
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: #1ed760;
    font-weight: 700;
  }
  .title {
    margin: 0;
    font-size: 38px;
    font-weight: 800;
    line-height: 1.1;
    color: var(--text-primary);
  }
  .description {
    margin: 0;
    color: var(--text-secondary);
    font-size: 13px;
    max-width: 60ch;
  }
  .stats {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    color: var(--text-muted);
    font-size: 12px;
  }
  .stats .resolved-count { color: var(--accent); font-weight: 600; }
  .actions { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; margin-top: 8px; }
  .btn-primary,
  .btn-secondary {
    background: var(--accent);
    color: var(--bg-base);
    border: none;
    padding: 9px 14px;
    border-radius: 999px;
    font-weight: 700;
    cursor: pointer;
    font-size: 13px;
  }
  .btn-secondary {
    background: rgba(255, 255, 255, 0.07);
    color: var(--text-primary);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }
  .btn-primary:disabled,
  .btn-secondary:disabled { opacity: 0.5; cursor: not-allowed; }
  .resolving-badge {
    font-size: 11px;
    color: var(--text-muted);
    font-style: italic;
  }
  .toast {
    margin: 8px 0 0;
    font-size: 12px;
    padding: 8px 12px;
    border-radius: 8px;
    width: fit-content;
  }
  .toast.success { background: rgba(125, 200, 175, 0.12); color: var(--accent); }
  .toast.error { background: rgba(239, 68, 68, 0.12); color: #ef4444; }

  .tracks { list-style: none; margin: 0; padding: 0; }
  .row {
    display: grid;
    grid-template-columns: 36px 44px minmax(0, 1fr) auto auto auto auto;
    gap: 14px;
    align-items: center;
    padding: 8px 12px;
    border-radius: 8px;
    cursor: pointer;
    transition: background 100ms ease;
  }
  .row:hover { background: rgba(255, 255, 255, 0.04); }
  .row.disabled { cursor: default; opacity: 0.55; }
  .row.disabled:hover { background: none; }
  .row.disabled .row-actions { opacity: 1; }
  .rank { color: var(--text-muted); text-align: center; font-variant-numeric: tabular-nums; }
  .thumb {
    width: 44px;
    height: 44px;
    border-radius: 4px;
    background-size: cover;
    background-position: center;
    background-color: var(--bg-raised);
  }
  .thumb.fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
  }
  .row-meta { display: flex; flex-direction: column; min-width: 0; }
  .row-title {
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 14px;
    font-weight: 500;
  }
  .row-artist {
    color: var(--text-secondary);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .playcount {
    color: var(--text-muted);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .status {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 2px 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-muted);
  }
  .status--resolved { background: rgba(125, 200, 175, 0.16); color: var(--accent); }
  .status--low_confidence { background: rgba(245, 200, 70, 0.14); color: #f5c846; }
  .status--pending { background: rgba(255, 255, 255, 0.06); color: var(--text-muted); font-style: italic; }
  .status--unresolved, .status--error { background: rgba(239, 68, 68, 0.10); color: #ef4444; }
  .dur {
    color: var(--text-muted);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    min-width: 36px;
    text-align: right;
  }
  .row-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 4px;
    opacity: 0;
    transition: opacity 100ms ease;
  }
  .row:hover .row-actions,
  .row:focus-within .row-actions { opacity: 1; }
  .row-btn {
    border: none;
    min-width: 30px;
    height: 30px;
    padding: 0 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 11px;
    font-weight: 700;
  }
  .row-btn:hover {
    background: rgba(255, 255, 255, 0.12);
    color: var(--text-primary);
  }
  .row-btn:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }
  @media (max-width: 760px) {
    .page { padding: 24px 16px 88px; }
    .header { grid-template-columns: 96px 1fr; gap: 16px; align-items: start; }
    .cover { width: 96px; height: 96px; border-radius: 10px; }
    .title { font-size: 26px; }
    .row {
      grid-template-columns: 28px 40px minmax(0, 1fr) auto;
      gap: 10px;
    }
    .playcount,
    .status,
    .dur { display: none; }
    .row-actions { opacity: 1; }
  }
</style>
