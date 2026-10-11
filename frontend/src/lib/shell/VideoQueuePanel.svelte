<!--
	Contents of the video-session side panel (or bottom bar): current video, radio
	controls and the up-next list. The artwork slot doubles as the video dock stage.
-->
<script lang="ts">
	import PlayerLayoutSelect from '$lib/shell/PlayerLayoutSelect.svelte';
	import QueueEmpty from '$lib/shell/QueueEmpty.svelte';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import type { EffectivePlayerLayout } from '$lib/stores/playerLayout';
	import {
		playQueuedVideo,
		clearVideoSession,
		videoPanelAnchor,
		videoSession,
		videoSessionUpcoming,
		type VideoSessionItem,
	} from '$lib/stores/video_session';
	import { createArtworkFallback } from '$lib/utils/artwork_fallback.svelte';
	import { formatTrackDuration } from '$lib/utils/format';

	interface Props {
		layout: EffectivePlayerLayout;
		/** Bottom layout: the up-next list opens as a drawer. */
		drawerOpen: boolean;
	}

	let { layout, drawerOpen = $bindable() }: Props = $props();

	const artwork = createArtworkFallback();
	const artworkCandidate = artwork.candidate;
	const markArtworkFailed = artwork.markFailed;
	let currentVideoArtwork = $derived(artworkCandidate($videoSession.current?.artwork_url, 320));

	function openVideoQueueMenu(video: VideoSessionItem, event: MouseEvent) {
		openContextMenu(event, buildVideoMenu(video, { inQueue: true }), video.title);
	}

	function videoQueueKeydown(video: VideoSessionItem, event: KeyboardEvent) {
		if (event.key !== 'ContextMenu' && !(event.shiftKey && event.key === 'F10')) return;
		event.preventDefault();
		event.stopPropagation();
		openMenuAtElement(event.currentTarget as HTMLElement, buildVideoMenu(video, { inQueue: true }), video.title);
	}

	function formatVideoSourceLabel(source: string, label: string | null): string {
		if (source === 'mix') return label ?? 'Video mix';
		if (source === 'search') return label ? `Search: ${label}` : 'Video search';
		if (source === 'direct') return 'Direct video';
		return 'Video session';
	}

	// The panel's artwork slot doubles as a stage: the video dock plays there
	// while the panel is open (see VideoDock). Cleared when the panel unmounts.
	let videoPanelArtWrap = $state<HTMLElement | null>(null);
	$effect(() => {
		videoPanelAnchor.set(videoPanelArtWrap);
		return () => videoPanelAnchor.set(null);
	});
</script>

