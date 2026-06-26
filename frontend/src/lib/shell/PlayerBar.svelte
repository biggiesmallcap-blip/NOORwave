<script lang="ts">
	import NowPlayingMetadata from '$lib/components/now-playing/NowPlayingMetadata.svelte';
	import NowPlayingProgress from '$lib/components/now-playing/NowPlayingProgress.svelte';
	import NowPlayingTransport from '$lib/components/now-playing/NowPlayingTransport.svelte';
	import type { StreamDisplayInfo, Track } from '$lib/api/client';
	import { formatResolutionShort } from '$lib/player/stream_display';
	import {
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize,
	} from '$lib/utils/artwork';
	import { getQualityClass } from '$lib/utils/format';
	import { downloadTrack, defaultDownloadFormat } from '$lib/stores/downloads';

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
	let downloadPending = $state(false);

	async function handleDownloadCurrent() {
		if (!track || downloadPending) return;
		downloadPending = true;
		try {
			await downloadTrack(track.id, $defaultDownloadFormat);
		} finally {
			downloadPending = false;
		}
	}

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

		{#if streamDisplay}
			<span class={`quality-badge np-quality ${getQualityClass(streamDisplay.audio_quality)}`}>
				{formatQuality(streamDisplay.audio_quality)}
			</span>
			{#if formatResolutionShort(streamDisplay)}
				<span class="quality-badge np-resolution" title="Actual playback resolution (bit-depth / kHz)">
					{formatResolutionShort(streamDisplay)}
				</span>
			{/if}
		{:else if track?.best_quality}
			<span class={`quality-badge np-quality ${getQualityClass(track.best_quality)}`}>
				{formatQuality(track.best_quality)}
			</span>
		{/if}

		{#if track}
			<button
				class="np-fullscreen-btn"
				aria-label="Enter quiet mode"
				title="Quiet mode"
				onclick={onEnterQuietMode}
			>⛶</button>
			<button
				class="np-art-fav"
				class:active={track?.is_favorite}
				aria-label={track?.is_favorite ? 'Remove from favorites' : 'Add to favorites'}
				title={track?.is_favorite ? 'Remove from favorites' : 'Add to favorites'}
				aria-pressed={track?.is_favorite}
				disabled={favoritePending}
				onclick={onToggleFavorite}
			>{track?.is_favorite ? '♥' : '♡'}</button>
			<button
				class="np-art-dl"
				class:pending={downloadPending}
				aria-label="Download this track"
				title={`Download (${$defaultDownloadFormat.toUpperCase()})`}
				disabled={downloadPending}
				onclick={handleDownloadCurrent}
			>⤓</button>
		{/if}
	</div>

	<NowPlayingMetadata
		track={track}
		nowPlayingAttribution={nowPlayingAttribution}
		streamDetail={streamDetail}
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
		>{volume === 0 ? '🔇' : '🔊'}</button>
		<label class="volume-control" onwheel={handleVolumeWheel}>
			<span>Vol</span>
			<input
				type="range"
				min="0"
				max="1"
				step="0.01"
				value={volume}
				oninput={handleVolumeInput}
				onchange={handleVolumeChange}
				aria-label="Volume"
			/>
			<span class="volume-pct">{displayVolume}%</span>
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

	.np-art-fav {
		position: absolute;
		bottom: 10px;
		right: 10px;
		width: 36px;
		height: 36px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-size: var(--font-size-lg);
		line-height: 1;
		color: rgba(255, 255, 255, 0.92);
		background: rgba(0, 0, 0, 0.45);
		border: 1px solid rgba(255, 255, 255, 0.18);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		cursor: pointer;
		transition:
			transform 160ms ease,
			background 160ms ease,
			color 160ms ease,
			border-color 160ms ease,
			box-shadow 160ms ease;
	}

	.np-art-fav:hover {
		background: rgba(0, 0, 0, 0.65);
		transform: translateY(-1px);
	}

	.np-art-fav:active {
		transform: scale(0.92);
	}

	.np-art-fav:disabled {
		opacity: 0.55;
		cursor: not-allowed;
	}

	.np-art-fav.active {
		color: #ff4d6d;
		background: color-mix(in srgb, #ff4d6d 24%, rgba(0, 0, 0, 0.55));
		border-color: color-mix(in srgb, #ff4d6d 60%, transparent);
		box-shadow: 0 0 14px color-mix(in srgb, #ff4d6d 40%, transparent);
	}

	.np-art-dl {
		position: absolute;
		bottom: 10px;
		left: 10px;
		width: 36px;
		height: 36px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-size: var(--font-size-lg);
		line-height: 1;
		color: rgba(255, 255, 255, 0.92);
		background: rgba(0, 0, 0, 0.45);
		border: 1px solid rgba(255, 255, 255, 0.18);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		cursor: pointer;
		transition:
			transform 160ms ease,
			background 160ms ease,
			color 160ms ease;
	}

	.np-art-dl:hover {
		background: rgba(0, 0, 0, 0.65);
		transform: translateY(-1px);
	}

	.np-art-dl:active {
		transform: scale(0.92);
	}

	.np-art-dl:disabled {
		opacity: 0.55;
		cursor: progress;
	}

	.np-art-dl.pending {
		animation: np-dl-pulse 900ms ease-in-out infinite;
	}

	@keyframes np-dl-pulse {
		0%, 100% { opacity: 0.55; }
		50% { opacity: 0.9; }
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

	.np-quality {
		position: absolute;
		top: 10px;
		right: 10px;
	}

	.np-resolution {
		position: absolute;
		top: 36px;
		right: 10px;
		font-variant-numeric: tabular-nums;
		font-size: var(--font-size-2xs);
		letter-spacing: 0.04em;
		opacity: 0.85;
	}

	.np-controls {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.np-mute-btn {
		width: 32px;
		height: 32px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: color-mix(in srgb, var(--instrument-surface) 82%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		flex-shrink: 0;
		cursor: pointer;
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}

	.np-mute-btn:hover {
		background: color-mix(in srgb, var(--instrument-surface-strong) 92%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 82%, transparent);
	}

	.np-mute-btn[aria-pressed='true'] {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.volume-control {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px 12px;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
		background: color-mix(in srgb, var(--instrument-surface) 82%, transparent);
	}

	.volume-control span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.volume-pct {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		min-width: 3ch;
		text-align: right;
		flex-shrink: 0;
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

	.np-top.queue-expanded :global(.np-copy .np-eyebrow) {
		display: none;
	}

	.np-top.queue-expanded :global(.np-copy .np-album),
	.np-top.queue-expanded :global(.np-copy .np-source),
	.np-top.queue-expanded :global(.np-copy .np-stream-detail) {
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

	.np-top.queue-expanded :global(.np-progress) {
		max-height: 0;
		opacity: 0;
		overflow: hidden;
		pointer-events: none;
	}

	.np-top.queue-expanded :global(.transport) {
		gap: 6px;
	}

	.np-top.queue-expanded :global(.tp-btn),
	.np-top.queue-expanded :global(.tp-play) {
		width: 30px;
		height: 30px;
	}

	.np-top.queue-expanded .np-quality {
		display: none;
	}
</style>
