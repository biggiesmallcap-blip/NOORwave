<script lang="ts">
	import NowPlayingMetadata from '$lib/components/now-playing/NowPlayingMetadata.svelte';
	import NowPlayingProgress from '$lib/components/now-playing/NowPlayingProgress.svelte';
	import NowPlayingTransport from '$lib/components/now-playing/NowPlayingTransport.svelte';
	import PlayerLayoutSelect from './PlayerLayoutSelect.svelte';
	import type { EffectivePlayerLayout } from '$lib/stores/playerLayout';
	import { playerArtworkStyle } from '$lib/stores/playerArtwork';
	import { bottomQualityDisplay, sideQualityDisplay } from '$lib/stores/playerInformation';
	import type { StreamDisplayInfo, Track } from '$lib/api/client';
	import {
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize,
	} from '$lib/utils/artwork';
	import { getQualityClass } from '$lib/utils/format';

	type PlayerBarError = {
		message: string;
		retry?: () => Promise<void>;
	};

	const VOLUME_WHEEL_STEP = 0.05;

	let {
		track,
		streamDisplay,
		nowPlayingAttribution,
		streamDetail,
		position,
		bufferedMs = 0,
		isPlaying,
		shuffleMode,
		repeatMode,
		volume,
		displayVolume,
		playerError,
		favoritePending,
		queueExpanded,
		layout,
		queueCount,
		queueOpen,
		onToggleQueue,
		onEnterQuietMode,
		onToggleFavorite,
		onSeek,
		onScrubStart,
		onScrubEnd,
		onCycleShuffle,
		onPrev,
		onPlayPause,
		onNext,
		onCycleRepeat,
		onOpenMore,
		onToggleMute,
		onVolumePreview,
		onVolumeChange,
		onRetryPlayerError,
		onDismissPlayerError,
	}: {
		track: Track | null;
		streamDisplay: StreamDisplayInfo | null;
		nowPlayingAttribution: string | null;
		streamDetail: string;
		position: number;
		bufferedMs?: number;
		isPlaying: boolean;
		shuffleMode: string;
		repeatMode: string;
		volume: number;
		displayVolume: number;
		playerError: PlayerBarError | null;
		favoritePending: boolean;
		queueExpanded: boolean;
		layout: EffectivePlayerLayout;
		queueCount: number;
		queueOpen: boolean;
		onToggleQueue: () => void;
		onEnterQuietMode: () => void;
		onToggleFavorite: () => void;
		onSeek: (positionMs: number) => void;
		onScrubStart: () => void;
		onScrubEnd: () => void;
		onCycleShuffle: () => void;
		onPrev: () => void;
		onPlayPause: () => void;
		onNext: () => void;
		onCycleRepeat: () => void;
		onOpenMore: (anchor: HTMLElement) => void;
		onToggleMute: () => void;
		onVolumePreview: (volumePercent: number) => void;
		onVolumeChange: (volume: number) => void;
		onRetryPlayerError: (retry: () => Promise<void>) => void;
		onDismissPlayerError: () => void;
	} = $props();

	let failedArtworkUrls = $state<Record<string, boolean>>({});

	let nowPlayingArtwork = $derived(artworkCandidate(track?.artwork_url, 640));

	function artworkCandidate(
		rawUrl: string | null | undefined,
		size: TidalArtworkSize,
	): string | null {
		if (!rawUrl) return null;
		for (const candidateSize of tidalArtworkFallbackSizes(rawUrl, size)) {
			const candidate = upscaleTidalArtwork(rawUrl, candidateSize);
			if (candidate && !failedArtworkUrls[candidate]) return candidate;
		}
		return null;
	}

	function markArtworkFailed(renderedUrl: string | null | undefined) {
		if (!renderedUrl) return;
		failedArtworkUrls = { ...failedArtworkUrls, [renderedUrl]: true };
	}

	// One quality statement for the whole panel. The live stream wins over the
	// track's catalogue tier; the exact bit-depth / kHz rides along in
	// streamDetail, so the artwork carries no badges of its own.
	let qualityTier = $derived(streamDisplay?.audio_quality ?? track?.best_quality ?? null);
	let qualityLabel = $derived(formatQuality(qualityTier));
	let qualityClass = $derived(qualityTier ? getQualityClass(qualityTier) : '');
	let qualityDisplay = $derived(layout === 'bottom' ? $bottomQualityDisplay : $sideQualityDisplay);

	function formatQuality(q: string | null) {
		if (!q) return '';
		if (q === 'HI_RES_LOSSLESS') return 'HiRes Lossless';
		if (q === 'LOSSLESS') return 'Lossless';
		if (q === 'HIGH') return 'High';
		if (q === 'LOW') return 'Low';
		return q.replaceAll('_', ' ');
	}

	function handleVolumeInput(event: Event) {
		const nextVolume = Number((event.currentTarget as HTMLInputElement).value);
		onVolumePreview(Math.round(nextVolume * 100));
	}

	function handleVolumeChange(event: Event) {
		onVolumeChange(Number((event.currentTarget as HTMLInputElement).value));
	}

	function clampVolume(value: number) {
		return Math.min(1, Math.max(0, value));
	}

	function handleVolumeWheel(event: WheelEvent) {
		if (event.deltaY === 0) return;
		event.preventDefault();
		event.stopPropagation();
		const direction = event.deltaY < 0 ? 1 : -1;
		const nextVolume = clampVolume(volume + direction * VOLUME_WHEEL_STEP);
		onVolumePreview(Math.round(nextVolume * 100));
		onVolumeChange(nextVolume);
	}

	function handleRetryPlayerError() {
		const retry = playerError?.retry;
		onDismissPlayerError();
		if (retry) onRetryPlayerError(retry);
	}
