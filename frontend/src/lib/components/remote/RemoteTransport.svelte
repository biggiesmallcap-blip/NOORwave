<script lang="ts">
	import type { Track } from '$lib/api/client';
	import {
		automixEnabled,
		currentStreamDisplay,
		playbackRuntimeInfo,
		cyclePlayerRepeatMode,
		cyclePlayerShuffleMode,
		playNextTrack,
		playPreviousTrack,
		repeatMode,
		setPlayerPosition,
		setPlayerVolume,
		shuffleMode,
		togglePlayback,
		togglePlayerAutomix,
		toggleTrackFavorite
	} from '$lib/stores/player';
	import { exclusiveStatus } from '$lib/stores/exclusive_status';
	import { formatPlayerStreamDetail } from '$lib/player/stream_display';
	import { formatTrackDuration } from '$lib/utils/format';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { goto } from '$app/navigation';
	import { openActionSheet } from '$lib/remote/action_sheet';
	import { hapticAccent, hapticCommit, hapticTap } from '$lib/remote/haptics';
	import { longPress } from '$lib/remote/long_press';
	import {
		buildTidalTrackMenu,
		buildTrackMenu,
		type MenuTrack
	} from '$lib/player/track_menu';
	import { trackToTidalPlayable } from '$lib/utils/track';

	let {
		track,
		isPlaying,
		position,
		volume
	}: {
		track: Track | null;
		isPlaying: boolean;
		position: number;
		volume: number;
	} = $props();

	let isScrubbing = $state(false);
	let localPosition = $state(0);
	let volumeDisplay = $state(0);

	// Don't let the 250ms position ticker snap the slider back from under a drag.
	$effect(() => {
		if (!isScrubbing) localPosition = position;
	});

	// One-way: the slider reads `volumeDisplay`, `oninput` writes it back, so the
	// value the user dragged to is the value shown. An external volume change
	// (another client) flows in here without fighting an in-progress drag.
	// `.pre` so the correct value is on screen for the first paint.
	$effect.pre(() => {
		volumeDisplay = Math.round(volume * 100);
	});

	let isFavorite = $derived(track?.is_favorite === true);
	let streamDetail = $derived(
		formatPlayerStreamDetail({
			stream: $currentStreamDisplay,
			runtime: $playbackRuntimeInfo,
			exclusiveEngaged: $exclusiveStatus.engaged,
		})
	);
	let shuffleLabel = $derived.by(() => {
		switch ($shuffleMode) {
			case 'true':
				return 'On';
			case 'weighted':
				return 'Weighted';
			case 'genre':
				return 'Genre';
			default:
				return 'Off';
		}
	});
	let repeatLabel = $derived.by(() => {
		switch ($repeatMode) {
			case 'all':
				return 'All';
			case 'one':
				return 'One';
			default:
				return 'Off';
		}
	});

	function beginSeek() {
		isScrubbing = true;
	}

	function commitSeek() {
		isScrubbing = false;
		void setPlayerPosition(localPosition);
	}

	function previewVolume(event: Event) {
		volumeDisplay = Number((event.currentTarget as HTMLInputElement).value);
	}

	function commitVolume(event: Event) {
		void setPlayerVolume(Number((event.currentTarget as HTMLInputElement).value) / 100);
	}

	function onFavorite() {
		if (!track || (track.id <= 0 && !track.tidal_id)) return;
		hapticAccent();
		void toggleTrackFavorite(track.id, track.is_favorite ?? false);
	}

	let tidalTrack = $derived(track ? trackToTidalPlayable(track) : null);
	let canFavorite = $derived(!!track && (track.id > 0 || !!track.tidal_id));

	function openTrackActions() {
		if (!track) return;
		if (tidalTrack != null) {
			openActionSheet({
				title: track.title,
				subtitle: track.artist_name,
				items: buildTidalTrackMenu(tidalTrack, { remoteRoutes: true })
			});
			return;
		}
		if (track.id <= 0) return;
		const menuTrack: MenuTrack = {
			id: track.id,
			title: track.title,
			artist_id: track.artist_id ?? null,
			artist_name: track.artist_name ?? null,
			album_id: track.album_id ?? null,
			album_title: track.album_title ?? null,
			is_favorite: track.is_favorite ?? false
		};
		openActionSheet({
			title: track.title,
			subtitle: track.artist_name,
			items: buildTrackMenu(menuTrack, { remoteRoutes: true })
		});
	}

	let artistNavTarget = $derived.by(() => {
		if (track?.artist_id && track.artist_id > 0) return `/remote/artists/${track.artist_id}`;
		if (track?.artist_tidal_id && track.artist_tidal_id > 0)
			return `/remote/tidal/artists/${track.artist_tidal_id}`;
		return null;
	});
	let albumNavTarget = $derived.by(() => {
		if (track?.album_id && track.album_id > 0) return `/remote/albums/${track.album_id}`;
		if (track?.album_tidal_id && track.album_tidal_id > 0)
			return `/remote/tidal/albums/${track.album_tidal_id}`;
		return null;
	});

	function goToArtist() {
		if (!artistNavTarget) return;
		void goto(artistNavTarget);
	}

	function goToAlbum() {
		if (!albumNavTarget) return;
		void goto(albumNavTarget);
	}

	function onShuffle() {
		hapticTap();
		void cyclePlayerShuffleMode();
	}

	function onRepeat() {
		hapticTap();
		void cyclePlayerRepeatMode();
	}

	function onAutomix() {
		hapticTap();
		void togglePlayerAutomix();
	}

	// Mashing Play / Pause / Next fires concurrent HTTP requests and the
	// backend can end up in a confused state where playback just stops. Gate
	// the transport buttons behind an in-flight flag so a second tap is a
	// no-op until the previous request resolves. A 220ms minimum guarantees
	// the visual disabled state is noticeable rather than feeling like a
	// missed tap.
	let transportBusy = $state(false);
	async function runTransport(fn: () => Promise<unknown>): Promise<void> {
		if (transportBusy) return;
		transportBusy = true;
		const minDelay = new Promise<void>((resolve) => setTimeout(resolve, 220));
		try {
			await fn();
		} finally {
			await minDelay;
			transportBusy = false;
		}
	}

	function onPlayPause() {
		hapticTap();
		void runTransport(() => togglePlayback());
	}

	function onNext() {
		hapticTap();
		void runTransport(() => playNextTrack());
	}

	function onPrevious() {
		hapticTap();
		void runTransport(() => playPreviousTrack());
	}

	// Horizontal swipe on the album art skips tracks, with a live drag animation
	// (art follows the finger, tilts, lifts, and a glass sheen sweeps across it).
	// 60px threshold to commit; 1.4x dominance check vs vertical so a scroll
	// gesture starting on the art doesn't accidentally skip.
	const SWIPE_COMMIT_PX = 60;
	const SWIPE_LOCK_PX = 6;
	const SWIPE_MAX_PX = 160; // saturation point for visual effects
	type SwipeMode = 'idle' | 'pending' | 'horizontal' | 'vertical' | 'releasing';

	let swipeMode: SwipeMode = $state('idle');
	let swipeOffset = $state(0);
	let swipeStartX = 0;
	let swipeStartY = 0;
	let releaseTimer: ReturnType<typeof setTimeout> | null = null;

	let swipeProgress = $derived(Math.min(1, Math.abs(swipeOffset) / SWIPE_MAX_PX));
	let swipeRotation = $derived(
		`${Math.max(-6, Math.min(6, swipeOffset * 0.04)).toFixed(2)}deg`
	);

	function onSwipeStart(event: PointerEvent) {
		if (event.pointerType === 'mouse' && event.button !== 0) return;
		if (releaseTimer) {
			clearTimeout(releaseTimer);
			releaseTimer = null;
		}
		swipeMode = 'pending';
		swipeOffset = 0;
		swipeStartX = event.clientX;
		swipeStartY = event.clientY;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	}

	function onSwipeMove(event: PointerEvent) {
		if (swipeMode === 'idle' || swipeMode === 'releasing') return;
		const dx = event.clientX - swipeStartX;
		const dy = event.clientY - swipeStartY;
		if (swipeMode === 'pending') {
			if (Math.abs(dx) < SWIPE_LOCK_PX && Math.abs(dy) < SWIPE_LOCK_PX) return;
			swipeMode = Math.abs(dx) > Math.abs(dy) ? 'horizontal' : 'vertical';
		}
		if (swipeMode === 'horizontal') {
			swipeOffset = dx;
		}
	}

	function onSwipeEnd() {
		const wasHorizontal = swipeMode === 'horizontal';
		const dx = swipeOffset;
		swipeMode = 'releasing';
		swipeOffset = 0;
		if (wasHorizontal && Math.abs(dx) >= SWIPE_COMMIT_PX) {
			hapticCommit();
			if (dx < 0) void runTransport(() => playNextTrack());
			else void runTransport(() => playPreviousTrack());
		}
		releaseTimer = setTimeout(() => {
			swipeMode = 'idle';
			releaseTimer = null;
		}, 320);
	}

	function onSwipeCancel() {
		swipeMode = 'releasing';
		swipeOffset = 0;
		releaseTimer = setTimeout(() => {
			swipeMode = 'idle';
			releaseTimer = null;
		}, 320);
	}
