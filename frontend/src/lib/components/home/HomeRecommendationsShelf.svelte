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
	import ChartMural, { type ChartMuralItem } from '$lib/components/charts/ChartMural.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import { playChartTidalTrack, playChartTidalTracks } from '$lib/player/play_trending';
	import { playTrackNow } from '$lib/stores/player';

	type State = 'hidden' | 'loading' | 'ready' | 'empty' | 'error';

	const ROTATE_MS = 5500;
	const PANEL_LIMIT = 20;

	let shelves = $state<ProviderRecommendationShelf[]>([]);
	let viewState = $state<State>('hidden');
	let errorMsg = $state('');
	let currentIndexes = $state<Record<string, number>>({});
	let pausedShelves = $state<Record<string, boolean>>({});
	let playingAllShelves = $state<Record<string, boolean>>({});
	let resolvingItems = $state<Record<string, boolean>>({});
	let lazyArtwork = $state<Record<string, string>>({});

	let visibleShelves = $derived(shelves.filter((shelf) => shelf.items.length > 0));

	onMount(() => {
		void load();
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
		errorMsg = '';
		try {
			const [lastfm, listenbrainz] = await Promise.allSettled([
				api.getLastfmStatus(),
				api.getListenBrainzStatus()
			]);
			const lastfmCanRecommend = lastfm.status === 'fulfilled' && Boolean(lastfm.value.recommendations);
			const listenbrainzCanRecommend = listenbrainz.status === 'fulfilled' && Boolean(listenbrainz.value.recommendations);
			if (!lastfmCanRecommend && !listenbrainzCanRecommend) {
				viewState = 'hidden';
				return;
			}

			viewState = 'loading';
			const response = await api.getHomeRecommendations();
			shelves = response.shelves ?? [];
			currentIndexes = {};
			viewState = shelves.some((shelf) => shelf.items.length > 0) ? 'ready' : 'empty';
		} catch (err) {
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
		return item.entity_type ?? 'track';
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

	function itemArtwork(shelf: ProviderRecommendationShelf, item: ProviderRecommendationItem, index: number): string | null {
		return lazyArtwork[itemKey(shelf, item, index)] ?? item.artwork_url ?? null;
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
					query: {
						artist: itemEntity(item) === 'artist' ? item.title : item.artist_name,
						title: itemEntity(item) === 'artist' ? '' : item.title,
					},
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

	function itemActionLabel(item: ProviderRecommendationItem): string {
		if (item.local_track_id) return 'Play';
		if ((item.tidal_id ?? 0) > 0) return 'Play from TIDAL';
		return 'Resolve on TIDAL';
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
		if (itemEntity(item) !== 'track') {
			await openRecommendationItem(item);
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
		if (entity === 'artist') {
			if (item.local_artist_id) return goto(`/artists/${item.local_artist_id}`);
			if (item.tidal_artist_id) return goto(`/tidal/artists/${item.tidal_artist_id}`);
			return goto(`/search?q=${encodeURIComponent(item.title)}`);
		}
		if (entity === 'album') {
			if (item.local_album_id) return goto(`/albums/${item.local_album_id}`);
			if (item.tidal_album_id) return goto(`/tidal/albums/${item.tidal_album_id}`);
			const query = [item.artist_name, item.title].filter(Boolean).join(' ');
			return goto(`/search?q=${encodeURIComponent(query)}`);
		}
	}

	function actionLabel(item: ProviderRecommendationItem): string {
		const entity = itemEntity(item);
		if (entity === 'artist') return item.local_artist_id || item.tidal_artist_id ? 'Open artist' : 'Search artist';
		if (entity === 'album') return item.local_album_id || item.tidal_album_id ? 'Open album' : 'Search album';
		return itemActionLabel(item);
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
	<section class="profile-recommendations" data-section="provider-recommendations">
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
		{#each visibleShelves as shelf (shelfKey(shelf))}
			{@const currentItem = currentItemFor(shelf)}
			{@const currentIndex = currentIndexFor(shelf)}
			{@const key = shelfKey(shelf)}
			{#if currentItem}
				<section class="profile-recommendations" data-section={`provider-recommendations-${shelf.provider}-${shelf.entity_type ?? 'track'}`}>
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
