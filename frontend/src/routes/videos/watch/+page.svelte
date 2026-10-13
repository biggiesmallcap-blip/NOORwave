<script lang="ts">
	import { onMount } from 'svelte';
	import { goto, replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import { api, type TidalSearchVideo } from '$lib/api/client';
	import { loadSavedVideos, savedVideoIds, savingVideo, toggleSavedVideo } from '$lib/stores/saved_videos';
	import VideoCard from '$lib/components/video/VideoCard.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { assertOnline } from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import { audioSettings } from '$lib/stores/audio_settings';
	import { formatTrackDuration } from '$lib/utils/format';
	import {
		clearVideoSession,
		playVideo,
		videoSession,
		videoStageAnchor,
	} from '$lib/stores/video_session';
	import { WATCH_PATH, watchUrl } from '$lib/video/section';
	import {
		FOCUS_ROWS,
		MAX_FOCUS_LEVEL,
		focusLevel,
		gridColumns,
		mergeFeed,
		needsMore,
		placeholderCount,
		radioExclusions,
		trimToRows,
	} from '$lib/video/related_feed';
	import type { ExploreVideo } from '$lib/api/video_explore_types';
	import StageRow from '$lib/video/archive/StageRow.svelte';
	import VersionsDrawer from '$lib/video/archive/VersionsDrawer.svelte';
	import { CUT_LABELS, hueColor } from '$lib/video/archive/lenses';
	import { concertLength, versionCaption } from '$lib/video/archive/versions';
	import { playConcert, playFromList, playVersion } from '$lib/video/archive/play';
	import {
		EMPTY_STAGE,
		loadStage,
		needsFallback,
		viaLabel,
		type StageItem,
		type StageRows,
	} from '$lib/video/archive/stage';

	// The watch page: the one place the big player lives. The persistent dock
	// positions the live <video> over this page's stage; everything else here
	// is about what is playing - who it is and where to go from it. What comes
	// next is the app's video queue panel, not a second list here. Back
	// returns to the tab the video was picked on.
	//
	// Under the player, the Stage rows: This concert, Same scene, One step
	// out, Other cuts of this song (lib/video/archive/stage.ts). When the
	// exploring rows come back empty (the Atlas endpoints answer 501 before
	// their build ships, or the artist is uncharted) the older related grid
	// below takes over, with its radio-fed extension.

	let stageAnchor = $state<HTMLDivElement | null>(null);
	let relatedVideos = $state<(TidalSearchVideo & { why?: string })[]>([]);
	let relatedLoading = $state(false);
	let relatedRequest = 0;
	// Below the close picks the grid keeps going from the video radio feed,
	// softer the further it gets (see related_feed.ts).
	const FEED_BATCH_MAX = 10;
	const FEED_RETRY_MS = 3000;
	let feedLoading = $state(false);
	let feedDone = $state(false);
	let feedWaiting = $state(false);
	let feedBatches = 0;
	let feedMisses = 0;
	let feedRetry: ReturnType<typeof setTimeout> | null = null;
	let gridEl = $state<HTMLDivElement | null>(null);
	let columns = $state(1);
	let rowLevels = $state<number[]>([]);
	let nearEnd = $state(false);

	let shownRelated = $derived(feedDone ? trimToRows(relatedVideos, columns) : relatedVideos);
	let placeholders = $derived(placeholderCount(shownRelated.length, columns, feedLoading || feedWaiting));

	let stageRows = $state<StageRows>(EMPTY_STAGE);
	let stageSettled = $state(false);
	let stageRequest = 0;
	let versionsOpen = $state(false);
	let useFallback = $derived(stageSettled && needsFallback(stageRows));

	let current = $derived($videoSession.current);
	let streamUrl = $derived($videoSession.streamUrl);
	let loadingStream = $derived($videoSession.loading);
	let hasSession = $derived(Boolean(current || streamUrl || loadingStream));
	let videoIsSaved = $derived(current ? $savedVideoIds.has(current.tidal_id) : false);

	async function playRelated(video: TidalSearchVideo) {
		if (!assertOnline()) {
			showToast('Server is reconnecting.', 'error', 3200);
			return;
		}
		const ok = await playVideo(video, {
			queue: relatedVideos,
			source: 'mix',
			sourceLabel: 'Related video radio',
			autoplay: true,
			continuous: true,
			resetRadio: true,
		});
		if (!ok) showToast($videoSession.error ?? 'This video could not be loaded.', 'error', 3200);
	}

	/** Unmeasured rows below the sharp ones start out of focus, so a new
	 *  batch never flashes sharp before the first measurement. */
	function tileFocus(index: number): number {
		const row = Math.floor(index / columns);
		return rowLevels[row] ?? (row < FOCUS_ROWS ? 0 : MAX_FOCUS_LEVEL);
	}

	function resetFeed() {
		feedLoading = false;
		feedDone = false;
		feedWaiting = false;
		feedBatches = 0;
		feedMisses = 0;
		if (feedRetry) clearTimeout(feedRetry);
		feedRetry = null;
	}

	/** One more batch for the extension, from the video radio feed seeded on
	 *  the playing video. Stops after FEED_BATCH_MAX batches or when the feed
	 *  stays dry; a feed that is still being built is retried shortly. */
	async function loadMoreRelated() {
		const item = current;
		if (!item || feedLoading || feedDone || feedWaiting || relatedLoading) return;
		const seq = relatedRequest;
		feedLoading = true;
		const { exclude, recentArtists } = radioExclusions(relatedVideos, item.tidal_id);
		try {
			const { items, building } = await api.getVideoRadioNext({
				seed_artist_id: item.artist_id,
				seed_artist_name: item.artist_name,
				seed_video_id: item.tidal_id,
				exclude_video_ids: exclude,
				recent_video_ids: [],
				recent_songs: [],
				recent_artist_ids: recentArtists,
			});
			if (seq !== relatedRequest) return;
			const merged = mergeFeed(relatedVideos, items, item.tidal_id);
			feedBatches += 1;
			if (merged.length > relatedVideos.length) {
				relatedVideos = merged;
				feedMisses = 0;
			} else {
				feedMisses += 1;
			}
			if (feedBatches >= FEED_BATCH_MAX || (feedMisses > 0 && !building) || feedMisses >= 3) {
				feedDone = true;
			} else if (feedMisses > 0) {
				feedWaiting = true;
				feedRetry = setTimeout(() => {
					feedRetry = null;
					feedWaiting = false;
				}, FEED_RETRY_MS);
			}
		} catch {
			if (seq === relatedRequest) feedDone = true;
		} finally {
			if (seq === relatedRequest) feedLoading = false;
		}
	}

	function closeVideo() {
		clearVideoSession();
	}

	function stagePlay(video: ExploreVideo, list: ExploreVideo[], label: string) {
		if (!assertOnline()) {
			showToast('Server is reconnecting.', 'error', 3200);
			return;
		}
		void playFromList(video, list, label);
	}

	let concertItems = $derived<StageItem[]>(
		stageRows.concert?.videos.map((video, index) => ({
			video,
			index: index + 1,
			current: video.tidal_id === current?.tidal_id,
		})) ?? []
	);
	let sceneItems = $derived<StageItem[]>(
		stageRows.scene?.videos.map((video) => ({ video, caption: video.why || null })) ?? []
	);
	let stepOutItems = $derived<StageItem[]>(
		stageRows.stepOut.map(({ video, neighbor }) => ({ video, caption: viaLabel(neighbor.source) }))
	);
	let versionItems = $derived<StageItem[]>(
		stageRows.versions?.others.map((version) => ({
			video: version.video,
			caption: versionCaption(version, CUT_LABELS[version.cut]),
		})) ?? []
	);
	let stageEmpty = $derived(
		!stageRows.concert && !stageRows.scene && stageRows.stepOut.length === 0 && !stageRows.versions
	);

	/** Cold open (reload, copied link, a link from outside the section): play
	 *  the video in the URL unless it is already the one playing. */
	function openFromUrl() {
		const params = page.url.searchParams;
		const videoId = Number(params.get('videoId'));
		if (!Number.isFinite(videoId) || videoId <= 0) return;
		if ($videoSession.current?.tidal_id === videoId) return;
		const video: TidalSearchVideo = {
			tidal_id: videoId,
			title: params.get('title') ?? `TIDAL video ${videoId}`,
			duration_ms: null,
			artist_id: Number(params.get('artistId')) || null,
			artist_name: params.get('artistName'),
			album_tidal_id: null,
			artwork_url: null,
			quality: null,
			explicit: null,
			type: 'video',
		};
		const radio = params.get('radio') === '1';
		void playVideo(video, radio
			? { queue: [video], source: 'direct', sourceLabel: `${video.artist_name ?? 'Video'} radio`, autoplay: true, continuous: true, resetRadio: true }
			: { queue: [video], source: 'direct', sourceLabel: null }
		).then((ok) => {
			if (!ok) showToast($videoSession.error ?? 'This video could not be loaded.', 'error', 3200);
		});
	}

	onMount(() => {
		void loadSavedVideos();
		void audioSettings.load();
		openFromUrl();
	});

	// Keep the address on the video that is actually playing (autoplay moves
	// on), so a reload or a copied link reopens it. Shallow: no navigation.
	$effect(() => {
		const id = current?.tidal_id;
		if (id == null || page.url.pathname !== WATCH_PATH) return;
		if (page.url.searchParams.get('videoId') === String(id)) return;
		replaceState(watchUrl(id, {
			title: current?.title,
			artistId: current?.artist_id,
			artistName: current?.artist_name,
		}), page.state);
	});

	// Hand the stage to the persistent dock.
	$effect(() => {
		videoStageAnchor.set(stageAnchor);
		return () => videoStageAnchor.set(null);
	});

	// Stage rows: rebuilt per video, filled in as each endpoint answers; a
	// late answer for a previous video is dropped.
	$effect(() => {
		const item = current;
		const seq = ++stageRequest;
		stageRows = EMPTY_STAGE;
		stageSettled = false;
		versionsOpen = false;
		if (!item) return;
		const timer = setTimeout(() => {
			void loadStage(
				api,
				item,
				(patch) => (stageRows = { ...stageRows, ...patch }),
				() => seq === stageRequest
			).then(() => {
				if (seq === stageRequest) stageSettled = true;
			});
		}, 250);
		return () => clearTimeout(timer);
	});

	// Related row (the fallback): waits for a real selection and for the
	// Stage to come back empty, and a response for a previous video never
	// overwrites the new one.
	$effect(() => {
		const item = current;
		const fallback = useFallback;
		const seq = ++relatedRequest;
		relatedVideos = [];
		resetFeed();
		relatedLoading = fallback && Boolean(item?.artist_id || item?.artist_name);
		if (!fallback || !item || (!item.artist_id && !item.artist_name)) return;
		const controller = new AbortController();
		let attempts = 0;
		let timer: ReturnType<typeof setTimeout>;
		const fetchRelated = async () => {
			try {
				const { items, building } = await api.getRelatedVideos({
					seed_artist_id: item.artist_id,
					seed_artist_name: item.artist_name,
					exclude_video_ids: [item.tidal_id],
				}, controller.signal);
				if (seq !== relatedRequest) return;
				relatedVideos = items.filter((video) => video.tidal_id !== item.tidal_id);
				if (building && ++attempts < 10) {
					timer = setTimeout(() => void fetchRelated(), 3000);
				} else {
					relatedLoading = false;
				}
			} catch {
				if (seq === relatedRequest) relatedLoading = false;
			}
		};
		timer = setTimeout(() => void fetchRelated(), 400);
		return () => { clearTimeout(timer); controller.abort(); ++relatedRequest; };
	});

	// Depth of field: lower rows sit out of focus until they scroll up past
	// the focus line (three quarters down the visible area), and the feed
	// loads more when the grid's end is within a screen of view. Measured on
	// the workspace's scroll (the app scrolls main.workspace, not the window)
	// and whenever the grid resizes or grows.
	const NEAR_END_PX = 900;
	function measureRelated() {
		const grid = gridEl;
		const scroller = grid?.closest('main.workspace') ?? null;
		if (!grid) return;
		columns = gridColumns(getComputedStyle(grid).gridTemplateColumns);
		const view = scroller?.getBoundingClientRect() ?? { top: 0, bottom: window.innerHeight, height: window.innerHeight };
		const focusLine = view.top + view.height * 0.75;
		const tiles = grid.children;
		const firstTop = tiles[0]?.getBoundingClientRect().top ?? 0;
		const rowHeight = tiles[columns]
			? tiles[columns].getBoundingClientRect().top - firstTop
			: (tiles[0]?.getBoundingClientRect().height ?? 1);
		const levels: number[] = [];
		for (let row = 0; row * columns < tiles.length; row += 1) {
			const top = tiles[row * columns].getBoundingClientRect().top;
			levels.push(focusLevel(row, top, focusLine, rowHeight));
		}
		if (levels.join() !== rowLevels.join()) rowLevels = levels;
		nearEnd = grid.getBoundingClientRect().bottom - view.bottom < NEAR_END_PX;
	}

	$effect(() => {
		const grid = gridEl;
		if (!grid) return;
		const scroller = grid.closest('main.workspace') ?? window;
		const observer = new ResizeObserver(() => measureRelated());
		observer.observe(grid);
		scroller.addEventListener('scroll', measureRelated, { passive: true });
		window.addEventListener('resize', measureRelated);
		return () => {
			observer.disconnect();
			scroller.removeEventListener('scroll', measureRelated);
			window.removeEventListener('resize', measureRelated);
		};
	});

	// New tiles: re-measure once they are in the DOM.
	$effect(() => {
		void shownRelated.length;
		void placeholders;
		queueMicrotask(measureRelated);
	});

	// Fill the sharp rows first, then keep going while the end is near.
	$effect(() => {
		if (!useFallback || !current || relatedLoading || feedLoading || feedDone || feedWaiting) return;
		if (needsMore(relatedVideos.length, columns) || nearEnd) void loadMoreRelated();
	});
</script>

<svelte:head>
	<title>{current ? `${current.title} - NOOR` : 'Watch - NOOR'}</title>
</svelte:head>

{#if !hasSession}
	<EmptyState title="Nothing playing" copy="Pick a video from any tab and it plays here.">
		{#snippet actions()}
			<a class="btn btn-glass" href="/videos">Browse videos</a>
		{/snippet}
	</EmptyState>
{:else}
	<div class="watch">
		<div class="watch-main">
			<!-- Placeholder the persistent dock positions its live player over.
			     The <video> lives in VideoDock so it survives navigation. -->
			<div class="stage-anchor" bind:this={stageAnchor}>
				{#if loadingStream && !streamUrl}
					<Skeleton rows={4} label="Loading video" />
				{/if}
			</div>

			{#if current}
				<div class="now-playing">
					<div class="np-copy">
						<h1>{current.title}</h1>
						<div class="meta-line">
							{#if current.artist_name}
								<button
									type="button"
									class="meta-link"
									oncontextmenu={(event) => {
										if (current?.artist_id == null) return;
										event.preventDefault();
										event.stopPropagation();
										openContextMenu(
											event,
											buildArtistMenu({ tidal_id: current.artist_id, name: current.artist_name ?? 'Artist' }, { isLocal: false }),
											current.artist_name ?? undefined
										);
									}}
									onclick={() => {
										if (current?.artist_id != null) void goto(`/tidal/artists/${current.artist_id}`);
									}}
								>{current.artist_name}</button>
							{/if}
							{#if current.duration_ms}
								<span>{formatTrackDuration(current.duration_ms)}</span>
							{/if}
							{#if $videoSession.sourceLabel}
								<span class="meta-source">from {$videoSession.sourceLabel}</span>
							{/if}
						</div>
					</div>
					<div class="np-actions">
						<button
							type="button"
							class="pill-btn"
							class:saved={videoIsSaved}
							aria-pressed={videoIsSaved}
							aria-label={videoIsSaved ? 'Remove video from likes' : 'Save video to likes'}
							disabled={$savingVideo}
							onclick={() => void toggleSavedVideo(current)}
						><span aria-hidden="true">{videoIsSaved ? '♥' : '♡'}</span> {videoIsSaved ? 'Saved' : 'Save'}</button>
						<button type="button" class="pill-btn" aria-label="Stop and close the video" onclick={closeVideo}>Close</button>
					</div>
				</div>
			{/if}
		</div>
	</div>

	{#if current}
		<div class="stage" class:quiet={stageSettled && stageEmpty}>
			<div class="stage-rows">
				{#if stageRows.concert}
					{@const concert = stageRows.concert}
					<StageRow
						title="This concert"
						meta={`${concert.label}, ${concert.video_count} songs${concert.total_s > 0 ? `, ${concertLength(concert.total_s)}` : ''}`}
						items={concertItems}
						onplay={(video) => void playConcert(concert, video.tidal_id)}
					>
						{#snippet actions()}
							<button type="button" class="pill-btn" onclick={() => void playConcert(concert)}>Play concert</button>
						{/snippet}
					</StageRow>
				{/if}

				{#if stageRows.scene}
					{@const scene = stageRows.scene.scene}
					<StageRow
						title={`Same scene: ${scene.name}`}
						meta={scene.channel_no != null ? `Channel ${scene.channel_no}` : ''}
						hue={hueColor(scene.hue)}
						items={sceneItems}
						onplay={(video) => stagePlay(video, stageRows.scene?.videos ?? [], scene.name)}
					>
						{#snippet actions()}
							<a class="pill-btn" href={`/videos/archive?scene=${scene.id}`}>Open in Archive</a>
						{/snippet}
					</StageRow>
				{/if}

				{#if stepOutItems.length > 0}
					<StageRow
						title="One step out"
						meta={`Next to ${current.artist_name ?? 'this artist'}, new to you`}
						items={stepOutItems}
						onplay={(video) => stagePlay(video, stageRows.stepOut.map((pick) => pick.video), 'One step out')}
					/>
				{/if}

				{#if stageRows.versions}
					{@const song = stageRows.versions.song}
					<StageRow
						title="Other cuts of this song"
						meta={song.song_year ? `${song.title}, ${song.song_year}` : song.title}
						items={versionItems}
						onplay={(video) => {
							const version = song.versions.find((item) => item.video.tidal_id === video.tidal_id);
							if (version) void playVersion(version, stageRows.versions?.others ?? [], song.title);
						}}
					>
						{#snippet actions()}
							<button type="button" class="pill-btn" onclick={() => (versionsOpen = true)}>All {song.versions.length} cuts</button>
						{/snippet}
					</StageRow>
				{/if}

				{#if !stageSettled && stageEmpty}
					<div class="stage-skeleton" aria-hidden="true">
						<div class="sk-heading"></div>
						<div class="sk-cards">
							{#each Array(4) as _, i (i)}<div class="sk-card"></div>{/each}
						</div>
					</div>
				{/if}
			</div>
			<!-- The Atlas mini map mounts here. Empty, it takes no space. -->
			<div class="atlas-minimap-slot"></div>
		</div>
	{/if}

	{#if current && useFallback && (relatedLoading || feedLoading || relatedVideos.length > 0)}
		<section class="related" aria-label="Related videos">
			<div class="section-heading">
				<h2>Related to {current.artist_name ?? 'this video'}</h2>
			</div>
			{#if relatedVideos.length > 0 || feedLoading}
				<!-- Two full rows in focus, then the extension softens row by row;
				     hover or focus brings a tile back into focus. -->
				<div class="video-grid" bind:this={gridEl}>
					{#each shownRelated as video, index (video.tidal_id)}
						<div class="related-card" data-focus={tileFocus(index)}>
							<VideoCard {video} onSelect={(item) => !('id' in item) && void playRelated(item)} />
							{#if video.why}<span class="related-why">{video.why}</span>{/if}
						</div>
					{/each}
					{#each Array(placeholders) as _, offset (offset)}
						<div class="related-card placeholder" data-focus={tileFocus(shownRelated.length + offset)} aria-hidden="true">
							<div class="placeholder-frame"></div>
						</div>
					{/each}
				</div>
				{#if feedDone && shownRelated.length > FOCUS_ROWS * columns}
					<p class="related-end">End of the line</p>
				{/if}
			{:else}
				<p class="related-loading">Finding a few connected videos...</p>
			{/if}
		</section>
	{/if}

	{#if versionsOpen && stageRows.versions}
		{@const song = stageRows.versions.song}
		<VersionsDrawer
			song={{
				song_key: song.song_key,
				title: song.title,
				artist_id: song.artist_id,
				artist_name: song.artist_name,
				artwork_url: song.versions[0]?.video.artwork_url ?? null,
				cut_count: song.versions.length,
			}}
			onclose={() => (versionsOpen = false)}
		/>
	{/if}
{/if}

<style>
	/* One column: the player, what is playing, then the related row.
	   Borderless, like the rest of the video section. The width cap keeps a
	   16:9 player short enough that its title and actions stay on screen,
	   including above the bottom player bar when that layout is on. */
	.watch {
		width: 100%;
		max-width: max(480px, calc((100dvh - var(--bottom-player-height, 0px) - 300px) * 16 / 9));
		margin: 0 auto;
		animation: watch-in var(--motion-slow) both;
	}

	/* Fade only: the player glides into the stage, so a slide here would
	   move the stage under it and nudge the video a second time. */
	@keyframes watch-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.watch-main {
		display: grid;
		gap: var(--space-4);
		min-width: 0;
	}

	/* An empty 16/9 frame the fixed dock sits on top of. Dark fill so it
	   reads as a video surface in the frame before the dock paints. */
	.stage-anchor {
		position: relative;
		width: 100%;
		aspect-ratio: 16 / 9;
		border-radius: 8px;
		background: #030305;
		display: grid;
		align-items: center;
		overflow: hidden;
	}

	.now-playing {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
		min-width: 0;
	}

	.np-copy {
		display: grid;
		gap: 6px;
		min-width: 0;
	}

	.np-copy h1 {
		margin: 0;
		font-size: var(--font-size-xl);
		line-height: var(--line-height-tight);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		line-clamp: 2;
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

	.meta-link {
		color: var(--accent-strong);
		font-weight: var(--font-weight-bold);
	}

	.meta-source {
		color: var(--text-secondary);
	}

	.np-actions {
		display: flex;
		gap: var(--space-2);
		flex: 0 0 auto;
	}

	.pill-btn {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: var(--control-h);
		padding: 0 14px;
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
	}

	.pill-btn:hover,
	.pill-btn:focus-visible {
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.pill-btn.saved {
		color: var(--accent-strong);
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}

	.pill-btn:disabled {
		opacity: 0.6;
	}

	/* The Stage rows: full content width under the player, each row a rail.
	   The mini map slot sits beside them on wide windows once it has content. */
	.stage {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: start;
		margin-top: 28px;
	}

	.stage.quiet {
		margin-top: 0;
	}

	.stage-rows {
		display: grid;
		gap: var(--space-6);
		min-width: 0;
	}

	.atlas-minimap-slot:not(:empty) {
		position: sticky;
		top: var(--space-4);
		width: 240px;
		margin-left: var(--space-6);
	}

	a.pill-btn {
		text-decoration: none;
	}

	.stage-skeleton {
		display: grid;
		gap: var(--group-gap);
		animation: stage-skeleton-in var(--motion-fast) 150ms both;
	}

	.sk-heading {
		width: 180px;
		height: 18px;
		border-radius: 6px;
		background: var(--bg-surface);
	}

	.sk-cards {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 14px;
	}

	.sk-card {
		aspect-ratio: 16 / 9;
		border-radius: var(--radius-md);
		background: var(--bg-surface);
		animation: placeholder-pulse 1.4s ease-in-out infinite;
	}

	@keyframes stage-skeleton-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.related {
		display: grid;
		gap: 14px;
		margin-top: 28px;
		padding-top: 24px;
		border-top: 1px solid var(--border-subtle);
	}

	.section-heading {
		display: flex;
		align-items: baseline;
		gap: 12px;
	}


	.section-heading h2 {
		margin: 0;
		color: var(--text-primary);
		font-size: var(--font-size-lg);
	}

	.video-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 14px;
	}

	.related-card {
		min-width: 0;
		display: grid;
		align-content: start;
		gap: 5px;
	}

	/* Out of focus below the fold: a soft blur and dim that deepen for
	   three rows, then hold, and clear as the row scrolls into view. */
	.related-card {
		transition: filter var(--motion-slow), opacity var(--motion-slow);
	}
	.related-card[data-focus='1'] { filter: blur(0.6px); opacity: 0.82; }
	.related-card[data-focus='2'] { filter: blur(1.1px); opacity: 0.68; }
	.related-card[data-focus='3'] { filter: blur(1.6px); opacity: 0.56; }
	.related-card:hover,
	.related-card:focus-within {
		filter: none;
		opacity: 1;
	}

	.placeholder-frame {
		aspect-ratio: 16 / 9;
		border-radius: 8px;
		background: var(--bg-raised);
		animation: placeholder-pulse 1.4s ease-in-out infinite;
	}
	@keyframes placeholder-pulse {
		50% { opacity: 0.55; }
	}

	.related-end {
		margin: 4px 0 0;
		text-align: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.related-why,
	.related-loading {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.related-loading {
		margin: 0;
	}

	@media (max-width: 1100px) {
		.stage {
			grid-template-columns: minmax(0, 1fr);
		}

		.atlas-minimap-slot:not(:empty) {
			position: static;
			width: auto;
			margin: var(--space-5) 0 0;
		}
	}

	@media (max-width: 620px) {
		.now-playing {
			flex-direction: column;
		}

		.sk-cards {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}

		.video-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
