<!--
	Phone now-playing sheet: artwork, scrub, transport, modes and the up-next
	list (passed in as children, since the layout owns the queue rows).
-->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import PlayPauseIcon from '$lib/components/ui/PlayPauseIcon.svelte';
	import { REPEAT_ICONS, REPEAT_LABELS, SHUFFLE_ICONS, SHUFFLE_LABELS } from '$lib/player/mode_labels';
	import { formatResolutionShort } from '$lib/player/stream_display';
	import {
		automixEnabled,
		currentStreamDisplay,
		currentTrack,
		cyclePlayerRepeatMode,
		cyclePlayerShuffleMode,
		isPlaying,
		playNextTrack,
		playPreviousTrack,
		repeatMode,
		shuffleMode,
		toggleTrackFavorite,
		togglePlayback,
		togglePlayerAutomix,
	} from '$lib/stores/player';
	import { createArtworkFallback } from '$lib/utils/artwork_fallback.svelte';
	import { formatQualityTier, formatTrackDuration, getQualityClass } from '$lib/utils/format';

	interface Props {
		onclose: () => void;
		/** Playhead in ms; follows playback except while the user drags. */
		scrubPosition: number;
		progressWidth: string;
		onscrubstart: () => void;
		onscrubcommit: () => void;
		queueCountLabel: string;
		children: Snippet;
	}

	let {
		onclose,
		scrubPosition = $bindable(),
		progressWidth,
		onscrubstart,
		onscrubcommit,
		queueCountLabel,
		children,
	}: Props = $props();

	const artwork = createArtworkFallback();
	const artworkCandidate = artwork.candidate;
	const markArtworkFailed = artwork.markFailed;
	let mobileNowPlayingArtwork = $derived(artworkCandidate($currentTrack?.artwork_url, 640));
	let mobileFavoritePending = $state(false);

	async function handleMobileFavoriteToggle() {
		if (!$currentTrack || mobileFavoritePending) return;
		mobileFavoritePending = true;
		try {
			await toggleTrackFavorite($currentTrack.id);
		} finally {
			mobileFavoritePending = false;
		}
	}
</script>

