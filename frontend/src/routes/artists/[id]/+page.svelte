<script lang="ts">
	import { page } from '$app/state';
	import type { Snapshot } from './$types';
	import { api, type Track, type TidalDiscographyAlbum, type SpotifyArtistStats } from '$lib/api/client';
	import {
		playArtist,
		shuffleArtist,
		startArtistRadio,
		playTidalAlbum,
		playAlbum,
		toggleTrackFavorite,
		currentTrack,
		isPlaying,
		togglePlayback
	} from '$lib/stores/player';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';

	let artistId = $derived(Number(page.params.id));

	let tracks = $state<Track[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let tidalAlbums = $state<TidalDiscographyAlbum[]>([]);
	let tidalLoading = $state(false);
	let tidalAvailable = $state(false);

	let spotifyStats = $state<SpotifyArtistStats | null>(null);
	let playcountByIsrc = $derived.by(() => {
		const map = new Map<string, number>();
		for (const t of spotifyStats?.tracks ?? []) {
			if (t.playcount != null) map.set(t.isrc, t.playcount);
		}
		return map;
	});

	function formatStreamCount(n: number): string {
		if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
		if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
		if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
		return n.toString();
	}

	// Phase 5B — back/forward state via SvelteKit snapshot.
	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: typeof window !== 'undefined' ? window.scrollY : 0 }),
		restore: (saved) => {
			requestAnimationFrame(() => window.scrollTo({ top: saved.scrollY, behavior: 'auto' }));
		}
	};

	async function load() {
		loading = true;
		error = null;
		try {
			const res = await api.getArtistTracks(artistId);
			tracks = res.tracks;
		} catch (err) {
			error = `Failed to load artist: ${err}`;
		} finally {
			loading = false;
		}
	}

	async function loadDiscography() {
		tidalLoading = true;
		try {
			const res = await api.getArtistDiscography(artistId);
			tidalAlbums = res.albums;
			tidalAvailable = res.available;
		} catch (err) {
			console.error('Failed to load TIDAL discography', err);
		} finally {
			tidalLoading = false;
		}
	}

	async function loadSpotifyStats() {
		try {
			spotifyStats = await api.getArtistSpotifyStats(artistId);
		} catch (err) {
			console.error('Failed to load Spotify stats', err);
			spotifyStats = null;
		}
	}

	$effect(() => {
		artistId;
		tidalAlbums = [];
		tidalAvailable = false;
		spotifyStats = null;
		void load();
		void loadDiscography();
		void loadSpotifyStats();
	});

	let header = $derived(() => {
		const first = tracks[0];
		if (!first) return null;
		return {
			name: first.artist_name ?? 'Unknown artist',
			artwork_url: first.artwork_url,
			track_count: tracks.length
		};
	});

	let showAllPopular = $state(false);
	let popular = $derived(
		[...tracks]
			.sort((a, b) => b.play_count - a.play_count)
			.slice(0, showAllPopular ? Infinity : 10)
	);
	let popularMaxPlays = $derived(popular[0]?.play_count ?? 1);
	let totalPopularCandidates = $derived(tracks.length);

	let libraryAlbumMap = $derived.by(() => {
		const map = new Map<number, { id: number; title: string; artwork_url: string | null; count: number }>();
		for (const t of tracks) {
			if (t.album_id == null) continue;
			const existing = map.get(t.album_id);
			if (existing) {
				existing.count += 1;
			} else {
				map.set(t.album_id, {
					id: t.album_id,
					title: t.album_title ?? 'Album',
					artwork_url: t.artwork_url,
					count: 1
				});
			}
		}
		return map;
	});

	function releaseYear(d: string | null): number | null {
		if (!d) return null;
		const y = parseInt(d.slice(0, 4), 10);
		return Number.isFinite(y) ? y : null;
	}

	type DiscoCategory = 'album' | 'ep_single' | 'compilation' | 'live';
	function categorize(a: TidalDiscographyAlbum): DiscoCategory {
		const type = (a.release_type ?? '').toUpperCase();
		if (type === 'COMPILATION') return 'compilation';
		if (type === 'LIVE') return 'live';
		if (type === 'SINGLE' || type === 'EP') return 'ep_single';
		if (type === 'ALBUM') return 'album';
		return (a.number_of_tracks ?? 0) >= 3 ? 'album' : 'ep_single';
	}

	function sortByDate(list: TidalDiscographyAlbum[]): TidalDiscographyAlbum[] {
		return [...list].sort(
			(a, b) => (releaseYear(b.release_date) ?? 0) - (releaseYear(a.release_date) ?? 0)
		);
	}

	let tidalFullAlbums = $derived(sortByDate(tidalAlbums.filter((a) => categorize(a) === 'album')));
	let tidalSinglesEPs = $derived(sortByDate(tidalAlbums.filter((a) => categorize(a) === 'ep_single')));
	let tidalCompilations = $derived(sortByDate(tidalAlbums.filter((a) => categorize(a) === 'compilation')));
	let tidalLiveAlbums = $derived(sortByDate(tidalAlbums.filter((a) => categorize(a) === 'live')));

	// Fallback (used when TIDAL unavailable): group library tracks into albums.
	let fallbackAlbums = $derived.by(() => {
		const map = new Map<
			string,
			{ id: number | null; title: string; artwork_url: string | null; tracks: Track[] }
		>();
		for (const t of tracks) {
			const key = t.album_id == null ? `single-${t.id}` : String(t.album_id);
			const existing = map.get(key);
			if (existing) {
				existing.tracks.push(t);
			} else {
				map.set(key, {
					id: t.album_id,
					title: t.album_title ?? t.title,
					artwork_url: t.artwork_url,
					tracks: [t]
				});
			}
		}
		return Array.from(map.values()).sort((a, b) => b.tracks.length - a.tracks.length);
	});

	let fallbackFullAlbums = $derived(fallbackAlbums.filter((a) => a.tracks.length >= 3));
	let fallbackSinglesEPs = $derived(fallbackAlbums.filter((a) => a.tracks.length < 3));

	function onHeroPlay() {
		const current = $currentTrack;
		if (current && tracks.some((t) => t.id === current.id)) {
			void togglePlayback();
		} else {
			void playArtist(artistId);
		}
	}

	let isArtistPlaying = $derived(
		$isPlaying && tracks.some((t) => t.id === $currentTrack?.id)
	);

	let radioPending = $state(false);
	async function onRadioClick() {
		if (radioPending) return;
		radioPending = true;
		try {
			await startArtistRadio(artistId);
		} finally {
			radioPending = false;
		}
	}

	async function onHeartClick(track: Track, event: MouseEvent) {
		event.stopPropagation();
		// Optimistic local flip on the row data so the icon swaps before round-trip.
		const previous = track.is_favorite;
		tracks = tracks.map((t) =>
			t.id === track.id ? { ...t, is_favorite: !previous } : t
		);
		try {
			await toggleTrackFavorite(track.id, previous);
		} catch {
			// Roll back local row state on failure.
			tracks = tracks.map((t) =>
				t.id === track.id ? { ...t, is_favorite: previous } : t
			);
		}
	}

	function onAlbumCardPlay(album: { id: number | null }, event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		if (album.id != null) void playAlbum(album.id);
	}

	let filterQuery = $state('');

	const filteredPopular = $derived(
		filterQuery
			? popular.filter((t) => t.title.toLowerCase().includes(filterQuery.toLowerCase()))
			: popular
	);

	const filteredTidalFullAlbums = $derived(
		filterQuery
			? tidalFullAlbums.filter((a) => a.title.toLowerCase().includes(filterQuery.toLowerCase()))
			: tidalFullAlbums
	);

	const filteredTidalSinglesEPs = $derived(
		filterQuery
			? tidalSinglesEPs.filter((a) => a.title.toLowerCase().includes(filterQuery.toLowerCase()))
			: tidalSinglesEPs
	);

	const filteredTidalCompilations = $derived(
		filterQuery
			? tidalCompilations.filter((a) => a.title.toLowerCase().includes(filterQuery.toLowerCase()))
			: tidalCompilations
	);

	const filteredTidalLiveAlbums = $derived(
		filterQuery
			? tidalLiveAlbums.filter((a) => a.title.toLowerCase().includes(filterQuery.toLowerCase()))
			: tidalLiveAlbums
	);

	function discographyAlbumMenu(album: TidalDiscographyAlbum, artistName: string) {
		return buildAlbumMenu(
			{
				local_id: album.local_id,
				tidal_id: album.tidal_id,
				title: album.title,
				artist_name: artistName,
				in_library: album.in_library,
			},
			{ isLocal: album.in_library && album.local_id != null }
		);
	}
