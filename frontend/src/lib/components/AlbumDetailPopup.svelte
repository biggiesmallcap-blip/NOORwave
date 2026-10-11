<script lang="ts">
	import type { Album, Track } from '$lib/api/client';
	import { currentTrack, isPlaying, playAlbum, shuffleAlbum } from '$lib/stores/player';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { formatTrackDuration } from '$lib/utils/format';
	import { portal } from '$lib/actions/portal';
	import ActionBar from '$lib/components/ui/ActionBar.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { createArtworkFallback } from '$lib/utils/artwork_fallback.svelte';
	import { goto } from '$app/navigation';

	let { album, tracks, loading, onClose, isLocal = true, onPlay, onShuffle, onPlayFrom, artistHref = null, albumHref = null }: {
		album: Album;
		tracks: Track[];
		loading: boolean;
		onClose: () => void;
		/**
		 * False when this album is not in the library, so `album.id` is not a
		 * real local id. Library always owns its albums and leaves this alone.
		 *
		 * Recommendations do not: a Last.fm album is usually TIDAL-only, and every
		 * default action here is keyed on a local id - `playAlbum(album.id)`,
		 * `shuffleAlbum(album.id)`, and the menu builders' `isLocal: true`. Passing
		 * false swaps the "N in library" chip out and requires the caller to supply
		 * the play handlers, rather than silently calling playAlbum with an id that
		 * does not exist.
		 */
		isLocal?: boolean;
		onPlay?: () => void;
		onShuffle?: () => void;
		/** Play the album starting from this track. */
		onPlayFrom?: (track: Track) => void;
		/**
		 * Where the artist name goes. Only needed when the artist is not a library
		 * row: a TIDAL-only album has no `artist_id`, so the local href cannot be
		 * derived and the caller passes `/tidal/artists/{id}` instead.
		 */
		artistHref?: string | null;
		/**
		 * The full album page. Derived for library albums; a TIDAL-only album
		 * passes `/tidal/albums/{id}`. Null hides the link.
		 */
		albumHref?: string | null;
	} = $props();
	// Artwork URL per size, stepping down a size each time one fails to load.
	const artwork = createArtworkFallback();
	const artworkCandidate = artwork.candidate;
	const markArtworkFailed = artwork.markFailed;
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

	function handleKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			requestClose();
		}
	}

	// Clicking a track plays the album from that track, matching the album page.
	// playAlbum (not playTracksInContext over the popup's owned rows) so a
	// partially-owned album queues its TIDAL-only remainder too instead of a
	// short owned-only queue that automix pads with unrelated tracks.
	function playFromHere(track: Track) {
		if (onPlayFrom) return onPlayFrom(track);
		void playAlbum(album.id, track.id);
	}

	function playWholeAlbum() {
		if (onPlay) return onPlay();
		void playAlbum(album.id);
	}

	function shuffleWholeAlbum() {
		if (onShuffle) return onShuffle();
		void shuffleAlbum(album.id);
	}

	function trackMenu(track: Track) {
		return buildTrackMenu(track);
	}

	function openAlbumContextMenu(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildAlbumMenu(album, { isLocal }), album.title);
	}

	// The artist is a link, not just a right-click target. Library albums derive
	// it from the row; anything else has to be told, because a TIDAL-only album
	// carries no local artist id.
	let resolvedArtistHref = $derived(
		artistHref ?? (isLocal && album.artist_id ? `/artists/${album.artist_id}` : null),
	);

	// Close first, then navigate. Going straight to goto() leaves the popup
	// mounted over the page it just opened, and its outside-click handler is on
	// the window, so the next click anywhere would dismiss instead of doing what
	// it looked like it would do.
	let resolvedAlbumHref = $derived(albumHref ?? (isLocal && album.id != null ? `/albums/${album.id}` : null));

	function openAlbumPage() {
		if (!resolvedAlbumHref) return;
		const href = resolvedAlbumHref;
		requestClose();
		void goto(href);
	}

	function openArtistPage() {
		if (!resolvedArtistHref) return;
		const href = resolvedArtistHref;
		requestClose();
		void goto(href);
	}

	function openAlbumArtistContextMenu(event: MouseEvent) {
		if (!album.artist_name) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildArtistMenu({ id: album.artist_id, name: album.artist_name, in_library: isLocal }, { isLocal }),
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
					{#if resolvedArtistHref}
						<button
							type="button"
							class="popup-artist popup-artist-link"
							title={`Open ${album.artist_name ?? 'artist'}`}
							oncontextmenu={openAlbumArtistContextMenu}
							onclick={openArtistPage}
						>{album.artist_name ?? 'Unknown Artist'}</button>
					{:else}
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<p
							class="popup-artist"
							oncontextmenu={openAlbumArtistContextMenu}
						>{album.artist_name ?? 'Unknown Artist'}</p>
					{/if}
					<div class="popup-meta-row">
						{#if album.year}<span class="popup-chip">{album.year}</span>{/if}
						{#if album.release_type}<span class="popup-chip">{album.release_type}</span>{/if}
						<!-- track_count counts OWNED rows only, so label it as library
						     coverage rather than claiming it's the album's track count. -->
						{#if album.track_count}
							<span class="popup-chip">
								{album.track_count} {isLocal ? 'in library' : 'tracks'}
							</span>
						{/if}
						<span class="popup-chip">{album.source}</span>
					</div>
					<div class="popup-actions">
						<!-- The same action bar as the album page, so the quick view and
						     the page never drift apart (audit round 2). -->
						<ActionBar
							onplay={playWholeAlbum}
							onshuffle={shuffleWholeAlbum}
							shuffleHint="Play this album in random order"
						/>
						{#if resolvedAlbumHref}
							<button type="button" class="popup-link" onclick={openAlbumPage}>Open album page</button>
						{/if}
					</div>
				</div>
			</div>
		</div>

		{#if loading}
			<Skeleton rows={6} label="Loading tracks" />
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
		animation: backdrop-fade var(--motion-base) both;
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
		animation: popup-bloom var(--motion-base) both;
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
		animation: popup-collapse var(--motion-exit) forwards;
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
		transition: background var(--motion-fast), color var(--motion-fast), transform var(--motion-fast);
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

	/* Underline on hover only, the same restraint as artist names in a track row:
	   the name is a link, but it should not compete with the album title. */
	.popup-artist-link {
		background: none;
		border: 0;
		padding: 0;
		font: inherit;
		text-align: left;
		cursor: pointer;
	}

	.popup-artist-link:hover,
	.popup-artist-link:focus-visible {
		color: var(--text-primary);
		text-decoration: underline;
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
		align-items: center;
		gap: 10px;
		margin-top: 10px;
	}

	/* A quiet text link, pushed to the end of the row so Play stays the CTA. */
	.popup-link {
		margin-left: auto;
		padding: 4px 2px;
		background: none;
		border: 0;
		color: var(--text-secondary);
		font: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold, 600);
		cursor: pointer;
	}

	.popup-link:hover {
		color: var(--text-primary);
		text-decoration: underline;
	}

	.popup-link:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
		border-radius: 4px;
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
		transition: background var(--motion-fast);
		min-width: 0;
	}
	.popup-track-row:hover {
		background: var(--bg-hover);
	}
	.popup-track-row.playing {
		background: var(--playing-soft);
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
		transition: opacity var(--motion-fast);
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
		color: var(--playing);
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
		transition: background var(--motion-fast), color var(--motion-fast), opacity var(--motion-fast);
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