</script>

<section class="remote-transport" aria-label="Playback controls">
	<div
		class="remote-art"
		class:swiping={swipeMode === 'horizontal'}
		class:releasing={swipeMode === 'releasing'}
		style="--swipe-x: {swipeOffset}px; --swipe-rot: {swipeRotation}; --swipe-progress: {swipeProgress};"
		role="presentation"
		onpointerdown={onSwipeStart}
		onpointermove={onSwipeMove}
		onpointerup={onSwipeEnd}
		onpointercancel={onSwipeCancel}
		use:longPress={openTrackActions}
	>
		<ArtworkImage
			className="remote-art-image"
			src={track?.artwork_url ?? null}
			size={640}
			fallbackText="NOOR"
			decorative={true}
		/>
		<span class="remote-art-sheen" aria-hidden="true"></span>
	</div>

	<div class="remote-copy">
		<div class="remote-title-row">
			<strong>{track?.title ?? 'Nothing playing'}</strong>
			<button
				class="remote-favorite"
				class:active={isFavorite}
				type="button"
				disabled={!canFavorite}
				aria-label={isFavorite ? 'Unfavorite track' : 'Favorite track'}
				aria-pressed={isFavorite}
				onclick={onFavorite}
			>
				<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
					<path
						d="M12 21s-6.7-4.35-9.33-8.5C.78 9.66 2.4 5.5 6.1 5.5c2.06 0 3.4 1.06 4.9 3.06 1.5-2 2.84-3.06 4.9-3.06 3.7 0 5.32 4.16 3.43 7C18.7 16.65 12 21 12 21z"
						fill={isFavorite ? 'currentColor' : 'none'}
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linejoin="round"
					/>
				</svg>
			</button>
		</div>
		{#if track?.artist_name && artistNavTarget}
			<button
				type="button"
				class="remote-copy-link"
				aria-label="Go to {track.artist_name}"
				onclick={goToArtist}
			>
				{track.artist_name}
			</button>
		{:else}
			<span>{track?.artist_name ?? 'Choose a track to begin playback.'}</span>
		{/if}
		{#if track?.album_title && albumNavTarget}
			<button
				type="button"
				class="remote-copy-link remote-copy-album"
				aria-label="Go to {track.album_title}"
				onclick={goToAlbum}
			>
				{track.album_title}
			</button>
		{/if}
		{#if streamDetail}
			<span class="remote-stream" aria-label="Stream quality">{streamDetail}</span>
		{/if}
	</div>

	<div class="remote-seek">
		<input
			type="range"
			min="0"
			max={track?.duration_ms ?? 0}
			step="1000"
			bind:value={localPosition}
			oninput={beginSeek}
			onchange={commitSeek}
			disabled={!track?.duration_ms}
			aria-label="Seek playback"
		/>
		<div class="remote-time">
			<span>{formatTrackDuration(localPosition)}</span>
			<span>{formatTrackDuration(track?.duration_ms ?? 0)}</span>
		</div>
	</div>

	<div class="remote-buttons" role="group" aria-label="Transport">
		<button
			type="button"
			aria-label="Previous"
			disabled={transportBusy}
			onclick={onPrevious}
		>
			Prev
		</button>
		<button
			class="primary"
			type="button"
			aria-label="Play or pause"
			disabled={transportBusy}
			onclick={onPlayPause}
		>
			{isPlaying ? 'Pause' : 'Play'}
		</button>
		<button type="button" aria-label="Next" disabled={transportBusy} onclick={onNext}>
			Next
		</button>
	</div>

	<div class="remote-modes" role="group" aria-label="Playback modes">
		<button
			class="remote-mode"
			class:active={$shuffleMode !== 'off'}
			type="button"
			aria-label="Cycle shuffle mode"
			aria-pressed={$shuffleMode !== 'off'}
			onclick={onShuffle}
		>
			<span class="remote-mode-label">Shuffle</span>
			<span class="remote-mode-value">{shuffleLabel}</span>
		</button>
		<button
			class="remote-mode"
			class:active={$repeatMode !== 'off'}
			type="button"
			aria-label="Cycle repeat mode"
			aria-pressed={$repeatMode !== 'off'}
			onclick={onRepeat}
		>
			<span class="remote-mode-label">Repeat</span>
			<span class="remote-mode-value">{repeatLabel}</span>
		</button>
		<button
			class="remote-mode"
			class:active={$automixEnabled}
			type="button"
			aria-label="Toggle automix"
			aria-pressed={$automixEnabled}
			onclick={onAutomix}
		>
			<span class="remote-mode-label">Automix</span>
			<span class="remote-mode-value">{$automixEnabled ? 'On' : 'Off'}</span>
		</button>
	</div>

	<label class="remote-volume">
		<span>Volume {volumeDisplay}%</span>
		<input
			type="range"
			min="0"
			max="100"
			step="1"
			value={volumeDisplay}
			oninput={previewVolume}
			onchange={commitVolume}
			aria-label="Volume"
		/>
	</label>
</section>

<style>
	.remote-transport {
		display: grid;
		gap: 16px;
	}

	.remote-art {
		position: relative;
		width: min(58vw, 240px);
		margin-inline: auto;
		aspect-ratio: 1;
		border-radius: 14px;
		overflow: hidden;
		background: var(--surface-1);
		touch-action: pan-y;
		user-select: none;
		-webkit-user-select: none;
		transform: translate3d(var(--swipe-x, 0px), 0, 0)
			rotate(var(--swipe-rot, 0deg))
			scale(calc(1 - var(--swipe-progress, 0) * 0.04));
		transition:
			transform 320ms cubic-bezier(0.22, 1.2, 0.36, 1),
			box-shadow 240ms ease,
			border-radius 240ms ease;
		box-shadow:
			0 calc(8px + var(--swipe-progress, 0) * 24px)
				calc(16px + var(--swipe-progress, 0) * 32px)
				rgba(0, 0, 0, calc(0.18 + var(--swipe-progress, 0) * 0.25));
		will-change: transform;
	}

	.remote-art.swiping {
		transition: box-shadow 120ms ease, border-radius 120ms ease;
	}

	.remote-art.swiping,
	.remote-art.releasing {
		border-radius: 22px;
	}

	.remote-art :global(.remote-art-image) {
		width: 100%;
		height: 100%;
	}

	.remote-art :global(img.remote-art-image) {
		object-fit: cover;
		pointer-events: none;
	}

	.remote-art :global(.remote-art-image.fallback) {
		display: grid;
		place-items: center;
		background: var(--surface-1);
		color: var(--text-muted);
	}

	/* Diagonal frosted sheen that sweeps across the cover as you drag. */
	.remote-art-sheen {
		position: absolute;
		inset: 0;
		pointer-events: none;
		opacity: var(--swipe-progress, 0);
		background: linear-gradient(
			115deg,
			rgba(255, 255, 255, 0) 30%,
			rgba(255, 255, 255, 0.28) 50%,
			rgba(255, 255, 255, 0) 70%
		);
		mix-blend-mode: screen;
		transition: opacity 220ms ease;
	}

	.remote-art.swiping .remote-art-sheen {
		transition: none;
	}

	.remote-copy,
	.remote-seek,
	.remote-volume {
		display: grid;
		gap: 8px;
	}

	.remote-copy {
		text-align: center;
	}

	.remote-title-row {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		min-width: 0;
	}

	.remote-title-row strong {
		min-width: 0;
	}

	.remote-copy strong,
	.remote-copy span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-copy-link {
		display: inline-block;
		justify-self: center;
		max-width: 100%;
		padding: 2px 6px;
		margin-inline: auto;
		background: transparent;
		color: var(--text-primary);
		font: inherit;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		border-radius: 6px;
	}

	.remote-copy-link:active {
		background: var(--surface-1);
	}

	.remote-copy-album {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-stream {
		display: inline-block;
		justify-self: center;
		padding: 2px 10px;
		border-radius: 999px;
		background: var(--surface-1);
		color: var(--text-muted);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.04em;
		text-transform: uppercase;
	}

	.remote-favorite {
		flex-shrink: 0;
		width: 36px;
		height: 36px;
		display: grid;
		place-items: center;
		background: transparent;
		color: var(--text-muted);
	}

	.remote-favorite svg {
		width: 22px;
		height: 22px;
	}

	.remote-favorite.active {
		color: var(--accent);
	}

	.remote-favorite:disabled {
		opacity: 0.4;
	}

	.remote-time,
	.remote-buttons {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}

	.remote-buttons button {
		min-height: 52px;
		flex: 1;
		border-radius: 10px;
		background: var(--surface-1);
		color: var(--text-primary);
		font-weight: var(--font-weight-semibold);
	}

	.remote-buttons button:active {
		background: var(--surface-2);
	}

	.remote-buttons .primary {
		flex: 1.4;
		background: var(--accent);
		color: var(--surface-0);
	}

	.remote-modes {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 8px;
	}

	.remote-mode {
		min-height: 44px;
		padding: 4px 6px;
		border-radius: 8px;
		background: var(--surface-1);
		color: var(--text-primary);
		display: grid;
		gap: 1px;
		align-content: center;
		justify-items: center;
	}

	.remote-mode:active {
		background: var(--surface-2);
	}

	.remote-mode.active {
		background: var(--accent);
		color: var(--surface-0);
	}

	.remote-mode-label {
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		opacity: 0.7;
	}

	.remote-mode-value {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}
</style>
