<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { fade } from 'svelte/transition';
	import { goto } from '$app/navigation';
	import {
		api,
		ApiError,
		type TidalHomeItem,
		type TidalHomeModule,
		type TidalSearchVideo,
		type TidalVideoMixItem,
		type VideoDiscoverSet,
	} from '$lib/api/client';
	import TidalDiscoverShelves from '$lib/components/search/TidalDiscoverShelves.svelte';
	import VideoCard from '$lib/components/video/VideoCard.svelte';
	import VideoSetShelf from '$lib/components/video/VideoSetShelf.svelte';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { assertOnline } from '$lib/stores/player';
	import { formatTrackDuration } from '$lib/utils/format';
	import { showToast } from '$lib/stores/toast';
	import { audioSettings } from '$lib/stores/audio_settings';
	import {
		videoClearRequest,
		videoAutoplayToggleRequest,
		videoJumpRequest,
		videoSession,
		videoStageAnchor,
		videoBrowseMode,
		setVideoBrowseMode,
		playVideo,
		clearVideoSession,
		type VideoSessionItem,
		type VideoSessionSource,
	} from '$lib/stores/video_session';

	const PAGE_SIZE = 40;
	const RECENT_KEY = 'noor_recent_video_searches';
	const SESSION_SNAPSHOT_KEY = 'noor_video_session_snapshot';
	const RECENT_MAX = 8;
	const HINTS = ['music video', 'live session', 'official video', 'visualizer'];
	// While the server assembles today's set (building: true, no snapshot yet),
	// re-fetch a few times so the picks appear without a manual reload.
	const BUILD_POLL_MS = 6000;
	const BUILD_POLL_MAX = 6;
	// TIDAL's videos page ships several modules; a couple is plenty next to the
	// library-derived shelves.
	const EDITORIAL_MODULE_MAX = 3;

	interface VideoPageSnapshot {
		selectedVideo: TidalSearchVideo | TidalVideoMixItem | null;
		videos: TidalSearchVideo[];
		mixItems: TidalVideoMixItem[];
		playlistItems: TidalSearchVideo[];
		query: string;
		lastQuery: string;
		activeMixId: string | null;
		activePlaylistId: string | null;
		hasMore: boolean;
		offset: number;
		streamUrl: string | null;
		streamExpiresAt: string | null;
	}

	function saveSessionSnapshot() {
		if (typeof sessionStorage === 'undefined' || !selectedVideo) return;
		try {
			const snap: VideoPageSnapshot = {
				selectedVideo, videos, mixItems, playlistItems, query, lastQuery,
				activeMixId, activePlaylistId, hasMore, offset, streamUrl, streamExpiresAt,
			};
			sessionStorage.setItem(SESSION_SNAPSHOT_KEY, JSON.stringify(snap));
		} catch {}
	}

	function loadSessionSnapshot(): VideoPageSnapshot | null {
		if (typeof sessionStorage === 'undefined') return null;
		try {
			const raw = sessionStorage.getItem(SESSION_SNAPSHOT_KEY);
			return raw ? (JSON.parse(raw) as VideoPageSnapshot) : null;
		} catch { return null; }
	}

	function clearSessionSnapshot() {
		if (typeof sessionStorage === 'undefined') return;
		sessionStorage.removeItem(SESSION_SNAPSHOT_KEY);
	}

	function snapshotHasRestorableContext(snap: VideoPageSnapshot): boolean {
		return Boolean(
			snap.query?.trim() ||
			snap.lastQuery?.trim() ||
			snap.activeMixId ||
			snap.activePlaylistId ||
			(snap.videos?.length ?? 0) > 0 ||
			(snap.mixItems?.length ?? 0) > 0 ||
			(snap.playlistItems?.length ?? 0) > 0
		);
	}

	let query = $state('');
	let inputEl = $state<HTMLInputElement | null>(null);
	let videos = $state<TidalSearchVideo[]>([]);
	let mixItems = $state<TidalVideoMixItem[]>([]);
	let playlistItems = $state<TidalSearchVideo[]>([]);
	let loadingSearch = $state(false);
	let loadingMore = $state(false);
	let loadingMix = $state(false);
	let loadingPlaylist = $state(false);
	let error = $state<string | null>(null);
	let mixError = $state<string | null>(null);
	let playlistError = $state<string | null>(null);
	let offset = $state(0);
	let hasMore = $state(false);
	let lastQuery = $state('');
	let activeMixId = $state<string | null>(null);
	let activePlaylistId = $state<string | null>(null);
	let sentinel = $state<HTMLDivElement | null>(null);
	let recent = $state<string[]>(loadRecent());
	let stageAnchor = $state<HTMLDivElement | null>(null);
	let debounceTimer: ReturnType<typeof setTimeout> | null = null;
	let searchAbort: AbortController | null = null;
	let loadMoreSeq = 0;
	let mixLoadSeq = 0;
	let playlistLoadSeq = 0;
	// Seed from the current request nonces, not 0: these stores outlive the
	// route, so a request consumed before this instance mounted (a queue-row
	// jump, then navigating away and back) must not re-fire and restart the
	// already-playing video. Only nonces that change after mount are new.
	let handledJumpNonce = $videoJumpRequest?.nonce ?? 0;
	let handledAutoplayToggleNonce = $videoAutoplayToggleRequest;
	let handledClearNonce = $videoClearRequest;

	// The playing video lives in the persistent dock + store; the route reads it
	// for hero meta and card highlighting, and writes picks via playVideo().
	let selectedVideo = $derived($videoSession.current);
	let streamUrl = $derived($videoSession.streamUrl);
	let streamExpiresAt = $derived($videoSession.streamExpiresAt);
	let loadingStream = $derived($videoSession.loading);
	let autoplayNext = $derived($videoSession.autoplay);

	let heroTitle = $derived(selectedVideo?.title ?? 'TIDAL video');
	let heroArtist = $derived(selectedVideo?.artist_name ?? null);

	// --- Editorial browse state ---
	// The resting state of the page: a daily-picks shelf plus TIDAL's own
	// editorial video shelf. It fades under search focus (stays mounted, so
	// clearing the field restores the same set at the same tile) and yields
	// the page entirely to the player hero while a video session is active.
	let discoverSets = $state<VideoDiscoverSet[]>([]);
	let editorialModules = $state<TidalHomeModule[]>([]);
	let loadingBrowse = $state(true);
	let browsePollTimer: ReturnType<typeof setTimeout> | null = null;
	let browsePolls = 0;

	// The daily set leads as the first rail; every other built set is its own
	// rail below it.
	let dailySet = $derived(discoverSets.find((s) => s.slug === 'daily-picks') ?? null);
	let shelfSets = $derived(
		discoverSets.filter((s) => s.slug !== 'daily-picks' && s.items.length > 0)
	);
	let videoSessionActive = $derived(Boolean(selectedVideo || streamUrl || loadingStream));
	// Browse mode: a video is playing but the listener stepped back to the
	// picks, so the dock goes mini and the shelves own the page again.
	let browseMode = $derived($videoBrowseMode);
	let playerOwnsStage = $derived(videoSessionActive && !browseMode);
	let searchFocused = $derived(query.trim().length > 0);
	let hasBrowseContent = $derived(
		Boolean(dailySet) || shelfSets.length > 0 || editorialModules.length > 0
	);
	let showEditorialLayer = $derived(!playerOwnsStage && (hasBrowseContent || loadingBrowse));

	async function loadBrowse() {
		try {
			const [discover, page] = await Promise.allSettled([
				api.getVideosDiscover(),
				api.getTidalPage('videos'),
			]);
			if (discover.status === 'fulfilled') {
				discoverSets = discover.value.sets ?? [];
				// Sets build one at a time server-side, so keep polling while
				// more are on the way - the page fills in shelf by shelf.
				if (discover.value.building && browsePolls < BUILD_POLL_MAX) {
					browsePolls += 1;
					browsePollTimer = setTimeout(() => void loadBrowse(), BUILD_POLL_MS);
				}
			}
			if (page.status === 'fulfilled' && editorialModules.length === 0) {
				editorialModules = (page.value.modules ?? [])
					.filter((m) => m.items.length >= 4)
					.slice(0, EDITORIAL_MODULE_MAX);
			}
		} finally {
			loadingBrowse = false;
		}
	}

	async function playFromSet(set: VideoDiscoverSet, index: number) {
		const video = set.items[index];
		if (!video) return;
		await playFromQueue(video, set.items, set.title, true);
	}

	function editorialItemToVideo(item: TidalHomeItem): TidalSearchVideo {
		return {
			tidal_id: Number(item.id),
			title: item.title,
			duration_ms: item.duration != null ? item.duration * 1000 : null,
			artist_id: item.artist_id ?? null,
			artist_name: item.artist_name ?? null,
			album_tidal_id: item.album_id ?? null,
			artwork_url: item.artwork_url ?? null,
			quality: null,
			explicit: null,
			type: 'Music Video',
		};
	}

	/** Claim clicks inside the TIDAL editorial shelves. Their default handling
	 *  navigates to /videos, which does nothing when we are already here, so
	 *  the route plays videos and loads playlists in place instead. */
	function handleEditorialSelect(item: TidalHomeItem): boolean {
		if (item.kind === 'video' || item.kind === 'track') {
			const owner = editorialModules.find((m) => m.items.some((i) => i.id === item.id));
			const queue = (owner?.items ?? [item])
				.filter((i) => i.kind === 'video' || i.kind === 'track')
				.map(editorialItemToVideo);
			void playFromQueue(editorialItemToVideo(item), queue, owner?.title ?? "TIDAL's picks");
			return true;
		}
		if (item.kind === 'playlist') {
			setVideoBrowseMode(false);
			void loadPlaylist(item.id, true);
			return true;
		}
		return false;
	}

	/** Shared play path for every editorial surface: daily picks, set shelves, and
	 *  the TIDAL modules. The whole shelf becomes the autoplay queue. */
	async function playFromQueue(
		video: TidalSearchVideo,
		queue: TidalSearchVideo[],
		label: string,
		autoplay = $videoSession.autoplay
	) {
		if (!assertOnline()) {
			showToast('Server is reconnecting.', 'error', 3200);
			return;
		}
		const ok = await playVideo(video, {
			queue: queue.length > 0 ? queue : [video],
			source: 'mix',
			sourceLabel: label,
			autoplay,
		});
		if (!ok) showToast($videoSession.error ?? 'This video could not be loaded.', 'error', 3200);
	}

	function backToPicks() {
		setVideoBrowseMode(true);
	}

	function backToPlayer() {
		setVideoBrowseMode(false);
	}

	let hasVideoChoices = $derived(videos.length > 0 || mixItems.length > 0 || playlistItems.length > 0);
	let showChooseVideoPrompt = $derived(
		!selectedVideo &&
		!streamUrl &&
		!loadingStream &&
		query.trim().length > 0 &&
		hasVideoChoices
	);
	// In browse mode the stage anchor must not render: its absence is what
	// tells the dock to fall back to its mini corner player.
	let showVideoHero = $derived(
		!browseMode &&
		Boolean(selectedVideo || streamUrl || loadingStream || showChooseVideoPrompt)
	);

	function loadRecent(): string[] {
		if (typeof localStorage === 'undefined') return [];
		try {
			const parsed = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
			return Array.isArray(parsed) ? parsed.filter((item) => typeof item === 'string').slice(0, RECENT_MAX) : [];
		} catch {
			return [];
		}
	}

	function pushRecent(value: string) {
		const trimmed = value.trim();
		if (!trimmed || typeof localStorage === 'undefined') return;
		recent = [trimmed, ...recent.filter((item) => item.toLowerCase() !== trimmed.toLowerCase())].slice(0, RECENT_MAX);
		localStorage.setItem(RECENT_KEY, JSON.stringify(recent));
	}

	function clearRecent() {
		recent = [];
		if (typeof localStorage !== 'undefined') localStorage.removeItem(RECENT_KEY);
	}

	function clearVideoPageSession() {
		videos = [];
		mixItems = [];
		playlistItems = [];
		query = '';
		lastQuery = '';
		activeMixId = null;
		activePlaylistId = null;
		hasMore = false;
		offset = 0;
		error = null;
		mixError = null;
		playlistError = null;
		loadMoreSeq += 1;
		mixLoadSeq += 1;
		playlistLoadSeq += 1;
		clearSessionSnapshot();
		clearVideoSession();
		void goto('/videos', { replaceState: true, keepFocus: true });
	}

	function normalizeError(errorValue: unknown, fallback: string): string {
		if (errorValue instanceof ApiError) return errorValue.message;
		if (errorValue instanceof Error) return errorValue.message;
		return fallback;
	}

	async function runSearch(nextQuery: string, replaceState = true) {
		const q = nextQuery.trim();
		searchAbort?.abort();
		searchAbort = null;
		loadMoreSeq += 1;
		loadingMore = false;
		if (!q) {
			videos = [];
			offset = 0;
			hasMore = false;
			lastQuery = '';
			error = null;
			if (replaceState) void goto('/videos', { replaceState: true, keepFocus: true });
			return;
		}

		const controller = new AbortController();
		searchAbort = controller;
		loadingSearch = true;
		error = null;
		try {
			const result = await api.searchTidalVideos(q, PAGE_SIZE, 0, controller.signal);
			if (controller.signal.aborted) return;
			videos = result.videos;
			offset = result.videos.length;
			hasMore = result.videos.length >= PAGE_SIZE;
			lastQuery = q;
			pushRecent(q);
			if (replaceState) {
				void goto(`/videos?q=${encodeURIComponent(q)}`, { replaceState: true, keepFocus: true });
			}
		} catch (err) {
			if (controller.signal.aborted || (err as Error)?.name === 'AbortError') return;
			error = normalizeError(err, 'Video search failed.');
			showToast(error, 'error', 3200);
		} finally {
			if (searchAbort === controller) searchAbort = null;
			if (!controller.signal.aborted) loadingSearch = false;
		}
	}

	function onInput() {
		if (debounceTimer) clearTimeout(debounceTimer);
		loadMoreSeq += 1;
		loadingMore = false;
		debounceTimer = setTimeout(() => void runSearch(query), 250);
	}

	async function loadMore(): Promise<number> {
		if (loadingMore || loadingSearch || !hasMore || !lastQuery) return 0;
		const seq = ++loadMoreSeq;
		const pageQuery = lastQuery;
		const pageOffset = offset;
		const isCurrentLoadMore = () =>
			seq === loadMoreSeq &&
			lastQuery === pageQuery &&
			offset === pageOffset;
		loadingMore = true;
		try {
			const result = await api.searchTidalVideos(pageQuery, PAGE_SIZE, pageOffset);
			if (!isCurrentLoadMore()) return 0;
			const seen = new Set(videos.map((video) => video.tidal_id));
			const fresh = result.videos.filter((video) => !seen.has(video.tidal_id));
			videos = [...videos, ...fresh];
			offset += result.videos.length;
			hasMore = result.videos.length >= PAGE_SIZE;
			return fresh.length;
		} catch (err) {
			if (!isCurrentLoadMore()) return 0;
			hasMore = false;
			showToast(normalizeError(err, 'Could not load more videos.'), 'error', 2800);
			return 0;
		} finally {
			if (seq === loadMoreSeq) loadingMore = false;
		}
	}

	function buildPlayContext(video: VideoSessionItem) {
		const isMix = 'mix_id' in video && video.mix_id != null;
		if (isMix) {
			return {
				queue: mixItems,
				source: 'mix' as VideoSessionSource,
				sourceLabel: activeMixId ? `Video mix ${activeMixId}` : 'Video mix',
				autoplay: $videoSession.autoplay,
			};
		}
		const inPlaylist =
			activePlaylistId != null &&
			playlistItems.some((item) => item.tidal_id === video.tidal_id);
		if (inPlaylist) {
			return {
				queue: playlistItems,
				source: 'mix' as VideoSessionSource,
				sourceLabel: 'Video playlist',
				autoplay: $videoSession.autoplay,
			};
		}
		// Anything the search results don't own but the live session does came
		// from a shelf or an editorial module. Keep that queue and its label:
		// falling through to the search results would swap a twelve-video shelf
		// for an empty array, killing autoplay and emptying the queue panel.
		const inResults = videos.some((item) => item.tidal_id === video.tidal_id);
		const session = $videoSession;
		if (!inResults && session.queue.some((item) => item.tidal_id === video.tidal_id)) {
			return {
				queue: session.queue,
				source: session.source === 'none' ? ('mix' as VideoSessionSource) : session.source,
				sourceLabel: session.sourceLabel,
				autoplay: session.autoplay,
			};
		}
		return {
			queue: videos,
			source: (lastQuery ? 'search' : 'direct') as VideoSessionSource,
			sourceLabel: lastQuery || null,
			autoplay: $videoSession.autoplay,
		};
	}

	async function selectVideo(video: VideoSessionItem, updateUrl = true): Promise<boolean> {
		if (!assertOnline()) {
			showToast('Server is reconnecting.', 'error', 3200);
			return false;
		}
		const ok = await playVideo(video, buildPlayContext(video));
		if (!ok) {
			showToast($videoSession.error ?? 'This video could not be loaded.', 'error', 3200);
			return false;
		}
		if (updateUrl) {
			const params = new URLSearchParams();
			if ('mix_id' in video && video.mix_id) {
				params.set('mixId', String(video.mix_id));
			} else if (
				activePlaylistId != null &&
				playlistItems.some((item) => item.tidal_id === video.tidal_id)
			) {
				params.set('playlistId', activePlaylistId);
			} else if (lastQuery) {
				params.set('q', lastQuery);
			}
			params.set('videoId', String(video.tidal_id));
			void goto(`/videos?${params.toString()}`, { keepFocus: true });
		}
		return true;
	}

	async function loadMix(mixId: string, autoPlayFirst = false) {
		const seq = ++mixLoadSeq;
		const isCurrentMixLoad = () => seq === mixLoadSeq && activeMixId === mixId;
		loadMoreSeq += 1;
		loadingMore = false;
		loadingMix = true;
		mixError = null;
		activeMixId = mixId;
		mixItems = [];
		try {
			const result = await api.getTidalVideoMixItems(mixId);
			if (!isCurrentMixLoad()) return;
			mixItems = result.items.map((item) => ({ ...item, mix_id: mixId }));
			if (mixItems.length === 0) mixError = 'This mix did not return video items.';
			if (autoPlayFirst && isCurrentMixLoad() && mixItems.length > 0) {
				videoSession.setAutoplay(true);
				await selectVideo(mixItems[0], false);
			}
		} catch (err) {
			if (!isCurrentMixLoad()) return;
			mixError = normalizeError(err, 'Video mix items could not load.');
			if (isCurrentMixLoad()) showToast(mixError, 'error', 3200);
		} finally {
			if (seq === mixLoadSeq) loadingMix = false;
		}
	}

	async function loadPlaylist(playlistId: string, autoPlayFirst = false) {
		const seq = ++playlistLoadSeq;
		const isCurrentPlaylistLoad = () => seq === playlistLoadSeq && activePlaylistId === playlistId;
		loadMoreSeq += 1;
		loadingMore = false;
		loadingPlaylist = true;
		playlistError = null;
		activePlaylistId = playlistId;
		playlistItems = [];
		try {
			const result = await api.getTidalVideoPlaylistItems(playlistId);
			if (!isCurrentPlaylistLoad()) return;
			playlistItems = result.items;
			if (playlistItems.length === 0) playlistError = 'This playlist did not return video items.';
			if (autoPlayFirst && isCurrentPlaylistLoad() && playlistItems.length > 0) {
				videoSession.setAutoplay(true);
				await selectVideo(playlistItems[0], false);
			}
		} catch (err) {
			if (!isCurrentPlaylistLoad()) return;
			playlistError = normalizeError(err, 'Playlist videos could not load.');
			if (isCurrentPlaylistLoad()) showToast(playlistError, 'error', 3200);
		} finally {
			if (seq === playlistLoadSeq) loadingPlaylist = false;
		}
	}

	function pickSearch(value: string) {
		query = value;
		void runSearch(value);
		inputEl?.focus();
	}

	function selectCard(video: TidalSearchVideo | TidalVideoMixItem) {
		void selectVideo(video);
	}

	function findVideoInCurrentContext(videoId: number): VideoSessionItem | null {
		// The live session queue is authoritative for whatever is playing, and
		// it is the only place shelf and editorial picks ever land - they never
		// populate the route's local arrays. Searching just those left every
		// queue row dead for those sources. Local arrays still come first:
		// their items carry the richer context (mix_id, playlist membership)
		// that buildPlayContext keys off.
		return (
			[...mixItems, ...playlistItems, ...videos, ...$videoSession.queue].find(
				(item) => item.tidal_id === videoId
			) ?? null
		);
	}

	function toggleVideoAutoplay() {
		videoSession.setAutoplay(!$videoSession.autoplay);
	}

	async function parseUrl() {
		const params = new URLSearchParams(window.location.search);
		const q = params.get('q') ?? '';
		const videoId = Number(params.get('videoId'));
		const mixId = params.get('mixId');
		const playlistId = params.get('playlistId');
		const shouldPlayCollection = params.get('play') === '1';
		query = q;
		if (q) await runSearch(q, false);
		if (mixId) {
			await loadMix(mixId, shouldPlayCollection);
			if (!shouldPlayCollection && Number.isFinite(videoId) && videoId > 0) {
				const fromMix = mixItems.find((item) => item.tidal_id === videoId);
				if (fromMix) {
					void selectVideo(fromMix, false);
					return;
				}
			}
		}
		if (playlistId) {
			await loadPlaylist(playlistId, shouldPlayCollection);
			if (!shouldPlayCollection && Number.isFinite(videoId) && videoId > 0) {
				const fromPlaylist = playlistItems.find((item) => item.tidal_id === videoId);
				if (fromPlaylist) {
					void selectVideo(fromPlaylist, false);
					return;
				}
			}
		}
		if (Number.isFinite(videoId) && videoId > 0) {
			const fromContext = findVideoInCurrentContext(videoId);
			if (fromContext) {
				void selectVideo(fromContext, false);
				return;
			}
			void selectVideo({
				tidal_id: videoId,
				title: `TIDAL video ${videoId}`,
				duration_ms: null,
				artist_id: null,
				artist_name: null,
				album_tidal_id: null,
				artwork_url: null,
				quality: null,
				explicit: null,
				type: 'video',
			}, false);
		}
	}

	onMount(() => {
		void audioSettings.load();
		void loadBrowse();
		const params = new URLSearchParams(window.location.search);
		const hasExplicitParams =
			params.has('q') || params.has('videoId') || params.has('mixId') || params.has('playlistId');

		if (!hasExplicitParams) {
			const snap = loadSessionSnapshot();
			if (snap?.selectedVideo && snapshotHasRestorableContext(snap)) {
				videos = snap.videos ?? [];
				mixItems = snap.mixItems ?? [];
				playlistItems = snap.playlistItems ?? [];
				query = snap.query ?? '';
				lastQuery = snap.lastQuery ?? '';
				activeMixId = snap.activeMixId ?? null;
				activePlaylistId = snap.activePlaylistId ?? null;
				hasMore = snap.hasMore ?? false;
				offset = snap.offset ?? 0;

				// The dock unmounted on full reload, so rehydrate playback through
				// the store (re-fetches a fresh stream rather than trusting a
				// possibly-expired snapshot URL).
				if (!$videoSession.active) void selectVideo(snap.selectedVideo, false);

				const onPop = () => void parseUrl();
				window.addEventListener('popstate', onPop);
				return () => window.removeEventListener('popstate', onPop);
			}
			clearSessionSnapshot();
		}

		void parseUrl();
		const onPop = () => void parseUrl();
		window.addEventListener('popstate', onPop);
		return () => window.removeEventListener('popstate', onPop);
	});

	$effect(() => {
		if (!sentinel) return;
		const observer = new IntersectionObserver((entries) => {
			if (entries.some((entry) => entry.isIntersecting)) void loadMore();
		}, { rootMargin: '480px 0px' });
		observer.observe(sentinel);
		return () => observer.disconnect();
	});

	// Hand the route's hero placeholder to the persistent dock so it can dock
	// the live player into it while on /videos.
	$effect(() => {
		videoStageAnchor.set(stageAnchor);
		return () => videoStageAnchor.set(null);
	});

	// Snapshot for full-page reload recovery (in-app nav is handled by the
	// persistent dock, which never unmounts).
	$effect(() => {
		if (selectedVideo && (lastQuery || activeMixId || activePlaylistId)) saveSessionSnapshot();
		else clearSessionSnapshot();
	});

	$effect(() => {
		const request = $videoJumpRequest;
		if (!request || request.nonce === handledJumpNonce) return;
		handledJumpNonce = request.nonce;
		const next = findVideoInCurrentContext(request.videoId);
		if (!next) return;
		void selectVideo(next);
	});

	$effect(() => {
		const nonce = $videoAutoplayToggleRequest;
		if (nonce === handledAutoplayToggleNonce) return;
		handledAutoplayToggleNonce = nonce;
		toggleVideoAutoplay();
	});

	$effect(() => {
		const nonce = $videoClearRequest;
		if (nonce === handledClearNonce) return;
		handledClearNonce = nonce;
		clearVideoPageSession();
	});

	onDestroy(() => {
		if (debounceTimer) clearTimeout(debounceTimer);
		if (browsePollTimer) clearTimeout(browsePollTimer);
		searchAbort?.abort();
		mixLoadSeq += 1;
	});
