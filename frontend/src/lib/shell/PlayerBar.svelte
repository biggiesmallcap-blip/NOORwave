<script lang="ts">
	import NowPlayingMetadata from '$lib/components/now-playing/NowPlayingMetadata.svelte';
	import NowPlayingProgress from '$lib/components/now-playing/NowPlayingProgress.svelte';
	import NowPlayingTransport from '$lib/components/now-playing/NowPlayingTransport.svelte';
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
		playerState,
		isScrubbing,
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
		playerState: string;
		isScrubbing: boolean;
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

<div class="np-top" class:queue-expanded={queueExpanded}>
	<div class="np-artwork-wrap">
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

		{#if track}
			<button
				class="np-fullscreen-btn"
				aria-label="Enter quiet mode"
				title="Quiet mode"
				onclick={onEnterQuietMode}
			>⛶</button>
		{/if}
	</div>

	<NowPlayingMetadata
		track={track}
		nowPlayingAttribution={nowPlayingAttribution}
		streamDetail={streamDetail}
		qualityLabel={qualityLabel}
		qualityClass={qualityClass}
		playerState={playerState}
		isScrubbing={isScrubbing}
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
	/>

	<div class="np-controls">
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
		padding: 16px 16px 0;
		display: flex;
		flex-direction: column;
		gap: 16px;
		flex-shrink: 0;
	}

	.np-artwork-wrap {
		position: relative;
		aspect-ratio: 1;
		border-radius: 22px;
		overflow: hidden;
		background:
			linear-gradient(135deg, var(--bg-hover), transparent),
			var(--bg-surface);
		border: 1px solid var(--border-subtle);
		flex-shrink: 0;
	}

	.np-fullscreen-btn {
		position: absolute;
		top: 10px;
		left: 10px;
		width: 30px;
		height: 30px;
		border-radius: 8px;
		display: grid;
		place-items: center;
		font-size: var(--font-size-sm);
		color: #fff;
		background: rgba(0, 0, 0, 0.45);
		border: 1px solid rgba(255, 255, 255, 0.18);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		opacity: 0;
		transform: translateY(-4px);
		transition: opacity 160ms ease, transform 160ms ease, background 160ms ease;
		cursor: pointer;
	}

	.np-artwork-wrap:hover .np-fullscreen-btn,
	.np-fullscreen-btn:focus-visible {
		opacity: 1;
		transform: translateY(0);
	}

	.np-fullscreen-btn:hover {
		background: rgba(0, 0, 0, 0.65);
	}

	.np-artwork {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
		animation: artwork-fade-in 320ms ease both;
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
		width: 20px;
		height: 20px;
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
		.np-artwork-wrap,
		.np-top :global(.np-progress),
		.np-top :global(.np-info),
		.np-top :global(.transport) {
			transition: none;
		}
	}

	.np-top.queue-expanded .np-artwork-wrap {
		max-height: 64px;
		overflow: hidden;
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

	/* The artwork is a 64px strip once the queue is expanded - anything
	   floating over it collides with everything else, so the quiet-mode
	   button steps aside (it stays on the collapsed artwork and in the
	   right-click menu). */
	.np-top.queue-expanded .np-fullscreen-btn {
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

	/* Keep the scrubber alive with the queue expanded - only the time labels
	   go, so position stays visible and seekable. */
	.np-top.queue-expanded :global(.np-times) {
		display: none;
	}

	.np-top.queue-expanded :global(.transport) {
		gap: 6px;
	}

	.np-top.queue-expanded :global(.tp-btn),
	.np-top.queue-expanded :global(.tp-play) {
		width: 30px;
		height: 30px;
	}
</style>
