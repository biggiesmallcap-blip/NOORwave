<script lang="ts">
	import type { Album, Track } from '$lib/api/client';
	import { currentTrack, isPlaying, playTracksInContext, playAlbum, shuffleAlbum } from '$lib/stores/player';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { formatTrackDuration } from '$lib/utils/format';
	import {
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize,
	} from '$lib/utils/artwork';
	import { portal } from '$lib/actions/portal';

	let { album, tracks, loading, onClose }: {
		album: Album;
		tracks: Track[];
		loading: boolean;
		onClose: () => void;
	} = $props();
	let failedArtworkUrls = $state<Record<string, boolean>>({});
	let popupArtwork = $derived(artworkCandidate(album.artwork_url, 640));

	// Scroll-to-dismiss. The track list scrolls normally; once it can't scroll
	// further (or the wheel lands on the surrounding backdrop) a scroll gesture
	// gently collapses the popup so the user never has to reach for the close
	// button. `closing` plays the exit animation, then finishClose unmounts.
	let panelEl = $state<HTMLDivElement | null>(null);
	let closing = $state(false);
	let closed = false;
	const WHEEL_DISMISS_THRESHOLD = 6;

	function finishClose() {
		if (closed) return;
		closed = true;
		onClose();
	}

	function requestClose() {
		if (closing) return;
		closing = true;
		// Fallback in case animationend doesn't fire (reduced-motion, interrupted).
		setTimeout(finishClose, 240);
	}

	function isInsidePanel(target: EventTarget | null): boolean {
		return panelEl != null && target instanceof Node && panelEl.contains(target);
	}

	// Scrolling over the panel browses the track list; a scroll on the page area
	// behind dismisses. Crucially the backdrop never intercepts the wheel, so the
	// page's own scroller is the wheel target from the first notch and keeps
	// scrolling straight through the dismiss. (Chromium latches a wheel gesture to
	// its initial target for the whole sequence; if the backdrop were the target
	// and we removed it, the page wouldn't resume scrolling until the mouse moved.)
	function handleWheel(e: WheelEvent) {
		if (closing || isInsidePanel(e.target)) return;
		if (Math.abs(e.deltaY) < WHEEL_DISMISS_THRESHOLD) return;
		requestClose();
	}

	// With the backdrop non-blocking, a click outside the panel is caught here:
	// close, and swallow it so a card behind the popup isn't activated.
	function handleOutsideClick(e: MouseEvent) {
		if (closing || isInsidePanel(e.target)) return;
		e.preventDefault();
		e.stopPropagation();
		requestClose();
	}

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

	function handleKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			requestClose();
		}
	}

	// Clicking a track plays the album in context (this track and the ones after
	// it), matching every other list surface, rather than playing one orphan track.
	function playFromHere(track: Track) {
		void playTracksInContext(tracks.map((t) => t.id), track.id);
	}

	function trackMenu(track: Track) {
		return buildTrackMenu(track);
	}

	function openAlbumContextMenu(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildAlbumMenu(album, { isLocal: true }), album.title);
	}

	function openAlbumArtistContextMenu(event: MouseEvent) {
		if (!album.artist_name) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildArtistMenu({ id: album.artist_id, name: album.artist_name, in_library: true }, { isLocal: true }),
			album.artist_name
		);
	}

	function openTrackArtistContextMenu(event: MouseEvent, track: Track) {
		if (!track.artist_name || !track.artist_id) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildArtistMenu({ id: track.artist_id, name: track.artist_name, in_library: true }, { isLocal: true }),
			track.artist_name
		);
	}
</script>

<svelte:window onkeydown={handleKey} onwheel={handleWheel} onclickcapture={handleOutsideClick} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
	class="popup-backdrop"
	class:closing
	role="presentation"
	use:portal
