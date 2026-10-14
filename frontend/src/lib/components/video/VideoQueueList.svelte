<!--
	Up-next rows of the video session, shared by the shell's video queue panel
	and the fullscreen player's queue. Click plays, right-click (or the menu
	key) opens the shared video menu; `actions` adds inline Move next and Remove.
	`inverted` stacks the rows upward from the bottom (next nearest the
	controls), for the fullscreen queue that grows out of its button.

	Rows fold in and out: a played video folds away and the rest close up
	under it, a refill unfolds at the far end. Keys are the video id plus its
	occurrence, so rows keep their identity as the head of the queue moves.
-->
<script lang="ts">
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import {
		playQueuedVideo,
		playVideoNext,
		removeVideoFromQueue,
		type VideoSessionItem,
	} from '$lib/stores/video_session';
	import { createArtworkFallback } from '$lib/utils/artwork_fallback.svelte';
	import { formatTrackDuration } from '$lib/utils/format';
	import { prefersReducedMotion } from '$lib/stores/motion';
	import { cubicOut } from 'svelte/easing';

	interface Props {
		videos: VideoSessionItem[];
		actions?: boolean;
		inverted?: boolean;
	}

	let { videos, actions = false, inverted = false }: Props = $props();

	const FOLD_IN_MS = 220;
	const FOLD_OUT_MS = 180;

	/** Stable keys: a video queued twice gets its second occurrence's key. */
	let rows = $derived.by(() => {
		const seen = new Map<number, number>();
		return videos.map((video) => {
			const n = seen.get(video.tidal_id) ?? 0;
			seen.set(video.tidal_id, n + 1);
			return { video, key: `${video.tidal_id}:${n}` };
		});
	});

	/** Height, opacity and a short slide toward where the row comes from. */
	function fold(node: HTMLElement, { duration, from }: { duration: number; from: number }) {
		if (prefersReducedMotion()) return { duration: 0 };
		const height = node.offsetHeight;
		return {
			duration,
			easing: cubicOut,
			css: (t: number) =>
				`overflow: hidden; height: ${t * height}px; opacity: ${t}; transform: translateY(${(1 - t) * from}px);`,
		};
	}

	// The fullscreen list fades out at its far end when it scrolls.
	let listEl = $state<HTMLDivElement | null>(null);
	let overflowing = $state(false);
	$effect(() => {
		const el = listEl;
		if (!el) return;
		const measure = () => (overflowing = el.scrollHeight > el.clientHeight + 1);
		const observer = new ResizeObserver(measure);
		observer.observe(el);
		void rows.length;
		measure();
		return () => observer.disconnect();
	});

	const artwork = createArtworkFallback();
	const artworkCandidate = artwork.candidate;
	const markArtworkFailed = artwork.markFailed;

	function openVideoQueueMenu(video: VideoSessionItem, event: MouseEvent) {
		openContextMenu(event, buildVideoMenu(video, { inQueue: true }), video.title);
	}

	function videoQueueKeydown(video: VideoSessionItem, event: KeyboardEvent) {
		if (event.key !== 'ContextMenu' && !(event.shiftKey && event.key === 'F10')) return;
		event.preventDefault();
		event.stopPropagation();
		openMenuAtElement(event.currentTarget as HTMLElement, buildVideoMenu(video, { inQueue: true }), video.title);
	}
</script>

