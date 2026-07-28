<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import {
		ApiError,
		api,
		type ProviderRecommendationItem,
		type ProviderRecommendationShelf,
		type TidalPlayable,
	} from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import ChartMural, { type ChartMuralItem } from '$lib/components/charts/ChartMural.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import {
		recommendationActionLabel,
		recommendationEntity,
		recommendationHrefFromSearch,
		recommendationKnownHref,
		recommendationSearchHref,
		recommendationSearchQuery,
	} from '$lib/components/home/recommendation_navigation';
	import { playChartTidalTrack, playChartTidalTracks } from '$lib/player/play_trending';
	import {
		playRecommendationAlbum,
		playRecommendationArtist,
	} from '$lib/player/play_recommendations';
	import { playTrackNow } from '$lib/stores/player';
	import { openContextMenu, type MenuItem } from '$lib/stores/context_menu';
	import { buildTrackMenu, buildTidalTrackMenu } from '$lib/player/track_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { composeTidalArtQuery, peekTidalArt } from '$lib/actions/lazy-tidal-art';

	type State = 'hidden' | 'loading' | 'ready' | 'empty' | 'error';

	// Position in the home stack; stagger only. See YourMixesShelf. Each shelf
	// this component renders steps one slot further down so a batch that lands
	// together cascades rather than appearing as a block.
	let { index = 0 }: { index?: number } = $props();

	const ROTATE_MS = 5500;
	const PANEL_LIMIT = 20;

	// Seed from cache so the shelf paints instantly on launch. The provider gate is
	// preserved: only seed 'ready' if the cached Last.fm/ListenBrainz status says a
	// provider can recommend AND we have cached shelves. getSnapshot() (not getState)
	// hydrates the persisted copy. onMount's load() then revalidates - with
	// staticOptions those reads return cached instantly and refresh in the background.
	const seededCanRecommend =
		Boolean(cachedApi.lastfmStatusQuery().getSnapshot().data?.recommendations) ||
		Boolean(cachedApi.listenBrainzStatusQuery().getSnapshot().data?.recommendations);
	const seededShelves = seededCanRecommend
		? (cachedApi.homeRecommendationsQuery().getSnapshot().data?.shelves ?? [])
		: [];

	let shelves = $state<ProviderRecommendationShelf[]>(seededShelves);
	let viewState = $state<State>(
		seededCanRecommend && seededShelves.some((shelf) => shelf.items.length > 0) ? 'ready' : 'hidden'
	);
	let errorMsg = $state('');
	let currentIndexes = $state<Record<string, number>>({});
	let pausedShelves = $state<Record<string, boolean>>({});
	let playingAllShelves = $state<Record<string, boolean>>({});
	let resolvingItems = $state<Record<string, boolean>>({});
	let lazyArtwork = $state<Record<string, string>>({});
	let loadSeq = 0;

	let visibleShelves = $derived(shelves.filter((shelf) => shelf.items.length > 0));

	onMount(() => {
		void load();
		return () => { loadSeq += 1; };
	});

	$effect(() => {
		for (const shelf of visibleShelves) {
			const key = shelfKey(shelf);
			const count = shelfItems(shelf).length;
			if ((currentIndexes[key] ?? 0) >= count) currentIndexes = { ...currentIndexes, [key]: 0 };
		}
	});

	$effect(() => {
		const timer = setInterval(() => {
			for (const shelf of visibleShelves) {
				const key = shelfKey(shelf);
				if (!pausedShelves[key] && shelfItems(shelf).length > 1) jumpItem(shelf, 1);
			}
		}, ROTATE_MS);
		return () => clearInterval(timer);
	});

	async function load() {
		const seq = ++loadSeq;
		errorMsg = '';
		try {
			const [lastfm, listenbrainz] = await Promise.allSettled([
				cachedApi.getLastfmStatus(),
				cachedApi.getListenBrainzStatus()
			]);
			if (seq !== loadSeq) return;
			const lastfmCanRecommend = lastfm.status === 'fulfilled' && Boolean(lastfm.value.recommendations);
			const listenbrainzCanRecommend = listenbrainz.status === 'fulfilled' && Boolean(listenbrainz.value.recommendations);
			if (!lastfmCanRecommend && !listenbrainzCanRecommend) {
				viewState = 'hidden';
				return;
			}

			viewState = 'loading';
			const response = await cachedApi.getHomeRecommendations();
			if (seq !== loadSeq) return;
			shelves = response.shelves ?? [];
			currentIndexes = {};
			viewState = shelves.some((shelf) => shelf.items.length > 0) ? 'ready' : 'empty';
		} catch (err) {
			if (seq !== loadSeq) return;
			if (err instanceof ApiError && err.status === 404) {
				viewState = 'hidden';
				return;
			}
			viewState = 'error';
			errorMsg = err instanceof Error ? err.message : 'Recommendations could not be loaded.';
		}
	}

	function shelfKey(shelf: ProviderRecommendationShelf): string {
		return `${shelf.provider}:${shelf.entity_type ?? 'track'}:${shelf.title}`;
	}

	function itemEntity(item: ProviderRecommendationItem): string {
		return recommendationEntity(item);
	}

	function isTrackShelf(shelf: ProviderRecommendationShelf): boolean {
		return (shelf.entity_type ?? 'track') === 'track';
	}

	function shelfItems(shelf: ProviderRecommendationShelf): ProviderRecommendationItem[] {
		return shelf.items.slice(0, PANEL_LIMIT);
	}

	function currentIndexFor(shelf: ProviderRecommendationShelf): number {
		return currentIndexes[shelfKey(shelf)] ?? 0;
	}

	function currentItemFor(shelf: ProviderRecommendationShelf): ProviderRecommendationItem | null {
		const items = shelfItems(shelf);
		return items[currentIndexFor(shelf)] ?? items[0] ?? null;
	}

	function itemKey(shelf: ProviderRecommendationShelf, item: ProviderRecommendationItem, index: number): string {
		const entity = itemEntity(item);
		const localId = item.local_track_id;
		if (entity === 'track' && typeof localId === 'number' && localId > 0) return `track:local:${localId}`;
		const tidalId = item.tidal_id;
		if (entity === 'track' && typeof tidalId === 'number' && tidalId > 0) return `track:tidal:${tidalId}`;
		if (entity === 'artist' && item.local_artist_id) return `artist:local:${item.local_artist_id}`;
		if (entity === 'artist' && item.tidal_artist_id) return `artist:tidal:${item.tidal_artist_id}`;
		if (entity === 'album' && item.local_album_id) return `album:local:${item.local_album_id}`;
		if (entity === 'album' && item.tidal_album_id) return `album:tidal:${item.tidal_album_id}`;
		return `${shelfKey(shelf)}:${index}:${item.artist_name ?? ''}:${item.title}`;
	}

	// Search terms the lazy Tidal-art lookup resolves against (artists search by
	// name only; tracks by "artist title"). Shared by the mural's lazy action and
	// the synchronous cache peek so both hit the exact same cache key.
	function itemLazyQuery(item: ProviderRecommendationItem): { artist: string | null; title: string } {
		const isArtist = itemEntity(item) === 'artist';
		return {
			artist: isArtist ? item.title : (item.artist_name ?? null),
			title: isArtist ? '' : item.title,
		};
	}

	function itemArtwork(shelf: ProviderRecommendationShelf, item: ProviderRecommendationItem, index: number): string | null {
		const resolved = lazyArtwork[itemKey(shelf, item, index)] ?? item.artwork_url;
		if (resolved) return resolved;
		// Fall back to previously-resolved artwork from the persistent cache so the
		// panel paints a full collage on first launch instead of empty tiles, then
		// swaps to fresh art as the live lookups land.
		const query = itemLazyQuery(item);
		return peekTidalArt(composeTidalArtQuery(query.artist, query.title));
	}

	function itemFallbackText(item: ProviderRecommendationItem): string {
		return (item.title.trim()[0] ?? 'N').toUpperCase();
	}

	function itemSubtitle(item: ProviderRecommendationItem, index: number): string {
		if (itemEntity(item) === 'artist') return `#${index + 1} - Artist`;
		return `#${index + 1} - ${item.artist_name ?? 'Unknown artist'}`;
	}

	function itemMetric(shelf: ProviderRecommendationShelf, item: ProviderRecommendationItem, index: number): string {
		const count = shelfItems(shelf).length;
		const position = `${index + 1} of ${count}`;
		return item.reason ? `${position} - ${item.reason}` : position;
	}

	function shelfMuralItems(shelf: ProviderRecommendationShelf): ChartMuralItem[] {
		return shelfItems(shelf).map((item, index) => {
			const key = itemKey(shelf, item, index);
			return {
				id: key,
				title: item.title,
				subtitle: itemSubtitle(item, index),
				artwork: itemArtwork(shelf, item, index),
				fallbackText: itemFallbackText(item),
				tileLabel: `Select ${item.title}`,
				tileTitle: `${index + 1}. ${item.title} - ${item.artist_name ?? 'Unknown artist'}`,
				lazy: {
					enabled: itemArtwork(shelf, item, index) === null,
					query: itemLazyQuery(item),
					onResolve: (url: string) => {
						lazyArtwork = { ...lazyArtwork, [key]: url };
					},
				},
			};
		});
	}

	function itemToTidalPlayable(item: ProviderRecommendationItem): TidalPlayable {
		return {
			tidal_id: item.tidal_id ?? 0,
			title: item.title,
			artist_name: item.artist_name,
			album_title: item.album_title,
			artwork_url: item.artwork_url,
			duration_ms: null,
			artist_tidal_id: null,
			track_id: item.local_track_id ?? undefined,
			local_id: item.local_track_id,
			is_in_library: Boolean(item.local_track_id),
		};
	}

	function selectItem(shelf: ProviderRecommendationShelf, index: number) {
		currentIndexes = { ...currentIndexes, [shelfKey(shelf)]: index };
	}

	function jumpItem(shelf: ProviderRecommendationShelf, delta: number) {
		const items = shelfItems(shelf);
		if (items.length === 0) return;
		const key = shelfKey(shelf);
		const index = currentIndexes[key] ?? 0;
		currentIndexes = { ...currentIndexes, [key]: (index + delta + items.length) % items.length };
	}

	async function playItem(shelf: ProviderRecommendationShelf, item: ProviderRecommendationItem, index: number) {
		const entity = itemEntity(item);
		if (entity === 'album' || entity === 'artist') {
			const key = itemKey(shelf, item, index);
			resolvingItems = { ...resolvingItems, [key]: true };
			try {
				if (entity === 'album') await playRecommendationAlbum(item);
				else await playRecommendationArtist(item);
			} finally {
				resolvingItems = { ...resolvingItems, [key]: false };
			}
			return;
		}
		if (item.local_track_id) {
			void playTrackNow(item.local_track_id);
			return;
		}
		const key = itemKey(shelf, item, index);
		resolvingItems = { ...resolvingItems, [key]: true };
		try {
			await playChartTidalTrack(itemToTidalPlayable(item));
		} finally {
			resolvingItems = { ...resolvingItems, [key]: false };
		}
	}

	// Double-clicking any tile plays/opens that entry; single click still just
	// brings it into focus so the mural stays browsable.
	function activateItem(shelf: ProviderRecommendationShelf, index: number) {
		const item = shelfItems(shelf)[index];
		if (item) void playItem(shelf, item, index);
	}

	// One right-click menu per entity, reusing the shared builders the rest of the
	// app uses. Unresolved Last.fm albums/artists (no ids yet) get a resolve-then-act
	// menu so "Add to queue"-style actions still work before a TIDAL match exists.
	function recommendationItemMenu(item: ProviderRecommendationItem): MenuItem[] {
		const entity = itemEntity(item);
		if (entity === 'track') {
			if (item.local_track_id) {
				return buildTrackMenu({
					id: item.local_track_id,
					title: item.title,
					artist_id: item.local_artist_id ?? null,
					artist_name: item.artist_name,
					album_id: item.local_album_id ?? null,
					album_title: item.album_title,
				});
			}
			return buildTidalTrackMenu(itemToTidalPlayable(item));
		}
		if (entity === 'album') {
			if (item.local_album_id || item.tidal_album_id) {
				return buildAlbumMenu({
					id: item.local_album_id ?? null,
					tidal_id: item.tidal_album_id ?? null,
					title: item.title,
					artist_id: item.local_artist_id ?? null,
					artist_name: item.artist_name,
					in_library: Boolean(item.local_album_id),
				});
			}
			return [
				{ label: 'Play album', icon: '▶', onSelect: () => void playRecommendationAlbum(item) },
				{ separator: true, label: '' },
				{ label: 'Open album page', icon: '↗', onSelect: () => void openRecommendationItem(item) },
			];
		}
		// artist
		if (item.local_artist_id) {
			return buildArtistMenu({
				id: item.local_artist_id,
				tidal_id: item.tidal_artist_id ?? null,
				name: item.title,
				in_library: true,
			});
		}
		return [
			{ label: 'Play top tracks', icon: '▶', onSelect: () => void playRecommendationArtist(item) },
			{ separator: true, label: '' },
			{ label: 'Open artist', icon: '↗', onSelect: () => void openRecommendationItem(item) },
		];
	}

	function openItemMenu(event: MouseEvent, item: ProviderRecommendationItem) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, recommendationItemMenu(item), item.title);
	}

	async function playAllRecommendations(shelf: ProviderRecommendationShelf) {
		const key = shelfKey(shelf);
		const items = shelfItems(shelf);
		if (playingAllShelves[key] || items.length === 0) return;
		playingAllShelves = { ...playingAllShelves, [key]: true };
		try {
			await playChartTidalTracks(items.map(itemToTidalPlayable), 'recommendations');
		} finally {
			playingAllShelves = { ...playingAllShelves, [key]: false };
		}
	}

	async function openRecommendationItem(item: ProviderRecommendationItem) {
		const entity = itemEntity(item);
		if (entity !== 'artist' && entity !== 'album') return;
		const knownHref = recommendationKnownHref(item);
		if (knownHref) return goto(knownHref);
		try {
			const results = await api.searchTidal(recommendationSearchQuery(item), 5);
			const resolvedHref = recommendationHrefFromSearch(item, results);
			if (resolvedHref) return goto(resolvedHref);
		} catch {
			// Search route fallback keeps the user moving when TIDAL lookup fails.
		}
		return goto(recommendationSearchHref(item));
	}

	function actionLabel(item: ProviderRecommendationItem): string {
		return recommendationActionLabel(item);
	}

	function shelfSubtitle(shelf: ProviderRecommendationShelf): string {
		if (shelf.provider === 'lastfm' && shelf.entity_type === 'artist') return 'Artists similar to your Last.fm profile.';
		if (shelf.provider === 'lastfm' && shelf.entity_type === 'album') return 'Albums from artists near your Last.fm taste.';
		if (shelf.provider === 'lastfm') return 'Based on your Last.fm loved, recent, and top tracks.';
		return 'Based on your connected listening profile.';
	}