>
	<div
		class="popup-panel"
		class:closing
		bind:this={panelEl}
		role="dialog"
		tabindex="-1"
		aria-modal="true"
		aria-label={album.title}
		onclick={(e) => e.stopPropagation()}
		onanimationend={() => { if (closing) finishClose(); }}
	>
		<button class="popup-close" aria-label="Close" onclick={requestClose}>
			<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
				<path d="M18 6 6 18M6 6l12 12" />
			</svg>
		</button>

		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="popup-hero" oncontextmenu={openAlbumContextMenu}>
			{#if popupArtwork}
				<div class="popup-ambient" style:background-image={`url("${popupArtwork}")`} aria-hidden="true"></div>
			{/if}
			<div class="popup-hero-inner">
				{#if popupArtwork}
					<img
						class="popup-art"
						src={popupArtwork}
						alt={album.title}
						onerror={() => markArtworkFailed(popupArtwork)}
					/>
				{:else}
					<div class="popup-art placeholder">♫</div>
				{/if}
				<div class="popup-info">
					<h2>{album.title}</h2>
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<p
						class="popup-artist"
						oncontextmenu={openAlbumArtistContextMenu}
					>{album.artist_name ?? 'Unknown Artist'}</p>
					<div class="popup-meta-row">
						{#if album.year}<span class="popup-chip">{album.year}</span>{/if}
						{#if album.release_type}<span class="popup-chip">{album.release_type}</span>{/if}
						{#if album.track_count}<span class="popup-chip">{album.track_count} {album.track_count === 1 ? 'track' : 'tracks'}</span>{/if}
						<span class="popup-chip">{album.source}</span>
					</div>
					<div class="popup-actions">
						<button class="popup-cta popup-cta--primary" onclick={() => void playAlbum(album.id)}>
							<svg viewBox="0 0 24 24" width="15" height="15" aria-hidden="true"><path d="M8 5v14l11-7z" fill="currentColor" /></svg>
							Play
						</button>
						<button class="popup-cta popup-cta--ghost" onclick={() => void shuffleAlbum(album.id)}>
							<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
								<path d="M16 3h5v5" /><path d="M4 20 21 3" /><path d="M21 16v5h-5" /><path d="m15 15 6 6" /><path d="M4 4l5 5" />
							</svg>
							Shuffle
						</button>
					</div>
				</div>
			</div>
		</div>

		{#if loading}
			<div class="popup-loading"><div class="spinner spinner-sm"></div><span>Loading tracks…</span></div>
		{:else if tracks.length === 0}
			<div class="popup-empty">No tracks synced yet.</div>
		{:else}
			<div class="popup-track-list">
				{#each tracks as track, i (track.id)}
					{@const isCurrent = $currentTrack?.id === track.id}
					<div
						class="popup-track-row"
						class:playing={isCurrent}
						role="button"
						tabindex="0"
						onclick={() => playFromHere(track)}
						onkeydown={(e) => e.key === 'Enter' && playFromHere(track)}
						oncontextmenu={(e) => {
							e.preventDefault();
							e.stopPropagation();
							openContextMenu(e, trackMenu(track), track.title);
						}}
					>
						<span class="popup-track-index">
							{#if isCurrent}
								<span class="popup-eq" class:paused={!$isPlaying} aria-hidden="true"><i></i><i></i><i></i></span>
							{:else}
								<span class="popup-track-num">{i + 1}</span>
								<svg class="popup-row-play" viewBox="0 0 24 24" width="12" height="12" aria-hidden="true"><path d="M8 5v14l11-7z" fill="currentColor" /></svg>
							{/if}
						</span>
						<span class="popup-track-title">{track.title}</span>
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<span
							class="popup-track-artist"
							oncontextmenu={(e) => openTrackArtistContextMenu(e, track)}
						>{track.artist_name ?? ''}</span>
						<span class="popup-track-duration">{formatTrackDuration(track.duration_ms)}</span>
						<button
							class="popup-track-menu"
							aria-label="Track actions"
							onclick={(e) => {
								e.preventDefault();
								e.stopPropagation();
								openMenuAtElement(e.currentTarget, trackMenu(track), track.title);
							}}
						>
							<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true"><circle cx="5" cy="12" r="1.6" /><circle cx="12" cy="12" r="1.6" /><circle cx="19" cy="12" r="1.6" /></svg>
						</button>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>

<style>
	.popup-backdrop {
		position: fixed;
		inset: 0;
		background: transparent;
		z-index: 80;
		display: flex;
		align-items: center;
		justify-content: center;
		/* Never intercept the wheel: the page behind stays the scroll target so a
		   scroll-dismiss doesn't freeze page scrolling. The panel re-enables pointer
		   events for its own interactions; outside clicks are handled on window. */
		pointer-events: none;
		animation: backdrop-fade 180ms ease-out both;
	}

	@keyframes backdrop-fade {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.popup-panel {
		position: relative;
		width: min(820px, 92vw);
		max-height: 86vh;
		pointer-events: auto;
		display: flex;
		flex-direction: column;
		border-radius: var(--radius-lg, 18px);
		border: 1px solid var(--panel-border);
		background: var(--bg-elevated);
		box-shadow:
			0 28px 70px -22px rgba(0, 0, 0, 0.6),
			0 2px 8px -2px rgba(0, 0, 0, 0.3);
		animation: popup-bloom 240ms cubic-bezier(0.22, 1, 0.36, 1) both;
		overflow: hidden;
	}

	@keyframes popup-bloom {
		from {
			opacity: 0;
			transform: scale(0.975) translateY(10px);
		}
		to {
			opacity: 1;
			transform: scale(1) translateY(0);
		}
	}

	.popup-panel.closing {
		animation: popup-collapse 200ms cubic-bezier(0.4, 0, 1, 1) forwards;
		pointer-events: none;
	}

	@keyframes popup-collapse {
		from {
			opacity: 1;
			transform: scale(1) translateY(0);
		}
		to {
			opacity: 0;
			transform: scale(0.965) translateY(16px);
		}
	}

	.popup-close {
		position: absolute;
		top: 14px;
		right: 14px;
		z-index: 3;
		display: grid;
		place-items: center;
		background: color-mix(in srgb, var(--bg-elevated) 55%, transparent);
		border: 1px solid var(--panel-border);
		border-radius: 999px;
		width: 32px;
		height: 32px;
		color: var(--text-secondary);
		cursor: pointer;
		backdrop-filter: blur(8px);
		-webkit-backdrop-filter: blur(8px);
		transition: background 120ms ease, color 120ms ease, transform 120ms ease;
	}
	.popup-close:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
		transform: scale(1.06);
	}

	/* ── Hero with ambient artwork wash ─────────────────────────────────────── */
	.popup-hero {
		position: relative;
		padding: 30px 28px 24px;
		overflow: hidden;
		isolation: isolate;
	}

	/* Blurred, color-bled copy of the cover that gives the modal the album's own
	   palette. Faded into the panel surface by the scrim below so text stays
	   legible in both themes. */
	.popup-ambient {
		position: absolute;
		inset: -50% -15% auto -15%;
		height: 220%;
		background-size: cover;
		background-position: center;
		filter: blur(48px) saturate(1.45);
		opacity: 0.45;
		transform: scale(1.15);
		z-index: -2;
		pointer-events: none;
	}
	.popup-hero::after {
		content: '';
		position: absolute;
		inset: 0;
		background: linear-gradient(
			180deg,
			color-mix(in srgb, var(--bg-elevated) 30%, transparent) 0%,
			color-mix(in srgb, var(--bg-elevated) 78%, transparent) 62%,
			var(--bg-elevated) 100%
		);
		z-index: -1;
		pointer-events: none;
	}

	.popup-hero-inner {
		display: grid;
		grid-template-columns: 168px 1fr;
		gap: 22px;
		align-items: end;
	}

	.popup-art {
		width: 168px;
		height: 168px;
		border-radius: 14px;
		object-fit: cover;
		box-shadow: 0 16px 40px -12px rgba(0, 0, 0, 0.6);
	}
	.popup-art.placeholder {
		display: grid;
		place-items: center;
		font-size: var(--font-size-4xl);
		color: var(--text-tertiary);
		background: var(--surface-1);
	}

	.popup-info {
		display: flex;
		flex-direction: column;
		gap: 9px;
		min-width: 0;
		padding-bottom: 2px;
	}

	.popup-info h2 {
		margin: 0;
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-bold, 700);
		line-height: var(--line-height-snug);
		letter-spacing: -0.01em;
		color: var(--text-primary);
	}

	.popup-artist {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-medium);
		cursor: context-menu;
		width: fit-content;
	}

	.popup-meta-row {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 2px;
	}

	.popup-chip {
		padding: 3px 10px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--bg-elevated) 40%, transparent);
		border: 1px solid var(--panel-border);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		text-transform: capitalize;
		backdrop-filter: blur(6px);
		-webkit-backdrop-filter: blur(6px);
	}

	.popup-actions {
		display: flex;
		gap: 10px;
		margin-top: 10px;
	}

	.popup-cta {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		padding: 9px 20px;
		border-radius: 999px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold, 600);
		cursor: pointer;
		border: 1px solid transparent;
		transition: transform 120ms ease, background 140ms ease, border-color 140ms ease, box-shadow 140ms ease;
	}
	.popup-cta:active { transform: scale(0.97); }

	.popup-cta--primary {
		background: var(--accent);
		color: #fff;
		box-shadow: 0 8px 22px -8px var(--accent-glow);
	}
	.popup-cta--primary:hover {
		background: var(--accent-strong);
		box-shadow: 0 10px 26px -8px var(--accent-glow);
		transform: translateY(-1px);
	}

	.popup-cta--ghost {
		background: color-mix(in srgb, var(--bg-elevated) 45%, transparent);
		border-color: var(--panel-border);
		color: var(--text-primary);
		backdrop-filter: blur(6px);
		-webkit-backdrop-filter: blur(6px);
	}
	.popup-cta--ghost:hover {
		background: var(--bg-hover);
		border-color: var(--accent-line);
	}

	/* ── Track list ─────────────────────────────────────────────────────────── */
	.popup-track-list {
		max-height: 420px;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: 1px;
		padding: 6px 16px 18px;
		scrollbar-width: thin;
		scrollbar-color: var(--scrollbar-thumb, rgba(255,255,255,0.18)) transparent;
	}

	.popup-track-list::-webkit-scrollbar {
		width: 6px;
	}
	.popup-track-list::-webkit-scrollbar-track {
		background: transparent;
	}
	.popup-track-list::-webkit-scrollbar-thumb {
		background: var(--scrollbar-thumb, rgba(255,255,255,0.18));
		border-radius: 99px;
	}
	.popup-track-list::-webkit-scrollbar-thumb:hover {
		background: var(--scrollbar-thumb-hover, rgba(255,255,255,0.28));
	}

	.popup-track-row {
		display: grid;
		grid-template-columns: 28px minmax(0, 1.4fr) minmax(0, 1fr) 56px 30px;
		gap: 14px;
		align-items: center;
		padding: 9px 12px;
		border-radius: 10px;
		cursor: pointer;
		transition: background 120ms ease;
		min-width: 0;
	}
	.popup-track-row:hover {
		background: var(--bg-hover);
	}
	.popup-track-row.playing {
		background: var(--accent-soft);
	}

	.popup-track-index {
		position: relative;
		display: grid;
		place-items: center;
		width: 28px;
		height: 20px;
	}
	.popup-track-num,
	.popup-row-play {
		grid-area: 1 / 1;
		transition: opacity 120ms ease;
	}
	.popup-track-num {
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
		font-size: var(--font-size-sm);
	}
	.popup-row-play {
		opacity: 0;
		color: var(--text-primary);
	}
	.popup-track-row:hover .popup-track-num { opacity: 0; }
	.popup-track-row:hover .popup-row-play { opacity: 1; }

	.popup-track-title {
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.popup-track-row.playing .popup-track-title {
		color: var(--accent-strong);
		font-weight: var(--font-weight-semibold, 600);
	}

	.popup-track-artist {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		cursor: context-menu;
	}

	.popup-track-duration {
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
		font-size: var(--font-size-sm);
		text-align: right;
	}

	.popup-track-menu {
		display: grid;
		place-items: center;
		width: 30px;
		height: 30px;
		border-radius: 999px;
		background: transparent;
		border: none;
		color: var(--text-tertiary);
		cursor: pointer;
		opacity: 0;
		transition: background 120ms ease, color 120ms ease, opacity 120ms ease;
	}
	.popup-track-row:hover .popup-track-menu,
	.popup-track-row.playing .popup-track-menu {
		opacity: 1;
	}
	.popup-track-menu:hover {
		background: var(--surface-2);
		color: var(--text-primary);
	}

	/* Animated equalizer marking the active row. */
	.popup-eq {
		display: inline-flex;
		align-items: flex-end;
		gap: 2px;
		height: 13px;
	}
	.popup-eq i {
		width: 2.5px;
		border-radius: 2px;
		background: var(--accent);
		animation: eq-bounce 900ms ease-in-out infinite;
	}
	.popup-eq i:nth-child(1) { height: 40%; animation-delay: -200ms; }
	.popup-eq i:nth-child(2) { height: 90%; animation-delay: -500ms; }
	.popup-eq i:nth-child(3) { height: 60%; animation-delay: -100ms; }
	.popup-eq.paused i { animation-play-state: paused; }

	@keyframes eq-bounce {
		0%, 100% { transform: scaleY(0.45); }
		50% { transform: scaleY(1); }
	}

	@media (prefers-reduced-motion: reduce) {
		.popup-eq i { animation: none; height: 70%; }
		.popup-panel,
		.popup-panel.closing { animation: none; }
		.popup-backdrop { animation: none; }
	}

	.popup-loading,
	.popup-empty {
		display: flex;
		align-items: center;
		gap: 10px;
		justify-content: center;
		padding: 32px;
		color: var(--text-secondary);
	}

	@media (max-width: 560px) {
		.popup-hero-inner {
			grid-template-columns: 1fr;
			justify-items: center;
			text-align: center;
			gap: 16px;
		}
		.popup-info { align-items: center; }
		.popup-meta-row,
		.popup-actions { justify-content: center; }
		.popup-track-artist { display: none; }
		.popup-track-row {
			grid-template-columns: 28px minmax(0, 1fr) 56px 30px;
		}
	}
</style>
