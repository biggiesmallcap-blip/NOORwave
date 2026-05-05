<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { get } from 'svelte/store';
	import type Hls from 'hls.js';
	import type { VideoQualityMode } from '$lib/api/client';
	import { isPlaying, togglePlayback } from '$lib/stores/player';

	type Props = {
		src: string;
		poster?: string | null;
		title?: string;
		artist?: string | null;
		autoplay?: boolean;
		qualityMode?: VideoQualityMode;
		autoplayNext?: boolean;
		hasNext?: boolean;
		upNextTitle?: string | null;
		upNextArtist?: string | null;
		onEnded?: () => void;
		onToggleAutoplay?: () => void;
		refreshStream?: () => Promise<string>;
	};

	let {
		src,
		poster = null,
		title = 'Video',
		artist = null,
		autoplay = true,
		qualityMode = 'MAX',
		autoplayNext = false,
		hasNext = false,
		upNextTitle = null,
		upNextArtist = null,
		onEnded,
		onToggleAutoplay,
		refreshStream,
	}: Props = $props();

	let container: HTMLDivElement | null = $state(null);
	let videoEl: HTMLVideoElement | null = $state(null);
	let HlsClass: typeof import('hls.js').default | null = null;
	let hls: Hls | null = null;
	let currentSrc = '';
	let nativeHls = $state(false);
	let unsupported = $state(false);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let playing = $state(false);
	let duration = $state(0);
	let currentTime = $state(0);
	let volume = $state(1);
	let muted = $state(false);
	let fullscreen = $state(false);
	let levels = $state<{ index: number; label: string }[]>([]);
	let selectedLevel = $state(-1);
	let chromeVisible = $state(true);
	let upNextVisible = $state(false);
	let retryUsed = false;
	let mounted = false;
	let fullscreenListener: (() => void) | null = null;
	let surfaceClickTimer: ReturnType<typeof setTimeout> | null = null;
	let chromeHideTimer: ReturnType<typeof setTimeout> | null = null;

	const VOLUME_KEY = 'noor_video_volume';
	const MUTED_KEY = 'noor_video_muted';
	const SURFACE_CLICK_DELAY_MS = 220;
	const CHROME_HIDE_DELAY_MS = 2600;
	const UP_NEXT_WINDOW_SECONDS = 15;

	function formatTime(seconds: number): string {
		if (!Number.isFinite(seconds) || seconds <= 0) return '0:00';
		const total = Math.floor(seconds);
		const minutes = Math.floor(total / 60);
		const rest = total % 60;
		return `${minutes}:${rest.toString().padStart(2, '0')}`;
	}

	function restoreVolume() {
		if (!videoEl || typeof localStorage === 'undefined') return;
		const rawVolume = Number(localStorage.getItem(VOLUME_KEY));
		volume = Number.isFinite(rawVolume) ? Math.min(1, Math.max(0, rawVolume)) : 1;
		muted = localStorage.getItem(MUTED_KEY) === 'true';
		videoEl.volume = volume;
		videoEl.muted = muted;
	}

	function persistVolume() {
		if (typeof localStorage === 'undefined') return;
		localStorage.setItem(VOLUME_KEY, String(volume));
		localStorage.setItem(MUTED_KEY, String(muted));
	}

	function destroyHls() {
		hls?.destroy();
		hls = null;
		levels = [];
		selectedLevel = -1;
		nativeHls = false;
	}

	function highestLevelIndex(instance: Hls): number {
		let bestIndex = -1;
		let bestScore = -1;
		instance.levels.forEach((level, index) => {
			const score = (level.height ?? 0) * 10_000_000 + (level.bitrate ?? 0);
			if (score > bestScore) {
				bestScore = score;
				bestIndex = index;
			}
		});
		return bestIndex;
	}

	function defaultLevelForMode(instance: Hls): number {
		if (qualityMode !== 'MAX') return -1;
		return highestLevelIndex(instance);
	}

	function applyQualityMode() {
		if (!hls) return;
		const nextLevel = defaultLevelForMode(hls);
		selectedLevel = nextLevel;
		hls.currentLevel = nextLevel;
	}

	function clearChromeTimer() {
		if (!chromeHideTimer) return;
		clearTimeout(chromeHideTimer);
		chromeHideTimer = null;
	}

	function scheduleChromeHide() {
		clearChromeTimer();
		if (!fullscreen || !playing) return;
		chromeHideTimer = setTimeout(() => {
			chromeVisible = false;
			chromeHideTimer = null;
		}, CHROME_HIDE_DELAY_MS);
	}

	function revealChrome() {
		chromeVisible = true;
		scheduleChromeHide();
	}

	async function pauseAudioPlayback() {
		if (get(isPlaying)) await togglePlayback();
	}

	async function retryWithFreshStream() {
		if (retryUsed || !refreshStream || !videoEl) return false;
		retryUsed = true;
		const resumeAt = videoEl.currentTime;
		try {
			const next = await refreshStream();
			await load(next, resumeAt);
			return true;
		} catch {
			return false;
		}
	}

	function handleFatalError(message = 'This video could not be loaded.') {
		void retryWithFreshStream().then((recovered) => {
			if (recovered) return;
			error = message;
			loading = false;
		});
	}

	async function load(nextSrc: string, resumeAt = 0) {
		if (!videoEl || !HlsClass) return;
		currentSrc = nextSrc;
		destroyHls();
		error = null;
		unsupported = false;
		loading = true;
		retryUsed = resumeAt > 0 ? retryUsed : false;
		videoEl.removeAttribute('src');
		videoEl.load();

		if (HlsClass.isSupported()) {
			const instance = new HlsClass({ enableWorker: true });
			hls = instance;
			instance.on(HlsClass.Events.MANIFEST_PARSED, () => {
				levels = instance.levels
					.map((level, index) => ({
						index,
						label: level.height ? `${level.height}p` : level.bitrate ? `${Math.round(level.bitrate / 1000)} kbps` : `Level ${index + 1}`,
					}))
					.filter((level) => level.label);
				applyQualityMode();
				if (resumeAt > 0) videoEl!.currentTime = resumeAt;
				if (autoplay) void videoEl!.play().catch(() => {
					error = 'Press play to start this video.';
				});
			});
			instance.on(HlsClass.Events.ERROR, (_event, data) => {
				if (!data?.fatal) return;
				if (data.type === HlsClass?.ErrorTypes.NETWORK_ERROR) {
					handleFatalError('The video stream expired or the network dropped.');
				} else {
					handleFatalError('This HLS stream could not be played.');
				}
			});
			instance.attachMedia(videoEl);
			instance.loadSource(nextSrc);
			return;
		}

		if (videoEl.canPlayType('application/vnd.apple.mpegurl')) {
			nativeHls = true;
			videoEl.src = nextSrc;
			if (resumeAt > 0) videoEl.currentTime = resumeAt;
			if (autoplay) void videoEl.play().catch(() => {
				error = 'Press play to start this video.';
			});
			return;
		}

		unsupported = true;
		error = 'This device cannot play HLS video streams.';
		loading = false;
	}

	function togglePlay() {
		if (!videoEl) return;
		if (videoEl.paused) void videoEl.play();
		else videoEl.pause();
	}

	function seek(value: number) {
		if (!videoEl) return;
		videoEl.currentTime = value;
		currentTime = value;
	}

	function setVolume(value: number) {
		if (!videoEl) return;
		volume = Math.min(1, Math.max(0, value));
		videoEl.volume = volume;
		if (volume > 0) muted = false;
		videoEl.muted = muted;
		persistVolume();
	}

	function toggleMute() {
		if (!videoEl) return;
		muted = !muted;
		videoEl.muted = muted;
		persistVolume();
	}

	function selectQuality(index: number) {
		if (!hls) return;
		selectedLevel = index;
		hls.currentLevel = index;
	}

	async function toggleFullscreen() {
		if (!container) return;
		if (document.fullscreenElement) await document.exitFullscreen();
		else await container.requestFullscreen();
	}

	function handleSurfaceClick() {
		if (surfaceClickTimer) clearTimeout(surfaceClickTimer);
		revealChrome();
		surfaceClickTimer = setTimeout(() => {
			surfaceClickTimer = null;
			togglePlay();
		}, SURFACE_CLICK_DELAY_MS);
	}

	function handleSurfaceDoubleClick() {
		if (surfaceClickTimer) {
			clearTimeout(surfaceClickTimer);
			surfaceClickTimer = null;
		}
		revealChrome();
		void toggleFullscreen();
	}

	function skipTypingTarget(target: EventTarget | null): boolean {
		if (!(target instanceof HTMLElement)) return false;
		const tag = target.tagName.toLowerCase();
		return tag === 'input' || tag === 'textarea' || tag === 'select' || target.isContentEditable;
	}

	function handleKeydown(event: KeyboardEvent) {
		if (skipTypingTarget(event.target)) return;
		if (event.key === ' ') {
			event.preventDefault();
			togglePlay();
		} else if (event.key.toLowerCase() === 'f') {
			event.preventDefault();
			void toggleFullscreen();
		} else if (event.key.toLowerCase() === 'm') {
			event.preventDefault();
			toggleMute();
		} else if (event.key === 'ArrowLeft') {
			event.preventDefault();
			seek(Math.max(0, currentTime - 10));
		} else if (event.key === 'ArrowRight') {
			event.preventDefault();
			seek(Math.min(duration || currentTime + 10, currentTime + 10));
		}
	}

	onMount(async () => {
		mounted = true;
		const mod = await import('hls.js');
		HlsClass = mod.default;
		restoreVolume();
		fullscreenListener = () => {
			fullscreen = document.fullscreenElement === container;
			if (fullscreen) revealChrome();
			else {
				chromeVisible = true;
				clearChromeTimer();
			}
		};
		document.addEventListener('fullscreenchange', fullscreenListener);
		await load(src);
	});

	$effect(() => {
		if (!mounted || !src || src === currentSrc) return;
		void load(src);
	});

	$effect(() => {
		if (!mounted || !hls) return;
		applyQualityMode();
	});

	$effect(() => {
		if (!mounted) return;
		if (fullscreen && playing) scheduleChromeHide();
		else {
			chromeVisible = true;
			clearChromeTimer();
		}
	});

	$effect(() => {
		if (!mounted) return;
		const remaining = duration - currentTime;
		upNextVisible =
			autoplayNext &&
			Boolean(upNextTitle) &&
			Number.isFinite(duration) &&
			duration > 0 &&
			remaining > 0 &&
			remaining <= UP_NEXT_WINDOW_SECONDS;
	});

	onDestroy(() => {
		mounted = false;
		if (surfaceClickTimer) clearTimeout(surfaceClickTimer);
		clearChromeTimer();
		videoEl?.pause();
		destroyHls();
		if (fullscreenListener) document.removeEventListener('fullscreenchange', fullscreenListener);
	});