<div class="video-panel-top">
	<div class="video-panel-heading"><p class="eyebrow">Video session</p><PlayerLayoutSelect effective={layout} /></div>
	<div class="video-panel-art-wrap" bind:this={videoPanelArtWrap}>
		{#if currentVideoArtwork}
			<img
				class="video-panel-art"
				src={currentVideoArtwork}
				alt=""
				onerror={() => markArtworkFailed(currentVideoArtwork)}
			/>
		{:else}
			<div class="video-panel-art placeholder">▶</div>
		{/if}
	</div>
	<div class="video-panel-copy">
		<strong>{$videoSession.current?.title ?? 'Video queue'}</strong>
		<span>{$videoSession.current?.artist_name ?? formatVideoSourceLabel($videoSession.source, $videoSession.sourceLabel)}</span>
	</div>
	<button id="video-queue-trigger" class="video-queue-trigger" type="button" aria-label="Video queue, {$videoSessionUpcoming.length} up next" aria-expanded={drawerOpen} onclick={() => { drawerOpen = !drawerOpen; }}>Queue {$videoSessionUpcoming.length}</button>
	<div class="video-panel-actions">
		<button
			class="video-panel-chip"
			class:active={$videoSession.continuous}
			type="button"
			aria-pressed={$videoSession.continuous}
			disabled={!$videoSession.active}
			onclick={() => $videoSession.continuous ? videoSession.stopRadio() : videoSession.startRadio()}
		>
			{$videoSession.continuous ? 'Stop radio' : 'Start radio'}
		</button>
		<button
			class="video-panel-chip"
			class:active={$videoSession.autoplay}
			type="button"
			aria-pressed={$videoSession.autoplay}
			onclick={() => videoSession.setAutoplay(!$videoSession.autoplay)}
		>
			› {$videoSession.autoplay ? 'On' : 'Autoplay'}
		</button>
	</div>
	<p class="video-panel-source" aria-live="polite">
		{$videoSession.radioIssue ?? ($videoSession.continuous
			? (!$videoSession.autoplay ? 'Radio paused. Turn on autoplay to resume.'
				: $videoSession.radioSearching ? `Checking ${$videoSession.radioSeedArtistName ?? 'this artist'} and related artists…`
				: $videoSession.radioDiscoveryMessage ?? 'Radio checks related artists as the queue plays.')
			: 'Radio adds new videos beyond this queue.')}
	</p>
	{#if $videoSession.continuous && $videoSession.radioHits.length > 0}
		<div class="video-radio-hits" aria-label="Recent radio discoveries">
			{#each $videoSession.radioHits as hit, index (`${hit.artist}-${index}`)}
				<div class="video-radio-hit"><strong>+{hit.count}</strong><span>{hit.count === 1 ? 'video' : 'videos'} from {hit.artist}</span></div>
			{/each}
		</div>
	{/if}
	{#if $videoSession.error}
		<p class="video-panel-error">{$videoSession.error}</p>
	{/if}
</div>

<section class="video-panel-queue">
	<div class="video-panel-queue-head">
		<span class="eyebrow">Queue</span>
		<span>{$videoSessionUpcoming.length} up next</span>
		<button
			class="video-panel-queue-clear"
			type="button"
			title="Clear video queue"
			onclick={() => clearVideoSession()}
		>⌫</button>
	</div>
	{#if $videoSessionUpcoming.length > 0}
		<div class="video-panel-list">
			{#each $videoSessionUpcoming.slice(0, 60) as video, i (`video-${video.tidal_id}-${i}`)}
				{@const videoArt = artworkCandidate(video.artwork_url, 320)}
				<button
					type="button"
					class="video-panel-row"
					onclick={() => void playQueuedVideo(video.tidal_id)}
					oncontextmenu={(event) => openVideoQueueMenu(video, event)}
					onkeydown={(event) => videoQueueKeydown(video, event)}
					aria-label={`Play ${video.title}`}
				>
					{#if videoArt}
						<img
							class="video-panel-row-art"
							src={videoArt}
							alt=""
							onerror={() => markArtworkFailed(videoArt)}
						/>
					{:else}
						<span class="video-panel-row-art placeholder">▶</span>
					{/if}
					<span class="video-panel-row-copy">
						<strong>{video.title}</strong>
						<span>{video.artist_name ?? 'Unknown artist'}</span>
					</span>
					<span class="video-panel-row-time">{formatTrackDuration(video.duration_ms ?? 0)}</span>
				</button>
			{/each}
		</div>
	{:else}
		<QueueEmpty title={$videoSession.continuous ? 'Finding more videos…' : 'No videos up next.'}>
			{$videoSession.continuous ? 'More from this artist and related artists will appear here.' : 'Start radio to keep listening.'}
		</QueueEmpty>
	{/if}
</section>

<style>
	.video-panel-heading {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
	}

	.video-queue-trigger {
		min-height: 40px;
		padding: 0 12px;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: var(--bg-surface);
		color: var(--text-secondary);
		font: inherit;
		cursor: pointer;
	}

	.video-queue-trigger:hover,
	.video-queue-trigger[aria-expanded='true'] {
		border-color: var(--accent-line);
		color: var(--accent-strong);
		background: var(--accent-soft);
	}

	:global(.app-shell:not([data-player-layout='bottom'])) .video-queue-trigger { display: none; }

	:global(.app-shell[data-player-layout='bottom']) .video-panel-top {
		display: grid;
		grid-template-columns: 96px minmax(140px, 1fr) minmax(180px, 1.25fr) auto auto 40px;
		grid-template-areas: 'art copy source actions queue heading';
		align-items: center;
		column-gap: 12px;
	}
	:global(.app-shell[data-player-layout='bottom']) .video-panel-heading { grid-area: heading; }
	:global(.app-shell[data-player-layout='bottom']) .video-panel-heading .eyebrow { display: none; }
	:global(.app-shell[data-player-layout='bottom']) .video-panel-art-wrap { grid-area: art; width: 96px; }
	:global(.app-shell[data-player-layout='bottom']) .video-panel-copy { grid-area: copy; }
	:global(.app-shell[data-player-layout='bottom']) .video-panel-actions { grid-area: actions; }
	:global(.app-shell[data-player-layout='bottom']) .video-queue-trigger { grid-area: queue; }
	:global(.app-shell[data-player-layout='bottom']) .video-panel-source { grid-area: source; }
	:global(.app-shell[data-player-layout='bottom']) .video-panel-source {
		min-width: 0;
		margin: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	:global(.app-shell[data-player-layout='bottom']) .video-radio-hits { display: none; }
	:global(.app-shell[data-player-layout='bottom']) .video-panel-error { grid-column: 2 / -1; }
	:global(.app-shell[data-player-layout='bottom']) .video-panel-queue { display: none; }
	:global(.app-shell[data-player-layout='bottom'] .video-queue-panel.queue-drawer-open) .video-panel-queue {
		position: fixed;
		z-index: calc(var(--z-overlay) + 1);
		right: 16px;
		bottom: calc(var(--bottom-player-height) + var(--space-2));
		display: flex;
		flex-direction: column;
		width: min(420px, calc(100vw - 32px));
		max-height: min(60dvh, 520px);
		padding: 16px;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-lg);
		background: var(--bg-surface-strong);
		box-shadow: var(--panel-shadow);
		overflow-y: auto;
	}

	.video-panel-top,
	.video-panel-queue {
		min-width: 0;
	}

	.video-panel-top {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.video-panel-art-wrap {
		aspect-ratio: 16 / 9;
		width: 100%;
		border-radius: 8px;
		overflow: hidden;
		background: color-mix(in srgb, var(--instrument-surface-strong) 75%, transparent);
		border: 1px solid var(--border-subtle);
	}

	.video-panel-art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.video-panel-art.placeholder {
		display: flex;
		align-items: center;
		justify-content: center;
		color: var(--text-secondary);
		font-size: var(--font-size-2xl);
	}

	.video-panel-copy {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}

	.video-panel-copy strong {
		color: var(--text-primary);
		font-size: var(--font-size-md);
		line-height: var(--line-height-snug);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.video-panel-copy span,
	.video-panel-source,
	.video-panel-queue-head {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.video-panel-actions {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 10px;
	}

	.video-panel-source { margin: -4px 0 0; line-height: var(--line-height-normal); }
	.video-radio-hits {
		max-height: 76px;
		overflow-y: auto;
		display: grid;
		gap: 5px;
		padding: 2px 0;
	}
	.video-radio-hit {
		display: flex;
		align-items: baseline;
		gap: 7px;
		min-width: 0;
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}
	.video-radio-hit strong { color: var(--accent-strong); white-space: nowrap; }
	.video-radio-hit span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

	.video-panel-chip {
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		background: color-mix(in srgb, var(--instrument-surface) 80%, transparent);
		color: var(--text-primary);
		font: inherit;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		padding: 6px 10px;
		cursor: pointer;
	}

	.video-panel-chip.active {
		border-color: color-mix(in srgb, var(--accent-line) 70%, transparent);
		background: color-mix(in srgb, var(--accent-soft) 75%, transparent);
	}

	.video-panel-error {
		margin: 0;
		color: var(--state-error);
		font-size: var(--font-size-xs);
	}

	.video-panel-queue {
		flex: 1;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		border-top: 1px solid var(--border-subtle);
		padding-top: 14px;
	}

	.video-panel-queue-head {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 10px;
		padding-bottom: 10px;
	}

	.video-panel-queue-clear {
		margin-left: auto;
		background: none;
		border: none;
		color: var(--text-muted);
		cursor: pointer;
		font-size: var(--font-size-sm);
		padding: 0.15rem 0.3rem;
		border-radius: 4px;
		line-height: 1;
	}
	.video-panel-queue-clear:hover {
		color: var(--text-primary);
		background: var(--bg-hover);
	}

	.video-panel-list {
		display: flex;
		flex-direction: column;
		gap: 6px;
		overflow-y: auto;
		padding-right: 2px;
	}

	.video-panel-row {
		width: 100%;
		min-width: 0;
		display: grid;
		grid-template-columns: 48px minmax(0, 1fr) auto;
		align-items: center;
		gap: 10px;
		border: 1px solid transparent;
		border-radius: 8px;
		background: transparent;
		color: inherit;
		font: inherit;
		text-align: left;
		padding: 7px;
		cursor: pointer;
	}

	.video-panel-row:hover,
	.video-panel-row:focus-visible {
		background: color-mix(in srgb, var(--instrument-surface) 78%, transparent);
		border-color: var(--border-subtle);
		outline: none;
	}

	.video-panel-row-art {
		width: 48px;
		aspect-ratio: 16 / 9;
		border-radius: 4px;
		object-fit: cover;
		background: color-mix(in srgb, var(--instrument-surface-strong) 85%, transparent);
	}

	.video-panel-row-art.placeholder {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.video-panel-row-copy {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.video-panel-row-copy strong,
	.video-panel-row-copy span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.video-panel-row-copy strong {
		font-size: var(--font-size-sm);
		color: var(--text-primary);
	}

	.video-panel-row-copy span,
	.video-panel-row-time {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}

	@media (max-width: 1050px) and (min-width: 680px) {
		:global(.app-shell[data-player-layout='bottom']) .video-panel-top {
			grid-template-columns: 72px minmax(0, 1fr) auto auto 40px;
			grid-template-areas: 'art copy actions queue heading';
		}
		:global(.app-shell[data-player-layout='bottom']) .video-panel-art-wrap { width: 72px; }
		:global(.app-shell[data-player-layout='bottom']) .video-panel-source { display: none; }
	}
</style>
