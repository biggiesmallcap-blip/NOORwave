<script lang="ts">
	import { get } from 'svelte/store';
	import { api, type HomeShufflePicksResponse, type HomeSuggestionsResponse, type Track } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { lazyTidalArt, composeTidalArtQuery, peekTidalArt } from '$lib/actions/lazy-tidal-art';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { playTrackNow, shuffleMode } from '$lib/stores/player';
	import {
		HOME_MURAL_ITEM_LIMIT,
		buildMuralPanels,
		fallbackLetters,
		muralItemKey,
		muralItemLazyQuery,
		panelQueueTrackIds,
		toAlbumCard,
		uniqueById,
		type HomeAlbumCard,
		type HomeMuralItem,
		type HomeMuralPanel,
	} from './library_murals';
	import { MURAL_REFRESH_DELAYS_MS, watchMuralQuery } from './mural_query_watch';

	// The Library landing's four suggestion murals: listen-history suggestions
	// and library shuffle picks, each painted as a collage of the picks. A tile
	// plays (tracks) or opens (albums) its item; the round button plays the
	// whole track panel.
	let {
		onOpenAlbum,
		onAlbumContextMenu,
		riseIndex = 1,
	}: {
		onOpenAlbum: (card: HomeAlbumCard) => void;
		onAlbumContextMenu: (event: MouseEvent, card: HomeAlbumCard) => void;
		/** Slot in the host page's entrance cascade. See `rise-in-shelf` in app.css. */
		riseIndex?: number;
	} = $props();

	// Both murals subscribe to reactive cache queries: the last payload paints at
	// once, and the background refresh replaces it when it lands. Neither waits
	// on the library store. The server owns seed selection, recency exclusion and
	// ranking for the suggestions; there is deliberately no client-side fallback.
	// An empty panel is a correct outcome; a panel full of what was just played
	// is not.
	const shuffleQuery = cachedApi.homeShufflePicksQuery(HOME_MURAL_ITEM_LIMIT);
	const suggestionsQuery = cachedApi.homeSuggestionsQuery(50);

	function shuffleSources(data: HomeShufflePicksResponse | undefined) {
		return {
			tracks: uniqueById(data?.tracks ?? []),
			albums: uniqueById(data?.albums ?? []).map(toAlbumCard),
		};
	}

	function suggestionSources(data: HomeSuggestionsResponse | undefined) {
		return {
			tracks: data?.tracks ?? [],
			albums: (data?.albums ?? []).map(toAlbumCard),
		};
	}

	const initialShuffle = shuffleSources(shuffleQuery.getSnapshot().data);
	const initialSuggestions = suggestionSources(suggestionsQuery.getSnapshot().data);
	let randomTracks = $state<Track[]>(initialShuffle.tracks);
	let randomAlbums = $state<HomeAlbumCard[]>(initialShuffle.albums);
	let suggestionTracks = $state<Track[]>(initialSuggestions.tracks);
	let suggestionAlbums = $state<HomeAlbumCard[]>(initialSuggestions.albums);

	// Per-tile lazy artwork, keyed by `muralItemKey` so tracks and albums with
	// the same id never collide.
	let lazyArt = $state<Record<string, string>>({});

	let panels = $derived(buildMuralPanels({ suggestionTracks, suggestionAlbums, randomTracks, randomAlbums }));

	// Empty or failed samples keep refreshing on a bounded backoff while the page
	// stays open; the last-good payload stays on screen meanwhile.
	$effect(() => {
		const stops = [
			watchMuralQuery(shuffleQuery, {
				isEmpty: (data) => (data.tracks?.length ?? 0) + (data.albums?.length ?? 0) === 0,
				onData: (data) => {
					const next = shuffleSources(data);
					randomTracks = next.tracks;
					randomAlbums = next.albums;
				},
				onError: (error) => console.error('Failed to load library shuffle picks:', error),
				delaysMs: MURAL_REFRESH_DELAYS_MS,
			}),
			watchMuralQuery(suggestionsQuery, {
				isEmpty: (data) => (data.tracks?.length ?? 0) + (data.albums?.length ?? 0) === 0,
				onData: (data) => {
					const next = suggestionSources(data);
					suggestionTracks = next.tracks;
					suggestionAlbums = next.albums;
				},
				onError: (error) => console.error('Failed to load home suggestions:', error),
				delaysMs: MURAL_REFRESH_DELAYS_MS,
			}),
		];
		return () => {
			for (const stop of stops) stop();
		};
	});

	// Baked art, then resolved lazy art, then previously cached art (peek), so a
	// first launch paints a full collage while live lookups swap in fresh art.
	function tileArtwork(item: HomeMuralItem): string | null {
		const resolved = item.artwork_url ?? lazyArt[muralItemKey(item)];
		if (resolved) return resolved;
		const query = muralItemLazyQuery(item);
		return peekTidalArt(composeTidalArtQuery(query.artist, query.title));
	}

	async function playFromPanel(panel: HomeMuralPanel, trackId: number) {
		const trackIds = panelQueueTrackIds(panel, trackId);
		try {
			const replaced = await api.replacePlaybackQueue(
				trackIds.map((track_id) => ({ track_id })),
				{ shuffleMode: get(shuffleMode) }
			);
			const selected = replaced.queue.find((queueItem) => queueItem.track.id === trackId);
			if (selected) await api.playQueueItem(selected.id);
		} catch (error) {
			console.error('Failed to play library mural track:', error);
			await playTrackNow(trackId);
		}
	}

	function activateTile(item: HomeMuralItem, panel: HomeMuralPanel) {
		if (item.kind === 'track' && item.track) {
			void playFromPanel(panel, item.track.id);
		} else if (item.kind === 'album' && item.album) {
			onOpenAlbum(item.album);
		}
	}

	function openTileMenu(event: MouseEvent, item: HomeMuralItem) {
		event.preventDefault();
		event.stopPropagation();
		if (item.kind === 'track' && item.track) {
			openContextMenu(event, buildTrackMenu(item.track), item.title);
		} else if (item.kind === 'album' && item.album) {
			onAlbumContextMenu(event, item.album);
		}
	}

	function playPanel(panel: HomeMuralPanel) {
		const first = panel.items.find((item) => item.track);
		if (first?.track) void playFromPanel(panel, first.track.id);
	}