</script>

<div
	bind:this={container}
	class="video-player"
	class:fullscreen
	class:chrome-hidden={fullscreen && !chromeVisible && playing}
	role="button"
	aria-label="Video player"
	tabindex="0"
	onkeydown={handleKeydown}
	onmousemove={revealChrome}
	onpointermove={revealChrome}
	onfocusin={revealChrome}
>
	<video
		bind:this={videoEl}
		poster={poster ?? undefined}
		crossorigin="anonymous"
		playsinline
		preload="metadata"
		onplay={() => {
			playing = true;
			void pauseAudioPlayback();
		}}
		onpause={() => (playing = false)}
		onended={() => {
			playing = false;
			onEnded?.();
		}}
		onwaiting={() => (loading = true)}
		onplaying={() => {
			loading = false;
			error = null;
		}}
		onloadedmetadata={() => {
			duration = videoEl?.duration ?? 0;
			loading = false;
		}}
		ondurationchange={() => (duration = videoEl?.duration ?? 0)}
		ontimeupdate={() => (currentTime = videoEl?.currentTime ?? 0)}
		onvolumechange={() => {
			volume = videoEl?.volume ?? volume;
			muted = videoEl?.muted ?? muted;
			persistVolume();
		}}
		onerror={() => handleFatalError('This video could not be loaded.')}
		onclick={handleSurfaceClick}
		ondblclick={handleSurfaceDoubleClick}
	></video>

	{#if loading && !error && !unsupported}
		<div class="status">Loading video…</div>
	{/if}

	{#if error}
		<div class="status error">{error}</div>
	{/if}

	<div class="top-meta">
		<strong>{title}</strong>
		{#if artist}<span>{artist}</span>{/if}
	</div>

	{#if autoplayNext && upNextTitle && upNextVisible}
		<div class="up-next-pill">
			<span>Coming next</span>
			<strong>{upNextArtist ? `${upNextArtist} - ${upNextTitle}` : upNextTitle}</strong>
		</div>
	{/if}

	<div class="controls">
		<button type="button" class="icon-btn primary" onclick={togglePlay} aria-label={playing ? 'Pause' : 'Play'}>
			{playing ? '⏸' : '▶'}
		</button>
		<span class="time">{formatTime(currentTime)}</span>
		<input
			class="seek"
			type="range"
			min="0"
			max={duration || 0}
			step="0.1"
			value={currentTime}
			oninput={(event) => seek(Number(event.currentTarget.value))}
			aria-label="Seek"
		/>
		<span class="time">{formatTime(duration)}</span>
		<button type="button" class="icon-btn" onclick={toggleMute} aria-label={muted ? 'Unmute' : 'Mute'}>
			{muted || volume === 0 ? '🔇' : '🔊'}
		</button>
		<input
			class="volume"
			type="range"
			min="0"
			max="1"
			step="0.01"
			value={muted ? 0 : volume}
			oninput={(event) => setVolume(Number(event.currentTarget.value))}
			aria-label="Volume"
		/>
		<button
			type="button"
			class="autoplay-pill"
			class:enabled={autoplayNext}
			onclick={() => onToggleAutoplay?.()}
			disabled={!hasNext && !autoplayNext}
			aria-pressed={autoplayNext}
			aria-label={autoplayNext ? 'Disable video autoplay' : 'Enable video autoplay'}
		>
			{autoplayNext ? '> On' : '> Autoplay'}
		</button>
		{#if !nativeHls && levels.length > 1}
			<select
				class="quality"
				value={selectedLevel}
				onchange={(event) => selectQuality(Number(event.currentTarget.value))}
				aria-label="Quality"
			>
				<option value="-1">Auto</option>
				{#each levels as level (level.index)}
					<option value={level.index}>{level.label}</option>
				{/each}
			</select>
		{/if}
		<button type="button" class="icon-btn" onclick={() => void toggleFullscreen()} aria-label="Fullscreen">
			{fullscreen ? '⤢' : '⛶'}
		</button>
	</div>
</div>

<style>
	.video-player {
		position: relative;
		width: 100%;
		aspect-ratio: 16 / 9;
		min-height: 280px;
		border-radius: 8px;
		overflow: hidden;
		background: #030305;
		outline: none;
	}

	video {
		width: 100%;
		height: 100%;
		object-fit: contain;
		background: #030305;
		display: block;
	}

	.top-meta,
	.controls,
	.status {
		position: absolute;
		left: 0;
		right: 0;
		z-index: 2;
	}

	.top-meta {
		top: 0;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 14px 16px 38px;
		background: linear-gradient(180deg, rgba(0, 0, 0, 0.62), transparent);
		pointer-events: none;
		transition: opacity 0.18s ease, transform 0.18s ease;
	}

	.up-next-pill {
		position: absolute;
		top: 14px;
		left: 50%;
		z-index: 3;
		display: grid;
		gap: 2px;
		max-width: min(520px, calc(100% - 32px));
		padding: 9px 14px;
		border-radius: 999px;
		background: rgba(12, 12, 16, 0.76);
		border: 1px solid rgba(255, 255, 255, 0.18);
		box-shadow: 0 14px 40px rgba(0, 0, 0, 0.28);
		backdrop-filter: blur(16px);
		color: rgba(255, 255, 255, 0.92);
		transform: translate(-50%, 0);
		animation: up-next-drop 0.24s ease both;
		pointer-events: none;
	}

	.up-next-pill span,
	.up-next-pill strong {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.up-next-pill span {
		color: rgba(255, 255, 255, 0.58);
		font-size: 0.62rem;
		font-weight: 800;
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}

	.up-next-pill strong {
		font-size: 0.78rem;
	}

	.top-meta strong,
	.top-meta span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.top-meta strong {
		font-size: 0.9rem;
	}

	.top-meta span {
		color: rgba(255, 255, 255, 0.62);
		font-size: 0.78rem;
	}

	.status {
		top: 50%;
		transform: translateY(-50%);
		text-align: center;
		color: rgba(255, 255, 255, 0.72);
		font-weight: 700;
		pointer-events: none;
	}

	.status.error {
		color: var(--state-error);
	}

	.controls {
		bottom: 0;
		display: grid;
		grid-template-columns: 40px auto 1fr auto 40px 88px auto auto 40px;
		align-items: center;
		gap: 8px;
		padding: 36px 14px 14px;
		background: linear-gradient(0deg, rgba(0, 0, 0, 0.72), transparent);
		transition: opacity 0.18s ease, transform 0.18s ease;
	}

	.chrome-hidden {
		cursor: none;
	}

	.chrome-hidden .top-meta,
	.chrome-hidden .controls {
		opacity: 0;
		pointer-events: none;
	}

	.chrome-hidden .top-meta {
		transform: translateY(-10px);
	}

	.chrome-hidden .controls {
		transform: translateY(12px);
	}

	.icon-btn {
		width: 34px;
		height: 34px;
		border-radius: 999px;
		display: grid;
		place-items: center;
		background: rgba(255, 255, 255, 0.1);
		color: rgba(255, 255, 255, 0.92);
	}

	.icon-btn.primary {
		background: var(--accent);
		color: white;
	}

	.time {
		font-size: 0.72rem;
		font-variant-numeric: tabular-nums;
		color: rgba(255, 255, 255, 0.72);
	}

	.seek,
	.volume {
		width: 100%;
		accent-color: var(--accent);
	}

	.quality {
		max-width: 86px;
		min-width: 70px;
		background: rgba(255, 255, 255, 0.1);
		border: 1px solid rgba(255, 255, 255, 0.16);
		border-radius: 999px;
		color: rgba(255, 255, 255, 0.9);
		padding: 6px 8px;
		font-size: 0.72rem;
	}

	.autoplay-pill {
		min-width: 92px;
		height: 34px;
		border-radius: 999px;
		padding: 0 12px;
		background: rgba(255, 255, 255, 0.1);
		border: 1px solid rgba(255, 255, 255, 0.16);
		color: rgba(255, 255, 255, 0.84);
		font-size: 0.72rem;
		font-weight: 800;
		white-space: nowrap;
	}

	.autoplay-pill.enabled {
		background: var(--accent);
		border-color: var(--accent);
		color: white;
	}

	.autoplay-pill:disabled {
		cursor: not-allowed;
		opacity: 0.42;
	}

	.fullscreen {
		border-radius: 0;
	}

	@keyframes up-next-drop {
		from {
			opacity: 0;
			transform: translate(-50%, -16px);
		}
		to {
			opacity: 1;
			transform: translate(-50%, 0);
		}
	}

	@media (max-width: 720px) {
		.video-player {
			min-height: 210px;
		}

		.controls {
			grid-template-columns: 38px auto 1fr auto 38px auto 38px;
		}

		.volume,
		.quality {
			display: none;
		}
	}
</style>
