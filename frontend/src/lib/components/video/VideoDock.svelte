<script lang="ts">
	import { onDestroy } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '$lib/api/client';
	import VideoPlayer from '$lib/components/video/VideoPlayer.svelte';
	import { audioSettings } from '$lib/stores/audio_settings';
	import { isPlaying } from '$lib/stores/player';
	import {
		advanceVideo,
		clearVideoSession,
		refreshVideoStream,
		setVideoBrowseMode,
		videoBrowseMode,
		videoSession,
		videoSessionUpcoming,
		videoStageAnchor,
		type PreloadedVideoStream,
	} from '$lib/stores/video_session';

	// The dock renders a single VideoPlayer that never unmounts while a session
	// is active, so audio keeps playing across route changes. On /videos it is
	// positioned over the route's placeholder (full); elsewhere it docks into
	// the corner as a small moving thumbnail (mini). Browse mode is the third
	// case: still on /videos, but the listener stepped back to the picks, so
	// the route withdraws its stage and the player takes the corner instead.
	let onVideosRoute = $derived(page.url.pathname.startsWith('/videos'));
	let active = $derived($videoSession.active && Boolean($videoSession.streamUrl));
	let mode = $derived(onVideosRoute && !$videoBrowseMode ? 'full' : 'mini');

	let qualityMode = $derived($audioSettings.settings?.video_quality_mode ?? 'MAX');
	let upNext = $derived($videoSessionUpcoming[0] ?? null);
	let hasNext = $derived($videoSessionUpcoming.length > 0);

	// ─── Full-mode rect tracking ─────────────────────────────────────────────
	let rect = $state<{ top: number; left: number; width: number; height: number } | null>(null);
	let rafId = 0;

	function trackAnchor() {
		const anchor = $videoStageAnchor;
		if (active && mode === 'full' && anchor) {
			const r = anchor.getBoundingClientRect();
			rect = { top: r.top, left: r.left, width: r.width, height: r.height };
		} else if (rect !== null) {
			rect = null;
		}
		rafId = requestAnimationFrame(trackAnchor);
	}
	rafId = requestAnimationFrame(trackAnchor);

	// ─── Prefetch the next stream for gapless autoplay ───────────────────────
	let prefetched = $state<PreloadedVideoStream & { videoId: number } | null>(null);
	let prefetchSeq = 0;

	$effect(() => {
		const next = upNext;
		const autoplay = $videoSession.autoplay;
		if (!autoplay || !next) {
			prefetchSeq += 1;
			prefetched = null;
			return;
		}
		if (prefetched?.videoId === next.tidal_id) return;
		const seq = ++prefetchSeq;
		void api
			.getTidalVideoStream(next.tidal_id)
			.then((stream) => {
				if (seq !== prefetchSeq) return;
				prefetched = { videoId: next.tidal_id, url: stream.hls_url, expiresAt: stream.expires_at };
			})
			.catch(() => {
				if (seq === prefetchSeq) prefetched = null;
			});
	});

	// ─── Music takes the device back: starting music stops the video ─────────
	let wasPlayingAudio = $isPlaying;
	$effect(() => {
		const nowPlaying = $isPlaying;
		if (nowPlaying && !wasPlayingAudio && $videoSession.active) {
			clearVideoSession();
		}
		wasPlayingAudio = nowPlaying;
	});

	async function handleEnded() {
		const preloaded = prefetched?.videoId === upNext?.tidal_id ? prefetched : null;
		const advanced = await advanceVideo({ preloaded });
		if (!advanced) videoSession.setAutoplay(false);
	}

	function handlePlay() {
		// Free the WASAPI exclusive endpoint so the WebView can output the
		// video's audio in shared mode. No-op server-side when exclusive is off.
		void api.releaseExclusivePlayback();
	}

	function toggleAutoplay() {
		videoSession.setAutoplay(!$videoSession.autoplay);
	}

	function returnToVideos() {
		// Also the way out of browse mode: on /videos this hands the hero slot
		// back to the player, elsewhere it navigates there first.
		setVideoBrowseMode(false);
		if (!onVideosRoute) void goto('/videos');
	}

	function closeDock() {
		clearVideoSession();
	}

	onDestroy(() => {
		if (rafId) cancelAnimationFrame(rafId);
	});
