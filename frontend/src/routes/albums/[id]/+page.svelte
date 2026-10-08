<script lang="ts">
	import { page } from '$app/state';
	import type { Snapshot } from './$types';
	import { type Track, type TidalDiscographyTrack, type SpotifyTrackStats } from '$lib/api/client';
	import { cachedApi, invalidateLibraryCaches } from '$lib/cache/api_queries';
	import {
		playAlbum,
		shuffleAlbum,
		startAlbumRadio,
		toggleTrackFavorite,
		toggleAlbumFavorite,
		saveTidalAlbumToLibrary,
		currentTrack,
		isPlaying,
		togglePlayback,
		type AlbumTracksData
	} from '$lib/stores/player';
	import { canPlayTrack } from '$lib/player/playable';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import DetailHero from '$lib/components/ui/DetailHero.svelte';
	import ActionBar from '$lib/components/ui/ActionBar.svelte';
	import { goBack } from '$lib/navigation/back';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildTidalTrackMenu } from '$lib/player/track_menu';
	import {
		firstArtworkUrl,
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize,
	} from '$lib/utils/artwork';
	import { formatTotalDuration, formatTrackDuration } from '$lib/utils/format';
	import { groupWorks } from '$lib/album/album_works';
	import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';
	import { currentTrackMatchesTracks, mergeAlbumTracks } from '$lib/utils/track';

	let albumId = $derived(Number(page.params.id));

	let tracks = $state<Track[]>([]);
	let tidalOnlyTracks = $state<TidalDiscographyTrack[]>([]);
	let albumTidalId = $state<number | null>(null);
	let albumIsFavorite = $state(false);
	let favoritePending = $state(false);
	let savePending = $state(false);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let failedArtworkUrls = $state<Record<string, boolean>>({});
	let loadSeq = 0;

	let artistTracks = $state<Track[]>([]);
	let moreLoading = $state(false);
	let moreLoaded = $state(false);
	let moreLoadSeq = 0;
	let spotifyStats = $state<SpotifyTrackStats | null>(null);
	let playcountByIsrc = $derived.by(() => {
		const map = new Map<string, number>();
		for (const t of spotifyStats?.tracks ?? []) {
			if (t.playcount != null) map.set(t.isrc, t.playcount);
		}
		return map;
	});
	// Plays only earn a column when they vary: world plays from Spotify. A
	// column of "0 local" on every row was repeated noise.
	let showPlays = $derived(playcountByIsrc.size > 0);

	// Phase 5B: back/forward state via SvelteKit snapshot.
	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: captureScroll() }),
		restore: (saved) => {
			restoreScroll(saved.scrollY);
		}
	};

	async function load(id: number) {
		const seq = ++loadSeq;
		loading = true;
		error = null;
		try {
			const res = await cachedApi.getAlbumTracks(id);
			if (seq !== loadSeq) return;
			tracks = res.tracks;
			tidalOnlyTracks = res.tidal_tracks ?? [];
			albumTidalId = res.album_tidal_id ?? null;
			albumIsFavorite = res.album_is_favorite ?? false;
		} catch (err) {
			if (seq !== loadSeq) return;
			error = `Failed to load album: ${err}`;
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	$effect(() => {
		const id = albumId;
		failedArtworkUrls = {};
		tracks = [];
		tidalOnlyTracks = [];
		albumTidalId = null;
		albumIsFavorite = false;
		void load(id);
		artistTracks = [];
		moreLoaded = false;
		moreLoading = false;
		moreLoadSeq += 1;
		spotifyStats = null;
		void loadSpotifyStats(id);
	});

	$effect(() => {
		const artistId = tracks[0]?.artist_id;
		const sourceAlbumId = albumId;
		if (artistId != null && !moreLoaded && !moreLoading) {
			void loadMore(artistId, sourceAlbumId);
		}
	});

	async function loadMore(artistId: number, sourceAlbumId: number) {
		const seq = ++moreLoadSeq;
		moreLoading = true;
		try {
			const res = await cachedApi.getArtistTracks(artistId);
			if (seq !== moreLoadSeq || albumId !== sourceAlbumId) return;
			artistTracks = res.tracks;
			moreLoaded = true;
		} catch (err) {
			if (seq !== moreLoadSeq || albumId !== sourceAlbumId) return;
			console.error('Failed to load artist tracks', err);
		} finally {
			if (seq === moreLoadSeq) moreLoading = false;
		}
	}

	async function loadSpotifyStats(albumIdToLoad: number) {
		try {
			const stats = await cachedApi.getAlbumSpotifyStats(albumIdToLoad);
			if (albumId === albumIdToLoad) spotifyStats = stats;
		} catch (err) {
			console.error('Failed to load Spotify stats', err);
			if (albumId === albumIdToLoad) spotifyStats = null;
		}
	}

	let header = $derived(() => {
		const firstLocal = tracks[0];
		const firstTidal = tidalOnlyTracks[0];
		if (!firstLocal && !firstTidal) return null;
		const totalMsLocal = tracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0);
		const totalMsTidal = tidalOnlyTracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0);
		return {
			title: firstLocal?.album_title ?? firstTidal?.album_title ?? 'Unknown album',
			artist_name: firstLocal?.artist_name ?? firstTidal?.artist_name ?? 'Unknown artist',
			artist_id: firstLocal?.artist_id ?? null,
			artwork_url: firstArtworkUrl(tracks, tidalOnlyTracks),
			library_track_count: tracks.length,
			total_track_count: tracks.length + tidalOnlyTracks.length,
			total_ms: totalMsLocal + totalMsTidal,
		};
	});

	function artworkCandidate(
		rawUrl: string | null | undefined,
		size: TidalArtworkSize,
	): string | null {
		if (!rawUrl) return null;
		for (const candidateSize of tidalArtworkFallbackSizes(rawUrl, size)) {
			const candidate = upscaleTidalArtwork(rawUrl, candidateSize);
			if (candidate && !failedArtworkUrls[candidate]) return candidate;
		}
		return null;
	}

	function markArtworkFailed(renderedUrl: string | null | undefined) {
		if (!renderedUrl) return;
		failedArtworkUrls = { ...failedArtworkUrls, [renderedUrl]: true };
	}

	let otherAlbums = $derived.by(() => {
		const map = new Map<
			number,
			{ id: number; title: string; artwork_url: string | null; count: number }
		>();
		for (const t of artistTracks) {
			if (t.album_id == null || t.album_id === albumId) continue;
			const existing = map.get(t.album_id);
			if (existing) {
				existing.count += 1;
				if (!existing.artwork_url && t.artwork_url) existing.artwork_url = t.artwork_url;
			} else {
				map.set(t.album_id, {
					id: t.album_id,
					title: t.album_title ?? 'Album',
					artwork_url: t.artwork_url,
					count: 1
				});
			}
		}
		return Array.from(map.values()).sort((a, b) => b.count - a.count).slice(0, 8);
	});

	// The full album in (disc, track) order: owned rows interleaved with
	// TIDAL-only rows, exactly the order playAlbum queues. Rendering this list
	// (instead of owned-then-TIDAL blocks) keeps "click a song to start the
	// album from there" visually truthful for scattered ownership.
	let displayEntries = $derived(mergeAlbumTracks(tracks, tidalOnlyTracks));
	// Long works ("Suite No. 1: I. Prelude") get a work header; rows show the
	// movement. Ordinary albums stay a flat list.
	let works = $derived(
		groupWorks(displayEntries.map((entry) => (entry.kind === 'local' ? entry.local.title : entry.tidal.title))),
	);
	let workStarts = $derived(new Map(works.groups.map((group) => [group.start, group])));
	function entryDurationMs(index: number): number {
		const entry = displayEntries[index];
		return (entry?.kind === 'local' ? entry.local.duration_ms : entry?.tidal.duration_ms) ?? 0;
	}
	function workDurationMs(start: number, end: number): number {
		let total = 0;
		for (let index = start; index <= end; index += 1) total += entryDurationMs(index);
		return total;
	}

	// Hand playAlbum/shuffleAlbum the listing already on screen so playing
	// doesn't refetch (a live TIDAL round trip for partial albums) and the
	// queue always matches what the user sees.
	function albumData(): AlbumTracksData {
		return { tracks, tidal_tracks: tidalOnlyTracks, album_tidal_id: albumTidalId };
	}

	function onRowClick(track: Track) {
		void playAlbum(albumId, track.id, albumData());
	}

	function onHeroPlay() {
		if (currentTrackMatchesTracks($currentTrack, tracks, tidalOnlyTracks)) {
			void togglePlayback();
			return;
		}
		// playAlbum queues the FULL album (owned rows + TIDAL-only rows) in track
		// order, so partial-library users hear the whole thing 1..N. Owned rows
		// play from the library; TIDAL-only rows stream.
		void playAlbum(albumId, undefined, albumData());
	}

	let isAlbumPlaying = $derived(
		$isPlaying && currentTrackMatchesTracks($currentTrack, tracks, tidalOnlyTracks)
	);

	let radioPending = $state(false);
	async function onRadioClick() {
		if (radioPending) return;
		radioPending = true;
		try {
			await startAlbumRadio(albumId);
		} finally {
			radioPending = false;
		}
	}

	async function onLikeAlbum() {
		if (favoritePending) return;
		favoritePending = true;
		const previous = albumIsFavorite;
		albumIsFavorite = !previous; // optimistic flip
		try {
			albumIsFavorite = await toggleAlbumFavorite(albumId, previous);
		} finally {
			favoritePending = false;
		}
	}

	// True when the library is missing some of the album's tracks (they exist
	// on TIDAL but aren't imported), so "Save full album" has something to do.
	let isPartialAlbum = $derived(albumTidalId != null && tidalOnlyTracks.length > 0);

	async function onSaveAlbum() {
		if (savePending || albumTidalId == null) return;
		savePending = true;
		try {
			const localId = await saveTidalAlbumToLibrary(albumTidalId);
			if (localId != null) {
				invalidateLibraryCaches();
				await load(albumId);
			}
		} finally {
			savePending = false;
		}
	}

	async function onHeartClick(track: Track, event: MouseEvent) {
		event.stopPropagation();
		const previous = track.is_favorite;
		tracks = tracks.map((t) =>
			t.id === track.id ? { ...t, is_favorite: !previous } : t
		);
		try {
			await toggleTrackFavorite(track.id, previous);
		} catch {
			tracks = tracks.map((t) =>
				t.id === track.id ? { ...t, is_favorite: previous } : t
			);
		}
	}
