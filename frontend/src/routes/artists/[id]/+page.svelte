<script lang="ts">
	import { page } from '$app/state';
	import type { Snapshot } from './$types';
	import { type Track, type TidalDiscographyAlbum, type TidalDiscographyTrack, type TidalArtistVideo, type TidalSimilarArtist, type TidalArtistBio, type SpotifyArtistStats } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { letterColor } from '$lib/utils/color';
	import {
		playArtist,
		shuffleArtist,
		startArtistRadio,
		playTidalAlbum,
		playTidalTrackNow,
		playTidalTracksNow,
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
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildTidalTrackMenu } from '$lib/player/track_menu';
	import { canPlayTrack } from '$lib/player/playable';
	import {
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize,
	} from '$lib/utils/artwork';
	import { formatCompactCount } from '$lib/utils/format';
	import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';
	import { cleanArtistBio } from '../artist_bio';
	import { artistCurrentTrackMatchesArtist } from '../artist_playback';

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
	let loadSeq = 0;

	// View-time fallback when our local artist row has no `photo_url`.
	// Populated from `tidal_artist_profile.picture_url`. Cleared on artist
	// change so we don't flash the previous artist's photo on transition.
	let tidalPictureUrl = $state<string | null>(null);

	let tidalAlbums = $state<TidalDiscographyAlbum[]>([]);
	let tidalTopTracks = $state<TidalDiscographyTrack[]>([]);
	let tidalVideos = $state<TidalArtistVideo[]>([]);
	let tidalSimilarArtists = $state<TidalSimilarArtist[]>([]);
	let tidalBio = $state<TidalArtistBio | null>(null);
	let tidalLoading = $state(false);
	let tidalAvailable = $state(false);
	let failedArtworkUrls = $state<Record<string, boolean>>({});
	let tidalLoadSeq = 0;

	let spotifyStats = $state<SpotifyArtistStats | null>(null);
	let spotifyLoadSeq = 0;
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
			// Source-of-truth artist row (name, photo, biography, counts) is
			// fetched in parallel with the artist's local tracks. Either failure
			// is non-fatal; the page can render with whichever resolved.
			const [artistRes, tracksRes] = await Promise.allSettled([
				cachedApi.getArtist(id),
				cachedApi.getArtistTracks(id),
			]);
			if (seq !== loadSeq) return;
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
			if (seq === loadSeq) loading = false;
		}
	}

	async function loadDiscography(id: number) {
		const seq = ++tidalLoadSeq;
		tidalLoading = true;
		try {
			const res = await cachedApi.getArtistDiscography(id);
			if (seq !== tidalLoadSeq) return;
			tidalAlbums = res.albums;
			tidalTopTracks = res.top_tracks ?? [];
			tidalVideos = res.videos ?? [];
			tidalSimilarArtists = res.similar_artists ?? [];
			tidalBio = res.bio ?? null;
			tidalAvailable = res.available;
			// View-time portrait fallback, populated alongside the rest
			// of the discography so a missing local `photo_url` still
			// renders a proper hero portrait instead of the initials disc.
			if (res.picture_url) tidalPictureUrl = res.picture_url;
		} catch (err) {
			if (seq !== tidalLoadSeq) return;
			console.error('Failed to load TIDAL discography', err);
		} finally {
			if (seq === tidalLoadSeq) tidalLoading = false;
		}
	}

	async function loadSpotifyStats(id: number) {
		const seq = ++spotifyLoadSeq;
		try {
			const stats = await cachedApi.getArtistSpotifyStats(id);
			if (seq === spotifyLoadSeq) spotifyStats = stats;
		} catch (err) {
			if (seq !== spotifyLoadSeq) return;
			console.error('Failed to load Spotify stats', err);
			spotifyStats = null;
		}
	}

	$effect(() => {
		const id = artistId;
		artist = null;
		tracks = [];
		tidalPictureUrl = null;
		tidalAlbums = [];
		tidalTopTracks = [];
		tidalVideos = [];
		tidalSimilarArtists = [];
		tidalBio = null;
		tidalAvailable = false;
		spotifyStats = null;
		failedArtworkUrls = {};
		bioExpanded = false;
		void load(id);
		void loadDiscography(id);
		void loadSpotifyStats(id);
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
	//   2. The first available album cover, used as a glassmorphic backdrop
	//      with a frosted disc on top, matching the Quiet Mode aesthetic.
	//   3. Letter-color fallback inside the disc (handled in the markup).
	// Prefer the fresh TIDAL `picture_url` over the locally-cached
	// `artist.photo_url`. Some legacy sync runs stored 640x640 URLs which
	// TIDAL's CDN now returns AccessDenied for on certain artists (the
	// picture only exists at smaller sizes for those records). The Tidal
	// fetch in `loadDiscography` always builds at sizes we know work.
	let heroPortraitUrl = $derived(tidalPictureUrl ?? artist?.photo_url ?? null);
	let heroBackdropUrl = $derived(
		tidalPictureUrl
			?? artist?.photo_url
			?? tidalAlbums.find((a) => a.artwork_url)?.artwork_url
			?? tracks.find((t) => t.artwork_url)?.artwork_url
			?? null
	);
	let heroPortraitSrc = $derived(artworkCandidate(heroPortraitUrl, 640));
	let heroBackdropSrc = $derived(artworkCandidate(heroBackdropUrl, 1280));
	let heroHasPhoto = $derived(heroPortraitSrc != null);

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

	// Artist biographies can arrive with TIDAL link and HTML markup.
	// The helper keeps only readable text.
	let bioText = $derived(
		cleanArtistBio(tidalBio?.text)
			?? cleanArtistBio(tidalBio?.summary)
			?? cleanArtistBio(artist?.biography)
			?? null
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

	type PopularTrackItem =
		| { kind: 'local'; track: Track }
		| { kind: 'tidal'; track: TidalDiscographyTrack };

	function localPopularityScore(track: Track): number {
		const spotifyPlaycount = track.isrc ? playcountByIsrc.get(track.isrc) : undefined;
		return spotifyPlaycount ?? track.play_count ?? 0;
	}

	function popularItemKey(item: PopularTrackItem): string {
		return item.kind === 'local' ? `local-${item.track.id}` : `tidal-${item.track.tidal_id}`;
	}

	// "Top tracks" follows TIDAL's popularity-ranked top-tracks order when
	// available, replacing TIDAL rows with local rows where the user owns them.
	// Local-only leftovers are appended by known playcount as a fallback.
	let popularItems = $derived.by<PopularTrackItem[]>(() => {
		const byTidalId = new Map<number, Track>();
		for (const track of tracks) {
			if (track.tidal_id != null && track.tidal_id > 0) byTidalId.set(track.tidal_id, track);
		}

		const seenLocalIds = new Set<number>();
		const seenTidalIds = new Set<number>();
		const ordered: PopularTrackItem[] = [];

		for (const tidalTrack of tidalTopTracks) {
			if (seenTidalIds.has(tidalTrack.tidal_id)) continue;
			seenTidalIds.add(tidalTrack.tidal_id);
			const localTrack = byTidalId.get(tidalTrack.tidal_id);
			if (localTrack) {
				seenLocalIds.add(localTrack.id);
				ordered.push({ kind: 'local', track: localTrack });
			} else {
				ordered.push({ kind: 'tidal', track: tidalTrack });
			}
		}

		const localRemainder = tracks
			.filter((track) => !seenLocalIds.has(track.id))
			.sort((a, b) => localPopularityScore(b) - localPopularityScore(a));

		if (ordered.length === 0) {
			return localRemainder.map((track) => ({ kind: 'local', track }));
		}

		ordered.push(...localRemainder.map((track) => ({ kind: 'local' as const, track })));
		return ordered;
	});
	let popularMaxPlays = $derived(
		Math.max(1, ...popularItems.map((item) => item.kind === 'local' ? localPopularityScore(item.track) : 0))
	);
	let visiblePopularItems = $derived(popularItems.slice(0, 10));
	let totalPopularCandidates = $derived(popularItems.length);

	function artistTrackPlayable(track: TidalDiscographyTrack) {
		return tidalDiscographyTrackToPlayable(track, { artistTidalId: artist?.tidal_id ?? null });
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
		return map;
	});

	function releaseYear(d: string | null): number | null {
		if (!d) return null;
		const y = parseInt(d.slice(0, 4), 10);
		return Number.isFinite(y) ? y : null;
	}

	type DiscoCategory = 'album' | 'ep_single' | 'compilation' | 'live';
	function categorize(a: TidalDiscographyAlbum): DiscoCategory {
		// The TIDAL editorial filter is more authoritative than the per-album
		// release_type body field; a compilation tagged release_type:"ALBUM"
		// used to land in the Albums shelf and the Compilations shelf stayed
		// empty even though the data was fetched.
		switch (a.source_filter) {
			case 'COMPILATIONS':
				return 'compilation';
			case 'LIVE':
				return 'live';
			case 'EPSANDSINGLES':
				return 'ep_single';
			case 'ALBUMS':
				return 'album';
		}
		const type = (a.release_type ?? '').toUpperCase();
		if (type === 'COMPILATION') return 'compilation';
		if (type === 'LIVE') return 'live';
		if (type === 'SINGLE' || type === 'EP') return 'ep_single';
		if (type === 'ALBUM') return 'album';
		return (a.number_of_tracks ?? 0) >= 3 ? 'album' : 'ep_single';
	}

	function sortByDate(list: TidalDiscographyAlbum[]): TidalDiscographyAlbum[] {
		// Compare full ISO date strings (YYYY-MM-DD sorts lexicographically)
		// not just the year. A Dec 2024 release should sit above a Jan 2024
		// one. Missing dates sort to the bottom.
		return [...list].sort((a, b) => {
			const ad = a.release_date ?? '';
			const bd = b.release_date ?? '';
			if (ad === bd) return 0;
			if (!ad) return 1;
			if (!bd) return -1;
			return bd.localeCompare(ad);
		});
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
				if (!existing.artwork_url && t.artwork_url) existing.artwork_url = t.artwork_url;
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

	let heroPlayPending = $state(false);
	async function ensureTidalTopTracksForPlayback(id: number): Promise<TidalDiscographyTrack[]> {
		if (tidalTopTracks.length > 0) return tidalTopTracks;
		const res = await cachedApi.getArtistDiscography(id);
		if (artistId === id) {
			tidalAlbums = res.albums;
			tidalTopTracks = res.top_tracks ?? [];
			tidalVideos = res.videos ?? [];
			tidalSimilarArtists = res.similar_artists ?? [];
			tidalBio = res.bio ?? null;
			tidalAvailable = res.available;
			if (res.picture_url) tidalPictureUrl = res.picture_url;
		}
		return res.top_tracks ?? [];
	}

	async function onHeroPlay() {
		if (heroPlayPending) return;
		const current = $currentTrack;
		if (artistCurrentTrackMatchesArtist(current, tracks, artist?.tidal_id, tidalTopTracks)) {
			void togglePlayback();
			return;
		}
		if (tracks.length > 0) {
			void playArtist(artistId);
			return;
		}
		heroPlayPending = true;
		try {
			const requestedFor = artistId;
			const topTracks = await ensureTidalTopTracksForPlayback(requestedFor);
			if (artistId !== requestedFor) return;
			const playable = topTracks.map(artistTrackPlayable);
			if (playable.length > 0) {
				await playTidalTracksNow(playable, artist?.name ?? 'artist');
			} else {
				await playArtist(artistId);
			}
		} catch (error) {
			console.error('Failed to load TIDAL artist tracks for playback', error);
			await playArtist(artistId);
		} finally {
			heroPlayPending = false;
		}
	}

	let isArtistPlaying = $derived(
		$isPlaying && artistCurrentTrackMatchesArtist($currentTrack, tracks, artist?.tidal_id, tidalTopTracks)
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

	const filteredPopularItems = $derived(
		visiblePopularItems.filter((item) => matchesFilter(item.track.title))
	);
	const hasAnyPopular = $derived(
		filteredPopularItems.length > 0
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

	function similarArtistMenu(similar: TidalSimilarArtist) {
		return buildArtistMenu(
			{
				local_id: similar.local_id,
				tidal_id: similar.tidal_id,
				name: similar.name,
				in_library: similar.in_library,
			},
			{ isLocal: similar.in_library && similar.local_id != null }
		);
	}

	function fallbackAlbumMenu(album: { id: number | null; title: string; tracks: Track[] }) {
		const firstTrack = album.tracks[0];
		return buildAlbumMenu(
			{
				id: album.id,
				title: album.title,
				artist_id: artistId,
				artist_name: firstTrack?.artist_name ?? header()?.name ?? null,
				in_library: album.id != null,
			},
			{ isLocal: album.id != null }
		);
	}
</script>

<div class="artist-page">
	<a class="back-link" href="/library">← Back to library</a>
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
				<div class="hero-portrait-wrap">
					{#if heroPortraitSrc}
						<img
							class="hero-portrait"
							src={heroPortraitSrc}
							alt=""
							onerror={() => markArtworkFailed(heroPortraitSrc)}
						/>
					{:else if heroBackdropSrc}
						<!-- Glassmorphism fallback: blurred album art behind a frosted disc.
						     Mirrors the Quiet Mode aesthetic so artists missing a TIDAL/Spotify
						     photo still feel of-a-piece with the rest of the app. -->
						<div class="hero-portrait hero-portrait-glass">
							<img
								class="hero-portrait-glass-art"
								src={heroBackdropSrc}
								alt=""
								onerror={() => markArtworkFailed(heroBackdropSrc)}
							/>
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
							<span class="hero-listeners">{formatCompactCount(spotifyStats.monthly_listeners)} monthly listeners</span>
						{/if}
					</p>
					{#if spotifyStats && (spotifyStats.followers != null || spotifyStats.world_rank != null)}
						<p class="hero-pills">
							{#if spotifyStats.followers != null}
								<span class="hero-pill" title="Spotify followers">
									{formatCompactCount(spotifyStats.followers)} followers
								</span>
							{/if}
							{#if spotifyStats.world_rank != null}
								<span class="hero-pill" title="Spotify world rank">
									#{spotifyStats.world_rank.toLocaleString()} worldwide
								</span>
							{/if}
						</p>
					{/if}
					{#if spotifyStats?.top_cities && spotifyStats.top_cities.length > 0}
						<ul class="hero-top-cities">
							{#each spotifyStats.top_cities.slice(0, 3) as city (city.city + city.country)}
								<li class="hero-top-city">
									<span class="city-name">{city.city}{city.country ? `, ${city.country}` : ''}</span>
									<span class="city-listeners">{formatCompactCount(city.listeners)}</span>
								</li>
							{/each}
						</ul>
					{/if}
					{#if h.library_track_count > 0}
						<p class="hero-library-substat">
							{h.library_track_count.toLocaleString()} {h.library_track_count === 1 ? 'song' : 'songs'} in your library
						</p>
					{/if}
					{#if bioRendered}
						<div class="hero-bio-panel" class:expanded={bioExpanded}>
							<p class="hero-bio">
								{bioRendered}
							</p>
							{#if bioIsLong}
								<button
									type="button"
									class="bio-toggle"
									onclick={() => (bioExpanded = !bioExpanded)}
								>{bioExpanded ? 'Show less' : 'Show more'}</button>
							{/if}
						</div>
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
				class:pending={heroPlayPending}
				aria-label={isArtistPlaying ? 'Pause' : 'Play'}
				disabled={heroPlayPending}
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
					{#each filteredPopularItems as item, idx (popularItemKey(item))}
						{#if item.kind === 'local'}
							{@const track = item.track}
						{@const streamCount = track.isrc ? playcountByIsrc.get(track.isrc) : undefined}
						<div class="popular-row-wrap">
							<div
								class="pop-bar"
								style="width: {Math.max(4, (localPopularityScore(track) / popularMaxPlays) * 100)}%"
							></div>
							<TrackRow
								{track}
								variant="numbered"
								index={idx}
								isCurrent={$currentTrack?.id === track.id}
								isPlaying={$isPlaying}
								showArtist={false}
								showPlayCount={true}
								worldPlayCount={streamCount ?? null}
								onRowClick={() => void playArtist(artistId, track.id)}
								menuOptions={{ hideArtistActions: true }}
							/>
						</div>
						{:else}
							{@const track = item.track}
						{@const playable = artistTrackPlayable(track)}
						{@const playable_ok = canPlayTrack(playable)}
						{@const trackArt = artworkCandidate(track.artwork_url, 320)}
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
							oncontextmenu={(e) => {
								e.preventDefault();
								e.stopPropagation();
								openContextMenu(e, buildTidalTrackMenu(playable), track.title);
							}}
							onkeydown={(e) =>
								(e.key === 'Enter' || e.key === ' ')
								&& (e.preventDefault(), playable_ok && void playTidalTrackNow(playable))}
						>
							<span class="tidal-row-num">{idx + 1}</span>
							{#if trackArt}
								<img
									class="tidal-row-art"
									src={trackArt}
									alt=""
									onerror={() => markArtworkFailed(trackArt)}
								/>
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
						{/if}
					{/each}
				</ol>
				{#if totalPopularCandidates > 10}
					<a class="show-all-btn" href={`/artists/${artistId}/discography/tracks`}>
						See all {totalPopularCandidates}
					</a>
				{/if}
			</section>
		{/if}

		{#snippet discographyCard(album: TidalDiscographyAlbum, kind: DiscoCategory)}
			{@const year = releaseYear(album.release_date)}
			{@const kindLabel = kind === 'album' ? 'Album'
				: kind === 'compilation' ? 'Compilation'
				: kind === 'live' ? 'Live'
				: (album.release_type ?? '').toUpperCase() === 'EP' ? 'EP' : 'Single'}
			{@const albumArt = artworkCandidate(album.artwork_url, 320)}
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
					{#if albumArt}
						<img
							class="grid-art"
							src={albumArt}
							alt=""
							onerror={() => markArtworkFailed(albumArt)}
						/>
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
			{@const videoArt = artworkCandidate(video.artwork_url, 320)}
			<a
				class="grid-card video-card-rail"
				href={`/videos?videoId=${video.tidal_id}`}
			>
				<div class="grid-art-wrap video-art-wrap">
					{#if videoArt}
						<img
							class="grid-art"
							src={videoArt}
							alt=""
							onerror={() => markArtworkFailed(videoArt)}
						/>
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
			{@const similarArt = artworkCandidate(similar.artwork_url, 320)}
			<a
				class="similar-card"
				href={similar.local_id != null
					? `/artists/${similar.local_id}`
					: `/tidal/artists/${similar.tidal_id}`}
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, similarArtistMenu(similar), similar.name);
				}}
			>
				<div class="similar-portrait-wrap">
					{#if similarArt}
						<img
							class="similar-portrait"
							src={similarArt}
							alt=""
							onerror={() => markArtworkFailed(similarArt)}
						/>
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
						<a class="shelf-link" href={`/artists/${artistId}/discography/albums`}>See all</a>
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
						<a class="shelf-link" href={`/artists/${artistId}/discography/singles`}>See all</a>
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
						<a class="shelf-link" href={`/artists/${artistId}/discography/compilations`}>See all</a>
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
							{@const albumArt = artworkCandidate(album.artwork_url, 320)}
							<a
								class="grid-card"
								href={album.id != null ? `/albums/${album.id}` : undefined}
								oncontextmenu={(e) => {
									e.preventDefault();
									e.stopPropagation();
									openContextMenu(e, fallbackAlbumMenu(album), album.title);
								}}
							>
								<div class="grid-art-wrap">
									{#if albumArt}
										<img
											class="grid-art"
											src={albumArt}
											alt=""
											onerror={() => markArtworkFailed(albumArt)}
										/>
									{:else}
										<div class="grid-art placeholder">♫</div>
									{/if}
									{#if album.id != null}
										<PlayOverlay
											position="center"
											size="md"
											label="Play {album.title}"
											onclick={(e) => onAlbumCardPlay({ id: album.id }, e)}
										/>
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
							{@const albumArt = artworkCandidate(album.artwork_url, 320)}
							<a
								class="grid-card"
								href={album.id != null ? `/albums/${album.id}` : undefined}
								oncontextmenu={(e) => {
									e.preventDefault();
									e.stopPropagation();
									openContextMenu(e, fallbackAlbumMenu(album), album.title);
								}}
							>
								<div class="grid-art-wrap">
									{#if albumArt}
										<img
											class="grid-art"
											src={albumArt}
											alt=""
											onerror={() => markArtworkFailed(albumArt)}
										/>
									{:else}
										<div class="grid-art placeholder">♫</div>
									{/if}
									{#if album.id != null}
										<PlayOverlay
											position="center"
											size="md"
											label="Play {album.title}"
											onclick={(e) => onAlbumCardPlay({ id: album.id }, e)}
										/>
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

		<!-- Similar artists are not gated on tidalAvailable: a transient TIDAL
		     fetch hiccup on /artists used to drop this section silently. -->
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
	{/if}
</div>

<style>
	.artist-page {
		padding: 0 0 80px;
		display: flex;
		flex-direction: column;
	}

	.artist-page > .back-link {
		align-self: flex-start;
		margin-bottom: var(--space-3);
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
		padding: var(--space-6) var(--space-5) var(--space-4);
		display: flex;
		min-height: 300px;
		overflow: hidden;
		isolation: isolate;
		align-items: flex-start;
		border-radius: var(--radius-lg);
		border: 1px solid var(--border-subtle);
	}

	.hero-backdrop {
		position: absolute;
		inset: -80px;
		width: calc(100% + 160px);
		height: calc(100% + 160px);
		object-fit: cover;
		object-position: center;
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
		align-items: flex-start;
		gap: var(--space-5);
		width: 100%;
		max-width: var(--content-width);
	}

	.hero-portrait-wrap {
		flex-shrink: 0;
		align-self: flex-start;
	}

	.hero-portrait {
		width: clamp(140px, 16vw, 220px);
		aspect-ratio: 1 / 1;
		border-radius: 50%;
		object-fit: cover;
		display: block;
		box-shadow: 0 18px 40px -16px rgba(0, 0, 0, 0.7);
		background: var(--bg-surface);
	}

	/* Quiet Mode-aligned glassmorphism: blurred album art behind a frosted disc.
	   Used when the artist row has no photo but at least one album cover is
	   available, keeping the hero from collapsing to plain backdrop+text. */
	.hero-portrait-glass {
		position: relative;
		overflow: hidden;
		display: flex;
		align-items: center;
		justify-content: center;
		isolation: isolate;
		background: rgba(255, 255, 255, 0.04);
		backdrop-filter: var(--blur-overlay);
		-webkit-backdrop-filter: var(--blur-overlay);
	}
	.hero-portrait-glass-art {
		position: absolute;
		inset: -8%;
		width: 116%;
		height: 116%;
		object-fit: cover;
		object-position: center;
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
		font-size: var(--font-size-4xl);
		font-weight: var(--font-weight-bold);
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
			font-size: var(--font-size-4xl);
		}
	}

	.hero-library-substat {
		margin: 0;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
	}

	.hero-bio-panel {
		margin: 6px 0 0;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 6px;
		max-width: 800px;
	}

	.hero-bio {
		margin: 0;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		line-height: var(--line-height-loose);
		white-space: pre-line;
	}

	.hero-bio-panel.expanded .hero-bio {
		max-height: clamp(10rem, 30vh, 18rem);
		overflow-y: auto;
		padding-right: var(--space-1);
	}
	.bio-toggle {
		all: unset;
		color: var(--accent-strong);
		cursor: pointer;
		font-weight: var(--font-weight-semibold);
	}
	.bio-toggle:hover {
		text-decoration: underline;
	}
	.hero-bio-source {
		margin: 6px 0 0;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		letter-spacing: 0.04em;
		text-transform: uppercase;
	}

	/* Eyebrow above the Videos rail (small uppercase label, sits where the
	   h2 normally would, paired with the existing shelf-count). */
	.section-eyebrow {
		margin: 0;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--text-secondary);
	}

	/* Video rail card, reusing .grid-card sizing while the art slot is a
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

	/* Similar Artists rail, with round portrait and name below. Mirrors the hero
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
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-bold);
		color: rgba(255, 255, 255, 0.9);
		letter-spacing: -0.02em;
	}
	.similar-name {
		margin: 0;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		text-align: center;
		max-width: 100%;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.similar-sub {
		margin: 0;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
	}

	.eyebrow {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: var(--font-size-xs);
		color: var(--text-primary);
		margin: 0;
		font-weight: var(--font-weight-semibold);
	}

	.hero-title {
		font-family: var(--font-display);
		font-size: var(--font-size-4xl);
		line-height: var(--line-height-tight);
		letter-spacing: -0.03em;
		margin: 0;
		color: var(--text-primary);
		word-wrap: break-word;
	}

	.hero-sub {
		color: var(--text-secondary);
		margin: 4px 0 0;
		font-size: var(--font-size-sm);
	}

	.filter-bar {
		padding: 8px 32px 0;
	}

	.filter-input {
		background: var(--input-bg);
		border: 1px solid var(--input-border);
		border-radius: 20px;
		padding: 7px 16px;
		font-size: var(--font-size-sm);
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
		font-size: var(--font-size-xs);
		line-height: var(--line-height-normal);
	}

	.actions-microcopy strong {
		color: var(--text-secondary);
		font-weight: var(--font-weight-semibold);
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
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
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
		font-size: var(--font-size-sm);
	}

	.shelf-link {
		margin-left: auto;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		text-decoration: none;
	}

	.shelf-link:hover {
		color: var(--text-primary);
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

	.grid-art-wrap:hover :global(.play-overlay),
	.grid-card:focus-within :global(.play-overlay) {
		opacity: 1;
		transform: translateY(0);
	}

	.badge-new {
		position: absolute;
		top: 8px;
		right: 8px;
		padding: 3px 8px;
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.55);
		color: #fff;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.12em;
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
	}

	.grid-card.not-in-library .grid-title {
		color: var(--text-secondary);
	}

	.status.subtle {
		color: var(--text-tertiary);
		padding: 20px 32px;
		font-size: var(--font-size-sm);
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
		font-size: var(--font-size-2xl);
		color: var(--text-tertiary);
	}

	.grid-title {
		margin: 6px 0 0;
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.grid-sub {
		margin: 0;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	@media (max-width: 720px) {
		.hero { padding: 36px 20px 24px; min-height: 240px; }
		.hero-title { font-size: var(--font-size-3xl); }
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

	/* TIDAL-only top track row, same height as TrackRow's numbered variant
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
		font-size: var(--font-size-sm);
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
		font-size: var(--font-size-sm);
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.tidal-row-album {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.tidal-pill {
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

	.show-all-btn {
		margin: 12px auto 0;
		display: block;
		padding: 6px 16px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.09);
		color: var(--text-secondary, rgba(255, 255, 255, 0.7));
		font-size: var(--font-size-sm);
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

	.hero-pills {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin: 6px 0 0;
		padding: 0;
	}

	.hero-pill {
		display: inline-flex;
		align-items: center;
		padding: 2px 10px;
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.06);
		color: var(--text-secondary, rgba(255, 255, 255, 0.75));
		font-variant-numeric: tabular-nums;
	}

	.hero-top-cities {
		list-style: none;
		margin: 8px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
		max-width: 260px;
	}

	.hero-top-city {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		gap: 8px;
		font-size: var(--font-size-xs);
		color: var(--text-secondary, rgba(255, 255, 255, 0.7));
	}

	.hero-top-city .city-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.hero-top-city .city-listeners {
		font-variant-numeric: tabular-nums;
		opacity: 0.8;
	}

</style>