</script>

{#if viewState === 'hidden'}
	<!-- Hidden until a profile integration is connected. -->
{:else if viewState === 'loading'}
	<section class="profile-recommendations rise-in-shelf" data-section="provider-recommendations" style={`--rise-index: ${index}`}>
		<SectionHeader
			eyebrow="Connected profiles"
			title="Recommendations"
			subtitle="Building your Last.fm panels"
			variant="charts"
			level={2}
		/>
		<ChartMural
			ariaLabel="Loading Last.fm recommendations"
			kindLabel="Last.fm recommended"
			title="Loading recommendations"
			subtitle="Checking your profile seeds"
			loading
			loadingLabel="Loading Last.fm recommendations"
			accent="lastfm"
		/>
	</section>
{:else if viewState === 'ready'}
	<div class="profile-recommendations-list">
		{#each visibleShelves as shelf, shelfPosition (shelfKey(shelf))}
			{@const currentItem = currentItemFor(shelf)}
			{@const currentIndex = currentIndexFor(shelf)}
			{@const key = shelfKey(shelf)}
			{#if currentItem}
				<section
					class="profile-recommendations rise-in-shelf"
					data-section={`provider-recommendations-${shelf.provider}-${shelf.entity_type ?? 'track'}`}
					style={`--rise-index: ${index + shelfPosition}`}
				>
					<SectionHeader
						eyebrow="Connected profiles"
						title={shelf.title}
						subtitle={shelfSubtitle(shelf)}
						variant="charts"
						level={2}
					>
						{#snippet actions()}
							{#if isTrackShelf(shelf)}
								<button
									type="button"
									class="btn btn-glass"
									disabled={Boolean(playingAllShelves[key]) || shelfItems(shelf).length === 0}
									onclick={() => playAllRecommendations(shelf)}
								>
									{playingAllShelves[key] ? 'Resolving...' : 'Play all'}
								</button>
							{/if}
						{/snippet}
					</SectionHeader>
					<ChartMural
						items={shelfMuralItems(shelf)}
						currentIndex={currentIndex}
						ariaLabel={`${shelf.title} carousel`}
						kindLabel={shelf.provider === 'lastfm' ? `Last.fm ${shelf.entity_type ?? 'track'}s` : 'Connected profile'}
						title={currentItem.title}
						subtitle={itemSubtitle(currentItem, currentIndex)}
						metric={itemMetric(shelf, currentItem, currentIndex)}
						actionLabel={resolvingItems[itemKey(shelf, currentItem, currentIndex)] ? 'Resolving...' : actionLabel(currentItem)}
						actionDisabled={Boolean(resolvingItems[itemKey(shelf, currentItem, currentIndex)])}
						accent={shelf.provider === 'lastfm' ? 'lastfm' : 'accent'}
						onSelect={(index) => selectItem(shelf, index)}
						onJump={(delta) => jumpItem(shelf, delta)}
						onPlay={() => playItem(shelf, currentItem, currentIndex)}
						onItemActivate={(index) => activateItem(shelf, index)}
						onCardContext={(event) => openItemMenu(event, currentItem)}
						onItemContext={(event, index) => {
							const item = shelfItems(shelf)[index];
							if (item) openItemMenu(event, item);
						}}
						onPauseChange={(paused) => pausedShelves = { ...pausedShelves, [key]: paused }}
					/>
				</section>
			{/if}
		{/each}
	</div>
{:else if viewState === 'empty'}
	<section class="profile-recommendations" data-section="provider-recommendations-empty">
		<SectionHeader
			eyebrow="Connected profiles"
			title="Recommendations"
			subtitle="Last.fm has not returned profile recommendations yet"
			variant="charts"
			level={2}
		/>
		<EmptyState title="No profile recommendations yet" copy="Play, love, or backfill more tracks, then refresh this page." />
	</section>
{:else if viewState === 'error'}
	<section class="profile-recommendations" data-section="provider-recommendations-error">
		<SectionHeader
			eyebrow="Connected profiles"
			title="Recommendations"
			subtitle="Provider request failed"
			variant="charts"
			level={2}
		/>
		<EmptyState title="Could not load recommendations" copy={errorMsg}>
			{#snippet actions()}
				<button type="button" class="btn btn-glass" onclick={load}>Retry</button>
			{/snippet}
		</EmptyState>
	</section>
{/if}

<style>
	.profile-recommendations {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.profile-recommendations-list {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}
</style>