</script>

{#if panels.length > 0}
	<section class="murals rise-in-shelf" style={`--rise-index: ${riseIndex}`} aria-label="Picks from your library">
		<div class="mural-grid">
			{#each panels as panel, i (panel.id)}
				<article class="mural rise-in-card" aria-label={panel.label} style={`--rise-index: ${i}`}>
					<div class="mosaic">
						{#each panel.items as item (`${panel.id}-${item.kind}-${item.id}`)}
							{@const art = tileArtwork(item)}
							<!-- Native buttons activate on Enter and Space, so no keydown handler. -->
							<button
								class="tile"
								type="button"
								onclick={() => activateTile(item, panel)}
								oncontextmenu={(event) => openTileMenu(event, item)}
								aria-label={`${item.kind === 'track' ? 'Play' : 'Open'} ${item.title}`}
								title={`${item.title}${item.subtitle ? ` - ${item.subtitle}` : ''}`}
								use:lazyTidalArt={{
									enabled: art === null,
									query: muralItemLazyQuery(item),
									onResolve: (url) => (lazyArt[muralItemKey(item)] = url),
								}}
							>
								<ArtworkImage
									className="mural-art"
									src={art}
									size={320}
									fallbackText={fallbackLetters(item.title)}
									decorative={true}
									loading="eager"
									fadeIn={true}
								/>
							</button>
						{/each}
					</div>
					<div class="scrim" aria-hidden="true"></div>
					<div class="copy">
						<!-- One title; the source reads once in the line under it. -->
						<h3 class="title">{panel.label}</h3>
						<span class="count">{panel.items.length} picks {panel.caption}</span>
					</div>
					{#if panel.kind === 'track'}
						<button class="play" type="button" onclick={() => playPanel(panel)} aria-label={`Play ${panel.label}`}>
							<svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">
								<path d="M4 2.5l9.5 5.5L4 13.5V2.5z" />
							</svg>
						</button>
					{/if}
				</article>
			{/each}
		</div>
	</section>
{/if}

<style>
	.murals {
		container-type: inline-size;
	}

	.mural-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-4);
	}

	@container (max-width: 720px) {
		.mural-grid {
			grid-template-columns: minmax(0, 1fr);
		}
	}

	/* One surface: no border, the collage is the panel. Entrance motion comes
	   from the shared `rise-in-card` class in the markup. */
	.mural {
		position: relative;
		isolation: isolate;
		min-height: clamp(150px, 15vw, 210px);
		border-radius: var(--radius-md);
		overflow: hidden;
		background: var(--bg-raised);
	}

	/* Angled slices, the same signature as the top-artists hero. The grid bleeds
	   past the panel so the skewed edges never show a gap. */
	.mosaic {
		position: absolute;
		inset: -7%;
		display: grid;
		grid-template-columns: repeat(6, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
	}

	@container (max-width: 720px) {
		.mosaic {
			grid-template-columns: repeat(4, minmax(0, 1fr));
			grid-template-rows: repeat(3, minmax(0, 1fr));
		}
	}

	.tile {
		appearance: none;
		position: relative;
		min-width: 0;
		min-height: 0;
		padding: 0;
		border: 0;
		background: var(--bg-raised);
		cursor: pointer;
		overflow: hidden;
		filter: var(--art-collage-filter);
		transform: skewX(-7deg) scaleX(1.08);
		transition:
			filter var(--motion-fast),
			transform var(--motion-base),
			box-shadow var(--motion-base);
	}

	.tile:hover,
	.tile:focus-visible {
		z-index: var(--z-raised);
		filter: saturate(1.12) brightness(1.06);
		transform: skewX(-7deg) scaleX(1.08) scale(1.04);
		box-shadow:
			0 0 0 1px rgba(255, 255, 255, 0.4),
			0 12px 28px rgba(0, 0, 0, 0.32);
		outline: none;
	}

	.tile:focus-visible {
		box-shadow:
			0 0 0 2px var(--accent-strong),
			0 12px 28px rgba(0, 0, 0, 0.32);
	}

	.tile :global(.mural-art) {
		display: block;
		width: 100%;
		height: 100%;
	}

	.tile :global(.mural-art:not(.fallback)) {
		object-fit: cover;
		transform: skewX(7deg) scale(1.24);
		/* Opacity is listed so ArtworkImage's fadeIn still eases; this rule
		   outranks the component's own transition. */
		transition:
			transform var(--motion-base),
			opacity var(--motion-slow);
	}

	.tile:hover :global(.mural-art:not(.fallback)),
	.tile:focus-visible :global(.mural-art:not(.fallback)) {
		transform: skewX(7deg) scale(1.32);
	}

	.tile :global(.mural-art.fallback) {
		display: grid;
		place-items: center;
		background: linear-gradient(135deg, var(--bg-raised), color-mix(in srgb, var(--accent-soft) 28%, var(--bg-surface)));
		color: var(--text-secondary);
		transform: skewX(7deg) scale(1.08);
	}

	.tile :global(.mural-art.fallback span) {
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
	}

	/* Text sits on artwork, so it is white on a dark scrim in every theme. The
	   scrim only covers the corner the copy occupies; the rest of the collage
	   stays at full brightness. */
	.scrim {
		position: absolute;
		inset: 0;
		z-index: var(--z-base);
		pointer-events: none;
		background:
			linear-gradient(to top, rgba(8, 8, 12, 0.78) 0%, rgba(8, 8, 12, 0.36) 36%, rgba(8, 8, 12, 0) 64%),
			linear-gradient(90deg, rgba(8, 8, 12, 0.42) 0%, rgba(8, 8, 12, 0) 48%);
	}

	.copy {
		position: absolute;
		left: 0;
		bottom: 0;
		z-index: calc(var(--z-base) + 1);
		display: grid;
		gap: var(--space-1);
		max-width: min(24rem, 72%);
		padding: var(--space-4);
		color: #fff;
		text-shadow: 0 1px 12px rgba(0, 0, 0, 0.5);
		pointer-events: none;
	}


	.title {
		margin: 0;
		color: #fff;
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
	}

	.count {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		color: rgba(255, 255, 255, 0.76);
	}

	.play {
		position: absolute;
		right: var(--space-4);
		bottom: var(--space-4);
		z-index: calc(var(--z-base) + 1);
		display: grid;
		place-items: center;
		width: 44px;
		height: 44px;
		padding: 0;
		border: 0;
		border-radius: 50%;
		background: var(--accent);
		color: var(--text-on-accent);
		cursor: pointer;
		box-shadow: 0 8px 20px rgba(0, 0, 0, 0.4);
		opacity: 0;
		transform: translateY(4px);
		transition:
			opacity var(--motion-fast),
			transform var(--motion-base);
	}

	.mural:hover .play,
	.mural:focus-within .play {
		opacity: 1;
		transform: none;
	}

	.play:active {
		transform: scale(0.96);
	}

	.play:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	@media (hover: none) {
		.play {
			opacity: 1;
			transform: none;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.tile,
		.tile :global(.mural-art:not(.fallback)),
		.play {
			transition: none;
		}

		.tile:hover,
		.tile:focus-visible {
			transform: skewX(-7deg) scaleX(1.08);
		}

		.tile:hover :global(.mural-art:not(.fallback)),
		.tile:focus-visible :global(.mural-art:not(.fallback)) {
			transform: skewX(7deg) scale(1.24);
		}
	}
</style>
