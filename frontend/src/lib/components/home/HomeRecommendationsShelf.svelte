<script lang="ts">
	import { onMount } from 'svelte';
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
	} from '$lib/components/home/recommendation_navigation';
	import {
		openRecommendationItem,
		recommendationItemMenu,
		recommendationItemToTidalPlayable,
		recommendationShelfSlug,
	} from '$lib/components/home/recommendation_menu';
	import { playChartTidalTrack, playChartTidalTracks } from '$lib/player/play_trending';
	import {
		playRecommendationAlbum,
		playRecommendationArtist,
	} from '$lib/player/play_recommendations';
	import { playTrackNow } from '$lib/stores/player';
	import { openContextMenu } from '$lib/stores/context_menu';
	import {
		composeTidalArtQuery,
		lazyTidalArt,
		peekTidalArt,
		type LazyTidalArtKind,
	} from '$lib/actions/lazy-tidal-art';
	import { usableArtwork } from '$lib/utils/artwork';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';

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
			// The recommendations request goes out alongside the status checks
			// rather than behind them. Gating it on the gate meant a two-stage
			// waterfall on every mount even though both statuses are already in
			// the persisted query cache, and the recommendations call is by far
			// the slow one. If the gate turns out to be closed the response is
			// simply discarded; the request is cached either way, so the only
			// cost is a call we would have made moments later anyway.
			const statuses = Promise.allSettled([
				cachedApi.getLastfmStatus(),
				cachedApi.getListenBrainzStatus()
			]);
			const recommendations = cachedApi.getHomeRecommendations();
			// Nothing else awaits this promise on the gate-closed path, and an
			// unhandled rejection would surface as a console error.
			recommendations.catch(() => {});

			const [lastfm, listenbrainz] = await statuses;
			if (seq !== loadSeq) return;
			const lastfmCanRecommend = lastfm.status === 'fulfilled' && Boolean(lastfm.value.recommendations);
			const listenbrainzCanRecommend = listenbrainz.status === 'fulfilled' && Boolean(listenbrainz.value.recommendations);
			if (!lastfmCanRecommend && !listenbrainzCanRecommend) {
				viewState = 'hidden';
				return;
			}

			viewState = 'loading';
			const response = await recommendations;
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

	function isArtistShelf(shelf: ProviderRecommendationShelf): boolean {
		return shelf.entity_type === 'artist';
	}

	/**
	 * What this shelf shows in place, which is a soft cap, not the whole set.
	 *
	 * For the track shelf twenty is hard: the mural is a fixed 10x2 grid
	 * (`layout-count-20` in ChartMural.svelte), and a twenty-first tile would add
	 * a row and reshape the mosaic. For the rails it is a judgement - the server
	 * now returns fifty, and a rail you have to drag through fifty times is not a
	 * way to see fifty things. The rest lives behind "View all".
	 */
	function shelfItems(shelf: ProviderRecommendationShelf): ProviderRecommendationItem[] {
		return shelf.items.slice(0, PANEL_LIMIT);
	}

	/** True when the shelf is holding back items the rail is not showing. */
	function hasMoreThanShelf(shelf: ProviderRecommendationShelf): boolean {
		return shelf.items.length > PANEL_LIMIT;
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

	function itemKind(item: ProviderRecommendationItem): LazyTidalArtKind {
		const entity = itemEntity(item);
		return entity === 'artist' || entity === 'album' ? entity : 'track';
	}

	function itemArtwork(shelf: ProviderRecommendationShelf, item: ProviderRecommendationItem, index: number): string | null {
		// usableArtwork, not `??`: Last.fm hands back a real URL for a grey star
		// placeholder when it has no art. Treating that as present left the tile
		// showing the star forever and suppressed the TIDAL lookup that would
		// have found the actual cover.
		const resolved = usableArtwork(lazyArtwork[itemKey(shelf, item, index)], item.artwork_url);
		if (resolved) return resolved;
		// Fall back to previously-resolved artwork from the persistent cache so the
		// panel paints a full collage on first launch instead of empty tiles, then
		// swaps to fresh art as the live lookups land.
		const query = itemLazyQuery(item);
		return peekTidalArt(composeTidalArtQuery(query.artist, query.title), itemKind(item));
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

	// Built once per shelf and reused until the data or the resolved artwork
	// actually changes. The rotation timer ticks every 5.5s and only moves
	// currentIndexes; rebuilding twenty mural items (and re-running twenty
	// lazy-art action updates) on each tick churned exactly while the artwork
	// was still landing. Deriving off shelves + lazyArtwork makes that
	// independence explicit rather than incidental.
	const muralItemsByShelf = $derived.by(() => {
		const map: Record<string, ChartMuralItem[]> = {};
		for (const shelf of visibleShelves) {
			if (isTrackShelf(shelf)) map[shelfKey(shelf)] = shelfMuralItems(shelf);
		}
		return map;
	});

	const railItemsByShelf = $derived.by(() => {
		const map: Record<string, RailEntry[]> = {};
		for (const shelf of visibleShelves) {
			if (!isTrackShelf(shelf)) map[shelfKey(shelf)] = shelfRailItems(shelf);
		}
		return map;
	});

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
					kind: itemKind(item),
					query: itemLazyQuery(item),
					onResolve: (url: string) => {
						lazyArtwork = { ...lazyArtwork, [key]: url };
					},
				},
			};
		});
	}

	// Artists and albums render as rails rather than murals, and MediaRail's card
	// snippet only receives the item. Precomputing a view model here keeps the
	// snippet free of the per-shelf lookups (artwork, key, lazy query) it would
	// otherwise need the shelf in scope to do.
	type RailEntry = {
		key: string;
		item: ProviderRecommendationItem;
		artwork: string | null;
		fallbackText: string;
		lazyQuery: { artist: string | null; title: string };
	};

	function shelfRailItems(shelf: ProviderRecommendationShelf): RailEntry[] {
		return shelfItems(shelf).map((item, index) => ({
			key: itemKey(shelf, item, index),
			item,
			artwork: itemArtwork(shelf, item, index),
			fallbackText: itemFallbackText(item),
			lazyQuery: itemLazyQuery(item),
		}));
	}

	function resolveRailArtwork(key: string, url: string) {
		lazyArtwork = { ...lazyArtwork, [key]: url };
	}

	// Re-exported from the shared module so the "View all" page and this shelf
	// cannot drift apart.
	const itemToTidalPlayable = recommendationItemToTidalPlayable;

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

