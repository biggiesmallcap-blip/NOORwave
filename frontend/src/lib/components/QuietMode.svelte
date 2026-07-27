<script lang="ts">
	import { untrack } from 'svelte';
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
	import { contextMenu, openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { trackToTidalPlayable } from '$lib/utils/track';
	import { buildTrackMenu, buildTidalTrackMenu } from '$lib/player/track_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { getCmdOrCtrlLabel } from '$lib/util/platform';
	import NowPlayingMetadata from '$lib/components/now-playing/NowPlayingMetadata.svelte';
	import NowPlayingProgress from '$lib/components/now-playing/NowPlayingProgress.svelte';
	import NowPlayingTransport from '$lib/components/now-playing/NowPlayingTransport.svelte';
	import {
		normalizeTidalArtworkSize,
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize,
	} from '$lib/utils/artwork';

	let isScrubbing = $state(false);
	let favoritePending = $state(false);
	let failedArtworkUrls = $state<Record<string, boolean>>({});

	const shortcut = $derived(getCmdOrCtrlLabel());
	const playerState = $derived(
		$currentTrack ? ($isPlaying ? 'Playing' : 'Paused') : $playerReady ? 'Ready' : 'Connecting'
	);

	// Opening quiet mode must paint on the first frame. The player bar has already
	// fetched and decoded the 640 cover, so that is the size we render immediately;
	// a larger copy is only swapped in once it has fully decoded off-screen. Asking
	// for 1280 up front cost a cold ~450 KB fetch of a *progressive* JPEG, which the
	// browser paints one refinement scan at a time - the artwork visibly arriving in
	// stages, plus a third stage when the blurred backdrop finally appeared.
	const QUIET_ART_BASE_SIZE = 640;

	let artWrapWidth = $state(0);
	let upgradedArt = $state<{ source: string; url: string; size: number } | null>(null);
	let upgradeFailedUrls = $state<Record<string, boolean>>({});
	let artReadyFor = $state<string | null>(null);

	let quietArtworkBase = $derived(
		artworkCandidate($currentTrack?.artwork_url, QUIET_ART_BASE_SIZE)
	);
	let quietArtwork = $derived(
		upgradedArt && upgradedArt.source === $currentTrack?.artwork_url
			? upgradedArt.url
			: quietArtworkBase
	);
	// The reveal is gated on the bitmap, not on mount: the old fade ran on an empty
	// box and finished long before any pixels arrived.
	let artReady = $derived(!!artReadyFor && artReadyFor === $currentTrack?.artwork_url);
	let quietAlbumHref = $derived.by(() => {
		const track = $currentTrack;
		if (!track) return null;
		if (track.album_id != null) return `/albums/${track.album_id}`;
		if (track.album_tidal_id != null) return `/tidal/albums/${track.album_tidal_id}`;
		return null;
	});

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
		if (upgradedArt?.url === renderedUrl) upgradedArt = null;
		upgradeFailedUrls = { ...upgradeFailedUrls, [renderedUrl]: true };
		failedArtworkUrls = { ...failedArtworkUrls, [renderedUrl]: true };
	}

	// `load` only means the bytes arrived; decode() is what guarantees the bitmap is
	// ready to paint, so the fade can never run ahead of the pixels.
	function markArtworkReady(img: HTMLImageElement) {
		const source = $currentTrack?.artwork_url ?? null;
		void (async () => {
			try {
				await img.decode();
			} catch {
				// A decode failure surfaces through onerror; fall through and reveal
				// anyway rather than leaving the cover invisible.
			}
			if (source === ($currentTrack?.artwork_url ?? null)) artReadyFor = source;
		})();
	}

	// Upgrade to a sharper cover sized to the pixels actually on screen, and only
	// hand it to the DOM once decoded so the swap costs a single atomic frame. On a
	// 1x display the 640 base is already exact, so nothing extra is fetched at all.
	$effect(() => {
		const source = $currentTrack?.artwork_url ?? null;
		const width = artWrapWidth;
		if (!browser || !$quietModeOpen || !source || width <= 0) return;

		const dpr = window.devicePixelRatio || 1;
		const target = normalizeTidalArtworkSize(Math.min(1280, Math.round(width * dpr)));
		if (target <= QUIET_ART_BASE_SIZE) return;

		const url = upscaleTidalArtwork(source, target);
		if (!url || url === quietArtworkBase) return;

		const alreadyUpgraded = untrack(() => upgradedArt);
		if (alreadyUpgraded?.source === source && alreadyUpgraded.size >= target) return;
		if (untrack(() => upgradeFailedUrls)[url]) return;

		let cancelled = false;
		const preload = new Image();
		preload.src = url;
		void (async () => {
			try {
				await preload.decode();
				if (!cancelled) upgradedArt = { source, url, size: target };
			} catch {
				if (!cancelled) upgradeFailedUrls = { ...untrack(() => upgradeFailedUrls), [url]: true };
			}
		})();

		return () => {
			cancelled = true;
		};
	});

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

	function openQuietAlbumContextMenu(e: MouseEvent) {
		const track = $currentTrack;
		if (!track || (track.album_id == null && track.album_tidal_id == null)) return;
		e.preventDefault();
		e.stopPropagation();
		openContextMenu(e, buildAlbumMenu({
			id: track.album_id,
			tidal_id: track.album_tidal_id,
			title: track.album_title ?? track.title,
			artist_id: track.artist_id > 0 ? track.artist_id : null,
			artist_name: track.artist_name,
		}, { isLocal: track.album_id != null }), track.album_title ?? track.title);
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

{#snippet quietArtImage()}
	{#key $currentTrack?.artwork_url}
		{#if quietArtwork}
			<img
				class="quiet-art quiet-art-img"
				class:is-ready={artReady}
				src={quietArtwork}
				alt=""
				decoding="async"
				onload={(e) => markArtworkReady(e.currentTarget as HTMLImageElement)}
				onerror={() => markArtworkFailed(quietArtwork)}
			/>
		{:else}
			<div class="quiet-art quiet-art-placeholder">♫</div>
		{/if}
	{/key}
{/snippet}

{#if $quietModeOpen}
	<div
		class="quiet-backdrop"
		aria-hidden="true"
		transition:fade={{ duration: 220, easing: quintOut }}
	>
		{#if quietArtworkBase}
			<!-- Blurred to 60px, so it reuses the warm base cover; a second high-res
			     fetch bought no detail and landed as its own late visual stage. -->
			<img
				class="quiet-backdrop-art"
				class:is-ready={artReady}
				src={quietArtworkBase}
				alt=""
				decoding="async"
				onerror={() => markArtworkFailed(quietArtworkBase)}
			/>
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
			<div class="quiet-art-wrap" bind:clientWidth={artWrapWidth}>
				{#if quietAlbumHref}
					<a
						class="quiet-art-link"
						href={quietAlbumHref}
						aria-label="Open {$currentTrack.album_title ?? $currentTrack.title}"
						oncontextmenu={openQuietAlbumContextMenu}
					>
						{@render quietArtImage()}
					</a>
				{:else}
					{@render quietArtImage()}
				{/if}
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
		z-index: var(--z-modal);
		background: var(--bg-base, #0b0b14);
		overflow: hidden;
	}

	.quiet-backdrop-art {
		position: absolute;
		inset: -10%;
		width: 120%;
		height: 120%;
		object-fit: cover;
		opacity: 0;
		filter: blur(60px) saturate(160%);
		transform: scale(1.1);
		pointer-events: none;
		transition: opacity 320ms ease;
	}

	.quiet-backdrop-art.is-ready {
		opacity: 0.35;
	}

	.quiet-panel {
		--quiet-panel-pad: clamp(var(--space-3), 3vh, var(--space-6));
		--quiet-panel-gap: clamp(var(--space-2), 1.8vh, var(--space-4));
		--quiet-art-size: clamp(180px, 40vh, 520px);
		--quiet-panel-w: min(var(--quiet-art-size), calc(100vw - (2 * var(--quiet-panel-pad))));

		position: fixed;
		inset: 0;
		z-index: calc(var(--z-modal) + 1);
		display: grid;
		grid-template-columns: minmax(0, var(--quiet-panel-w));
		grid-auto-rows: min-content;
		justify-content: center;
		align-content: center;
		gap: var(--quiet-panel-gap);
		padding: var(--quiet-panel-pad);
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
		font-size: var(--font-size-md);
		cursor: pointer;
		backdrop-filter: var(--blur-overlay);
		-webkit-backdrop-filter: var(--blur-overlay);
		transition: background 160ms ease, transform 160ms ease;
	}

	.quiet-close:hover {
		background: rgba(0, 0, 0, 0.7);
		transform: scale(1.05);
	}

	.quiet-art-wrap {
		width: var(--quiet-panel-w);
		aspect-ratio: 1;
		justify-self: center;
		border-radius: var(--radius-lg);
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
	}

	/* Held back until the bitmap is decoded, so a progressive JPEG never shows its
	   intermediate scans and the fade always runs over real pixels. */
	.quiet-art-img {
		opacity: 0;
		transition: opacity 260ms ease;
	}

	.quiet-art-img.is-ready {
		opacity: 1;
	}

	.quiet-art-link {
		display: block;
		width: 100%;
		height: 100%;
		color: inherit;
		text-decoration: none;
	}

	.quiet-art-placeholder {
		display: grid;
		place-items: center;
		font-size: var(--font-size-4xl);
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
		font-size: var(--font-size-xs);
		cursor: pointer;
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		transition: background 160ms ease, color 160ms ease, border-color 160ms ease;
	}

	.quiet-search-pill:hover {
		background: rgba(255, 255, 255, 0.12);
		color: var(--text-primary, #fff);
		border-color: rgba(255, 255, 255, 0.22);
	}

	.quiet-search-icon {
		font-size: var(--font-size-sm);
		opacity: 0.8;
	}

	.quiet-search-kbd {
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: var(--font-size-xs);
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
		font-size: var(--font-size-xl);
		margin: 0;
		color: var(--text-primary, #fff);
	}

	.quiet-empty-sub {
		font-size: var(--font-size-md);
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
		font-size: var(--font-size-sm);
		cursor: pointer;
		transition: transform 160ms ease, box-shadow 160ms ease;
	}

	.quiet-empty-btn:hover {
		transform: translateY(-1px);
		box-shadow: 0 10px 30px var(--accent-glow, rgba(99, 102, 241, 0.4));
	}
</style>