</script>

<div class="album-page">
	<button class="back-link" type="button" onclick={() => goBack('/library')}>Back</button>
	{#if loading}
		<div class="status-wrap"><Skeleton rows={4} label="Loading album" /></div>
	{:else if error}
		<EmptyState title="Album could not load" copy={error}>
			{#snippet actions()}
				<a class="empty-action" href="/library">Back to library</a>
			{/snippet}
		</EmptyState>
	{:else if !header()}
		<EmptyState title="Album not found" copy="It may have been deleted or moved.">
			{#snippet actions()}
				<a class="empty-action" href="/library">Back to library</a>
			{/snippet}
		</EmptyState>
	{:else}
		{@const h = header()!}

		<DetailHero
			eyebrow="Album"
			title={h.title}
			artwork={h.artwork_url}
			backdrop={h.artwork_url}
			fallbackText={h.title.slice(0, 1)}
			variant="immersive"
		>
			{#snippet meta()}
						{#if h.artist_id != null}
							<a
								href="/artists/{h.artist_id}"
								class="hero-link"
								oncontextmenu={(e) => {
									e.preventDefault();
									e.stopPropagation();
									openContextMenu(e, buildArtistMenu({ id: h.artist_id, name: h.artist_name }, { isLocal: true }), h.artist_name);
								}}
							>{h.artist_name}</a>
						{:else}
							<span>{h.artist_name}</span>
						{/if}
						<span class="dot">·</span>
						<span>{h.total_track_count} {h.total_track_count === 1 ? 'song' : 'songs'}</span>
						<span class="dot">·</span>
						<span class="hero-duration">{formatTotalDuration(h.total_ms)}</span>
						{#if h.library_track_count > 0 && h.library_track_count < h.total_track_count}
							<span class="dot">·</span>
							<span>{h.library_track_count} in your library</span>
						{/if}
			{/snippet}
			{#snippet actions()}
				<ActionBar
					playing={isAlbumPlaying}
					onplay={onHeroPlay}
					onshuffle={() => void shuffleAlbum(albumId, albumData())}
					shuffleHint="Play this album in random order"
					onradio={onRadioClick}
					radioHint="Similar tracks across your library and TIDAL"
					{radioPending}
					liked={albumIsFavorite}
					onlike={onLikeAlbum}
					likeLabel="Save album to your library"
					unlikeLabel="Remove album from your library"
					likePending={favoritePending}
					onmore={(e) => openContextMenu(e, buildAlbumMenu({
						id: albumId,
						title: h.title,
						artist_id: h.artist_id,
						artist_name: h.artist_name,
					}, { isLocal: true, hideOpen: true }), h.title)}
				>
					{#if isPartialAlbum}
						<button
							class="save-album-btn"
							class:pending={savePending}
							disabled={savePending}
							onclick={onSaveAlbum}
						>
							{#if savePending}
								<span class="btn-spinner" aria-hidden="true"></span>
							{:else}
								<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="M12 5v14M5 12h14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg>
							{/if}
							Save full album
						</button>
					{/if}

				</ActionBar>
			{/snippet}
		</DetailHero>

		<section class="track-table" class:with-plays={showPlays}>
			<div class="track-header">
				<span class="col-num">#</span>
				<span class="col-title">Title</span>
				{#if showPlays}<span class="col-plays">Plays</span>{/if}
				<span class="col-status" aria-hidden="true"></span>
				<span class="col-duration"><svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2" fill="none"/><path d="M12 7v5l3 2" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg></span>
			</div>
			<ol class="track-list">
				{#each displayEntries as entry, idx (entry.kind === 'local' ? entry.local.id : `tidal-${entry.tidal.tidal_id}`)}
					{@const work = workStarts.get(idx)}
					{#if work}
						<li class="work-head">
							<span class="work-title">{work.work}</span>
							<span class="work-duration">{formatTotalDuration(workDurationMs(work.start, work.end))}</span>
						</li>
					{/if}
					{#if entry.kind === 'local'}
						{@const track = entry.local}
						<TrackRow
							{track}
							variant="indexed"
							index={idx}
							isCurrent={$currentTrack?.id === track.id}
							isPlaying={$isPlaying}
							showAlbum={false}
							showArtist={track.artist_name !== h.artist_name}
							displayTitle={works.displayTitles[idx]}
							showPlayCount={showPlays}
							worldPlayCount={track.isrc ? playcountByIsrc.get(track.isrc) : null}
							onRowClick={() => onRowClick(track)}
							menuOptions={{ hideAlbumActions: true }}
						/>
					{:else}
						{@const track = entry.tidal}
						{@const playable = tidalDiscographyTrackToPlayable(track)}
						{@const ok = canPlayTrack(playable)}
						<!-- TIDAL-only album track. Same row height as the library
						     rows so the listing scans as one continuous list. -->
						<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
						<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
						<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
						<li
							class="tidal-album-row"
							class:disabled={!ok}
							role="button"
							tabindex={ok ? 0 : -1}
							aria-disabled={!ok}
							onclick={() => ok && void playAlbum(albumId, track.tidal_id, albumData())}
							oncontextmenu={(e) => {
								e.preventDefault();
								e.stopPropagation();
								openContextMenu(e, buildTidalTrackMenu(playable), track.title);
							}}
							onkeydown={(e) =>
								(e.key === 'Enter' || e.key === ' ')
								&& (e.preventDefault(), ok && void playAlbum(albumId, track.tidal_id, albumData()))}
						>
							<span class="tidal-row-num">{track.track_number ?? idx + 1}</span>
							<span class="tidal-row-title">{works.displayTitles[idx]}</span>
							{#if showPlays}<span class="tidal-row-plays" aria-hidden="true"></span>{/if}
							<span class="status-glyph" title="Not in your library">{'\u25CB'}</span>
							<span class="tidal-row-duration">{formatTrackDuration(track.duration_ms)}</span>
						</li>
					{/if}
				{/each}
			</ol>
		</section>

		{#if otherAlbums.length > 0}
			<section class="more-section">
				<div class="more-head">
					<h2 class="more-title">More by {h.artist_name}</h2>
					{#if h.artist_id != null}
						<a
							class="show-all"
							href="/artists/{h.artist_id}"
							oncontextmenu={(e) => {
								e.preventDefault();
								e.stopPropagation();
								openContextMenu(e, buildArtistMenu({ id: h.artist_id, name: h.artist_name }, { isLocal: true }), h.artist_name);
							}}
						>View all →</a>
					{/if}
				</div>
				<MediaRail items={otherAlbums} getKey={(a) => a.id ?? a.title}>
					{#snippet card(album)}
						{@const albumArt = artworkCandidate(album.artwork_url, 320)}
						<a
							class="album-card"
							href={album.id != null ? `/albums/${album.id}` : undefined}
							oncontextmenu={(e) => {
								e.preventDefault();
								e.stopPropagation();
								if (album.id != null) {
									openContextMenu(e, buildAlbumMenu({
										id: album.id,
										title: album.title,
										artist_id: h.artist_id,
										artist_name: h.artist_name,
									}, { isLocal: true }), album.title);
								}
							}}
						>
							<div class="album-card-art-wrap">
								{#if albumArt}
									<img
										class="album-card-art"
										src={albumArt}
										alt=""
										onerror={() => markArtworkFailed(albumArt)}
									/>
								{:else}
									<div class="album-card-art placeholder">♫</div>
								{/if}
							</div>
							<p class="album-card-title">{album.title}</p>
							<!-- The rail only knows the artist's OWNED rows, so the number is
							     library coverage, not the album's real track count. Label it
							     honestly instead of claiming a 10-track album has "3 tracks". -->
							<p class="album-card-sub">
								{album.count} in library
							</p>
						</a>
					{/snippet}
				</MediaRail>
			</section>
		{/if}
	{/if}
</div>

<style>
	.album-page {
		padding: 0 0 calc(var(--space-7) * 2);
		display: flex;
		flex-direction: column;
	}

	.album-page > .back-link {
		align-self: flex-start;
		margin-bottom: var(--space-3);
	}

	.status-wrap {
		padding: var(--space-6);
	}

	.empty-action {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 8px 16px;
		border-radius: 999px;
		background: var(--accent-soft);
		color: var(--accent-strong);
		text-decoration: none;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		border: 1px solid var(--accent-line);
	}
	.empty-action:hover { background: var(--accent); color: var(--text-on-accent); }

	.btn-spinner {
		width: 16px;
		height: 16px;
		border-radius: 50%;
		border: 2px solid currentColor;
		border-right-color: transparent;
		display: inline-block;
		animation: btn-spin 0.7s linear infinite;
	}
	@keyframes btn-spin {
		to { transform: rotate(360deg); }
	}

	.hero-link {
		color: var(--text-primary);
		font-weight: var(--font-weight-bold);
		text-decoration: none;
	}
	.hero-link:hover { text-decoration: underline; }
	.dot { opacity: 0.5; }
	.hero-duration { color: var(--text-tertiary); }

	.save-album-btn {
		all: unset;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 8px 16px;
		border-radius: 999px;
		background: var(--accent-soft);
		color: var(--accent-strong);
		border: 1px solid var(--accent-line);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast);
	}
	.save-album-btn:hover { background: var(--accent); color: var(--text-on-accent); }
	.save-album-btn.pending { opacity: 0.85; cursor: progress; }
	.save-album-btn:disabled { cursor: progress; }





	.track-table {
		padding: var(--space-2) var(--space-6) 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.track-header {
		display: grid;
		grid-template-columns: 40px 1fr auto 64px;
		align-items: center;
		gap: var(--gap);
		padding: var(--space-2) var(--space-4) var(--space-3);
		border-bottom: 1px solid var(--border-subtle);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		font-weight: var(--font-weight-semibold);
	}

	.col-num { text-align: center; }
	.col-plays { text-align: right; }
	.track-table.with-plays .track-header,
	.track-table.with-plays .tidal-album-row {
		grid-template-columns: 40px 1fr 132px auto 64px;
	}
	.col-duration { display: grid; place-items: center; }

	.track-list {
		list-style: none;
		margin: 0;
		padding: 6px 0 0;
		display: flex;
		flex-direction: column;
		gap: 0;
	}

	/* TIDAL-only row in the album track list. Matches the 5-column grid of
	   .track-header so it lines up cleanly with TrackRow above. */
	.tidal-album-row {
		display: grid;
		grid-template-columns: 40px 1fr auto 64px;
		align-items: center;
		gap: var(--gap);
		padding: var(--space-2) var(--space-4);
		cursor: pointer;
		transition: background 120ms ease;
		min-height: 44px;
	}
	.tidal-album-row:hover { background: rgba(255, 255, 255, 0.04); }
	.tidal-album-row.disabled { cursor: not-allowed; opacity: 0.55; }
	.tidal-row-num {
		text-align: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		font-variant-numeric: tabular-nums;
	}
	.tidal-row-title {
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.status-glyph {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	/* A work inside a long album: the shared title prefix and its length. */
	.work-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-4) var(--space-4) var(--space-1);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.work-head:first-child {
		padding-top: var(--space-1);
	}

	.work-duration {
		color: var(--text-tertiary);
		font-weight: var(--font-weight-medium);
		font-variant-numeric: tabular-nums;
	}

	.tidal-row-duration {
		text-align: right;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		font-variant-numeric: tabular-nums;
	}



	.more-section {
		padding: var(--space-6) var(--space-6) 0;
		display: flex;
		flex-direction: column;
		gap: var(--gap);
	}

	.more-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
	}

	.more-title {
		font-family: var(--font-body);
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		margin: 0;
		letter-spacing: 0;
	}

	.show-all {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.1em;
		text-decoration: none;
		font-weight: var(--font-weight-semibold);
	}
	.show-all:hover { color: var(--text-primary); text-decoration: underline; }

	/* "More by artist" rail card: fixed width so the row stays uniform.
	   The MediaRail container handles horizontal scroll. */
	.album-card {
		flex: 0 0 158px;
		min-width: 158px;
		max-width: 158px;
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 0;
		border-radius: var(--radius-md);
		text-decoration: none;
		color: inherit;
		transition: transform var(--motion-base);
	}

	.album-card:hover {
		transform: translateY(-4px);
	}

	.album-card-art-wrap {
		width: 100%;
		aspect-ratio: 1/1;
		border-radius: var(--radius-md);
		overflow: hidden;
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);
		background: var(--bg-raised);
		margin-bottom: 6px;
		transition: box-shadow var(--motion-base);
	}

	.album-card:hover .album-card-art-wrap {
		box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5);
	}

	.album-card-art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.album-card-art.placeholder {
		display: grid;
		place-items: center;
		font-size: var(--font-size-2xl);
		color: var(--text-tertiary);
	}

	.album-card-title {
		margin: 6px 0 0;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.album-card-sub {
		margin: 0;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	@container workspace (max-width: 720px) {
		.track-table { padding: var(--space-2) var(--space-3) 0; }
		.track-header,
		.track-table.with-plays .track-header { grid-template-columns: 36px 1fr auto 56px; }
		.col-plays { display: none; }
		.more-section { padding: var(--space-5) var(--space-4) 0; }
	}
</style>