</script>

{#if active}
	<div
		class="video-dock"
		class:mini={mode === 'mini'}
		class:full={mode === 'full'}
		class:positioned={mode === 'full' && rect !== null}
		style:top={mode === 'full' && rect ? `${rect.top}px` : null}
		style:left={mode === 'full' && rect ? `${rect.left}px` : null}
		style:width={mode === 'full' && rect ? `${rect.width}px` : null}
		style:height={mode === 'full' && rect ? `${rect.height}px` : null}
	>
		<VideoPlayer
			src={$videoSession.streamUrl!}
			poster={$videoSession.current?.artwork_url}
			title={$videoSession.current?.title ?? 'Video'}
			artist={$videoSession.current?.artist_name ?? null}
			qualityMode={qualityMode}
			variant={mode === 'mini' ? 'mini' : 'full'}
			autoplayNext={$videoSession.autoplay}
			hasNext={hasNext}
			upNextTitle={upNext?.title ?? null}
			upNextArtist={upNext?.artist_name ?? null}
			onEnded={handleEnded}
			onToggleAutoplay={toggleAutoplay}
			onPlay={handlePlay}
			refreshStream={refreshVideoStream}
		/>

		{#if mode === 'mini'}
			<div class="mini-chrome">
				<button
					type="button"
					class="mini-btn"
					title={onVideosRoute ? 'Back to the player' : 'Back to videos'}
					onclick={returnToVideos}>⤢</button
				>
				<button type="button" class="mini-btn" title="Close video" onclick={closeDock}>✕</button>
			</div>
		{/if}
	</div>
{/if}

<style>
	.video-dock {
		z-index: 60;
	}

	/* Full mode: a fixed box copied each frame onto the route's placeholder rect,
	   so it reads as inline while actually persisting across navigation. Hidden
	   until the first rect lands to avoid a flash at (0,0). */
	.video-dock.full {
		position: fixed;
		opacity: 0;
		pointer-events: none;
	}

	.video-dock.full.positioned {
		opacity: 1;
		pointer-events: auto;
	}

	/* Mini mode: small docked moving thumbnail in the bottom-right corner. */
	.video-dock.mini {
		position: fixed;
		right: 18px;
		bottom: calc(18px + var(--safe-bottom, 0px));
		width: clamp(248px, 24vw, 340px);
		aspect-ratio: 16 / 9;
		border-radius: 10px;
		overflow: hidden;
		box-shadow: 0 18px 50px rgba(0, 0, 0, 0.5);
		border: 1px solid rgba(255, 255, 255, 0.12);
		animation: dock-in 0.22s ease both;
	}

	.mini-chrome {
		position: absolute;
		top: 6px;
		right: 6px;
		z-index: 3;
		display: flex;
		gap: 5px;
		opacity: 0;
		transition: opacity 0.16s ease;
	}

	.video-dock.mini:hover .mini-chrome {
		opacity: 1;
	}

	.mini-btn {
		width: 26px;
		height: 26px;
		border-radius: 999px;
		display: grid;
		place-items: center;
		background: rgba(10, 10, 14, 0.72);
		color: rgba(255, 255, 255, 0.92);
		border: 1px solid rgba(255, 255, 255, 0.16);
		font-size: var(--font-size-xs);
	}

	.mini-btn:hover {
		background: rgba(20, 20, 26, 0.92);
	}

	@keyframes dock-in {
		from {
			opacity: 0;
			transform: translateY(14px) scale(0.96);
		}
		to {
			opacity: 1;
			transform: translateY(0) scale(1);
		}
	}

	@media (max-width: 720px) {
		.video-dock.mini {
			right: 10px;
			bottom: calc(76px + var(--safe-bottom, 0px));
			width: min(64vw, 240px);
		}
	}
</style>
