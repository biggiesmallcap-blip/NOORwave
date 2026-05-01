<script lang="ts">
	import { fade, scale } from 'svelte/transition';
	import { quintOut } from 'svelte/easing';
	import { browser } from '$app/environment';
	import { page } from '$app/state';
	import { get } from 'svelte/store';
	import {
		currentTrack,
		isPlaying,
		position,
		shuffleMode,
		repeatMode,
		togglePlayback,
		playPreviousTrack,
		playNextTrack,
		setPlayerPosition,
		cyclePlayerShuffleMode,
		cyclePlayerRepeatMode,
		toggleTrackFavorite,
		playerReady,
	} from '$lib/stores/player';
	import { quietModeOpen, closeQuietMode } from '$lib/stores/quiet_mode';
	import { commandPaletteOpen } from '$lib/stores/command_palette';
	import { contextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { trackToTidalPlayable } from '$lib/utils/track';
	import { buildTrackMenu, buildTidalTrackMenu } from '$lib/player/track_menu';
	import { getCmdOrCtrlLabel } from '$lib/util/platform';
	import NowPlayingMetadata from '$lib/components/now-playing/NowPlayingMetadata.svelte';
	import NowPlayingProgress from '$lib/components/now-playing/NowPlayingProgress.svelte';
	import NowPlayingTransport from '$lib/components/now-playing/NowPlayingTransport.svelte';

	let isScrubbing = $state(false);
	let favoritePending = $state(false);

	const shortcut = $derived(getCmdOrCtrlLabel());
	const playerState = $derived(
		$currentTrack ? ($isPlaying ? 'Playing' : 'Paused') : $playerReady ? 'Ready' : 'Connecting'
	);

	// Esc handler — defers to context menu and palette if either is open.
	function onWindowKeydown(e: KeyboardEvent) {
		if (e.key !== 'Escape') return;
		if (!$quietModeOpen) return;
		if (get(contextMenu).open) return;
		if (get(commandPaletteOpen)) return;
		e.preventDefault();
		closeQuietMode();
	}

	// Body scroll lock (SSR-safe; cleanup on close).
	$effect(() => {
		if (!browser || !$quietModeOpen) return;
		const prev = document.body.style.overflow;
		document.body.style.overflow = 'hidden';
		return () => {
			document.body.style.overflow = prev;
		};
	});

	// Route-change cleanup with mount guard so initial hydration doesn't close.
	let lastPath = $state('');
	let hasInitialised = $state(false);
	$effect(() => {
		const path = page.url.pathname;
		if (!hasInitialised) {
			hasInitialised = true;
			lastPath = path;
			return;
		}
		if (path !== lastPath) {
			lastPath = path;
			if ($quietModeOpen) closeQuietMode();
		}
	});

	async function handleFavoriteToggle() {
		if (!$currentTrack || favoritePending) return;
		favoritePending = true;
		try {
			await toggleTrackFavorite($currentTrack.id);
		} finally {
			favoritePending = false;
		}
	}

	function openMore(anchor: HTMLElement) {
		const track = $currentTrack;
		if (!track) return;
		const tidal = trackToTidalPlayable(track);
		const items = tidal ? buildTidalTrackMenu(tidal) : buildTrackMenu(track);
		openMenuAtElement(anchor, items, track.title);
	}

	function openSearch() {
		commandPaletteOpen.set(true);
	}

	function onPanelClick(e: MouseEvent) {
		// Only close when the click landed on the panel itself (the empty space
		// around the centred column), not on any of its children.
		if (e.target === e.currentTarget) closeQuietMode();
	}
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#if $quietModeOpen}
	<div
		class="quiet-backdrop"
		aria-hidden="true"
		transition:fade={{ duration: 200 }}
	>
		{#if $currentTrack?.artwork_url}
			<img class="quiet-backdrop-art" src={$currentTrack.artwork_url} alt="" />
		{/if}
	</div>

	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<div
		class="quiet-panel"
		role="dialog"
		aria-modal="true"
		aria-label="Quiet mode"
		tabindex="-1"
		onclick={onPanelClick}
		transition:scale={{ duration: 220, start: 0.96, easing: quintOut }}
	>
		<button
			class="quiet-close"
			aria-label="Exit quiet mode"
			title="Exit quiet mode (Esc)"
			onclick={closeQuietMode}
		>✕</button>

		{#if $currentTrack}
			<div class="quiet-art-wrap">
				{#key $currentTrack.artwork_url}
					{#if $currentTrack.artwork_url}
						<img class="quiet-art" src={$currentTrack.artwork_url} alt="" />
					{:else}
						<div class="quiet-art quiet-art-placeholder">♫</div>
					{/if}
				{/key}
			</div>

			<div class="quiet-meta">
				<NowPlayingMetadata
					track={$currentTrack}
					eyebrow="Quiet mode"
					playerState={playerState}
					isScrubbing={isScrubbing}
					showStateBadge={false}
				/>
			</div>

			<div class="quiet-progress">
				<NowPlayingProgress
					position={$position}
					duration={$currentTrack.duration_ms ?? 0}
					onSeek={(p) => void setPlayerPosition(p)}
					onScrubStart={() => { isScrubbing = true; }}
					onScrubEnd={() => { isScrubbing = false; }}
				/>
			</div>

			<NowPlayingTransport
				track={$currentTrack}
				isPlaying={$isPlaying}
				shuffleMode={$shuffleMode}
				repeatMode={$repeatMode}
				favoritePending={favoritePending}
				onToggleFavorite={() => void handleFavoriteToggle()}
				onCycleShuffle={() => void cyclePlayerShuffleMode()}
				onPrev={() => void playPreviousTrack()}
				onPlayPause={() => void togglePlayback()}
				onNext={() => void playNextTrack()}
				onCycleRepeat={() => void cyclePlayerRepeatMode()}
				onOpenMore={(anchor) => openMore(anchor)}
			/>

			<button class="quiet-search-pill" onclick={openSearch}>
				<span class="quiet-search-icon" aria-hidden="true">⌕</span>
				<span class="quiet-search-text">Search or run a command</span>
				<kbd class="quiet-search-kbd">{shortcut}K</kbd>
			</button>
		{:else}
			<div class="quiet-empty">
				<p class="quiet-empty-title">Nothing playing</p>
				<p class="quiet-empty-sub">Start a track or press {shortcut}+K to search.</p>
				<button class="quiet-empty-btn" onclick={openSearch}>Open search</button>
			</div>
		{/if}
	</div>
{/if}

<style>
	.quiet-backdrop {
		position: fixed;
		inset: 0;
		z-index: 1500;
		background: var(--bg-base, #0b0b14);
		overflow: hidden;
	}

	.quiet-backdrop-art {
		position: absolute;
		inset: -10%;
		width: 120%;
		height: 120%;
		object-fit: cover;
		opacity: 0.35;
		filter: blur(60px) saturate(160%);
		transform: scale(1.1);
		pointer-events: none;
	}

	.quiet-panel {
		position: fixed;
		inset: 0;
		z-index: 1501;
		display: grid;
		grid-template-columns: minmax(0, 520px);
		grid-auto-rows: min-content;
		justify-content: center;
		align-content: center;
		gap: 22px;
		padding: 32px 24px;
		box-sizing: border-box;
		outline: none;
	}

	.quiet-close {
		position: fixed;
		top: 18px;
		right: 18px;
		width: 38px;
		height: 38px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: rgba(0, 0, 0, 0.45);
		border: 1px solid rgba(255, 255, 255, 0.18);
		color: #fff;
		font-size: 16px;
		cursor: pointer;
		backdrop-filter: blur(10px);
		transition: background 160ms ease, transform 160ms ease;
	}

	.quiet-close:hover {
		background: rgba(0, 0, 0, 0.7);
		transform: scale(1.05);
	}

	.quiet-art-wrap {
		width: 100%;
		max-width: min(60vh, 520px);
		aspect-ratio: 1;
		justify-self: center;
		border-radius: 22px;
		overflow: hidden;
		box-shadow:
			0 30px 80px -20px rgba(0, 0, 0, 0.7),
			0 8px 24px rgba(0, 0, 0, 0.45);
	}

	.quiet-art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
		animation: quiet-art-fade 360ms ease both;
	}

	@keyframes quiet-art-fade {
		from { opacity: 0; transform: scale(1.04); }
		to   { opacity: 1; transform: scale(1); }
	}

	.quiet-art-placeholder {
		display: grid;
		place-items: center;
		font-size: 4rem;
		color: var(--text-tertiary, rgba(255, 255, 255, 0.4));
		background: var(--bg-surface, #1a1a26);
	}

	.quiet-meta {
		text-align: center;
	}
	/* Center the metadata block inside its container */
	.quiet-meta :global(.np-info) {
		align-items: center;
		text-align: center;
	}
	.quiet-meta :global(.np-copy) {
		align-items: center;
		text-align: center;
	}

	.quiet-progress {
		width: 100%;
	}

	.quiet-search-pill {
		justify-self: center;
		margin-top: 4px;
		display: inline-flex;
		align-items: center;
		gap: 10px;
		padding: 8px 14px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.12);
		color: var(--text-secondary, rgba(255, 255, 255, 0.7));
		font-size: 0.78rem;
		cursor: pointer;
		backdrop-filter: blur(8px);
		transition: background 160ms ease, color 160ms ease, border-color 160ms ease;
	}

	.quiet-search-pill:hover {
		background: rgba(255, 255, 255, 0.12);
		color: var(--text-primary, #fff);
		border-color: rgba(255, 255, 255, 0.22);
	}

	.quiet-search-icon {
		font-size: 14px;
		opacity: 0.8;
	}

	.quiet-search-kbd {
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: 11px;
		padding: 1px 6px;
		border-radius: 4px;
		background: rgba(255, 255, 255, 0.1);
		border: 1px solid rgba(255, 255, 255, 0.15);
		color: var(--text-secondary, rgba(255, 255, 255, 0.85));
	}

	.quiet-empty {
		text-align: center;
		display: flex;
		flex-direction: column;
		gap: 12px;
		align-items: center;
		padding: 40px 20px;
	}

	.quiet-empty-title {
		font-family: var(--font-display);
		font-size: 1.6rem;
		margin: 0;
		color: var(--text-primary, #fff);
	}

	.quiet-empty-sub {
		font-size: 0.95rem;
		color: var(--text-secondary, rgba(255, 255, 255, 0.7));
		margin: 0;
	}

	.quiet-empty-btn {
		margin-top: 8px;
		padding: 10px 22px;
		border-radius: 999px;
		background: var(--accent, #6366f1);
		color: #fff;
		border: none;
		font-size: 0.9rem;
		cursor: pointer;
		transition: transform 160ms ease, box-shadow 160ms ease;
	}

	.quiet-empty-btn:hover {
		transform: translateY(-1px);
		box-shadow: 0 10px 30px var(--accent-glow, rgba(99, 102, 241, 0.4));
	}
</style>