</script>

<div class="videos-page">
	<header class="search-header">
		<div class="search-tools">
			<span class="tools-spacer" aria-hidden="true"></span>
			<SearchField
				bind:value={query}
				bind:inputEl
				variant="page"
				fill
				placeholder="Search TIDAL videos"
				oninput={onInput}
			/>
			<!-- One contextual slot, always in the same place: where you are and
			     how you get back. Falls back to the TIDAL entry point when there
			     is nothing to go back to. -->
			<div class="tools-action">
				{#if browseMode && videoSessionActive}
					<button type="button" class="header-action header-action--live" onclick={backToPlayer}>
						<span class="live-dot" aria-hidden="true"></span>
						<span class="header-action-label">Back to the player</span>
					</button>
				{:else if videoSessionActive && hasBrowseContent}
					<button type="button" class="header-action" onclick={backToPicks}>Back to picks</button>
				{:else}
					<a class="header-action" href="/tidal/videos">TIDAL editorial</a>
				{/if}
			</div>
		</div>
		{#if searchFocused && recent.length > 0}
			<div class="recent-inline">
				<span class="eyebrow">Recent</span>
				<div class="chips">
					{#each recent as item (item)}
						<button type="button" class="hint-chip" onclick={() => pickSearch(item)}>{item}</button>
					{/each}
				</div>
				<button type="button" class="text-btn" onclick={clearRecent}>Clear</button>
			</div>
		{/if}
	</header>

	{#if showEditorialLayer}
		<div
			class="editorial-layer"
			class:receded={searchFocused}
			aria-hidden={searchFocused}
			transition:fade={{ duration: 250 }}
		>
			<div class="editorial-inner" inert={searchFocused}>
				{#if dailySet}
					<VideoSetShelf
						eyebrow="Daily picks"
						title={dailySet.title}
						blurb={dailySet.blurb}
						items={dailySet.items}
						onSelect={(_video, index) => dailySet && playFromSet(dailySet, index)}
						onPlayAll={() => dailySet && playFromSet(dailySet, 0)}
					/>
				{:else if loadingBrowse}
					<p class="picks-loading">Assembling today's picks...</p>
				{/if}
				{#each shelfSets as set (set.slug)}
					<VideoSetShelf
						title={set.title}
						blurb={set.blurb}
						items={set.items}
						onSelect={(_video, index) => playFromSet(set, index)}
						onPlayAll={() => playFromSet(set, 0)}
					/>
				{/each}

				{#if editorialModules.length > 0}
					<section class="results-section">
						<div class="section-heading section-heading--split">
							<div class="section-heading">
								<p class="eyebrow">From TIDAL's desk</p>
								<h2>Editorial picks</h2>
							</div>
							<a class="text-btn" href="/tidal/videos">More from TIDAL</a>
						</div>
						<TidalDiscoverShelves
							modules={editorialModules}
							mediaKind="video"
							onItemSelect={handleEditorialSelect}
						/>
					</section>
				{/if}
			</div>
		</div>
	{/if}

	{#if showVideoHero}
	<section class="hero" class:hero--prompt={showChooseVideoPrompt}>
		<div class="player-shell">
			<!-- Placeholder the persistent dock positions its live player over while
			     on /videos. The actual <video> lives in VideoDock so it survives
			     navigation. -->
			<div class="stage-anchor" class:has-video={Boolean(streamUrl)} bind:this={stageAnchor}>
				{#if loadingStream && !streamUrl}
					<Skeleton rows={4} label="Loading video" />
				{:else if showChooseVideoPrompt}
					<div class="video-choice-prompt" aria-live="polite">
						<strong>Choose a video</strong>
						<span>Select any result to start playback.</span>
					</div>
				{/if}
			</div>
		</div>
		{#if !showChooseVideoPrompt}
		<div class="hero-meta">
			<p class="eyebrow">Now playing</p>
			<h1>{heroTitle}</h1>
			{#if selectedVideo}
				<div class="meta-line">
					{#if selectedVideo.artist_name}
						<button
							type="button"
							class="meta-link"
							oncontextmenu={(event) => {
								if (selectedVideo?.artist_id == null) return;
								event.preventDefault();
								event.stopPropagation();
								openContextMenu(
									event,
									buildArtistMenu({ tidal_id: selectedVideo.artist_id, name: selectedVideo.artist_name ?? 'Artist' }, { isLocal: false }),
									selectedVideo.artist_name ?? undefined
								);
							}}
							onclick={() => {
								if (selectedVideo?.artist_id != null) void goto(`/tidal/artists/${selectedVideo.artist_id}`);
							}}
						>
							{selectedVideo.artist_name}
						</button>
					{/if}
					{#if selectedVideo.duration_ms}
						<span>{formatTrackDuration(selectedVideo.duration_ms)}</span>
					{/if}
					{#if $videoSession.sourceLabel}
						<span class="meta-source">from {$videoSession.sourceLabel}</span>
					{/if}
				</div>
			{/if}
			{#if error}
				<p class="inline-error">{error}</p>
			{/if}
		</div>
		{/if}
	</section>
	{/if}

	<!-- Legacy landing chips: only when there is no editorial content to show
	     (no TIDAL session / empty library), so the degraded page stays exactly
	     the search-first page it was. -->
	{#if !query.trim() && videos.length === 0 && mixItems.length === 0 && !hasBrowseContent && !loadingBrowse}
		<section class="landing-row">
			{#if recent.length > 0}
				<div class="rail-block">
					<div class="rail-header">
						<span class="eyebrow">Recent</span>
						<button type="button" class="text-btn" onclick={clearRecent}>Clear</button>
					</div>
					<div class="chips">
						{#each recent as item (item)}
							<button type="button" class="hint-chip" onclick={() => pickSearch(item)}>{item}</button>
						{/each}
					</div>
				</div>
			{/if}
			<div class="rail-block">
				<span class="eyebrow">Try</span>
				<div class="chips">
					{#each HINTS as item (item)}
						<button type="button" class="hint-chip" onclick={() => pickSearch(item)}>{item}</button>
					{/each}
				</div>
			</div>
		</section>
	{/if}

	{#if loadingSearch}
		<div class="status-wrap"><Skeleton rows={4} label="Searching videos" /></div>
	{:else if error && query.trim() && videos.length === 0}
		<EmptyState title="Video search failed" copy={error} />
	{:else if query.trim() && videos.length === 0}
		<EmptyState title="No videos found" copy="Try a broader artist, song, or live-session search." />
	{/if}

	{#if videos.length > 0}
		<section class="results-section">
			<div class="section-heading">
				<p class="eyebrow">Results</p>
				<h2>{lastQuery}</h2>
			</div>
			<div class="video-grid">
				{#each videos as video (video.tidal_id)}
					<VideoCard {video} onSelect={(item) => !('id' in item) && selectCard(item)} />
				{/each}
			</div>
			<div bind:this={sentinel} class="infinite-sentinel" aria-hidden="true">
				{#if loadingMore}<span>Loading more…</span>{/if}
			</div>
		</section>
	{/if}

	{#if loadingMix || mixItems.length > 0 || mixError}
		<section class="results-section">
			<div class="section-heading">
				<p class="eyebrow">Video mix</p>
				<h2>Mix items</h2>
			</div>
			{#if loadingMix}
				<Skeleton rows={3} label="Loading mix" />
			{:else if mixError}
				<EmptyState title="Mix unavailable" copy={mixError} />
			{:else}
				<div class="video-grid">
					{#each mixItems as video (`${video.mix_id}-${video.tidal_id}`)}
						<VideoCard {video} onSelect={(item) => !('id' in item) && selectCard(item)} />
					{/each}
				</div>
			{/if}
		</section>
	{/if}

	{#if loadingPlaylist || playlistItems.length > 0 || playlistError}
		<section class="results-section">
			<div class="section-heading">
				<p class="eyebrow">Video playlist</p>
				<h2>Playlist videos</h2>
			</div>
			{#if loadingPlaylist}
				<Skeleton rows={3} label="Loading playlist" />
			{:else if playlistError}
				<EmptyState title="Playlist unavailable" copy={playlistError} />
			{:else}
				<div class="video-grid">
					{#each playlistItems as video (video.tidal_id)}
						<VideoCard {video} onSelect={(item) => !('id' in item) && selectCard(item)} />
					{/each}
				</div>
			{/if}
		</section>
	{/if}
</div>

<style>
	.videos-page {
		width: min(100%, var(--content-width));
		margin: 0 auto;
		display: grid;
		/* minmax(0, 1fr) not the default auto: an auto track sizes to its
		   widest child's min-content, so a shelf rail of 12 cards would blow the
		   column (and the page) past the container instead of scrolling. */
		grid-template-columns: minmax(0, 1fr);
		gap: 28px;
		padding-bottom: max(44px, var(--safe-bottom));
	}

	.search-header {
		width: 100%;
		max-width: var(--content-width);
		margin: 0 auto var(--space-5);
		padding: 0 4px;
	}

	/* Three columns so the field stays optically centered no matter how wide
	   the contextual action gets - the label changes with playback state and a
	   flex row would shunt the field sideways on every change. */
	.search-tools {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 560px) minmax(0, 1fr);
		align-items: center;
		gap: var(--space-3);
	}

	.tools-action {
		display: flex;
		justify-content: flex-end;
		min-width: 0;
	}

	.header-action {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		flex: 0 0 auto;
		padding: var(--space-2) var(--space-3);
		border-radius: 999px;
		border: 1px solid var(--panel-border);
		background: var(--bg-hover);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		text-decoration: none;
		white-space: nowrap;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.header-action:hover,
	.header-action:focus-visible {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--text-primary);
		outline: none;
	}

	/* Browsing with a video still running: the accent plus a live dot say the
	   session is alive somewhere off-screen, so the button reads as "return to
	   it" rather than "start something". */
	.header-action--live {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.live-dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--accent-strong);
		animation: live-pulse 2s ease-in-out infinite;
	}

	@keyframes live-pulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.35;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.live-dot {
			animation: none;
		}
	}

	/* The editorial browse layer fades and collapses under search focus but
	   stays mounted, so leaving focus restores the same set at the same tile.
	   The height collapse rides grid-template-rows (1fr -> 0fr), which
	   animates smoothly without measuring content. */
	.editorial-layer {
		display: grid;
		grid-template-rows: 1fr;
		min-width: 0;
		opacity: 1;
		transform: translateY(0);
		transition:
			grid-template-rows var(--motion-slow, 300ms) ease,
			opacity 250ms ease,
			transform 250ms ease;
	}

	.editorial-layer.receded {
		grid-template-rows: 0fr;
		opacity: 0;
		transform: translateY(-6px);
		pointer-events: none;
	}

	.editorial-inner {
		overflow: hidden;
		min-height: 0;
		display: grid;
		/* Same reason as .videos-page: pin the column to the container so each
		   shelf's rail scrolls inside it instead of stretching the page. */
		grid-template-columns: minmax(0, 1fr);
		gap: 28px;
	}

	.picks-loading {
		margin: 0;
		padding: 4px 2px;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.recent-inline {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-3);
		flex-wrap: wrap;
		margin-top: var(--space-3);
	}

	.section-heading--split {
		justify-content: space-between;
		width: 100%;
	}

	/* Borderless, like the search surface: the video is the object on the page,
	   so a panel frame around it is one box too many. */
	.hero {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(260px, 340px);
		gap: 24px;
		padding: 0;
	}

	.hero--prompt {
		grid-template-columns: minmax(0, 1fr);
		width: min(100%, 720px);
		margin: 0 auto;
		animation: video-choice-prompt-in var(--motion-slow) cubic-bezier(0.22, 0.7, 0.2, 1) both;
	}

	.player-shell {
		min-width: 0;
		min-height: clamp(160px, 24vw, 420px);
		display: grid;
		align-items: center;
	}

	.hero--prompt .player-shell {
		min-height: 0;
	}

	.stage-anchor {
		width: 100%;
		display: grid;
		align-items: center;
	}

	/* When a video is playing, this is an empty 16/9 frame the fixed dock sits
	   on top of. Dark fill so the inline area reads as a video surface even in
	   the frame between rect measurement and the dock painting over it. */
	.stage-anchor.has-video {
		aspect-ratio: 16 / 9;
		border-radius: 8px;
		background: #030305;
	}

	.video-choice-prompt {
		justify-self: stretch;
		width: 100%;
		padding: var(--space-4) var(--space-5);
		border: 1px solid var(--accent-line);
		border-radius: var(--radius-md);
		background:
			linear-gradient(135deg, color-mix(in srgb, var(--accent-soft) 58%, transparent), transparent 76%),
			color-mix(in srgb, var(--panel-bg) 82%, transparent);
		box-shadow: 0 14px 34px color-mix(in srgb, var(--accent-glow) 40%, transparent);
		color: var(--text-primary);
	}

	.video-choice-prompt strong,
	.video-choice-prompt span {
		display: block;
	}

	.video-choice-prompt span {
		margin-top: var(--space-2);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	@keyframes video-choice-prompt-in {
		from {
			opacity: 0;
			transform: translateY(8px) scale(0.985);
			filter: blur(6px);
		}
		to {
			opacity: 1;
			transform: translateY(0) scale(1);
			filter: blur(0);
		}
	}

	.hero-meta {
		display: flex;
		flex-direction: column;
		justify-content: center;
		gap: 10px;
		min-width: 0;
	}

	.hero-meta h1 {
		margin: 0;
		font-size: var(--font-size-2xl);
		line-height: var(--line-height-tight);
		/* Long official-video titles ran to four lines and pushed the metadata
		   out of the video's optical block. */
		display: -webkit-box;
		-webkit-line-clamp: 3;
		line-clamp: 3;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.meta-line {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		align-items: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	/* Which shelf this came from - the one piece of provenance worth showing,
	   replacing the old "Stream ready" line, which described our plumbing
	   rather than anything the listener chose. */
	.meta-source {
		color: var(--text-secondary);
	}

	.meta-link,
	.text-btn {
		color: var(--accent-strong);
		font-weight: var(--font-weight-bold);
	}

	.inline-error {
		color: var(--state-error);
		font-size: var(--font-size-sm);
	}

	.landing-row {
		width: 100%;
		max-width: 720px;
		margin: 0 auto;
		padding: 0 4px;
		display: grid;
		gap: var(--space-5);
	}

	.results-section {
		display: grid;
		gap: 14px;
	}

	.rail-block {
		display: grid;
		gap: var(--space-3);
	}

	.rail-header,
	.section-heading {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 12px;
	}

	.section-heading {
		align-items: baseline;
		justify-content: flex-start;
	}

	/* Matches VideoSetShelf's heading so a results section and a shelf read as
	   the same kind of thing rather than two competing scales. */
	.section-heading h2 {
		margin: 0;
		color: var(--text-primary);
		font-size: var(--font-size-lg);
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-start;
		gap: 8px;
	}

	.hint-chip {
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		border-radius: 999px;
		padding: 7px 13px;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
	}

	.hint-chip:hover {
		background: var(--accent-soft);
		color: var(--accent-strong);
		border-color: var(--accent-line);
	}

	.video-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 14px;
	}

	.status-wrap {
		padding: 12px;
	}

	.infinite-sentinel {
		min-height: 24px;
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	@media (max-width: 860px) {
		.hero {
			grid-template-columns: 1fr;
		}
	}

	@media (max-width: 860px) {
		/* The action drops under the field rather than squeezing it. */
		.search-tools {
			grid-template-columns: 1fr;
		}

		.tools-spacer {
			display: none;
		}

		.tools-action {
			justify-content: center;
		}
	}

	@media (max-width: 620px) {
		.videos-page {
			gap: 20px;
		}

		.video-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