{#snippet artistCard(entry: RailEntry)}
	<button
		type="button"
		class="rec-card rec-card-artist"
		title={entry.item.title}
		aria-label={`Open ${entry.item.title}`}
		onclick={() => void openRecommendationItem(entry.item)}
		oncontextmenu={(event) => openItemMenu(event, entry.item)}
		use:lazyTidalArt={{
			enabled: entry.artwork === null,
			kind: 'artist',
			query: entry.lazyQuery,
			onResolve: (url) => resolveRailArtwork(entry.key, url),
		}}
	>
		<div class="rec-avatar-wrap">
			<ArtworkImage
				className="rec-avatar"
				src={entry.artwork}
				alt={entry.item.title}
				size={320}
				tint={true}
				fadeIn={true}
				fallbackText={entry.fallbackText}
			/>
		</div>
		<p class="rec-title">{entry.item.title}</p>
	</button>
{/snippet}

{#snippet albumCard(entry: RailEntry)}
	<button
		type="button"
		class="rec-card"
		title={`${entry.item.title}${entry.item.artist_name ? ` - ${entry.item.artist_name}` : ''}`}
		aria-label={`Open ${entry.item.title}`}
		onclick={() => void openRecommendationItem(entry.item)}
		oncontextmenu={(event) => openItemMenu(event, entry.item)}
		use:lazyTidalArt={{
			enabled: entry.artwork === null,
			kind: 'album',
			query: entry.lazyQuery,
			onResolve: (url) => resolveRailArtwork(entry.key, url),
		}}
	>
		<div class="rec-art-wrap">
			<ArtworkImage
				className="rec-art"
				src={entry.artwork}
				alt={entry.item.title}
				size={320}
				tint={true}
				fadeIn={true}
				fallbackText={entry.fallbackText}
			/>
		</div>
		<p class="rec-title">{entry.item.title}</p>
		{#if entry.item.artist_name}
			<p class="rec-subtitle">{entry.item.artist_name}</p>
		{/if}
	</button>
{/snippet}

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
							<!-- Only when there is genuinely more than the rail shows, so the
							     link can never lead to the same cards again. -->
							{#if hasMoreThanShelf(shelf)}
								<a class="rec-view-all" href={`/recommendations/${recommendationShelfSlug(shelf)}`}>
									View all {shelf.items.length} &#8594;
								</a>
							{/if}
						{/snippet}
					</SectionHeader>
					{#if isTrackShelf(shelf)}
						<ChartMural
							items={muralItemsByShelf[key] ?? []}
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
					{:else}
						<MediaRail
							items={railItemsByShelf[key] ?? []}
							card={isArtistShelf(shelf) ? artistCard : albumCard}
							getKey={(entry) => entry.key}
							ariaLabel={shelf.title}
							fluid
							stagger
						/>
					{/if}
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

	.rec-view-all {
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		text-decoration: none;
		white-space: nowrap;
		transition: color var(--motion-base);
	}
	.rec-view-all:hover { color: var(--text-primary); }

	.profile-recommendations-list {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}

	/* Artists and albums used to each get their own full-width mural. Three
	   near-identical mosaics stacked was most of the visual weight of the page,
	   and a square mural tile is the wrong shape for an artist anyway. Tracks
	   keep the mural; these two become rails, matching how /search presents the
	   same two kinds. */
	.rec-card {
		width: 100%;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		background: none;
		border: 0;
		padding: 0;
		text-align: left;
		color: inherit;
		font: inherit;
		cursor: pointer;
		box-sizing: border-box;
		transition: transform var(--motion-base);
	}

	.rec-card:hover {
		transform: translateY(-4px);
	}

	.rec-card:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 4px;
	}

	.rec-card-artist {
		align-items: center;
		text-align: center;
	}

	.rec-art-wrap,
	.rec-avatar-wrap {
		position: relative;
		width: 100%;
		aspect-ratio: 1 / 1;
		overflow: hidden;
		background: var(--bg-raised);
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);
		transition: box-shadow var(--motion-base);
	}

	.rec-art-wrap {
		border-radius: var(--radius-md);
	}

	.rec-avatar-wrap {
		border-radius: 50%;
	}

	.rec-card:hover .rec-art-wrap,
	.rec-card:hover .rec-avatar-wrap {
		box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5);
	}

	.rec-card :global(.rec-art),
	.rec-card :global(.rec-avatar) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.rec-card :global(.rec-art.fallback),
	.rec-card :global(.rec-avatar.fallback) {
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.rec-card :global(.rec-art.fallback span),
	.rec-card :global(.rec-avatar.fallback span) {
		font-size: var(--font-size-3xl);
		color: rgba(255, 255, 255, 0.92);
	}

	.rec-title,
	.rec-subtitle {
		margin: 0;
		width: 100%;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: var(--line-height-snug);
	}

	.rec-title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
	}

	.rec-subtitle {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
	}
</style>
