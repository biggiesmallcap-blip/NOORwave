<script lang="ts">
	import { formatTrackDuration } from '$lib/utils/format';

	let {
		position,
		duration,
		bufferedMs = 0,
		onSeek,
		onScrubStart,
		onScrubEnd,
	}: {
		position: number;
		duration: number;
		/**
		 * How many ms of the current track are decoded into the playback
		 * buffer. Clamps the scrubber max so the user can't drag past the
		 * loaded portion (route-side 409 is the backstop). Defaults to 0 -
		 * read as "no buffer info / treat as fully seekable" via the
		 * effectiveBufferedMs fallback below so non-streaming tracks behave
		 * as today.
		 */
		bufferedMs?: number;
		onSeek: (positionMs: number) => void;
		onScrubStart?: () => void;
		onScrubEnd?: () => void;
	} = $props();

	let isScrubbing = $state(false);
	let scrubPosition = $state(0);

	$effect(() => {
		if (!isScrubbing) {
			scrubPosition = position;
		}
	});

	// When the backend hasn't published buffered_ms yet (cold start, older
	// payload, etc.) treat the whole track as seekable - same UX as before
	// this feature shipped. A non-zero buffered_ms means the runtime is
	// reporting real buffer state and we should clamp.
	let effectiveBufferedMs = $derived(bufferedMs > 0 ? bufferedMs : duration);

	// Range max: clamp to buffered region but never collapse below the
	// current play position (guards against a transient desync where the
	// buffered_ms WS message lags the position one). When duration is
	// unknown (<= 0) leave max at 0 and disable the input - clamping then
	// would lock the scrubber at 0 even if the engine was actually
	// streaming.
	let scrubMax = $derived(
		duration > 0
			? Math.min(duration, Math.max(scrubPosition, position, effectiveBufferedMs))
			: 0
	);

	let progressWidth = $derived(
		duration > 0 ? `${Math.min((scrubPosition / duration) * 100, 100)}%` : '0%'
	);

	let bufferedWidth = $derived(
		duration > 0
			? `${Math.min((effectiveBufferedMs / duration) * 100, 100)}%`
			: '0%'
	);

	function beginScrub() {
		isScrubbing = true;
		onScrubStart?.();
	}

	function commitScrub() {
		isScrubbing = false;
		onSeek(scrubPosition);
		onScrubEnd?.();
	}
</script>

<div class="np-progress">
	<div class="np-progress-track" style="--pct: {progressWidth}">
		<div class="np-progress-buffered" style={`width: ${bufferedWidth}`}></div>
		<div class="np-progress-fill" style={`width: ${progressWidth}`}></div>
		<input
			class="np-progress-input"
			type="range"
			min="0"
			max={scrubMax}
			step="1000"
			bind:value={scrubPosition}
			oninput={beginScrub}
			onchange={commitScrub}
			disabled={duration <= 0}
			aria-label="Seek playback"
		/>
	</div>

	<div class="np-times" class:scrubbing={isScrubbing}>
		<span>{formatTrackDuration(scrubPosition)}</span>
		<span>{formatTrackDuration(duration)}</span>
	</div>
</div>

<style>
	.np-progress {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.np-progress-track {
		position: relative;
		height: 3px;
		border-radius: 99px;
		background: color-mix(in srgb, var(--instrument-border) 35%, transparent);
		overflow: visible;
	}

	.np-progress-buffered {
		position: absolute;
		left: 0;
		top: 0;
		height: 100%;
		background: color-mix(in srgb, var(--instrument-border) 65%, transparent);
		border-radius: inherit;
		pointer-events: none;
		transition: width 200ms linear;
	}

	.np-progress-fill {
		position: absolute;
		left: 0;
		top: 0;
		height: 100%;
		background: var(--accent);
		border-radius: inherit;
		pointer-events: none;
		box-shadow: 0 0 18px color-mix(in srgb, var(--accent-glow) 70%, transparent);
	}

	.np-progress-track::after {
		content: '';
		position: absolute;
		left: var(--pct, 0%);
		top: 50%;
		transform: translate(-50%, -50%);
		width: 12px;
		height: 12px;
		border-radius: 50%;
		background: var(--accent);
		box-shadow: 0 0 0 3px var(--accent-glow);
		opacity: 0;
		transition: opacity var(--motion-fast), transform var(--motion-fast);
		pointer-events: none;
		z-index: 1;
	}

	.np-progress-track:hover::after,
	.np-progress-track:focus-within::after {
		opacity: 1;
	}

	.np-progress-track:active::after {
		transform: translate(-50%, -50%) scale(1.25);
	}

	.np-progress-input {
		position: absolute;
		inset: -8px 0;
		width: 100%;
		opacity: 0;
		cursor: pointer;
	}

	.np-times {
		display: flex;
		justify-content: space-between;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.np-times.scrubbing span:first-child {
		color: var(--accent-strong, var(--accent));
		font-weight: var(--font-weight-semibold);
	}
</style>