{#if $currentTrack}
	<button
		class="mobile-np-backdrop"
		type="button"
		aria-label="Close now playing"
		onclick={onclose}
	></button>
	<div class="mobile-np-sheet" role="dialog" aria-label="Now playing" aria-modal="true">
		<div class="mobile-np-handle"></div>

		<div class="mobile-np-art-wrap">
			{#key $currentTrack.artwork_url}
				{#if mobileNowPlayingArtwork}
					<img
						class="mobile-np-art"
						src={mobileNowPlayingArtwork}
						alt=""
						onerror={() => markArtworkFailed(mobileNowPlayingArtwork)}
					/>
				{:else}
					<div class="mobile-np-art placeholder">♫</div>
				{/if}
			{/key}
			{#if $currentStreamDisplay}
				<span class={`quality-badge mobile-np-quality ${getQualityClass($currentStreamDisplay.audio_quality)}`}>
					{formatQualityTier($currentStreamDisplay.audio_quality)}
				</span>
				{#if formatResolutionShort($currentStreamDisplay)}
					<span class="quality-badge mobile-np-resolution" title="Actual playback resolution (bit-depth / kHz)">
						{formatResolutionShort($currentStreamDisplay)}
					</span>
				{/if}
			{:else if $currentTrack.best_quality}
				<span class={`quality-badge mobile-np-quality ${getQualityClass($currentTrack.best_quality)}`}>
					{formatQualityTier($currentTrack.best_quality)}
				</span>
			{/if}
		</div>

		<div class="mobile-np-info">
			<div class="mobile-np-copy">
				<strong class="mobile-np-title">{$currentTrack.title}</strong>
				<span class="mobile-np-artist">{$currentTrack.artist_name ?? 'Unknown artist'}</span>
			</div>
			<button
				class="mobile-np-like"
				class:active={$currentTrack.is_favorite}
				type="button"
				aria-label={$currentTrack.is_favorite ? 'Remove from favorites' : 'Add to favorites'}
				disabled={mobileFavoritePending}
				onclick={() => void handleMobileFavoriteToggle()}
			>
				{$currentTrack.is_favorite ? '♥' : '♡'}
			</button>
		</div>

		<div class="mobile-np-scrub">
			<div class="mobile-np-scrub-track" style="--pct: {progressWidth}">
				<div class="mobile-np-scrub-fill" style="width: {progressWidth}"></div>
				<input
					class="mobile-np-scrub-input"
					type="range"
					min="0"
					max={$currentTrack.duration_ms ?? 0}
					step="1000"
					bind:value={scrubPosition}
					oninput={onscrubstart}
					onchange={onscrubcommit}
					disabled={!$currentTrack.duration_ms}
					aria-label="Seek playback"
				/>
			</div>
			<div class="mobile-np-times">
				<span>{formatTrackDuration(scrubPosition)}</span>
				<span>{formatTrackDuration($currentTrack.duration_ms ?? 0)}</span>
			</div>
		</div>

		<div class="mobile-np-transport">
			<button class="mobile-np-btn" type="button" aria-label="Previous" onclick={() => void playPreviousTrack()}>⏮</button>
			<button class="mobile-np-btn primary" type="button" aria-label="Play or pause" onclick={() => void togglePlayback()}>
				<PlayPauseIcon playing={$isPlaying} />
			</button>
			<button class="mobile-np-btn" type="button" aria-label="Next" onclick={() => void playNextTrack()}>⏭</button>
		</div>

		<div class="mobile-np-secondary">
			<button
				class="mobile-np-chip"
				class:active={$shuffleMode !== 'off'}
				type="button"
				aria-label={SHUFFLE_LABELS[$shuffleMode]}
				onclick={() => void cyclePlayerShuffleMode()}
			>
				<span>{SHUFFLE_ICONS[$shuffleMode]}</span>
				<span>{$shuffleMode === 'off' ? 'Shuffle' : SHUFFLE_LABELS[$shuffleMode]}</span>
			</button>
			<button
				class="mobile-np-chip"
				class:active={$repeatMode !== 'off'}
				type="button"
				aria-label={REPEAT_LABELS[$repeatMode]}
				onclick={() => void cyclePlayerRepeatMode()}
			>
				<span>{REPEAT_ICONS[$repeatMode]}</span>
				<span>{$repeatMode === 'off' ? 'Repeat' : REPEAT_LABELS[$repeatMode]}</span>
			</button>
			<button
				class="mobile-np-chip"
				class:active={$automixEnabled}
				type="button"
				aria-label={$automixEnabled ? 'Disable automix' : 'Enable automix'}
				onclick={() => void togglePlayerAutomix()}
			>
				<span aria-hidden="true">
					<svg width="13" height="13" viewBox="0 0 15 15" fill="none" xmlns="http://www.w3.org/2000/svg">
						<path
							d="M7.5 1.6A5.1 5.1 0 0 0 2.4 6.7v1.2h-.2a1 1 0 0 0-1 1v2.1a1 1 0 0 0 1 1h1.5a.6.6 0 0 0 .6-.6V6.7a3.2 3.2 0 0 1 6.4 0v4.7a.6.6 0 0 0 .6.6h1.5a1 1 0 0 0 1-1V8.9a1 1 0 0 0-1-1h-.2V6.7A5.1 5.1 0 0 0 7.5 1.6z"
							fill="currentColor"
						/>
					</svg>
				</span>
				<span>{$automixEnabled ? 'Automix on' : 'Automix'}</span>
			</button>
		</div>

		<div class="mobile-np-queue-header">
			<span class="eyebrow">Up next</span>
			<span class="mobile-np-queue-count">{queueCountLabel}</span>
		</div>

		{@render children()}
	</div>
{/if}

<style>
	.mobile-np-backdrop,
	.mobile-np-sheet {
		display: none;
	}

	@media (max-width: 679px) {
		/* Phone layout shows the sheet. */
		.mobile-np-backdrop {
			position: fixed;
			inset: 0;
			background: rgba(0, 0, 0, 0.52);
			z-index: 50;
			border: none;
			padding: 0;
			cursor: default;
		}

		.mobile-np-sheet {
			position: fixed;
			left: 0;
			right: 0;
			bottom: 0;
			max-height: 92dvh;
			overflow-y: auto;
			-webkit-overflow-scrolling: touch;
			background: var(--bg-elevated);
			border-radius: var(--radius-lg) var(--radius-lg) 0 0;
			border-top: 1px solid var(--border-subtle);
			z-index: 51;
			padding: 12px 20px calc(var(--safe-bottom) + 24px);
			display: flex;
			flex-direction: column;
			gap: 16px;
			box-shadow: 0 -16px 48px rgba(0, 0, 0, 0.32);
			animation: np-sheet-up var(--motion-slow) both;
		}

		@keyframes np-sheet-up {
			from { transform: translateY(100%); }
			to   { transform: translateY(0); }
		}

		.mobile-np-handle {
			width: 36px;
			height: 4px;
			border-radius: 999px;
			background: var(--border-strong);
			margin: 0 auto;
			flex-shrink: 0;
		}

		.mobile-np-art-wrap {
			position: relative;
			width: min(260px, calc(100vw - 80px));
			aspect-ratio: 1;
			border-radius: 20px;
			overflow: hidden;
			align-self: center;
			background: var(--bg-surface);
			border: 1px solid var(--border-subtle);
			flex-shrink: 0;
		}

		.mobile-np-art {
			width: 100%;
			height: 100%;
			object-fit: cover;
			display: block;
		}

		.mobile-np-art.placeholder {
			display: grid;
			place-items: center;
			color: var(--text-tertiary);
			font-size: var(--font-size-4xl);
		}

		.mobile-np-quality {
			position: absolute;
			top: 10px;
			right: 10px;
		}

		.mobile-np-resolution {
			position: absolute;
			top: 38px;
			right: 10px;
			font-variant-numeric: tabular-nums;
			font-size: var(--font-size-xs);
			letter-spacing: 0.04em;
			opacity: 0.85;
		}

		.mobile-np-info {
			display: flex;
			align-items: center;
			justify-content: space-between;
			gap: 12px;
			min-width: 0;
		}

		.mobile-np-copy {
			display: flex;
			flex-direction: column;
			gap: 4px;
			min-width: 0;
			flex: 1;
		}

		.mobile-np-title {
			font-family: var(--font-display);
			font-size: var(--font-size-lg);
			line-height: var(--line-height-tight);
			letter-spacing: -0.01em;
			white-space: nowrap;
			overflow: hidden;
			text-overflow: ellipsis;
			display: block;
		}

		.mobile-np-artist {
			color: var(--text-secondary);
			font-size: var(--font-size-sm);
			white-space: nowrap;
			overflow: hidden;
			text-overflow: ellipsis;
			display: block;
		}

		.mobile-np-like {
			width: 42px;
			height: 42px;
			border-radius: 50%;
			display: grid;
			place-items: center;
			font-size: var(--font-size-lg);
			color: var(--text-secondary);
			flex-shrink: 0;
			border: none;
			background: none;
			cursor: pointer;
			transition: color var(--motion-fast), transform var(--motion-fast);
			-webkit-tap-highlight-color: transparent;
		}

		.mobile-np-like:active { transform: scale(0.88); }
		.mobile-np-like.active { color: #ff4d6d; }

		.mobile-np-scrub {
			display: flex;
			flex-direction: column;
			gap: 8px;
		}

		.mobile-np-scrub-track {
			position: relative;
			height: 4px;
			border-radius: 999px;
			background: var(--border-subtle);
		}

		.mobile-np-scrub-fill {
			position: absolute;
			top: 0;
			left: 0;
			height: 100%;
			background: var(--accent);
			border-radius: inherit;
			pointer-events: none;
		}

		.mobile-np-scrub-input {
			position: absolute;
			inset: -14px 0;
			width: 100%;
			opacity: 0;
			cursor: pointer;
		}

		.mobile-np-times {
			display: flex;
			justify-content: space-between;
			color: var(--text-secondary);
			font-size: var(--font-size-xs);
			font-variant-numeric: tabular-nums;
		}

		.mobile-np-transport {
			display: flex;
			align-items: center;
			justify-content: center;
			gap: 20px;
		}

		.mobile-np-btn {
			width: 48px;
			height: 48px;
			border-radius: 50%;
			display: grid;
			place-items: center;
			background: var(--bg-surface);
			border: 1px solid var(--border-subtle);
			color: var(--text-primary);
			font-size: var(--font-size-lg);
			cursor: pointer;
			transition: transform var(--motion-fast), opacity var(--motion-fast);
			-webkit-tap-highlight-color: transparent;
		}

		.mobile-np-btn:active { transform: scale(0.92); }

		.mobile-np-btn.primary {
			width: 60px;
			height: 60px;
			background: var(--accent);
			border-color: transparent;
			color: var(--text-on-accent);
			font-size: var(--font-size-xl);
			box-shadow: 0 8px 24px var(--accent-glow);
		}

		.mobile-np-secondary {
			display: flex;
			align-items: center;
			justify-content: center;
			gap: 10px;
			flex-wrap: wrap;
		}

		.mobile-np-chip {
			display: inline-flex;
			align-items: center;
			gap: 6px;
			padding: 8px 14px;
			border-radius: 999px;
			border: 1px solid var(--border-subtle);
			background: var(--bg-surface);
			color: var(--text-secondary);
			font-size: var(--font-size-xs);
			font-weight: var(--font-weight-semibold);
			cursor: pointer;
			transition: background var(--motion-fast), color var(--motion-fast), border-color var(--motion-fast);
			-webkit-tap-highlight-color: transparent;
		}

		.mobile-np-chip.active {
			background: var(--accent-soft);
			border-color: var(--accent-line);
			color: var(--accent-strong);
		}

		.mobile-np-queue-header {
			display: flex;
			align-items: baseline;
			justify-content: space-between;
			padding-top: 8px;
			border-top: 1px solid var(--border-subtle);
		}

		.mobile-np-queue-count {
			color: var(--text-secondary);
			font-size: var(--font-size-sm);
		}
	}
</style>
