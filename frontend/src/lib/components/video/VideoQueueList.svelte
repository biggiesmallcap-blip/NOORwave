<!--
	Up-next rows of the video session, shared by the shell's video queue panel
	and the fullscreen player's queue. Click plays, right-click (or the menu
	key) opens the shared video menu; `actions` adds inline Move next and Remove.
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

	interface Props {
		videos: VideoSessionItem[];
		actions?: boolean;
	}

	let { videos, actions = false }: Props = $props();

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

<div class="video-queue-list">
	{#each videos as video, i (`video-${video.tidal_id}-${i}`)}
		{@const videoArt = artworkCandidate(video.artwork_url, 320)}
		<div class="video-queue-row">
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
		display: flex;
		align-items: center;
		gap: 2px;
		min-width: 0;
		padding-right: 4px;
		border: 1px solid transparent;
		border-radius: 8px;
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