</script>

<div class="np-top" class:queue-expanded={queueExpanded && layout !== 'bottom'} class:horizontal={layout === 'bottom'} class:banner-artwork={$playerArtworkStyle !== 'square' && layout !== 'bottom'} class:slim-artwork={$playerArtworkStyle === 'slim' && layout !== 'bottom'}>
	<div class="player-head">
		<span class="player-head-label">Now playing</span>
		<PlayerLayoutSelect effective={layout} />
	</div>
	<div class="np-artwork-wrap">
		{#if track}
			<button
				class="np-artwork-open"
				type="button"
				aria-label="Enter quiet mode"
				title="Enter quiet mode"
				onclick={onEnterQuietMode}
			><span class="np-artwork-quiet-cue" aria-hidden="true"><svg viewBox="0 0 20 20" focusable="false"><path d="M7 3H3v4m10-4h4v4M3 13v4h4m10-4v4h-4" /></svg></span></button>
		{/if}
		<div class="np-artwork-visual">
			{#key track?.artwork_url}
				{#if nowPlayingArtwork}
					<img
						class="np-artwork"
						src={nowPlayingArtwork}
						alt=""
						onerror={() => markArtworkFailed(nowPlayingArtwork)}
					/>
				{:else}
					<div class="np-artwork placeholder">♫</div>
				{/if}
			{/key}
		</div>
		{#if track && layout === 'bottom'}
			<button
				class="np-artwork-favorite"
				class:active={track.is_favorite}
				type="button"
				title={track.is_favorite ? 'Remove from favorites' : 'Add to favorites'}
				aria-label={track.is_favorite ? 'Remove from favorites' : 'Add to favorites'}
				aria-pressed={track.is_favorite}
				disabled={favoritePending}
				onclick={onToggleFavorite}
			>{track.is_favorite ? '♥' : '♡'}</button>
		{/if}
	</div>

	<NowPlayingMetadata
		track={track}
		nowPlayingAttribution={nowPlayingAttribution}
		streamDetail={streamDetail}
		qualityLabel={qualityLabel}
		qualityClass={qualityClass}
		qualityDisplay={qualityDisplay}
	/>

	<NowPlayingProgress
		position={position}
		duration={track?.duration_ms ?? 0}
		bufferedMs={bufferedMs}
		onSeek={onSeek}
		onScrubStart={onScrubStart}
		onScrubEnd={onScrubEnd}
	/>

	<NowPlayingTransport
		layout={layout === 'bottom' ? 'bottom' : 'side'}
		track={track}
		isPlaying={isPlaying}
		shuffleMode={shuffleMode}
		repeatMode={repeatMode}
		favoritePending={favoritePending}
		onToggleFavorite={onToggleFavorite}
		onCycleShuffle={onCycleShuffle}
		onPrev={onPrev}
		onPlayPause={onPlayPause}
		onNext={onNext}
		onCycleRepeat={onCycleRepeat}
		onOpenMore={onOpenMore}
		onEnterQuietMode={onEnterQuietMode}
	/>

	<div class="np-controls" data-transport>
		<button
			class="np-mute-btn"
			type="button"
			title={volume === 0 ? 'Unmute' : 'Mute'}
			aria-label={volume === 0 ? 'Unmute' : 'Mute'}
			aria-pressed={volume === 0}
			onclick={onToggleMute}
		>
			<svg width="15" height="15" viewBox="0 0 15 15" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
				<path
					d="M7.28 1.06a.5.5 0 0 1 .72.45v12a.5.5 0 0 1-.81.39L3.33 10.8H1.5a.5.5 0 0 1-.5-.5v-5.6a.5.5 0 0 1 .5-.5h1.83L7.19 1.1a.5.5 0 0 1 .09-.05z"
					fill="currentColor"
				/>
				{#if volume === 0}
					<path d="M10.3 5.3l3.4 3.4M13.7 5.3l-3.4 3.4" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" />
				{:else}
					<path
						d="M10.4 5.1a3.2 3.2 0 0 1 0 4.8M12.3 3.4a5.6 5.6 0 0 1 0 8.2"
						stroke="currentColor"
						stroke-width="1.1"
						stroke-linecap="round"
					/>
				{/if}
			</svg>
		</button>
		<label class="volume-control" onwheel={handleVolumeWheel} title={`Volume ${displayVolume}%`}>
			<input
				type="range"
				min="0"
				max="1"
				step="0.01"
				value={volume}
				oninput={handleVolumeInput}
				onchange={handleVolumeChange}
				aria-label="Volume"
				aria-valuetext={`${displayVolume}%`}
			/>
		</label>
	</div>

	<button
		id="player-queue-trigger"
		class="player-queue-trigger"
		type="button"
		aria-label={`${queueOpen ? 'Close' : 'Open'} queue${queueCount > 0 ? `, ${queueCount} up next` : ''}`}
		aria-expanded={queueOpen}
		aria-controls="queue-list"
		onclick={onToggleQueue}
	>
		<svg viewBox="0 0 20 20" aria-hidden="true" focusable="false">
			<path d="M3 5h2m3 0h9M3 10h2m3 0h9M3 15h2m3 0h9" />
		</svg>
		{#if queueCount > 0}<span class="queue-trigger-count">{queueCount}</span>{/if}
	</button>

	{#if playerError}
		<div class="player-error" role="alert">
			<span class="player-error-msg">{playerError.message}</span>
			{#if playerError.retry}
				<button class="player-error-btn" onclick={handleRetryPlayerError}>Retry</button>
			{/if}
			<button class="player-error-close" aria-label="Dismiss" onclick={onDismissPlayerError}>×</button>
		</div>
	{/if}
</div>

<style>
	.np-top {
		position: relative;
		padding: 16px 16px 0;
		display: flex;
		flex-direction: column;
		gap: 16px;
		flex-shrink: 0;
	}

	.player-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		min-height: 32px;
		padding-right: 40px;
	}

	.player-head-label {
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
	}

	.player-queue-trigger {
		position: absolute;
		top: 16px;
		right: 16px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 3px;
		width: 32px;
		height: 32px;
		padding: 0;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-tertiary);
		cursor: pointer;
	}

	.player-queue-trigger:hover,
	.player-queue-trigger[aria-expanded='true'] {
		color: var(--text-primary);
		background: var(--bg-hover);
		border-color: var(--border-subtle);
	}

	.player-queue-trigger:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.player-queue-trigger svg {
		width: 17px;
		height: 17px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
	}

	.queue-trigger-count {
		position: absolute;
		top: -5px;
		right: -5px;
		display: grid;
		place-items: center;
		min-width: 15px;
		height: 15px;
		padding-inline: 2px;
		border-radius: 999px;
		background: var(--bg-surface-strong);
		color: var(--text-secondary);
		font-size: var(--font-size-2xs);
		font-variant-numeric: tabular-nums;
	}

	.np-artwork-wrap {
		position: relative;
		width: min(100%, 38dvh, 320px);
		aspect-ratio: 1;
		align-self: center;
		border-radius: 22px;
		overflow: hidden;
		background:
			linear-gradient(135deg, var(--bg-hover), transparent),
			var(--bg-surface);
		border: 1px solid var(--border-subtle);
		box-shadow: 0 14px 36px var(--player-art-shadow), inset 0 0 0 1px var(--player-art-rim);
		flex-shrink: 0;
	}

	.np-top.banner-artwork .np-artwork-wrap {
		width: 100%;
		aspect-ratio: 16 / 9;
		max-height: min(24dvh, 220px);
	}

	.np-top.slim-artwork .np-artwork-wrap {
		aspect-ratio: 3 / 1;
		max-height: min(14dvh, 120px);
		border-radius: 16px;
	}

	.np-artwork-visual {
		width: 100%;
		height: 100%;
		transition: transform var(--motion-base);
	}

	.np-artwork-open {
		position: absolute;
		inset: 0;
		z-index: 1;
		display: flex;
		align-items: flex-end;
		justify-content: flex-end;
		padding: 8px;
		border: 0;
		border-radius: inherit;
		background: transparent;
		color: #fff;
		cursor: pointer;
	}

	.np-artwork-open:hover + .np-artwork-visual,
	.np-artwork-open:focus-visible + .np-artwork-visual {
		transform: scale(1.025);
	}

	.np-artwork-open:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -3px;
	}

	.np-artwork-quiet-cue {
		display: grid;
		place-items: center;
		width: 24px;
		height: 24px;
		border-radius: 6px;
		color: #fff;
		background: rgba(0, 0, 0, 0.5);
		opacity: 0;
		transition: opacity var(--motion-base);
	}

	.np-artwork-quiet-cue svg {
		width: 14px;
		height: 14px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.np-artwork-open:hover .np-artwork-quiet-cue,
	.np-artwork-open:focus-visible .np-artwork-quiet-cue {
		opacity: 1;
	}

	.horizontal .np-artwork-open {
		align-items: flex-start;
		justify-content: flex-start;
		padding: 5px;
	}

	.horizontal .np-artwork-quiet-cue {
		width: 20px;
		height: 20px;
	}

	.np-artwork-favorite {
		position: absolute;
		right: 5px;
		bottom: 5px;
		z-index: 2;
		display: grid;
		place-items: center;
		width: 30px;
		height: 30px;
		padding: 0;
		border: 1px solid rgba(255, 255, 255, 0.3);
		border-radius: 50%;
		background: rgba(9, 9, 14, 0.72);
		color: #fff;
		font-size: var(--font-size-md);
		cursor: pointer;
	}

	.np-artwork-favorite:hover,
	.np-artwork-favorite.active { color: var(--accent-strong); border-color: var(--accent-line); }
	.np-artwork-favorite:disabled { opacity: 0.55; cursor: default; }
	.np-artwork-favorite:focus-visible { outline: 2px solid var(--accent-strong); outline-offset: 2px; }

	@media (hover: none) {
		.np-artwork-quiet-cue { opacity: 0.65; }
	}

	.np-artwork {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
		animation: artwork-fade-in var(--motion-slow) both;
	}

	@keyframes artwork-fade-in {
		from { opacity: 0; transform: scale(1.04); }
		to { opacity: 1; transform: scale(1); }
	}

	.np-artwork.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-2xl);
	}

	/* Volume wears no chrome: a bare icon plus a hairline track, so it reads as
	   the same family as the progress bar instead of a second pill competing
	   with the transport. The percentage lives in the tooltip and
	   aria-valuetext rather than a permanent readout. */
	.np-controls {
		display: flex;
		align-items: center;
		gap: 10px;
	}

	.np-mute-btn {
		width: 40px;
		height: 40px;
		display: grid;
		place-items: center;
		background: transparent;
		border: 0;
		color: var(--text-secondary);
		flex-shrink: 0;
		cursor: pointer;
		transition: color var(--motion-fast);
	}

	.np-mute-btn:hover {
		color: var(--text-primary);
	}

	.np-mute-btn[aria-pressed='true'] {
		color: var(--accent-strong);
	}

	.volume-control {
		flex: 1;
		display: flex;
		align-items: center;
		min-width: 0;
	}

	.volume-control input {
		flex: 1;
		min-width: 0;
	}

	.player-error {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 10px 12px;
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--state-error) 12%, transparent);
		border: 1px solid color-mix(in srgb, var(--state-error) 24%, transparent);
		color: var(--state-error);
		font-size: var(--font-size-xs);
		margin-top: 8px;
	}

	.player-error-msg {
		flex: 1;
		min-width: 0;
	}

	.player-error-btn,
	.player-error-close {
		background: transparent;
		border: 1px solid color-mix(in srgb, var(--state-error) 40%, transparent);
		color: inherit;
		border-radius: 4px;
		cursor: pointer;
		font: inherit;
		padding: 2px 8px;
		flex-shrink: 0;
	}

	.player-error-btn:hover,
	.player-error-close:hover {
		background: color-mix(in srgb, var(--state-error) 18%, transparent);
	}

	.player-error-close {
		padding: 0 8px;
		font-size: var(--font-size-md);
		line-height: var(--line-height-snug);
	}

	.np-artwork-wrap,
	.np-top :global(.np-progress),
	.np-top :global(.np-info),
	.np-top :global(.transport) {
		transition:
			max-height var(--motion-base, 240ms) ease,
			opacity var(--motion-base, 240ms) ease,
			gap var(--motion-base, 240ms) ease,
			padding var(--motion-base, 240ms) ease;
	}

	@media (prefers-reduced-motion: reduce) {
		.np-artwork { animation: none; }
		.np-artwork-visual,
		.np-artwork-quiet-cue { transition: none; }
		.np-artwork-open:hover + .np-artwork-visual,
		.np-artwork-open:focus-visible + .np-artwork-visual { transform: none; }
		.np-artwork-wrap,
		.np-top :global(.np-progress),
		.np-top :global(.np-info),
		.np-top :global(.transport) {
			transition: none;
		}
	}

	.np-top.queue-expanded .np-artwork-wrap {
		width: 64px;
		height: 64px;
		max-height: 64px;
		aspect-ratio: 1;
		overflow: hidden;
	}

	.np-top.queue-expanded {
		display: grid;
		grid-template-columns: 64px minmax(0, 1fr);
		column-gap: 12px;
		row-gap: 10px;
		align-items: center;
	}

	.np-top.queue-expanded .player-head,
	.np-top.queue-expanded :global(.np-progress),
	.np-top.queue-expanded :global(.transport),
	.np-top.queue-expanded .np-controls,
	.np-top.queue-expanded .player-error {
		grid-column: 1 / -1;
	}

	.np-top.queue-expanded :global(.np-info) {
		grid-column: 2;
		grid-row: 2;
	}

	.np-top.queue-expanded .np-artwork {
		object-fit: cover;
		object-position: center 30%;
		height: 64px;
	}

	.np-top.queue-expanded :global(.np-info) {
		padding-block: 6px;
	}

	.np-top.queue-expanded :global(.np-copy .np-album),
	.np-top.queue-expanded :global(.np-copy .np-source),
	.np-top.queue-expanded :global(.badge-row) {
		display: none;
	}

	.np-top.queue-expanded :global(.np-copy .np-title) {
		font-size: var(--font-size-md);
		line-height: var(--line-height-snug);
		margin: 0;
	}

	.np-top.queue-expanded :global(.np-copy .np-artist) {
		font-size: var(--font-size-xs);
	}

	.np-top.queue-expanded :global(.np-byline) { gap: 3px; }

	/* Keep the scrubber alive with the queue expanded - only the time labels
	   go, so position stays visible and seekable. */
	.np-top.queue-expanded :global(.np-times) {
		display: none;
	}

	.np-top.queue-expanded :global(.transport) {
		gap: 6px;
	}

	/* The horizontal player composes the same live controls for width instead of
	   carrying a tall side-panel stack into the bottom row. */
	.np-top.horizontal {
		display: grid;
		grid-template-columns: 80px minmax(0, 1fr) auto minmax(0, 1fr) 32px 32px;
		grid-template-areas:
			'art info transport controls queue head'
			'art progress progress progress progress progress';
		align-items: center;
		column-gap: 16px;
		row-gap: 8px;
		padding: 12px 16px;
	}

	.horizontal .player-head {
		grid-area: head;
		padding: 0;
	}

	.horizontal .player-head-label { display: none; }

	.horizontal .np-artwork-wrap {
		grid-area: art;
		width: 80px;
		border-radius: var(--radius-md);
	}

	.horizontal :global(.np-info) { grid-area: info; }
	.horizontal :global(.np-progress) { grid-area: progress; }
	.horizontal :global(.transport) { grid-area: transport; }
	.horizontal .np-controls {
		grid-area: controls;
		justify-self: end;
		width: min(100%, 160px);
	}

	.horizontal .player-queue-trigger {
		position: relative;
		top: auto;
		right: auto;
		grid-area: queue;
		width: 32px;
		height: 32px;
	}

	.horizontal :global(.np-title) { font-size: var(--font-size-md); }
	.horizontal :global(.np-artist) { font-size: var(--font-size-xs); }
	.horizontal :global(.np-info) { gap: 3px; }
	.horizontal :global(.np-copy) { gap: 2px; }
	.horizontal :global(.np-byline) {
		flex-direction: row;
		align-items: baseline;
		gap: 6px;
		min-width: 0;
	}
	.horizontal :global(.np-byline .np-artist),
	.horizontal :global(.np-byline .np-album) { flex: 0 1 auto; min-width: 0; }
	.horizontal :global(.np-byline-separator) { display: inline; color: var(--text-secondary); }
	.horizontal :global(.badge-row) { display: flex; }
	.horizontal :global(.badge-row .state-badge) { font-size: var(--font-size-2xs); }
	.horizontal :global(.np-source) { display: none; }
	.horizontal :global(.np-quality-chip) { font-size: var(--font-size-2xs); }
	.horizontal :global(.badge-row) { flex-wrap: wrap; column-gap: 6px; row-gap: 3px; }
	.horizontal :global(.stream-micro) { max-width: 100%; }

	.horizontal .player-error { grid-column: 1 / -1; }

	@media (max-width: 900px) {
		.np-top.horizontal {
			grid-template-columns: 72px minmax(0, 1fr) auto minmax(0, 1fr) 32px 32px;
			column-gap: 8px;
			padding-inline: 12px;
		}

		.horizontal .np-artwork-wrap { width: 72px; }
		.horizontal .player-queue-trigger { width: 32px; }
	}

	@media (max-width: 760px) {
		.horizontal .np-controls { width: min(100%, 120px); }
	}
</style>