<div class="video-queue-list" class:inverted class:overflowing bind:this={listEl}>
	{#each rows as { video, key }, i (key)}
		{@const videoArt = artworkCandidate(video.artwork_url, 320)}
		<div
			class="video-queue-row"
			class:next={inverted && i === 0}
			style:--i={Math.min(i, 10)}
			in:fold={{ duration: FOLD_IN_MS, from: inverted ? -8 : 8 }}
			out:fold={{ duration: FOLD_OUT_MS, from: inverted ? 10 : -10 }}
		>
			<button
				type="button"
				class="video-queue-play"
				onclick={() => void playQueuedVideo(video.tidal_id)}
				oncontextmenu={(event) => openVideoQueueMenu(video, event)}
				onkeydown={(event) => videoQueueKeydown(video, event)}
				aria-label={`Play ${video.title}`}
			>
				{#if videoArt}
					<img
						class="video-queue-art"
						src={videoArt}
						alt=""
						onerror={() => markArtworkFailed(videoArt)}
					/>
				{:else}
					<span class="video-queue-art placeholder">&#x25B6;</span>
				{/if}
				<span class="video-queue-copy">
					<strong>{video.title}</strong>
					<span>{video.artist_name ?? 'Unknown artist'}</span>
				</span>
				<span class="video-queue-time">{formatTrackDuration(video.duration_ms ?? 0)}</span>
			</button>
			{#if actions}
				<button
					type="button"
					class="row-btn"
					onclick={() => playVideoNext(video)}
					disabled={i === 0}
					aria-label={`Play ${video.title} next`}
					title="Play next"
				>&#x2934;</button>
				<button
					type="button"
					class="row-btn"
					onclick={() => removeVideoFromQueue(video.tidal_id)}
					aria-label={`Remove ${video.title} from queue`}
					title="Remove from queue"
				>&#x2715;</button>
			{/if}
		</div>
	{/each}
</div>

<style>
	.video-queue-list {
		display: flex;
		flex-direction: column;
		gap: 6px;
		min-height: 0;
		overflow-y: auto;
		padding-right: 2px;
	}

	.video-queue-row {
		position: relative;
		display: flex;
		flex: none;
		align-items: center;
		gap: 2px;
		min-width: 0;
		padding-right: 4px;
		border: 1px solid transparent;
		border-radius: 8px;
	}

	/* Upward: the next video sits at the bottom, nearest the controls, and
	   the list starts scrolled there. The far end fades when it scrolls. */
	.video-queue-list.inverted {
		flex-direction: column-reverse;
	}

	.video-queue-list.inverted.overflowing {
		mask-image: linear-gradient(to bottom, transparent, #000 40px);
	}

	/* The next video: a short accent bar at its left edge. */
	.video-queue-row.next::before {
		content: '';
		position: absolute;
		left: 1px;
		top: 50%;
		width: 3px;
		height: 22px;
		border-radius: 2px;
		background: var(--accent);
		transform: translateY(-50%);
	}

	/* Opening the fullscreen queue: rows rise one after another from the
	   bottom, next first. */
	.video-queue-list.inverted .video-queue-row {
		animation: row-rise var(--motion-base) both;
		animation-delay: calc(var(--i) * 24ms);
	}

	@keyframes row-rise {
		from {
			opacity: 0;
			transform: translateY(10px);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.video-queue-list.inverted .video-queue-row {
			animation: none;
		}
	}

	:global(:root[data-motion='reduce']) .video-queue-list.inverted .video-queue-row {
		animation: none;
	}

	.video-queue-row:hover,
	.video-queue-row:focus-within {
		background: color-mix(in srgb, var(--instrument-surface) 78%, transparent);
		border-color: var(--border-subtle);
	}

	.video-queue-row:hover .row-btn,
	.video-queue-row:focus-within .row-btn {
		opacity: 1;
	}

	.video-queue-play {
		flex: 1;
		min-width: 0;
		display: grid;
		grid-template-columns: 48px minmax(0, 1fr) auto;
		align-items: center;
		gap: 10px;
		border: none;
		border-radius: 8px;
		background: transparent;
		color: inherit;
		font: inherit;
		text-align: left;
		padding: 7px;
		cursor: pointer;
	}

	.video-queue-play:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.video-queue-art {
		width: 48px;
		aspect-ratio: 16 / 9;
		border-radius: 4px;
		object-fit: cover;
		background: color-mix(in srgb, var(--instrument-surface-strong) 85%, transparent);
	}

	.video-queue-art.placeholder {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.video-queue-copy {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.video-queue-copy strong,
	.video-queue-copy span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.video-queue-copy strong {
		font-size: var(--font-size-sm);
		color: var(--text-primary);
	}

	.video-queue-copy span,
	.video-queue-time {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}

	.video-queue-time {
		font-variant-numeric: tabular-nums;
	}
</style>
