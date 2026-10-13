<script lang="ts">
  import { page } from '$app/stores';
  import { onDestroy } from 'svelte';
  import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
  import { goBack } from '$lib/navigation/back';
  import { formatTrackDuration } from '$lib/utils/format';
  import {
    api,
    ApiError,
    type Playlist,
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
  import { SPOTIFY_MOODS_BY_SLUG } from '$lib/components/moods/spotify-moods-data';
  import { putCachedSpotifyChartMeta } from '$lib/stores/spotify-chart-meta-cache';
  import DetailHero from '$lib/components/ui/DetailHero.svelte';
  import ActionBar from '$lib/components/ui/ActionBar.svelte';
  import SpotifySourceBadge from '$lib/components/ui/SpotifySourceBadge.svelte';
  import { openExternal } from '$lib/util/external';
  import { actionErrorMessage } from '$lib/utils/describe_error';

  // External Spotify playlist, played through TIDAL matches. Save copies the
  // matched tracks into the library; the saved copy remembers its Spotify
  // source, so the page shows it and Update appends tracks added on Spotify
  // since (server side, existing tracks are skipped and edits are kept).

  const spotifyId = $derived($page.params.id ?? '');
  const backLink = $derived(resolveBackLink($page.url.searchParams));

  let detail = $state<SpotifyPlaylistDetail | null>(null);
  let pendingIds = $state<string[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let savedPlaylist = $state<Playlist | null>(null);
  // One library action at a time: saving, updating from Spotify, rescanning.
  let busy = $state<'saving' | 'updating' | 'rescanning' | null>(null);
  let requestedSpotifyId = $state('');
  let lazyArt = $state<Record<string, string>>({});
  let loadSeq = 0;

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
        schedulePoll(seq);
      } else {
        clearPoll();
      }
    } catch (e) {
      if (seq !== loadSeq) return;
      // One bad poll round shouldn't kill the loop — back off and retry.
      console.warn('resolve status poll failed', e);
      schedulePoll(seq, POLL_INTERVAL_MS * 2);
    }
  }

  async function load(id: string) {
    const seq = ++loadSeq;
    if (!id.trim()) {
      error = 'Missing Spotify playlist ID';
      loading = false;
      return;
    }
    loading = true;
    error = null;
    detail = null;
    pendingIds = [];
    savedPlaylist = null;
    lazyArt = {};
    // Sportify proxy is third-party and flaky. Retry once on 5xx so a
    // transient upstream hiccup doesn't surface as a hard failure.
    await attemptLoad(id, /* retryOn5xx */ true, seq);
  }

  async function attemptLoad(id: string, retryOn5xx: boolean, seq: number) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 15_000);
    try {
      const res = await api.getSpotifyPlaylist(id, controller.signal);
      if (seq !== loadSeq) return;
      applyLoaded(id, res, seq);
      loading = false;
    } catch (e) {
      clearTimeout(timeout);
      const is5xx = e instanceof ApiError && e.status >= 500 && e.status < 600;
      if (is5xx && retryOn5xx) {
        // One automatic retry after a short delay for transient proxy outages.
        await new Promise((r) => setTimeout(r, 2_000));
        if (seq === loadSeq && spotifyId.trim() === id) {
          await attemptLoad(id, /* retryOn5xx */ false, seq);
          return;
        }
      }
      if (seq !== loadSeq) return;
      error = friendlyError(e);
      loading = false;
    } finally {
      clearTimeout(timeout);
    }
  }

  function applyLoaded(
    id: string,
    res: Awaited<ReturnType<typeof api.getSpotifyPlaylist>>,
    seq: number,
  ) {
    detail = res.playlist;
    savedPlaylist = res.savedPlaylist;
    putCachedSpotifyChartMeta(id, res.playlist);
    pendingIds = res.pendingSpotifyIds ?? [];
    pollDeadline = Date.now() + POLL_DEADLINE_MS;
    clearPoll();
    if (pendingIds.length > 0) {
      schedulePoll(seq);
    }
  }

  function friendlyError(e: unknown): string {
    if ((e as Error)?.name === 'AbortError') {
      return 'Timed out loading playlist metadata. Try again in a moment.';
    }
    if (e instanceof ApiError) {
      if (e.status === 503 || e.status === 502 || e.status === 504) {
        return 'The Spotify metadata proxy is temporarily unavailable. Try again in a minute.';
      }
      if (e.status === 404) {
        return 'Playlist not found on Spotify.';
      }
    }
    return (e as Error).message ?? 'Failed to load playlist';
  }

  function retry() {
    const id = spotifyId.trim();
    if (id) void load(id);
  }

  function resolveBackLink(params: URLSearchParams): { href: string; label: string } {
    const from = params.get('from');
    const mood = params.get('mood');
    const q = params.get('q')?.trim();
    if (from === 'moods' && mood && SPOTIFY_MOODS_BY_SLUG.has(mood)) {
      return { href: `/moods/${encodeURIComponent(mood)}`, label: 'Back to mood' };
    }
    if (from === 'search' && q) {
      return { href: `/search?q=${encodeURIComponent(q)}`, label: 'Back to search' };
    }
    return { href: '/search', label: 'Back to search' };
  }

  $effect(() => {
    // Drive loading from the route param itself. This also recovers if an
    // earlier load got stuck before `detail` was populated.
    const id = spotifyId.trim();
    if (!id) {
      loadSeq += 1;
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
    loadSeq += 1;
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
        icon: '▶',
        disabled,
        hint,
        onSelect: () => {
          if (track) void playTidalTrackNow(track);
        },
      },
      {
        label: 'Play next',
        icon: '⤴',
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
        icon: '◉',
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

  // Save skips tracks still being matched, so let the background matcher
  // finish first (bounded by the poll deadline).
  async function settlePending(seq: number) {
    while (seq === loadSeq && pendingIds.length > 0 && Date.now() <= pollDeadline) {
      await new Promise((r) => setTimeout(r, 400));
    }
  }

  function skippedNote(res: { unresolvedCount: number; importFailures: number }): string {
    const skipped = res.unresolvedCount + (res.importFailures ?? 0);
    return skipped > 0 ? ` ${skipped} not on TIDAL were skipped.` : '';
  }

  async function saveToLibrary(seq: number) {
    const res = await api.saveSpotifyPlaylist(spotifyId);
    if (seq !== loadSeq) return;
    savedPlaylist = res.playlist;
    const name = res.playlist.name;
    if (!res.updated) {
      showToast(`Saved ${res.added} tracks to ${name}.${skippedNote(res)}`, 'success');
    } else if (res.added > 0) {
      showToast(`Added ${res.added} new tracks to ${name}.${skippedNote(res)}`, 'success');
    } else {
      showToast(`${name} is up to date.`, 'info');
    }
  }

  async function save() {
    if (!detail || busy) return;
    const seq = loadSeq;
    busy = 'saving';
    try {
      await settlePending(seq);
      await saveToLibrary(seq);
    } catch (e) {
      showToast(actionErrorMessage(e, "Couldn't save this playlist. Try again."), 'error');
    } finally {
      busy = null;
    }
  }

  // Re-read the playlist from Spotify (not the server's cached copy), match
  // the new tracks, then append them to the saved copy.
  async function updateFromSpotify() {
    const id = spotifyId.trim();
    if (!detail || busy || !id) return;
    const seq = loadSeq;
    busy = 'updating';
    try {
      const res = await api.getSpotifyPlaylist(id, undefined, { refresh: true });
      if (seq !== loadSeq) return;
      applyLoaded(id, res, seq);
      if (!savedPlaylist) return;
      await settlePending(seq);
      await saveToLibrary(seq);
    } catch (e) {
      showToast(actionErrorMessage(e, "Couldn't update from Spotify. Try again."), 'error');
    } finally {
      busy = null;
    }
  }

  // Match again the tracks that found nothing on TIDAL or only a doubtful
  // match, ignoring cached misses (TIDAL adds catalog all the time).
  async function rescanUnmatched() {
    if (!detail || busy) return;
    const ids = detail.tracks
      .filter((t) => t.spotifyId && t.tidal.status !== 'resolved' && t.tidal.status !== 'pending')
      .map((t) => t.spotifyId as string);
    if (ids.length === 0) return;
    const seq = loadSeq;
    busy = 'rescanning';
    try {
      const entries = await api.resolveTidalBulk(ids, true);
      if (seq !== loadSeq || !detail) return;
      const byId = new Map(entries.map((e) => [e.spotifyId, e.tidal]));
      const before = playableCount;
      detail = {
        ...detail,
        tracks: detail.tracks.map((t) => {
          const next = t.spotifyId ? byId.get(t.spotifyId) : undefined;
          return next ? { ...t, tidal: next } : t;
        }),
      };
      const found = playableCount - before;
      showToast(
        found > 0 ? `Found ${found} more on TIDAL.` : 'No new matches on TIDAL yet.',
        found > 0 ? 'success' : 'info',
      );
    } catch (e) {
      showToast(actionErrorMessage(e, "Couldn't rescan tracks. Try again."), 'error');
    } finally {
      busy = null;
    }
  }

  function openOnSpotify() {
    const id = spotifyId.trim();
    if (id) void openExternal(`https://open.spotify.com/playlist/${encodeURIComponent(id)}`);
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
  // Tracks a rescan can help: no TIDAL match, a doubtful one, or an error.
  const unmatchedCount = $derived(
    detail?.tracks.filter((t) => t.tidal.status !== 'resolved' && t.tidal.status !== 'pending').length ?? 0,
  );

  // Secondary actions live in More, so every detail hero shares one action
  // order (STYLING.md "ActionBar").
  function openHeroMore(event: MouseEvent) {
    event.preventDefault();
    event.stopPropagation();
    const items: MenuItem[] = [
      { label: 'Play next', icon: '\u2934', disabled: playableCount === 0, onSelect: () => void playAllNext() },
      { label: 'Add to queue', icon: '\uFF0B', disabled: playableCount === 0, onSelect: () => void addAllToQueue() },
      { separator: true, label: '' },
      {
        label: 'Rescan unmatched tracks',
        icon: '\u21BB',
        disabled: unmatchedCount === 0 || busy !== null,
        hint: unmatchedCount === 0 ? 'Every track has a TIDAL match' : undefined,
        onSelect: () => void rescanUnmatched(),
      },
      { label: 'Open in Spotify', icon: '\u2197', onSelect: openOnSpotify },
    ];
    openMenuAtElement(event.currentTarget as HTMLElement, items, 'Spotify playlist');
  }
</script>

<svelte:head>
  <title>{detail?.title ?? 'Spotify playlist'} · NOOR</title>
</svelte:head>

<div class="page">
  <button class="back-link" type="button" onclick={() => goBack(backLink.href)}>{backLink.label}</button>
  {#if loading}
    <div class="state">Loading playlist…</div>
  {:else if error}
    <div class="state error">
      <p>Couldn't load this playlist: {error}</p>
      <button type="button" class="retry-btn" onclick={retry}>Retry</button>
    </div>
  {:else if detail}
    {@const heroDetail = detail as SpotifyPlaylistDetail}
    <DetailHero
      eyebrow="External playlist"
      title={heroDetail.title ?? 'Untitled playlist'}
      artwork={heroDetail.thumbnail}
      backdrop={heroDetail.thumbnail}
      fallbackText="♫"
      variant="immersive"
    >
      {#snippet meta()}
          <SpotifySourceBadge />
          {#if heroDetail.owner}
            <span>By {heroDetail.owner}</span>
          {/if}
          {#if heroDetail.followers !== null}
            <span>· {formatNumber(heroDetail.followers)} followers</span>
          {/if}
          <span>· {totalCount} tracks</span>
          <span class="resolved-count">· {resolvedCount} playable on TIDAL</span>
          {#if unmatchedCount > 0}
            <span>·</span>
            <button type="button" class="meta-link" disabled={busy !== null}
              title="Look for these tracks on TIDAL again" onclick={() => void rescanUnmatched()}>
              {busy === 'rescanning' ? 'Rescanning...' : `Rescan ${unmatchedCount} unmatched`}
            </button>
          {/if}
      {/snippet}
      {#snippet details()}
        {#if heroDetail.description}
          <p class="description">{heroDetail.description}</p>
        {/if}
        {#if savedPlaylist}
          <p class="saved-line">
            Saved to your library as
            <a href={`/playlists/${savedPlaylist.id}`}>{savedPlaylist.name}</a>
          </p>
        {/if}
      {/snippet}
      {#snippet actions()}
          <ActionBar
            onplay={playableCount > 0 ? playAll : undefined}
            playLabel="Play all"
            onshuffle={playableCount > 0 ? shuffleAll : undefined}
            onradio={playableCount > 0 ? startPlaylistRadio : undefined}
            radioLabel="Song radio"
            onmore={openHeroMore}
            moreLabel="More playlist actions"
          >
          {#if savedPlaylist}
            <button type="button" class="btn btn-secondary" disabled={busy !== null}
              title="Add tracks added on Spotify since you saved it"
              onclick={() => void updateFromSpotify()}>
              {busy === 'updating' ? 'Updating...' : 'Update from Spotify'}
            </button>
          {:else}
            <button type="button" class="btn btn-secondary" disabled={busy !== null || resolvedCount === 0}
              title="Save the TIDAL matches as a playlist in your library"
              onclick={() => void save()}>
              {busy === 'saving' ? 'Saving...' : 'Save playlist'}
            </button>
          {/if}
          {#if pendingIds.length > 0}
            <span class="resolving-badge">Matching {pendingIds.length} more on TIDAL...</span>
          {/if}
          </ActionBar>
      {/snippet}
    </DetailHero>

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
          <span class="dur">{formatTrackDuration(t.durationMs)}</span>
          <div class="row-actions">
            <button
              class="row-btn row-btn-text"
              disabled={!playable}
              title={playable ? `Play ${t.title ?? 'track'}` : statusLabel(t.tidal)}
              aria-label="Play {t.title ?? 'track'}"
              onclick={(e) => {
                e.stopPropagation();
                void play(t);
              }}
            >Play</button>
            <button
              class="row-btn row-btn-text"
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
              class="row-btn row-btn-text"
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
  .page {
    width: min(100%, var(--content-width));
    margin: 0 auto;
    padding: 0 0 calc(var(--space-7) * 2);
    display: flex;
    flex-direction: column;
  }
  .page > .back-link {
    align-self: flex-start;
    margin-bottom: var(--space-3);
  }
  .state { padding: calc(var(--space-7) * 2) 0; text-align: center; color: var(--text-muted); }
  .state.error { color: #ef4444; display: flex; flex-direction: column; align-items: center; gap: var(--space-3); }
  .state.error p { margin: 0; }
  .retry-btn { padding: 8px 18px; border-radius: 999px; background: var(--bg-hover); border: 1px solid var(--panel-border); color: var(--text-primary); font: inherit; font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); cursor: pointer; transition: background var(--motion-base), border-color var(--motion-base); }
  .retry-btn:hover, .retry-btn:focus-visible { background: var(--accent-soft); border-color: var(--accent-line); outline: none; }

  .description {
    margin: 0;
    color: var(--text-secondary);
    font-size: var(--font-size-sm);
    max-width: 60ch;
  }
  .resolved-count { color: var(--accent); font-weight: var(--font-weight-semibold); }
  .resolving-badge {
    font-size: var(--font-size-xs);
    color: var(--text-muted);
    font-style: italic;
  }
  .meta-link {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: var(--accent-line, #7dc8af);
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .meta-link:disabled { cursor: default; opacity: 0.6; }
  .saved-line {
    margin: 0;
    font-size: var(--font-size-sm);
    color: var(--text-secondary);
  }
  .saved-line a { color: var(--text-primary); font-weight: var(--font-weight-semibold); }

  .tracks { list-style: none; margin: 0; padding: 0; }
  .row {
    display: grid;
    grid-template-columns: 36px 44px minmax(0, 1fr) auto auto auto auto;
    gap: var(--gap);
    align-items: center;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background var(--motion-press);
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
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
  }
  .row-artist {
    color: var(--text-secondary);
    font-size: var(--font-size-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .playcount {
    color: var(--text-muted);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
  }
  .status {
    font-size: var(--font-size-2xs);
    font-weight: var(--font-weight-bold);
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
    font-size: var(--font-size-xs);
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
    transition: opacity var(--motion-press);
  }
  .row-actions .row-btn { opacity: 1; }
  .row:hover .row-actions,
  .row:focus-within .row-actions { opacity: 1; }
  @media (max-width: 760px) {
    .page { padding: 0 0 calc(var(--space-7) * 2); }
    .row {
      grid-template-columns: 28px 40px minmax(0, 1fr) auto;
      gap: var(--gap-sm);
    }
    .playcount,
    .status,
    .dur { display: none; }
    .row-actions { opacity: 1; }
  }
</style>