</script>

<div class="artist-page">
	{#if loading}
		<div class="status-wrap"><Skeleton rows={4} label="Loading artist" /></div>
	{:else if error}
		<EmptyState title="Artist could not load" copy={error}>
			{#snippet actions()}
				<a class="empty-action" href="/library">Back to library</a>
			{/snippet}
		</EmptyState>
	{:else if !header()}
		<EmptyState title="Artist not found" copy="It may have been deleted or moved.">
			{#snippet actions()}
				<a class="empty-action" href="/library">Back to library</a>
			{/snippet}
		</EmptyState>
	{:else}
		{@const h = header()!}

		<header class="hero">
			{#if h.artwork_url}
				<div class="hero-backdrop" style="background-image: url({h.artwork_url});"></div>
			{/if}
			<div class="hero-veil"></div>

			<div class="hero-body">
				<p class="eyebrow">
					<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M12 2l2.9 6.5 7.1.6-5.4 4.7 1.6 7-6.2-3.7L5.8 21l1.6-7L2 9.1l7.1-.6L12 2z" fill="currentColor"/></svg>
					Artist
				</p>
				<h1 class="hero-title display-face">{h.name}</h1>
				<p class="hero-sub">
					{h.track_count.toLocaleString()} {h.track_count === 1 ? 'song' : 'songs'} in your library
					{#if spotifyStats?.monthly_listeners != null}
						<span class="dot">·</span>
						<span class="hero-listeners">{formatStreamCount(spotifyStats.monthly_listeners)} monthly listeners</span>
					{/if}
				</p>
			</div>
		</header>

		<div class="actions-bar">
			<button
				class="play-fab"
				aria-label={isArtistPlaying ? 'Pause' : 'Play'}
				onclick={onHeroPlay}
			>
				{#if isArtistPlaying}
					<svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><rect x="6" y="5" width="4" height="14" rx="1" fill="currentColor"/><rect x="14" y="5" width="4" height="14" rx="1" fill="currentColor"/></svg>
				{:else}
					<svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><path d="M8 5.5v13a1 1 0 001.5.87l11-6.5a1 1 0 000-1.74l-11-6.5A1 1 0 008 5.5z" fill="currentColor"/></svg>
				{/if}
			</button>

			<button class="ghost-btn" aria-label="Shuffle" onclick={() => void shuffleArtist(artistId)}>
				<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><path d="M16 3h5v5M4 20l17-17M21 16v5h-5M4 4l5 5m6 6l6 6" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>
			</button>

			<button
				class="ghost-btn"
				class:pending={radioPending}
				aria-label="Artist radio"
				disabled={radioPending}
				onclick={onRadioClick}
			>
				{#if radioPending}
					<span class="btn-spinner" aria-hidden="true"></span>
				{:else}
					<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><circle cx="12" cy="12" r="3" fill="currentColor"/><path d="M8.5 8.5a5 5 0 000 7M15.5 8.5a5 5 0 010 7M5.5 5.5a9 9 0 000 13M18.5 5.5a9 9 0 010 13" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg>
				{/if}
			</button>
		</div>

		<p class="actions-microcopy">
			<strong>Shuffle</strong> plays this artist's tracks in random order.
			<strong>Radio</strong> finds similar tracks across your library and Tidal.
		</p>

		<div class="filter-bar">
			<input
				class="filter-input"
				type="text"
				placeholder="Filter tracks and albums…"
				bind:value={filterQuery}
			/>
		</div>

		{#if filteredPopular.length > 0}
			<section class="section">
				<h2 class="section-title">Popular</h2>
				<ol class="popular-list">
					{#each filteredPopular as track, idx (track.id)}
						{@const streamCount = track.isrc ? playcountByIsrc.get(track.isrc) : undefined}
						<div class="popular-row-wrap">
							<div
								class="pop-bar"
								style="width: {Math.max(4, ((track.play_count ?? 0) / popularMaxPlays) * 100)}%"
							></div>
							<TrackRow
								{track}
								variant="numbered"
								index={idx}
								isCurrent={$currentTrack?.id === track.id}
								isPlaying={$isPlaying}
								showArtist={false}
								showPlayCount={true}
								onRowClick={() => void playArtist(artistId, track.id)}
								menuOptions={{ hideArtistActions: true }}
							/>
							{#if streamCount != null}
								<span class="stream-badge" title="{streamCount.toLocaleString()} streams on Spotify">
									{formatStreamCount(streamCount)}
								</span>
							{/if}
						</div>
					{/each}
				</ol>
				{#if !showAllPopular && totalPopularCandidates > 10}
					<button class="show-all-btn" onclick={() => (showAllPopular = true)}>
						Show all {totalPopularCandidates}
					</button>
				{:else if showAllPopular && totalPopularCandidates > 10}
					<button class="show-all-btn" onclick={() => (showAllPopular = false)}>
						Show fewer
					</button>
				{/if}
			</section>
		{/if}

		{#snippet discographyCard(album: TidalDiscographyAlbum, kind: DiscoCategory)}
			{@const year = releaseYear(album.release_date)}
			{@const kindLabel = kind === 'album' ? 'Album'
				: kind === 'compilation' ? 'Compilation'
				: kind === 'live' ? 'Live'
				: (album.release_type ?? '').toUpperCase() === 'EP' ? 'EP' : 'Single'}
			<a
				class="grid-card"
				class:not-in-library={!album.in_library}
				href={album.local_id != null ? `/albums/${album.local_id}` : `/tidal/albums/${album.tidal_id}`}
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, discographyAlbumMenu(album, h.name), album.title);
				}}
			>
				<div class="grid-art-wrap">
					{#if album.artwork_url}
						<img class="grid-art" src={album.artwork_url} alt="" />
					{:else}
						<div class="grid-art placeholder">♫</div>
					{/if}
					{#if !album.in_library}
						<span class="badge-new">TIDAL</span>
						<button
							class="art-play-overlay"
							onclick={(e) => { e.preventDefault(); e.stopPropagation(); void playTidalAlbum(album.tidal_id) }}
							aria-label="Play {album.title}"
						>▶</button>
					{:else if album.local_id != null}
						<button
							class="art-play-overlay"
							onclick={(e) => onAlbumCardPlay({ id: album.local_id }, e)}
							aria-label="Play {album.title}"
						>▶</button>
					{/if}
				</div>
				<p class="grid-title">{album.title}</p>
				<p class="grid-sub">
					{#if year}{year} · {/if}{kindLabel}{#if album.in_library} · In library{/if}
				</p>
			</a>
		{/snippet}

		{#if tidalAvailable}
			{#if filteredTidalFullAlbums.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Albums</h2>
						<span class="shelf-count">{filteredTidalFullAlbums.length}</span>
					</div>
					<div class="card-row">
						{#each filteredTidalFullAlbums as album (album.tidal_id)}
							{@render discographyCard(album, 'album')}
						{/each}
					</div>
				</section>
			{/if}

			{#if filteredTidalSinglesEPs.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Singles and EPs</h2>
						<span class="shelf-count">{filteredTidalSinglesEPs.length}</span>
					</div>
					<div class="card-row">
						{#each filteredTidalSinglesEPs as album (album.tidal_id)}
							{@render discographyCard(album, 'ep_single')}
						{/each}
					</div>
				</section>
			{/if}

			{#if filteredTidalCompilations.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Compilations</h2>
						<span class="shelf-count">{filteredTidalCompilations.length}</span>
					</div>
					<div class="card-row">
						{#each filteredTidalCompilations as album (album.tidal_id)}
							{@render discographyCard(album, 'compilation')}
						{/each}
					</div>
				</section>
			{/if}

			{#if filteredTidalLiveAlbums.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Live</h2>
						<span class="shelf-count">{filteredTidalLiveAlbums.length}</span>
					</div>
					<div class="card-row">
						{#each filteredTidalLiveAlbums as album (album.tidal_id)}
							{@render discographyCard(album, 'live')}
						{/each}
					</div>
				</section>
			{/if}
		{:else}
			{#if fallbackFullAlbums.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Albums</h2>
						<span class="shelf-count">{fallbackFullAlbums.length}</span>
					</div>
					<div class="card-row">
						{#each fallbackFullAlbums as album (album.id ?? album.title)}
							<a class="grid-card" href={album.id != null ? `/albums/${album.id}` : '#'}>
								<div class="grid-art-wrap">
									{#if album.artwork_url}
										<img class="grid-art" src={album.artwork_url} alt="" />
									{:else}
										<div class="grid-art placeholder">♫</div>
									{/if}
									{#if album.id != null}
										<button
											class="art-play-overlay"
											onclick={(e) => onAlbumCardPlay({ id: album.id }, e)}
											aria-label="Play {album.title}"
										>▶</button>
									{/if}
								</div>
								<p class="grid-title">{album.title}</p>
								<p class="grid-sub">{album.tracks.length} tracks · Album</p>
							</a>
						{/each}
					</div>
				</section>
			{/if}

			{#if fallbackSinglesEPs.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Singles and EPs</h2>
						<span class="shelf-count">{fallbackSinglesEPs.length}</span>
					</div>
					<div class="card-row">
						{#each fallbackSinglesEPs as album (album.id ?? album.title)}
							<a class="grid-card" href={album.id != null ? `/albums/${album.id}` : '#'}>
								<div class="grid-art-wrap">
									{#if album.artwork_url}
										<img class="grid-art" src={album.artwork_url} alt="" />
									{:else}
										<div class="grid-art placeholder">♫</div>
									{/if}
									{#if album.id != null}
										<button
											class="art-play-overlay"
											onclick={(e) => onAlbumCardPlay({ id: album.id }, e)}
											aria-label="Play {album.title}"
										>▶</button>
									{/if}
								</div>
								<p class="grid-title">{album.title}</p>
								<p class="grid-sub">
									{album.tracks.length === 1 ? 'Single' : `${album.tracks.length} tracks · EP`}
								</p>
							</a>
						{/each}
					</div>
				</section>
			{/if}

			{#if tidalLoading}
				<p class="status subtle">Loading full discography from TIDAL…</p>
			{/if}
		{/if}
	{/if}
</div>

<style>
	.artist-page {
		padding: 0 0 80px;
		display: flex;
		flex-direction: column;
	}

	.status {
		padding: 48px 28px;
		text-align: center;
		color: var(--text-secondary);
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
		font-size: 0.85rem;
		font-weight: 600;
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
		padding: 46px 32px 30px;
		display: flex;
		min-height: 300px;
		overflow: hidden;
		isolation: isolate;
		align-items: flex-end;
	}

	.hero-backdrop {
		position: absolute;
		inset: -80px;
		background-size: cover;
		background-position: center;
		filter: blur(80px) saturate(1.8);
		transform: scale(1.3);
		z-index: -2;
		opacity: 0.85;
	}

	.hero-veil {
		position: absolute;
		inset: 0;
		background:
			linear-gradient(180deg, rgba(0,0,0,0.15) 0%, rgba(0,0,0,0.48) 70%, var(--bg-base) 100%);
		z-index: -1;
	}

	.hero-body {
		display: flex;
		flex-direction: column;
		gap: 10px;
		width: 100%;
		max-width: 1400px;
	}

	.eyebrow {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: 0.78rem;
		color: var(--text-primary);
		margin: 0;
		font-weight: 600;
	}

	.hero-title {
		font-family: var(--font-display);
		font-size: clamp(2.6rem, 6vw, 5rem);
		line-height: 1;
		letter-spacing: -0.03em;
		margin: 0;
		color: var(--text-primary);
		word-wrap: break-word;
	}

	.hero-sub {
		color: var(--text-secondary);
		margin: 4px 0 0;
		font-size: 0.88rem;
	}

	.filter-bar {
		padding: 8px 32px 0;
	}

	.filter-input {
		background: var(--input-bg);
		border: 1px solid var(--input-border);
		border-radius: 20px;
		padding: 7px 16px;
		font-size: 13px;
		color: var(--text-primary);
		outline: none;
		width: 260px;
		transition: border-color 0.15s;
	}
	.filter-input:focus { border-color: var(--accent); background: var(--input-focus); }

	.actions-bar {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 18px 32px 8px;
	}

	.actions-microcopy {
		margin: 0;
		padding: 0 32px 8px;
		color: var(--text-tertiary);
		font-size: 0.78rem;
		line-height: 1.4;
	}

	.actions-microcopy strong {
		color: var(--text-secondary);
		font-weight: 600;
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
		transition: transform var(--motion-fast), background var(--motion-fast);
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

	.section {
		padding: 24px 32px 0;
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	.section-title {
		font-family: var(--font-body);
		font-size: 1.15rem;
		font-weight: 700;
		margin: 0;
		letter-spacing: 0;
	}

	.shelf-head {
		display: flex;
		align-items: baseline;
		gap: 10px;
	}

	.shelf-count {
		color: var(--text-tertiary);
		font-size: 0.8rem;
	}

	.popular-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0;
	}

	.card-row {
		display: grid;
		grid-auto-flow: column;
		grid-auto-columns: minmax(180px, 200px);
		gap: 18px;
		overflow-x: auto;
		scroll-snap-type: x proximity;
		padding-bottom: 6px;
		scrollbar-width: thin;
	}

	.card-row::-webkit-scrollbar { height: 8px; }
	.card-row::-webkit-scrollbar-thumb {
		background: var(--border-strong);
		border-radius: 999px;
	}

	.grid-card {
		scroll-snap-align: start;
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 14px;
		border-radius: 10px;
		text-decoration: none;
		color: inherit;
		transition: background var(--motion-fast);
	}

	.grid-card:hover { background: var(--bg-hover); }

	.grid-art-wrap {
		position: relative;
		width: 100%;
		aspect-ratio: 1/1;
		border-radius: 6px;
		overflow: hidden;
		box-shadow: 0 10px 24px -12px rgba(0, 0, 0, 0.6);
		background: var(--bg-surface);
		margin-bottom: 6px;
	}

	.art-play-overlay {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: rgba(0, 0, 0, 0.45);
		color: #fff;
		font-size: 22px;
		border: none;
		cursor: pointer;
		border-radius: 6px;
		opacity: 0;
		transition: opacity 0.15s;
	}
	.grid-art-wrap:hover .art-play-overlay { opacity: 1; }

	.badge-new {
		position: absolute;
		top: 8px;
		right: 8px;
		padding: 3px 8px;
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.55);
		color: #fff;
		font-size: 0.62rem;
		font-weight: 700;
		letter-spacing: 0.12em;
		backdrop-filter: blur(8px);
	}

	.grid-card.not-in-library .grid-title {
		color: var(--text-secondary);
	}

	.status.subtle {
		color: var(--text-tertiary);
		padding: 20px 32px;
		font-size: 0.82rem;
	}

	.grid-art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.grid-art.placeholder {
		display: grid;
		place-items: center;
		font-size: 2rem;
		color: var(--text-tertiary);
	}

	.grid-title {
		margin: 6px 0 0;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.grid-sub {
		margin: 0;
		font-size: 0.8rem;
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	@media (max-width: 720px) {
		.hero { padding: 36px 20px 24px; min-height: 240px; }
		.hero-title { font-size: 2.6rem; }
		.actions-bar { padding: 12px 20px; }
		.section { padding: 20px 20px 0; }
	}

	.popular-row-wrap {
		position: relative;
		border-radius: var(--radius-sm, 8px);
		overflow: hidden;
		isolation: isolate;
	}

	.pop-bar {
		position: absolute;
		inset-block: 0;
		left: 0;
		background: linear-gradient(90deg, var(--accent-soft, rgba(125, 99, 255, 0.18)) 0%, transparent 100%);
		pointer-events: none;
		transition: width 400ms ease;
		z-index: 0;
	}

	.popular-row-wrap > :global(*:not(.pop-bar)) {
		position: relative;
		z-index: 1;
	}

	.show-all-btn {
		margin: 12px auto 0;
		display: block;
		padding: 6px 16px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.09);
		color: var(--text-secondary, rgba(255, 255, 255, 0.7));
		font-size: 0.85rem;
		cursor: pointer;
		transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
	}
	.show-all-btn:hover {
		background: rgba(255, 255, 255, 0.11);
		border-color: rgba(255, 255, 255, 0.16);
		color: var(--text-primary, #fff);
	}

	.hero-listeners {
		color: var(--text-secondary, rgba(255, 255, 255, 0.7));
		font-variant-numeric: tabular-nums;
	}

	.stream-badge {
		position: absolute;
		right: 60px;
		top: 50%;
		transform: translateY(-50%);
		padding: 2px 8px;
		border-radius: 999px;
		background: rgba(30, 215, 96, 0.14);
		border: 1px solid rgba(30, 215, 96, 0.32);
		color: rgba(30, 215, 96, 0.95);
		font-size: 0.72rem;
		font-variant-numeric: tabular-nums;
		font-weight: 600;
		pointer-events: none;
		z-index: 2;
	}
</style>
