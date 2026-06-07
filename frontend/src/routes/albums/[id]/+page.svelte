<script lang="ts">
	import { page } from '$app/state';
	import type { Snapshot } from './$types';
	import { type Track, type TidalDiscographyTrack, type SpotifyTrackStats } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import {
		playAlbum,
		playTidalAlbum,
		playTidalTrackNow,
		shuffleAlbum,
		startAlbumRadio,
		toggleTrackFavorite,
		currentTrack,
		isPlaying,
		togglePlayback
	} from '$lib/stores/player';
	import { canPlayTrack } from '$lib/player/playable';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
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
	import { formatTotalDuration } from '$lib/utils/format';
	import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';

	let albumId = $derived(Number(page.params.id));

	let tracks = $state<Track[]>([]);
	let tidalOnlyTracks = $state<TidalDiscographyTrack[]>([]);
	let albumTidalId = $state<number | null>(null);
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

	// Phase 5B: back/forward state via SvelteKit snapshot.
	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: typeof window !== 'undefined' ? window.scrollY : 0 }),
		restore: (saved) => {
			requestAnimationFrame(() => window.scrollTo({ top: saved.scrollY, behavior: 'auto' }));
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
	let heroArtworkSrc = $derived(artworkCandidate(header()?.artwork_url, 640));
	let heroBackdropSrc = $derived(artworkCandidate(header()?.artwork_url, 1280));

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

	function onRowClick(track: Track) {
		void playAlbum(albumId, track.id);
	}

	function onHeroPlay() {
		const current = $currentTrack;
		if (current && tracks.some((t) => t.id === current.id)) {
			void togglePlayback();
			return;
		}
		// Per "show everything" philosophy: if the album exists on TIDAL, play
		// the FULL TIDAL album so partial-library users still hear the whole
		// thing. Local-only albums (no tidal_id) keep the old behavior so
		// WASAPI exclusive bit-perfect output isn't routed through streaming
		// when there's no need.
		if (albumTidalId != null && tidalOnlyTracks.length > 0) {
			void playTidalAlbum(albumTidalId);
		} else {
			void playAlbum(albumId);
		}
	}

	let isAlbumPlaying = $derived(
		$isPlaying && tracks.some((t) => t.id === $currentTrack?.id)
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
	<a class="back-link" href="/library">← Back to library</a>
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

		<header class="hero">
			{#if heroBackdropSrc}
				<img
					class="hero-backdrop"
					src={heroBackdropSrc}
					alt=""
					onerror={() => markArtworkFailed(heroBackdropSrc)}
				/>
			{/if}
			<div class="hero-veil"></div>

			<div class="hero-body">
				<div class="hero-art-wrap">
					{#if heroArtworkSrc}
						<img
							class="hero-art"
							src={heroArtworkSrc}
							alt=""
							onerror={() => markArtworkFailed(heroArtworkSrc)}
						/>
					{:else}
						<div class="hero-art placeholder">♫</div>
					{/if}
				</div>
				<div class="hero-info">
					<p class="eyebrow">Album</p>
					<h1 class="hero-title display-face">{h.title}</h1>
					<p class="hero-sub">
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
					</p>
					{#if h.library_track_count > 0 && h.library_track_count < h.total_track_count}
						<p class="hero-library-substat">
							{h.library_track_count} in your library
						</p>
					{/if}
				</div>
			</div>
		</header>

		<div class="actions-bar">
			<button
				class="play-fab"
				aria-label={isAlbumPlaying ? 'Pause' : 'Play album'}
				onclick={onHeroPlay}
			>
				{#if isAlbumPlaying}
					<svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><rect x="6" y="5" width="4" height="14" rx="1" fill="currentColor"/><rect x="14" y="5" width="4" height="14" rx="1" fill="currentColor"/></svg>
				{:else}
					<svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><path d="M8 5.5v13a1 1 0 001.5.87l11-6.5a1 1 0 000-1.74l-11-6.5A1 1 0 008 5.5z" fill="currentColor"/></svg>
				{/if}
			</button>

			<button class="ghost-btn" aria-label="Shuffle" onclick={() => void shuffleAlbum(albumId)}>
				<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><path d="M16 3h5v5M4 20l17-17M21 16v5h-5M4 4l5 5m6 6l6 6" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>
			</button>

			<button
				class="ghost-btn"
				class:pending={radioPending}
				aria-label="Album radio"
				disabled={radioPending}
				onclick={onRadioClick}
			>
				{#if radioPending}
					<span class="btn-spinner" aria-hidden="true"></span>
				{:else}
					<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><circle cx="12" cy="12" r="3" fill="currentColor"/><path d="M8.5 8.5a5 5 0 000 7M15.5 8.5a5 5 0 010 7M5.5 5.5a9 9 0 000 13M18.5 5.5a9 9 0 010 13" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg>
				{/if}
			</button>

			<span class="actions-spacer"></span>

			<span class="actions-hint">Click a song to start the album from there</span>
		</div>

		<p class="actions-microcopy">
			<strong>Shuffle</strong> plays this album in random order.
			<strong>Radio</strong> finds similar tracks across your library and Tidal.
		</p>

		<section class="track-table">
			<div class="track-header">
				<span class="col-num">#</span>
				<span class="col-title">Title</span>
				<span class="col-plays">Plays</span>
				<span class="col-duration"><svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2" fill="none"/><path d="M12 7v5l3 2" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg></span>
			</div>
			<ol class="track-list">
				{#each tracks as track, idx (track.id)}
					<TrackRow
						{track}
						variant="indexed"
						index={idx}
						isCurrent={$currentTrack?.id === track.id}
						isPlaying={$isPlaying}
						showAlbum={false}
						showPlayCount={true}
						worldPlayCount={track.isrc ? playcountByIsrc.get(track.isrc) : null}
						onRowClick={() => onRowClick(track)}
						menuOptions={{ hideAlbumActions: true }}
					/>
				{/each}
				{#each tidalOnlyTracks as track, idx (`tidal-${track.tidal_id}`)}
					{@const playable = tidalDiscographyTrackToPlayable(track)}
					{@const ok = canPlayTrack(playable)}
					<!-- TIDAL-only album track. Same row height as the library
					     rows above so the listing scans as one continuous list. -->
					<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
					<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
					<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
					<li
						class="tidal-album-row"
						class:disabled={!ok}
						role="button"
						tabindex={ok ? 0 : -1}
						aria-disabled={!ok}
						onclick={() => ok && void playTidalTrackNow(playable)}
						oncontextmenu={(e) => {
							e.preventDefault();
							e.stopPropagation();
							openContextMenu(e, buildTidalTrackMenu(playable), track.title);
						}}
						onkeydown={(e) =>
							(e.key === 'Enter' || e.key === ' ')
							&& (e.preventDefault(), ok && void playTidalTrackNow(playable))}
					>
						<span class="tidal-row-num">{tracks.length + idx + 1}</span>
						<span class="tidal-row-title">{track.title}</span>
						<span class="tidal-row-plays" aria-hidden="true">-</span>
						<span class="tidal-row-pill" aria-label="From TIDAL">TIDAL</span>
						<span class="tidal-row-duration">
							{#if track.duration_ms}
								{Math.floor(track.duration_ms / 1000 / 60)}:{String(
									Math.round((track.duration_ms / 1000) % 60),
								).padStart(2, '0')}
							{/if}
						</span>
					</li>
				{/each}
			</ol>
		</section>

		<p class="footnote">{h.artist_name}</p>

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
						>Show all</a>
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
							<p class="album-card-sub">
								{album.count} {album.count === 1 ? 'track' : 'tracks'}
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
		padding: 0 0 80px;
		display: flex;
		flex-direction: column;
	}

	.album-page > .back-link {
		align-self: flex-start;
		margin-bottom: var(--space-3);
	}

	.status-wrap {
		padding: 32px;
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
	.empty-action:hover { background: var(--accent); color: #fff; }

	.btn-spinner {
		width: 16px;
		height: 16px;
		border-radius: 50%;
		border: 2px solid currentColor;
		border-right-color: transparent;
		display: inline-block;
		animation: btn-spin 0.7s linear infinite;
	}
	.ghost-btn.pending { opacity: 0.85; cursor: progress; }
	.ghost-btn:disabled { cursor: progress; }
	@keyframes btn-spin {
		to { transform: rotate(360deg); }
	}

	.hero {
		position: relative;
		padding: var(--space-5) var(--space-5) var(--space-4);
		display: flex;
		min-height: 300px;
		overflow: hidden;
		isolation: isolate;
		border-radius: var(--radius-lg);
		border: 1px solid var(--border-subtle);
	}

	.hero-backdrop {
		position: absolute;
		inset: -60px;
		width: calc(100% + 120px);
		height: calc(100% + 120px);
		object-fit: cover;
		object-position: center;
		filter: blur(72px) saturate(1.08) brightness(0.72);
		transform: scale(1.16);
		z-index: -2;
		opacity: 0.32;
	}

	.hero-veil {
		position: absolute;
		inset: 0;
		background:
			linear-gradient(180deg, rgba(11, 11, 15, 0.62) 0%, rgba(11, 11, 15, 0.78) 68%, var(--bg-base) 100%);
		z-index: -1;
	}

	.hero-body {
		display: grid;
		grid-template-columns: clamp(160px, 16vw, 240px) 1fr;
		gap: var(--space-5);
		align-items: end;
		width: 100%;
		max-width: var(--content-width);
	}

	.hero-art-wrap {
		width: clamp(160px, 16vw, 240px);
		aspect-ratio: 1 / 1;
		border-radius: var(--radius-md);
		overflow: hidden;
		box-shadow: 0 28px 70px -14px rgba(0, 0, 0, 0.7);
		background: var(--bg-surface);
	}

	.hero-art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.hero-art.placeholder {
		display: grid;
		place-items: center;
		font-size: var(--font-size-4xl);
		color: var(--text-tertiary);
	}

	.hero-info {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		min-width: 0;
	}

	.eyebrow {
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-primary);
		margin: 0;
		font-weight: var(--font-weight-semibold);
	}

	.hero-title {
		font-family: var(--font-display);
		font-size: var(--font-size-4xl);
		line-height: var(--line-height-tight);
		letter-spacing: -0.02em;
		margin: 0;
		color: var(--text-primary);
		word-wrap: break-word;
	}

	.hero-sub {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		color: var(--text-secondary);
		margin: 4px 0 0;
		font-size: var(--font-size-sm);
	}

	.hero-link {
		color: var(--text-primary);
		font-weight: var(--font-weight-bold);
		text-decoration: none;
	}
	.hero-link:hover { text-decoration: underline; }
	.dot { opacity: 0.5; }
	.hero-duration { color: var(--text-tertiary); }

	.actions-bar {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 18px 32px 4px;
	}

	.play-fab {
		all: unset;
		width: 56px;
		height: 56px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent);
		color: #fff;
		cursor: pointer;
		transition: transform var(--motion-fast), background var(--motion-fast), box-shadow var(--motion-fast);
		box-shadow: 0 8px 24px -8px var(--accent-glow);
	}

	.play-fab:hover {
		transform: scale(1.06);
		background: var(--accent-strong);
	}

	.play-fab:active { transform: scale(0.98); }

	.ghost-btn {
		all: unset;
		width: 40px;
		height: 40px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		color: var(--text-secondary);
		cursor: pointer;
		transition: color var(--motion-fast), background var(--motion-fast);
	}

	.ghost-btn:hover {
		color: var(--text-primary);
		background: var(--bg-hover);
	}

	.actions-spacer { flex: 1; }

	.actions-hint {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.actions-microcopy {
		margin: 0;
		padding: 0 32px 8px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-normal);
	}

	.actions-microcopy strong {
		color: var(--text-secondary);
		font-weight: var(--font-weight-semibold);
	}

	.track-table {
		padding: 8px 32px 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.track-header {
		display: grid;
		grid-template-columns: 40px 1fr 132px auto 64px;
		align-items: center;
		gap: 14px;
		padding: 6px 16px 10px;
		border-bottom: 1px solid var(--border-subtle);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		font-weight: var(--font-weight-semibold);
	}

	.col-num { text-align: center; }
	.col-plays { text-align: right; }
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
		grid-template-columns: 40px 1fr 132px auto 64px;
		align-items: center;
		gap: 14px;
		padding: 8px 16px;
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
	.tidal-row-plays {
		text-align: right;
		color: var(--text-tertiary);
	}
	.tidal-row-pill {
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.06em;
		padding: 3px 8px;
		border-radius: 4px;
		background: rgba(0, 184, 212, 0.16);
		color: rgba(120, 220, 240, 0.95);
		border: 1px solid rgba(0, 184, 212, 0.3);
		text-transform: uppercase;
	}
	.tidal-row-duration {
		text-align: right;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		font-variant-numeric: tabular-nums;
	}

	.hero-library-substat {
		margin: 4px 0 0;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
	}

	.footnote {
		padding: 22px 32px 4px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		margin: 0;
	}

	.more-section {
		padding: 28px 32px 0;
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.more-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 12px;
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
		flex: 0 0 170px;
		min-width: 170px;
		max-width: 170px;
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 14px;
		border-radius: 10px;
		text-decoration: none;
		color: inherit;
		transition: background var(--motion-fast);
	}

	.album-card:hover {
		background: var(--bg-hover);
	}

	.album-card-art-wrap {
		width: 100%;
		aspect-ratio: 1/1;
		border-radius: 6px;
		overflow: hidden;
		box-shadow: 0 10px 24px -12px rgba(0, 0, 0, 0.6);
		background: var(--bg-surface);
		margin-bottom: 6px;
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

	@media (max-width: 720px) {
		.hero { padding: 24px 20px 20px; min-height: auto; }
		.hero-body { grid-template-columns: 1fr; gap: 18px; }
		.hero-art-wrap { width: 180px; height: 180px; }
		.hero-title { font-size: var(--font-size-2xl); }
		.actions-bar { padding: 12px 20px; }
		.track-table { padding: 8px 12px 0; }
		.track-header { grid-template-columns: 36px 1fr auto 56px; }
		.col-plays { display: none; }
		.more-section { padding: 24px 20px 0; }
	}
</style>
