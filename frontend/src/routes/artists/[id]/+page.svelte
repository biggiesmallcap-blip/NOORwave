<script lang="ts">
	import { page } from '$app/state';
	import type { Snapshot } from './$types';
	import { api, type Track, type TidalDiscographyAlbum, type TidalDiscographyTrack, type TidalArtistVideo, type TidalSimilarArtist, type TidalArtistBio, type SpotifyArtistStats, type TidalPlayable } from '$lib/api/client';
	import { letterColor } from '$lib/utils/color';
	import {
		playArtist,
		shuffleArtist,
		startArtistRadio,
		playTidalAlbum,
		playTidalTrackNow,
		playAlbum,
		toggleTrackFavorite,
		currentTrack,
		isPlaying,
		togglePlayback
	} from '$lib/stores/player';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { canPlayTrack } from '$lib/player/playable';

	type ArtistRow = {
		id: number;
		tidal_id: number | null;
		name: string;
		biography: string | null;
		photo_url: string | null;
		track_count: number;
		album_count: number;
	};

	let artistId = $derived(Number(page.params.id));

	let artist = $state<ArtistRow | null>(null);
	let tracks = $state<Track[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let tidalAlbums = $state<TidalDiscographyAlbum[]>([]);
	let tidalTopTracks = $state<TidalDiscographyTrack[]>([]);
	let tidalVideos = $state<TidalArtistVideo[]>([]);
	let tidalSimilarArtists = $state<TidalSimilarArtist[]>([]);
	let tidalBio = $state<TidalArtistBio | null>(null);
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
			// Source-of-truth artist row (name, photo, biography, counts) is
			// fetched in parallel with the artist's local tracks. Either failure
			// is non-fatal — the page can render with whichever resolved.
			const [artistRes, tracksRes] = await Promise.allSettled([
				api.getArtist(artistId),
				api.getArtistTracks(artistId),
			]);
			if (artistRes.status === 'fulfilled') {
				artist = artistRes.value;
			} else {
				artist = null;
			}
			if (tracksRes.status === 'fulfilled') {
				tracks = tracksRes.value.tracks;
			} else {
				tracks = [];
				if (artistRes.status !== 'fulfilled') {
					error = `Failed to load artist: ${tracksRes.reason}`;
				}
			}
		} finally {
			loading = false;
		}
	}

	async function loadDiscography() {
		tidalLoading = true;
		try {
			const res = await api.getArtistDiscography(artistId);
			tidalAlbums = res.albums;
			tidalTopTracks = res.top_tracks ?? [];
			tidalVideos = res.videos ?? [];
			tidalSimilarArtists = res.similar_artists ?? [];
			tidalBio = res.bio ?? null;
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
		artist = null;
		tidalAlbums = [];
		tidalTopTracks = [];
		tidalVideos = [];
		tidalSimilarArtists = [];
		tidalBio = null;
		tidalAvailable = false;
		spotifyStats = null;
		void load();
		void loadDiscography();
		void loadSpotifyStats();
	});

	// Header sources from the artist row when available; falls back to the
	// first track only as a last resort (legacy artists missing a row, etc.).
	// Sourcing from `tracks[0]` was the historical bug that let a corrupt
	// track list rename the page header.
	let header = $derived(() => {
		if (artist) {
			return {
				name: artist.name,
				library_track_count: tracks.length,
			};
		}
		const first = tracks[0];
		if (!first) return null;
		return {
			name: first.artist_name ?? 'Unknown artist',
			library_track_count: tracks.length,
		};
	});

	// Hero portrait resolution, in priority order:
	//   1. The artist's own photo from TIDAL/Spotify (preferred).
	//   2. The first available album cover — used as a glassmorphic backdrop
	//      with a frosted disc on top, matching the Quiet Mode aesthetic.
	//   3. Letter-color fallback inside the disc (handled in the markup).
	let heroPortraitUrl = $derived(artist?.photo_url ?? null);
	let heroBackdropUrl = $derived(
		artist?.photo_url
			?? tidalAlbums.find((a) => a.artwork_url)?.artwork_url
			?? tracks.find((t) => t.artwork_url)?.artwork_url
			?? null
	);
	let heroHasPhoto = $derived(heroPortraitUrl != null);

	// TIDAL's [wimpLink] markup wraps in-text artist/album/track references.
	// Stripping is mechanical — we don't currently render them as links, so
	// keep just the visible text inside the brackets.
	function stripWimpLinks(s: string | null | undefined): string | null {
		if (!s) return null;
		return s.replace(/\[wimpLink[^\]]*\]([^\[]*)\[\/wimpLink\]/g, '$1');
	}
	let bioText = $derived(
		stripWimpLinks(tidalBio?.text)
			?? tidalBio?.summary
			?? artist?.biography
			?? null,
	);
	let bioSource = $derived(tidalBio?.text || tidalBio?.summary ? tidalBio?.source ?? null : null);
	let bioExpanded = $state(false);
	const BIO_TRUNCATE = 280;
	let bioIsLong = $derived((bioText?.length ?? 0) > BIO_TRUNCATE);
	let bioRendered = $derived(
		bioText == null
			? null
			: bioExpanded || !bioIsLong
				? bioText
				: bioText.slice(0, BIO_TRUNCATE).trimEnd() + '…'
	);

	let showAllPopular = $state(false);
	// Library tracks ordered by play_count — float favorites within that.
	let libraryPopular = $derived(
		[...tracks].sort((a, b) => {
			if (a.is_favorite !== b.is_favorite) return a.is_favorite ? -1 : 1;
			return b.play_count - a.play_count;
		})
	);
	// TIDAL top tracks the user does NOT already own. These render under the
	// library section in TIDAL's relevance order so the surface always shows
	// the artist's catalog even when the user has zero library matches.
	let tidalOnlyTopTracks = $derived(
		tidalTopTracks.filter(
			(tt) => !tracks.some((lt) => lt.tidal_id != null && lt.tidal_id === tt.tidal_id),
		),
	);
	let popularMaxPlays = $derived(libraryPopular[0]?.play_count ?? 1);
	// "Top tracks" = library + TIDAL-only, capped at 10 unless expanded.
	let popularDisplayCount = $derived(showAllPopular ? Infinity : 10);
	let visibleLibraryPopular = $derived(libraryPopular.slice(0, popularDisplayCount));
	let visibleTidalPopular = $derived(
		showAllPopular
			? tidalOnlyTopTracks
			: tidalOnlyTopTracks.slice(0, Math.max(0, popularDisplayCount - libraryPopular.length))
	);
	let totalPopularCandidates = $derived(libraryPopular.length + tidalOnlyTopTracks.length);

	function tidalDiscographyTrackToPlayable(t: TidalDiscographyTrack): TidalPlayable {
		return {
			tidal_id: t.tidal_id,
			title: t.title,
			artist_name: t.artist_name ?? artist?.name ?? null,
			album_title: t.album_title,
			artwork_url: t.artwork_url,
			duration_ms: t.duration_ms,
			artist_tidal_id: artist?.tidal_id ?? null,
			album_tidal_id: t.album_tidal_id ?? null,
		};
	}

	function artistInitials(name: string): string {
		return name
			.split(/\s+/)
			.filter((w) => w.length > 0)
			.slice(0, 2)
			.map((w) => w[0]?.toUpperCase() ?? '')
			.join('');
	}

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

	function matchesFilter(title: string): boolean {
		if (!filterQuery) return true;
		return title.toLowerCase().includes(filterQuery.toLowerCase());
	}

	const filteredLibraryPopular = $derived(
		visibleLibraryPopular.filter((t) => matchesFilter(t.title))
	);
	const filteredTidalPopular = $derived(
		visibleTidalPopular.filter((t) => matchesFilter(t.title))
	);
	const hasAnyPopular = $derived(
		filteredLibraryPopular.length > 0 || filteredTidalPopular.length > 0
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

		<header class="hero" class:hero-with-photo={heroHasPhoto}>
			{#if heroBackdropUrl}
				<div class="hero-backdrop" style="background-image: url({heroBackdropUrl});"></div>
			{/if}
			<div class="hero-veil"></div>

			<div class="hero-body">
				<div class="hero-portrait-wrap">
					{#if heroPortraitUrl}
						<img class="hero-portrait" src={heroPortraitUrl} alt="" />
					{:else if heroBackdropUrl}
						<!-- Glassmorphism fallback: blurred album art behind a frosted disc.
						     Mirrors the Quiet Mode aesthetic so artists missing a TIDAL/Spotify
						     photo still feel of-a-piece with the rest of the app. -->
						<div class="hero-portrait hero-portrait-glass">
							<div
								class="hero-portrait-glass-art"
								style="background-image: url({heroBackdropUrl});"
							></div>
							<span class="hero-portrait-initials display-face">{artistInitials(h.name)}</span>
						</div>
					{:else}
						<div
							class="hero-portrait hero-portrait-letter"
							style="background: {letterColor(h.name)};"
						>
							<span class="hero-portrait-initials display-face">{artistInitials(h.name)}</span>
						</div>
					{/if}
				</div>

				<div class="hero-info">
					<p class="eyebrow">
						<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M12 2l2.9 6.5 7.1.6-5.4 4.7 1.6 7-6.2-3.7L5.8 21l1.6-7L2 9.1l7.1-.6L12 2z" fill="currentColor"/></svg>
						Artist
					</p>
					<h1 class="hero-title display-face">{h.name}</h1>
					<p class="hero-sub">
						{#if artist?.track_count}
							{artist.track_count.toLocaleString()} {artist.track_count === 1 ? 'song' : 'songs'}
							<span class="dot">·</span>
						{/if}
						{#if artist?.album_count}
							{artist.album_count.toLocaleString()} {artist.album_count === 1 ? 'album' : 'albums'}
							{#if spotifyStats?.monthly_listeners != null}<span class="dot">·</span>{/if}
						{/if}
						{#if spotifyStats?.monthly_listeners != null}
							<span class="hero-listeners">{formatStreamCount(spotifyStats.monthly_listeners)} monthly listeners</span>
						{/if}
					</p>
					{#if h.library_track_count > 0}
						<p class="hero-library-substat">
							{h.library_track_count.toLocaleString()} {h.library_track_count === 1 ? 'song' : 'songs'} in your library
						</p>
					{/if}
					{#if bioRendered}
						<p class="hero-bio">
							{bioRendered}
							{#if bioIsLong}
								<button
									type="button"
									class="bio-toggle"
									onclick={() => (bioExpanded = !bioExpanded)}
								>{bioExpanded ? 'Show less' : 'Show more'}</button>
							{/if}
						</p>
						{#if bioSource}
							<p class="hero-bio-source">via TIDAL · {bioSource}</p>
						{/if}
					{/if}
				</div>
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

		{#if hasAnyPopular}
			<section class="section">
				<h2 class="section-title">Top tracks</h2>
				<ol class="popular-list">
					{#each filteredLibraryPopular as track, idx (track.id)}
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
					{#each filteredTidalPopular as track, idx (`tidal-${track.tidal_id}`)}
						{@const playable = tidalDiscographyTrackToPlayable(track)}
						{@const playable_ok = canPlayTrack(playable)}
						<!-- TIDAL-only top track. Renders inline (matches popular-list height)
						     with a TIDAL pill instead of library affordances. Click goes
						     through the existing playTidalTrackNow ephemeral path. -->
						<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
						<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
						<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
						<li
							class="tidal-popular-row"
							class:disabled={!playable_ok}
							role="button"
							tabindex={playable_ok ? 0 : -1}
							aria-disabled={!playable_ok}
							onclick={() => playable_ok && void playTidalTrackNow(playable)}
							onkeydown={(e) =>
								(e.key === 'Enter' || e.key === ' ')
								&& (e.preventDefault(), playable_ok && void playTidalTrackNow(playable))}
						>
							<span class="tidal-row-num">{filteredLibraryPopular.length + idx + 1}</span>
							{#if track.artwork_url}
								<img class="tidal-row-art" src={track.artwork_url} alt="" />
							{:else}
								<span class="tidal-row-art tidal-row-art-fallback">♫</span>
							{/if}
							<span class="tidal-row-meta">
								<span class="tidal-row-title">{track.title}</span>
								{#if track.album_title}
									<span class="tidal-row-album">{track.album_title}</span>
								{/if}
							</span>
							<span class="tidal-pill" aria-label="From TIDAL">TIDAL</span>
						</li>
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

		{#snippet videoCard(video: TidalArtistVideo)}
			<a
				class="grid-card video-card-rail"
				href={`/videos?videoId=${video.tidal_id}`}
			>
				<div class="grid-art-wrap video-art-wrap">
					{#if video.artwork_url}
						<img class="grid-art" src={video.artwork_url} alt="" />
					{:else}
						<div class="grid-art placeholder">▶</div>
					{/if}
					<div class="art-play-overlay video-play-overlay" aria-hidden="true">▶</div>
					<span class="badge-new">VIDEO</span>
				</div>
				<p class="grid-title">{video.title}</p>
				<p class="grid-sub">
					{#if video.duration_ms}{Math.round(video.duration_ms / 1000 / 60)}:{String(Math.round((video.duration_ms / 1000) % 60)).padStart(2, '0')}{/if}
				</p>
			</a>
		{/snippet}

		{#snippet similarArtistCard(similar: TidalSimilarArtist)}
			<a
				class="similar-card"
				href={similar.local_id != null
					? `/artists/${similar.local_id}`
					: `/tidal/artists/${similar.tidal_id}`}
			>
				<div class="similar-portrait-wrap">
					{#if similar.artwork_url}
						<img class="similar-portrait" src={similar.artwork_url} alt="" />
					{:else}
						<div
							class="similar-portrait similar-portrait-letter"
							style="background: {letterColor(similar.name)};"
						>
							<span class="similar-portrait-initials display-face">
								{artistInitials(similar.name)}
							</span>
						</div>
					{/if}
				</div>
				<p class="similar-name">{similar.name}</p>
				<p class="similar-sub">
					{#if similar.in_library}In library{:else}Artist{/if}
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
					<MediaRail
						items={filteredTidalFullAlbums}
						getKey={(a) => a.tidal_id}
					>
						{#snippet card(album)}
							{@render discographyCard(album, 'album')}
						{/snippet}
					</MediaRail>
				</section>
			{/if}

			{#if filteredTidalSinglesEPs.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Singles and EPs</h2>
						<span class="shelf-count">{filteredTidalSinglesEPs.length}</span>
					</div>
					<MediaRail
						items={filteredTidalSinglesEPs}
						getKey={(a) => a.tidal_id}
					>
						{#snippet card(album)}
							{@render discographyCard(album, 'ep_single')}
						{/snippet}
					</MediaRail>
				</section>
			{/if}

			{#if tidalVideos.length > 0}
				<section class="section">
					<div class="shelf-head">
						<p class="section-eyebrow">TIDAL · Videos</p>
						<span class="shelf-count">{tidalVideos.length}</span>
					</div>
					<MediaRail items={tidalVideos} getKey={(v) => v.tidal_id}>
						{#snippet card(video)}
							{@render videoCard(video)}
						{/snippet}
					</MediaRail>
				</section>
			{/if}

			{#if filteredTidalCompilations.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Compilations</h2>
						<span class="shelf-count">{filteredTidalCompilations.length}</span>
					</div>
					<MediaRail
						items={filteredTidalCompilations}
						getKey={(a) => a.tidal_id}
					>
						{#snippet card(album)}
							{@render discographyCard(album, 'compilation')}
						{/snippet}
					</MediaRail>
				</section>
			{/if}

			{#if filteredTidalLiveAlbums.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Live</h2>
						<span class="shelf-count">{filteredTidalLiveAlbums.length}</span>
					</div>
					<MediaRail
						items={filteredTidalLiveAlbums}
						getKey={(a) => a.tidal_id}
					>
						{#snippet card(album)}
							{@render discographyCard(album, 'live')}
						{/snippet}
					</MediaRail>
				</section>
			{/if}

			{#if tidalSimilarArtists.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Fans also like</h2>
						<span class="shelf-count">{tidalSimilarArtists.length}</span>
					</div>
					<MediaRail items={tidalSimilarArtists} getKey={(a) => a.tidal_id}>
						{#snippet card(similar)}
							{@render similarArtistCard(similar)}
						{/snippet}
					</MediaRail>
				</section>
			{/if}
		{:else}
			{#if fallbackFullAlbums.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Albums</h2>
						<span class="shelf-count">{fallbackFullAlbums.length}</span>
					</div>
					<MediaRail
						items={fallbackFullAlbums}
						getKey={(a) => a.id ?? a.title}
					>
						{#snippet card(album)}
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
						{/snippet}
					</MediaRail>
				</section>
			{/if}

			{#if fallbackSinglesEPs.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Singles and EPs</h2>
						<span class="shelf-count">{fallbackSinglesEPs.length}</span>
					</div>
					<MediaRail
						items={fallbackSinglesEPs}
						getKey={(a) => a.id ?? a.title}
					>
						{#snippet card(album)}
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
						{/snippet}
					</MediaRail>
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
		flex-direction: row;
		align-items: flex-end;
		gap: 28px;
		width: 100%;
		max-width: var(--content-width);
	}

	.hero-portrait-wrap {
		flex-shrink: 0;
	}

	.hero-portrait {
		width: 200px;
		height: 200px;
		border-radius: 50%;
		object-fit: cover;
		display: block;
		box-shadow: 0 18px 40px -16px rgba(0, 0, 0, 0.7);
		background: rgba(255, 255, 255, 0.04);
	}

	/* Quiet Mode-aligned glassmorphism: blurred album art behind a frosted disc.
	   Used when the artist row has no photo but at least one album cover is
	   available — keeps the hero from collapsing to plain backdrop+text. */
	.hero-portrait-glass {
		position: relative;
		overflow: hidden;
		display: flex;
		align-items: center;
		justify-content: center;
		isolation: isolate;
		background: rgba(255, 255, 255, 0.04);
		backdrop-filter: blur(10px);
		-webkit-backdrop-filter: blur(10px);
	}
	.hero-portrait-glass-art {
		position: absolute;
		inset: -8%;
		background-size: cover;
		background-position: center;
		filter: blur(18px) saturate(1.4);
		opacity: 0.55;
		z-index: -1;
	}
	.hero-portrait-letter {
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.hero-portrait-initials {
		font-family: var(--font-display);
		font-size: 4.5rem;
		font-weight: 700;
		color: rgba(255, 255, 255, 0.9);
		letter-spacing: -0.02em;
		text-shadow: 0 2px 8px rgba(0, 0, 0, 0.4);
		line-height: 1;
	}

	.hero-info {
		display: flex;
		flex-direction: column;
		gap: 10px;
		flex: 1;
		min-width: 0;
	}

	@media (max-width: 720px) {
		.hero-body {
			flex-direction: column;
			align-items: flex-start;
			gap: 16px;
		}
		.hero-portrait {
			width: 140px;
			height: 140px;
		}
		.hero-portrait-initials {
			font-size: 3rem;
		}
	}

	.hero-library-substat {
		margin: 0;
		font-size: 0.78rem;
		color: var(--text-tertiary);
	}

	.hero-bio {
		margin: 6px 0 0;
		font-size: 0.86rem;
		color: var(--text-secondary);
		line-height: 1.55;
		max-width: 800px;
	}
	.bio-toggle {
		all: unset;
		color: var(--accent-strong);
		cursor: pointer;
		font-weight: 600;
		margin-left: 4px;
	}
	.bio-toggle:hover {
		text-decoration: underline;
	}
	.hero-bio-source {
		margin: 6px 0 0;
		font-size: 0.7rem;
		color: var(--text-tertiary);
		letter-spacing: 0.04em;
		text-transform: uppercase;
	}

	/* Eyebrow above the Videos rail (small uppercase label, sits where the
	   h2 normally would, paired with the existing shelf-count). */
	.section-eyebrow {
		margin: 0;
		font-size: 0.78rem;
		font-weight: 600;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--text-secondary);
	}

	/* Video rail card — reuses .grid-card sizing but the art slot is a
	   wider 16:9 to match how videos render. */
	.video-card-rail {
		flex: 0 0 240px;
		min-width: 240px;
		max-width: 240px;
	}
	.video-art-wrap {
		aspect-ratio: 16 / 9;
	}
	.video-play-overlay {
		opacity: 0;
		transition: opacity 0.18s ease;
	}
	.video-card-rail:hover .video-play-overlay {
		opacity: 1;
	}

	/* Similar Artists rail — round portrait, name below. Mirrors the hero
	   portrait at smaller scale. */
	.similar-card {
		flex: 0 0 140px;
		min-width: 140px;
		max-width: 140px;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		padding: 12px 8px;
		text-decoration: none;
		color: inherit;
		border-radius: 12px;
		transition: background 140ms ease;
	}
	.similar-card:hover {
		background: rgba(255, 255, 255, 0.04);
	}
	.similar-portrait-wrap {
		width: 110px;
		height: 110px;
	}
	.similar-portrait {
		width: 110px;
		height: 110px;
		border-radius: 50%;
		object-fit: cover;
		display: block;
		background: rgba(255, 255, 255, 0.04);
	}
	.similar-portrait-letter {
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.similar-portrait-initials {
		font-family: var(--font-display);
		font-size: 2.2rem;
		font-weight: 700;
		color: rgba(255, 255, 255, 0.9);
		letter-spacing: -0.02em;
	}
	.similar-name {
		margin: 0;
		font-size: 0.86rem;
		font-weight: 600;
		color: var(--text-primary);
		text-align: center;
		max-width: 100%;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.similar-sub {
		margin: 0;
		font-size: 0.72rem;
		color: var(--text-tertiary);
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

	/* Discography rail cards retain their fixed width so the row stays
	   uniform regardless of title length; the rail container (MediaRail)
	   handles the horizontal scroll. */
	.grid-card {
		flex: 0 0 200px;
		min-width: 200px;
		max-width: 200px;
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
		font-size: var(--font-size-xl);
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

	/* TIDAL-only top track row — same height as TrackRow's numbered variant
	   so the merged Top tracks list scans as one continuous list. */
	.tidal-popular-row {
		display: grid;
		grid-template-columns: 32px 40px 1fr auto;
		align-items: center;
		gap: 12px;
		padding: 6px 12px;
		border-radius: var(--radius-sm, 8px);
		cursor: pointer;
		transition: background 120ms ease;
		min-height: 52px;
	}
	.tidal-popular-row:hover { background: rgba(255, 255, 255, 0.04); }
	.tidal-popular-row.disabled { cursor: not-allowed; opacity: 0.55; }
	.tidal-row-num {
		text-align: center;
		color: var(--text-tertiary);
		font-size: 0.85rem;
		font-variant-numeric: tabular-nums;
	}
	.tidal-row-art {
		width: 40px;
		height: 40px;
		border-radius: 4px;
		object-fit: cover;
		display: block;
	}
	.tidal-row-art-fallback {
		display: flex;
		align-items: center;
		justify-content: center;
		background: rgba(255, 255, 255, 0.06);
		color: rgba(255, 255, 255, 0.45);
		font-size: var(--font-size-lg);
	}
	.tidal-row-meta {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.tidal-row-title {
		font-size: 0.92rem;
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.tidal-row-album {
		font-size: 0.78rem;
		color: var(--text-tertiary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.tidal-pill {
		font-size: 0.62rem;
		font-weight: 700;
		letter-spacing: 0.06em;
		padding: 3px 8px;
		border-radius: 4px;
		background: rgba(0, 184, 212, 0.16);
		color: rgba(120, 220, 240, 0.95);
		border: 1px solid rgba(0, 184, 212, 0.3);
		text-transform: uppercase;
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
