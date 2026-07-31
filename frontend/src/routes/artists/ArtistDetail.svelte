<script lang="ts">
	import {
		type Track,
		type TidalDiscographyAlbum,
		type TidalDiscographyTrack,
		type TidalArtistVideo,
		type TidalSimilarArtist,
		type TidalArtistBio,
		type TidalPlayable
	} from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { letterColor } from '$lib/utils/color';
	import {
		playArtist,
		shuffleArtist,
		startArtistRadio,
		playTidalAlbum,
		playTidalTrackNow,
		playTidalTracksNow,
		shuffleTidalTracksNow,
		startTidalSongRadio,
		playAlbum,
		toggleTrackFavorite,
		toggleTidalTrackFavorite,
		currentTrack,
		isPlaying,
		togglePlayback
	} from '$lib/stores/player';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import { goBack } from '$lib/navigation/back';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildTidalTrackMenu } from '$lib/player/track_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { canPlayTrack } from '$lib/player/playable';
	import {
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize,
	} from '$lib/utils/artwork';
	import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';
	import { cleanArtistBio } from './artist_bio';
	import { artistCurrentTrackMatchesArtist } from './artist_playback';
	import {
		buildPopularTrackItems,
		categorizeTidalAlbum,
		popularTrackItemKey,
		sortTidalAlbumsByReleaseDate,
		type DiscoCategory,
		type PopularTrackItem,
	} from './artist_discography';

	// One artist view, two data sources. A library artist is keyed by local id
	// (rich local affordances: favorites, play counts, library albums). A
	// non-library artist found via search is keyed by its TIDAL id and
	// sourced entirely from the TIDAL profile endpoint. The local code path is
	// unchanged from when this lived in `/artists/[id]/+page.svelte`; the TIDAL
	// path is additive and guarded behind `source.kind === 'tidal'`.
	// Artist pages fetch from TIDAL and the local library ONLY - no Spotify
	// proxy calls in this flow (they ride flaky anonymous proxies and were
	// erroring in production logs while adding nothing the page needs).
	type ArtistSource =
		| { kind: 'local'; artistId: number }
		| { kind: 'tidal'; tidalArtistId: number };

	let { source }: { source: ArtistSource } = $props();

	type ArtistRow = {
		id: number;
		tidal_id: number | null;
		name: string;
		biography: string | null;
		photo_url: string | null;
		track_count: number;
		album_count: number;
	};

	// Local artist id when in library mode; 0 otherwise (every read of it is
	// already guarded by a `source.kind === 'local'` branch).
	let artistId = $derived(source.kind === 'local' ? source.artistId : 0);

	// Base path for the "See all" discography routes. Both the library and the
	// TIDAL artist route have their own discography section pages backed by the
	// same shared view, so the shelves link out in either mode.
	let discographyBase = $derived(
		source.kind === 'tidal' ? `/tidal/artists/${source.tidalArtistId}` : `/artists/${artistId}`
	);

	let artist = $state<ArtistRow | null>(null);
	let tracks = $state<Track[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let loadSeq = 0;

	// View-time fallback when our local artist row has no `photo_url`.
	// Populated from `tidal_artist_profile.picture_url`. Cleared on artist
	// change so we don't flash the previous artist's photo on transition.
	let tidalPictureUrl = $state<string | null>(null);

	// TIDAL-mode header name (no local artist row to read it from).
	let tidalProfileName = $state<string | null>(null);

	let tidalAlbums = $state<TidalDiscographyAlbum[]>([]);
	let tidalTopTracks = $state<TidalDiscographyTrack[]>([]);
	let tidalVideos = $state<TidalArtistVideo[]>([]);
	let tidalSimilarArtists = $state<TidalSimilarArtist[]>([]);
	let tidalBio = $state<TidalArtistBio | null>(null);
	let tidalLoading = $state(false);
	let tidalAvailable = $state(false);
	// TIDAL sub-fetches that failed or timed out server-side. Non-empty means
	// the shelves below are PARTIAL, and the page says so instead of passing
	// empty rails off as "this artist has no videos".
	let tidalSectionsFailed = $state<string[]>([]);
	let failedArtworkUrls = $state<Record<string, boolean>>({});
	let tidalLoadSeq = 0;

	// Active TIDAL artist id used for "is this artist currently playing" checks
	// and to stamp ephemeral TIDAL playables. In library mode it comes from the
	// local artist row; in TIDAL mode it's the route id itself.
	let activeTidalArtistId = $derived(
		source.kind === 'tidal' ? source.tidalArtistId : (artist?.tidal_id ?? null)
	);

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
			tidalSectionsFailed = res.sections_failed ?? [];
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

	// TIDAL mode: the profile endpoint returns the same rich shape as the
	// library discography route (categorized albums, top tracks, videos,
	// similar artists, bio, picture) keyed straight off the TIDAL id, so the
	// same markup below renders without a local artist row. Served through
	// cachedApi: in-flight dedupe plus stale-while-revalidate, so revisiting
	// an artist renders instantly instead of refetching the whole profile.
	async function loadTidalProfile(tidalId: number) {
		const seq = ++tidalLoadSeq;
		tidalLoading = true;
		loading = true;
		error = null;
		try {
			const res = await cachedApi.getTidalArtistProfile(tidalId);
			if (seq !== tidalLoadSeq) return;
			tidalProfileName = res.artist_name ?? null;
			tidalAlbums = res.albums ?? [];
			tidalTopTracks = res.top_tracks ?? [];
			tidalVideos = res.videos ?? [];
			tidalSimilarArtists = res.similar_artists ?? [];
			tidalBio = res.bio ?? null;
			tidalAvailable = res.available ?? true;
			tidalSectionsFailed = res.sections_failed ?? [];
			if (res.picture_url) tidalPictureUrl = res.picture_url;
			// TIDAL-mode artists have no local-track fallback, so an
			// all-fetches-failed response (`available: false`) means TIDAL is
			// unreachable, not that the artist is missing. Surface it as a load
			// error so the empty state shows instead of a hollow header.
			if (!tidalAvailable) {
				error = "Couldn't reach TIDAL. Try again.";
			}
		} catch (err) {
			if (seq !== tidalLoadSeq) return;
			error = String(err);
		} finally {
			if (seq === tidalLoadSeq) {
				tidalLoading = false;
				loading = false;
			}
		}
	}

	$effect(() => {
		artist = null;
		tracks = [];
		tidalPictureUrl = null;
		tidalProfileName = null;
		tidalAlbums = [];
		tidalTopTracks = [];
		tidalVideos = [];
		tidalSimilarArtists = [];
		tidalBio = null;
		tidalAvailable = false;
		tidalSectionsFailed = [];
		failedArtworkUrls = {};
		bioExpanded = false;
		if (source.kind === 'local') {
			const id = source.artistId;
			loading = true;
			void load(id);
			void loadDiscography(id);
		} else {
			void loadTidalProfile(source.tidalArtistId);
		}
	});

	// Header sources from the artist row when available; falls back to the
	// first track only as a last resort (legacy artists missing a row, etc.).
	// Sourcing from `tracks[0]` was the historical bug that let a corrupt
	// track list rename the page header.
	let header = $derived(() => {
		if (source.kind === 'tidal') {
			if (!tidalProfileName && tidalTopTracks.length === 0 && tidalAlbums.length === 0) {
				return null;
			}
			return {
				name: tidalProfileName ?? 'Artist',
				library_track_count: 0,
			};
		}
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
	// fetch always builds at sizes we know work.
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

	function localPopularityScore(track: Track): number {
		return track.play_count ?? 0;
	}

	function popularItemKey(item: PopularTrackItem): string {
		return popularTrackItemKey(item);
	}

	// A Top-tracks row as a TIDAL playable. Owned rows carry a tidal_id (the
	// player resolves it back to the local file), so the whole list can be one
	// queue. Returns null for a pure-local track with no tidal_id.
	function popularItemPlayable(item: PopularTrackItem): TidalPlayable | null {
		if (item.kind === 'tidal') return artistTrackPlayable(item.track);
		const t = item.track;
		if (t.tidal_id == null || t.tidal_id <= 0) return null;
		return {
			tidal_id: t.tidal_id,
			title: t.title,
			artist_name: t.artist_name ?? null,
			album_title: t.album_title ?? null,
			artwork_url: t.artwork_url ?? null,
			duration_ms: t.duration_ms ?? null,
			artist_tidal_id: t.artist_tidal_id ?? activeTidalArtistId,
			album_tidal_id: t.album_tidal_id ?? null,
			local_id: t.id,
			is_in_library: true,
			is_favorite: t.is_favorite,
		};
	}

	// Play the Top tracks list in context, starting at the clicked row (the rest
	// of the list becomes the queue), mirroring how the library plays a track
	// list instead of playing one orphan song. A pure-local track with no
	// tidal_id falls back to playing the artist's owned tracks in context.
	async function onTopTrackPlay(item: PopularTrackItem) {
		if (item.kind === 'local' && (item.track.tidal_id == null || item.track.tidal_id <= 0)) {
			void playArtist(artistId, item.track.id);
			return;
		}
		const startKey = popularItemKey(item);
		const playables: TidalPlayable[] = [];
		let startIdx = -1;
		for (const it of popularItems) {
			const playable = popularItemPlayable(it);
			if (!playable) continue;
			if (popularItemKey(it) === startKey) startIdx = playables.length;
			playables.push(playable);
		}
		if (startIdx < 0) {
			const single = popularItemPlayable(item);
			if (single) void playTidalTrackNow(single);
			return;
		}
		// Clicking a specific row plays from there in order: force shuffle off so
		// the global shuffle mode doesn't randomise the list out from under the click.
		await playTidalTracksNow(
			playables.slice(startIdx),
			header()?.name ?? tidalProfileName ?? 'artist',
			{ shuffleMode: 'off' },
		);
	}

	// "Top tracks" ordering lives in the shared artist_discography helper so
	// this page and the see-all section page can never drift apart again.
	let popularItems = $derived.by<PopularTrackItem[]>(() =>
		buildPopularTrackItems(tracks, tidalTopTracks, localPopularityScore)
	);
	let popularMaxPlays = $derived(
		Math.max(1, ...popularItems.map((item) => item.kind === 'local' ? localPopularityScore(item.track) : 0))
	);
	let visiblePopularItems = $derived(popularItems.slice(0, 10));
	let totalPopularCandidates = $derived(popularItems.length);

	function artistTrackPlayable(track: TidalDiscographyTrack) {
		return tidalDiscographyTrackToPlayable(track, { artistTidalId: activeTidalArtistId });
	}

	function artistInitials(name: string): string {
		return name
			.split(/\s+/)
			.filter((w) => w.length > 0)
			.slice(0, 2)
			.map((w) => w[0]?.toUpperCase() ?? '')
			.join('');
	}

	function releaseYear(d: string | null): number | null {
		if (!d) return null;
		const y = parseInt(d.slice(0, 4), 10);
		return Number.isFinite(y) ? y : null;
	}

	// Release bucketing + ordering are shared with the see-all section pages
	// via artist_discography.ts (they used to drift: LIVE releases were
	// bucketed differently between this page and the section page).
	const categorize = categorizeTidalAlbum;
	const sortByDate = sortTidalAlbumsByReleaseDate;

	let tidalFullAlbums = $derived(sortByDate(tidalAlbums.filter((a) => categorize(a) === 'album')));
	let tidalSinglesEPs = $derived(sortByDate(tidalAlbums.filter((a) => categorize(a) === 'ep_single')));
	let tidalCompilations = $derived(sortByDate(tidalAlbums.filter((a) => categorize(a) === 'compilation')));
	let tidalLiveAlbums = $derived(sortByDate(tidalAlbums.filter((a) => categorize(a) === 'live')));

	// Whether TIDAL actually returned any releases to show. The backend can
	// report `available: true` while every album fetch errored to empty (a
	// transient TIDAL failure, an expired token, or a stale `tidal_id`), which
	// used to collapse the page to just Top tracks: the TIDAL shelves were all
	// empty and the local-track fallback was gated off by `available`. Gate the
	// album shelves on real data so a library artist still groups its owned
	// tracks into albums when TIDAL hands us nothing.
	let hasAnyTidalAlbums = $derived(
		tidalFullAlbums.length > 0
			|| tidalSinglesEPs.length > 0
			|| tidalCompilations.length > 0
			|| tidalLiveAlbums.length > 0
	);

	// Fallback (used when TIDAL returns no releases): group library tracks into albums.
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
		if (source.kind === 'tidal') {
			if (artistCurrentTrackMatchesArtist(current, [], activeTidalArtistId, tidalTopTracks)) {
				void togglePlayback();
				return;
			}
			const playable = tidalTopTracks.map(artistTrackPlayable);
			if (playable.length > 0) {
				await playTidalTracksNow(playable, tidalProfileName ?? 'artist');
			}
			return;
		}
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
		$isPlaying && artistCurrentTrackMatchesArtist($currentTrack, tracks, activeTidalArtistId, tidalTopTracks)
	);

	let shufflePending = $state(false);
	async function onShuffleClick() {
		if (source.kind === 'tidal') {
			const playable = tidalTopTracks.map(artistTrackPlayable);
			if (playable.length > 0) {
				void shuffleTidalTracksNow(playable, tidalProfileName ?? 'artist');
			}
			return;
		}
		if (tracks.length > 0) {
			void shuffleArtist(artistId);
			return;
		}
		// Library artist with no owned tracks: shuffle the TIDAL top tracks
		// instead of dead-ending on "Artist has no tracks" (mirrors Play).
		if (shufflePending) return;
		shufflePending = true;
		try {
			const requestedFor = artistId;
			const topTracks = await ensureTidalTopTracksForPlayback(requestedFor);
			if (artistId !== requestedFor) return;
			const playable = topTracks.map(artistTrackPlayable);
			if (playable.length > 0) {
				await shuffleTidalTracksNow(playable, artist?.name ?? 'artist');
			} else {
				void shuffleArtist(artistId);
			}
		} catch (error) {
			console.error('Failed to load TIDAL artist tracks for shuffle', error);
			void shuffleArtist(artistId);
		} finally {
			shufflePending = false;
		}
	}

	let radioPending = $state(false);
	async function onRadioClick() {
		if (radioPending) return;
		radioPending = true;
		try {
			if (source.kind === 'tidal') {
				const seed = tidalTopTracks[0];
				if (seed) await startTidalSongRadio(artistTrackPlayable(seed));
				return;
			}
			if (tracks.length > 0) {
				await startArtistRadio(artistId);
				return;
			}
			// Library artist with no owned tracks: seed radio from the top TIDAL
			// track instead of dead-ending (mirrors Play).
			const topTracks = await ensureTidalTopTracksForPlayback(artistId);
			const seed = topTracks[0];
			if (seed) {
				await startTidalSongRadio(artistTrackPlayable(seed));
			} else {
				await startArtistRadio(artistId);
			}
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

	// Favourite a TIDAL top-track from a non-library artist. The row has no local
	// id yet, so toggleTidalTrackFavorite imports on demand; we optimistically flip
	// is_favorite on the source array (and remember the minted track_id so a second
	// toggle re-uses it) and roll back if the round-trip fails.
	async function onTidalTopHeartClick(track: TidalDiscographyTrack, event: MouseEvent) {
		event.stopPropagation();
		const previous = track.is_favorite ?? false;
		tidalTopTracks = tidalTopTracks.map((t) =>
			t.tidal_id === track.tidal_id ? { ...t, is_favorite: !previous } : t
		);
		const seed = { ...track, is_favorite: previous } as TidalPlayable;
		const result = await toggleTidalTrackFavorite(seed, previous);
		tidalTopTracks = tidalTopTracks.map((t) =>
			t.tidal_id === track.tidal_id
				? { ...t, is_favorite: result ? result.is_favorite : previous, track_id: result?.local_id ?? t.track_id }
				: t
		);
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
	<button class="back-link" type="button" onclick={() => goBack(source.kind === 'tidal' ? '/search' : '/library')}>Back</button>
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
					{#if source.kind === 'tidal'}
						<p class="hero-sub">
							{tidalTopTracks.length} top {tidalTopTracks.length === 1 ? 'track' : 'tracks'}
							<span class="dot">·</span>
							{tidalAlbums.length} {tidalAlbums.length === 1 ? 'release' : 'releases'}
						</p>
					{:else}
						<p class="hero-sub">
							{#if artist?.track_count}
								{artist.track_count.toLocaleString()} {artist.track_count === 1 ? 'song' : 'songs'}
								<span class="dot">·</span>
							{/if}
							{#if artist?.album_count}
								{artist.album_count.toLocaleString()} {artist.album_count === 1 ? 'album' : 'albums'}
							{/if}
						</p>
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

			<button class="ghost-btn" aria-label="Shuffle" onclick={onShuffleClick}>
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
			<SearchField
				bind:value={filterQuery}
				variant="page"
				size="sm"
				fill
				placeholder="Filter tracks and albums…"
			/>
		</div>

		{#if tidalAvailable && tidalSectionsFailed.length > 0}
			<p class="status subtle partial-note" role="status">
				TIDAL was slow; some sections are partial. They will fill in on the next visit.
			</p>
		{/if}

		{#if hasAnyPopular}
			<section class="section">
				<h2 class="section-title">Top tracks</h2>
				<ol class="popular-list">
					{#each filteredPopularItems as item, idx (popularItemKey(item))}
						{#if item.kind === 'local'}
							{@const track = item.track}
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
								onRowClick={() => void onTopTrackPlay(item)}
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
							onclick={() => playable_ok && void onTopTrackPlay(item)}
							oncontextmenu={(e) => {
								e.preventDefault();
								e.stopPropagation();
								openContextMenu(e, buildTidalTrackMenu(playable), track.title);
							}}
							onkeydown={(e) =>
								(e.key === 'Enter' || e.key === ' ')
								&& (e.preventDefault(), playable_ok && void onTopTrackPlay(item))}
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
							<button
								class="tidal-row-heart"
								class:on={track.is_favorite}
								aria-label={track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
								title={track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
								onclick={(e) => void onTidalTopHeartClick(track, e)}
							>{track.is_favorite ? '♥' : '♡'}</button>
							<span class="tidal-pill" aria-label="From TIDAL">TIDAL</span>
						</li>
						{/if}
					{/each}
				</ol>
				{#if totalPopularCandidates > 10}
					<a class="show-all-btn" href={`${discographyBase}/discography/tracks`}>
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
						><svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M5 3l8 5-8 5V3z" fill="currentColor" /></svg></button>
					{:else if album.local_id != null}
						<button
							class="art-play-overlay"
							onclick={(e) => onAlbumCardPlay({ id: album.local_id }, e)}
							aria-label="Play {album.title}"
						><svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M5 3l8 5-8 5V3z" fill="currentColor" /></svg></button>
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
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, buildVideoMenu(video), video.title);
				}}
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
					<div class="art-play-overlay video-play-overlay" aria-hidden="true"><svg viewBox="0 0 16 16" width="14" height="14"><path d="M5 3l8 5-8 5V3z" fill="currentColor" /></svg></div>
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

		{#if hasAnyTidalAlbums}
			{#if filteredTidalFullAlbums.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Albums</h2>
						<span class="shelf-count">{filteredTidalFullAlbums.length}</span>
						<a class="shelf-link" href={`${discographyBase}/discography/albums`}>See all</a>
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
						<a class="shelf-link" href={`${discographyBase}/discography/singles`}>See all</a>
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

			{#if filteredTidalCompilations.length > 0}
				<section class="section">
					<div class="shelf-head">
						<h2 class="section-title">Compilations</h2>
						<span class="shelf-count">{filteredTidalCompilations.length}</span>
						<a class="shelf-link" href={`${discographyBase}/discography/compilations`}>See all</a>
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
											position="corner"
											size="sm"
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
											position="corner"
											size="sm"
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

		<!-- Videos are independent of the album shelves: an artist can have videos
		     while the album fetch came back empty, and vice versa. Gating them on
		     the album presence used to hide them whenever TIDAL returned no
		     releases. -->
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
		max-width: calc(260px + 64px);
	}

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
		flex: 0 0 176px;
		min-width: 176px;
		max-width: 176px;
		scroll-snap-align: start;
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 0;
		border-radius: var(--radius-md);
		text-decoration: none;
		color: inherit;
		transition: transform var(--motion-base);
	}

	.grid-card:hover { transform: translateY(-4px); }

	.grid-art-wrap {
		position: relative;
		width: 100%;
		aspect-ratio: 1/1;
		border-radius: var(--radius-md);
		overflow: hidden;
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);
		background: var(--bg-raised);
		margin-bottom: 6px;
		transition: box-shadow var(--motion-base);
	}

	.grid-card:hover .grid-art-wrap {
		box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5);
	}

	.grid-art-wrap:hover :global(.play-overlay),
	.grid-card:focus-within :global(.play-overlay) {
		opacity: 1;
		transform: translateY(0);
	}

	/* Circular accent play button in the artwork's bottom-right, revealed on
	   hover (matches the library/search album cards). Previously .art-play-overlay
	   had no styling, so these buttons rendered as bare glyphs. */
	.art-play-overlay {
		position: absolute;
		right: 8px;
		bottom: 8px;
		display: grid;
		place-items: center;
		width: 40px;
		height: 40px;
		border: 0;
		border-radius: 50%;
		background: var(--accent);
		color: #fff;
		box-shadow: 0 6px 16px -4px rgba(0, 0, 0, 0.55);
		opacity: 0;
		transform: translateY(6px);
		transition: opacity var(--motion-base), transform var(--motion-base), filter var(--motion-fast);
		cursor: pointer;
		z-index: 2;
	}

	.art-play-overlay svg { margin-left: 1px; }

	.art-play-overlay:hover {
		transform: translateY(0) scale(1.06);
		filter: brightness(1.08);
	}

	.grid-art-wrap:hover .art-play-overlay,
	.grid-card:focus-within .art-play-overlay {
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
		font-size: var(--font-size-sm);
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
		grid-template-columns: 32px 40px 1fr auto auto;
		align-items: center;
		gap: 12px;
		padding: 6px 12px;
		border-radius: var(--radius-sm, 8px);
		cursor: pointer;
		transition: background 120ms ease;
		min-height: 52px;
	}
	.tidal-row-heart {
		all: unset;
		width: 30px;
		height: 30px;
		display: grid;
		place-items: center;
		border-radius: 999px;
		cursor: pointer;
		color: var(--text-secondary);
		font-size: var(--font-size-md);
		opacity: 0;
		transition: opacity 120ms ease, background 120ms ease, color 120ms ease;
	}
	.tidal-popular-row:hover .tidal-row-heart,
	.tidal-row-heart:focus-visible,
	.tidal-row-heart.on { opacity: 1; }
	.tidal-row-heart:hover { background: var(--bg-hover); color: var(--text-primary); }
	.tidal-row-heart.on { color: var(--accent); }
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

</style>
