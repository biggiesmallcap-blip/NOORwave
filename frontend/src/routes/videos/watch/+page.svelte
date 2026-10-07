<script lang="ts">
	import { onMount } from 'svelte';
	import { goto, replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import { api, type TidalSearchVideo } from '$lib/api/client';
	import VideoCard from '$lib/components/video/VideoCard.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { assertOnline } from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import { audioSettings } from '$lib/stores/audio_settings';
	import { formatTrackDuration } from '$lib/utils/format';
	import {
		clearVideoSession,
		playQueuedVideo,
		playVideo,
		videoSession,
		videoSessionUpcoming,
		videoStageAnchor,
	} from '$lib/stores/video_session';
	import { WATCH_PATH } from '$lib/video/section';

	// The watch page: the one place the big player lives. The persistent dock
	// positions the live <video> over this page's stage; everything else here
	// is about what is playing - who it is, what comes next, where to go from
	// it. Back returns to the tab the video was picked on.

	const UP_NEXT_MAX = 12;

	let stageAnchor = $state<HTMLDivElement | null>(null);
	let savedVideoIds = $state<Set<number>>(new Set());
	let savingVideo = $state(false);
	let savedVideoChanges = 0;
	let relatedVideos = $state<(TidalSearchVideo & { why?: string })[]>([]);
	let relatedLoading = $state(false);
	let relatedRequest = 0;

	let current = $derived($videoSession.current);
	let streamUrl = $derived($videoSession.streamUrl);
	let loadingStream = $derived($videoSession.loading);
	let hasSession = $derived(Boolean(current || streamUrl || loadingStream));
	let videoIsSaved = $derived(current ? savedVideoIds.has(current.tidal_id) : false);
	let upNext = $derived($videoSessionUpcoming.slice(0, UP_NEXT_MAX));

	async function toggleSavedVideo() {
		const item = current;
		if (!item || savingVideo) return;
		savingVideo = true;
		const saved = !savedVideoIds.has(item.tidal_id);
		try {
			const result = await api.setVideoSaved(item, saved);
			if (!result.ok) throw new Error('Could not save video.');
			const next = new Set(savedVideoIds);
			if (saved) next.add(item.tidal_id);
			else next.delete(item.tidal_id);
			savedVideoChanges += 1;
			savedVideoIds = next;
			showToast(saved ? 'Saved to liked videos.' : 'Removed from liked videos.');
		} catch (err) {
			showToast(err instanceof Error ? err.message : 'Could not update liked videos.', 'error');
		} finally {
			savingVideo = false;
		}
	}

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

	function closeVideo() {
		clearVideoSession();
	}

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
		const changesAtLoad = savedVideoChanges;
		void api.getSavedVideos().then(({ items }) => {
			if (savedVideoChanges === changesAtLoad) savedVideoIds = new Set(items.map((item) => item.tidal_id));
		}).catch(() => {});
		void audioSettings.load();
		openFromUrl();
	});

	// Keep the address on the video that is actually playing (autoplay moves
	// on), so a reload or a copied link reopens it. Shallow: no navigation.
	$effect(() => {
		const id = current?.tidal_id;
		if (id == null || page.url.pathname !== WATCH_PATH) return;
		if (page.url.searchParams.get('videoId') === String(id)) return;
		replaceState(`${WATCH_PATH}?videoId=${id}`, page.state);
	});

	// Hand the stage to the persistent dock.
	$effect(() => {
		videoStageAnchor.set(stageAnchor);
		return () => videoStageAnchor.set(null);
	});

	// Related row: waits for a real selection, and a response for a previous
	// video never overwrites the new one.
	$effect(() => {
		const item = current;
		const seq = ++relatedRequest;
		relatedVideos = [];
		relatedLoading = Boolean(item?.artist_id || item?.artist_name);
		if (!item || (!item.artist_id && !item.artist_name)) return;
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
							disabled={savingVideo}
							onclick={() => void toggleSavedVideo()}
						><span aria-hidden="true">{videoIsSaved ? '♥' : '♡'}</span> {videoIsSaved ? 'Saved' : 'Save'}</button>
						<button type="button" class="pill-btn" aria-label="Stop and close the video" onclick={closeVideo}>Close</button>
					</div>
				</div>
			{/if}
		</div>

		<aside class="up-next" aria-label="Up next">
			<div class="side-heading">
				<p class="eyebrow">Up next</p>
				{#if $videoSession.continuous && $videoSession.autoplay}
					<span class="side-note">Radio keeps going</span>
				{/if}
			</div>
			{#if upNext.length > 0}
				<ol class="up-next-list">
					{#each upNext as item (item.tidal_id)}
						<li>
							<button
								type="button"
								class="up-next-row"
								onclick={() => void playQueuedVideo(item.tidal_id)}
								oncontextmenu={(event) => {
									event.preventDefault();
									event.stopPropagation();
									openContextMenu(event, buildVideoMenu(item, { inQueue: true }), item.title);
								}}
							>
								<span class="thumb">
									<ArtworkImage src={item.artwork_url ?? null} size={160} fallbackText="VID" decorative={true} fadeIn={true} />
								</span>
								<span class="row-copy">
									<span class="row-title">{item.title}</span>
									<span class="row-sub">{item.artist_name ?? 'TIDAL video'}{item.duration_ms ? ` . ${formatTrackDuration(item.duration_ms)}` : ''}</span>
								</span>
							</button>
						</li>
					{/each}
				</ol>
			{:else}
				<p class="side-note">Nothing queued after this one.</p>
			{/if}
		</aside>
	</div>

	{#if current && (relatedLoading || relatedVideos.length > 0)}
		<section class="related" aria-label="Related videos">
			<div class="section-heading">
				<p class="eyebrow">Keep exploring</p>
				<h2>Related to {current.artist_name ?? 'this video'}</h2>
			</div>
			{#if relatedVideos.length > 0}
				<div class="video-grid">
					{#each relatedVideos as video (video.tidal_id)}
						<div class="related-card">
							<VideoCard {video} onSelect={(item) => !('id' in item) && void playRelated(item)} />
							{#if video.why}<span class="related-why">{video.why}</span>{/if}
						</div>
					{/each}
				</div>
			{:else}
				<p class="related-loading">Finding a few connected videos...</p>
			{/if}
		</section>
	{/if}
{/if}

<style>
	/* Player on the left, the queue beside it; the related row runs full
	   width below. Borderless, like the rest of the video section. */
	.watch {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(260px, 340px);
		gap: 24px;
		align-items: start;
		animation: watch-in 0.28s cubic-bezier(0.22, 0.7, 0.2, 1) both;
	}

	@keyframes watch-in {
		from { opacity: 0; transform: translateY(8px); }
		to { opacity: 1; transform: none; }
	}

	.watch-main {
		display: grid;
		gap: var(--space-4);
		min-width: 0;
	}

	/* An empty 16/9 frame the fixed dock sits on top of. Dark fill so it
	   reads as a video surface in the frame before the dock paints. */
	.stage-anchor {
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

	.up-next {
		display: grid;
		gap: var(--space-3);
		min-width: 0;
	}

	.side-heading {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
	}

	.side-heading .eyebrow {
		margin: 0;
	}

	.side-note {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.up-next-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 4px;
	}

	.up-next-row {
		display: grid;
		grid-template-columns: 112px minmax(0, 1fr);
		gap: var(--space-3);
		align-items: center;
		width: 100%;
		padding: 6px;
		border: 0;
		border-radius: 8px;
		background: transparent;
		text-align: left;
		cursor: pointer;
		transition: background var(--motion-fast);
	}

	.up-next-row:hover,
	.up-next-row:focus-visible {
		background: var(--bg-hover);
	}

	.thumb {
		display: block;
		aspect-ratio: 16 / 9;
		border-radius: 6px;
		overflow: hidden;
		background: var(--bg-raised);
	}

	.thumb :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.row-copy {
		display: grid;
		gap: 2px;
		min-width: 0;
	}

	.row-title,
	.row-sub {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}

	.row-title {
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.row-sub {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
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

	.section-heading .eyebrow {
		margin: 0;
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

	.related-why,
	.related-loading {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.related-loading {
		margin: 0;
	}

	@media (max-width: 980px) {
		.watch {
			grid-template-columns: minmax(0, 1fr);
		}
	}

	@media (max-width: 620px) {
		.now-playing {
			flex-direction: column;
		}

		.video-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
