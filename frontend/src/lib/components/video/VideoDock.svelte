<script lang="ts">
	import { onDestroy } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '$lib/api/client';
	import VideoPlayer from '$lib/components/video/VideoPlayer.svelte';
	import { audioSettings } from '$lib/stores/audio_settings';
	import { isPlaying } from '$lib/stores/player';
	import {
		advanceVideo,
		nextVideo,
		noteVideoProgress,
		previousVideo,
		clearVideoSession,
		refreshVideoStream,
		refillVideoRadio,
		reportVideoEnded,
		videoPanelAnchor,
		videoSession,
		videoSessionUpcoming,
		videoStageAnchor,
		type PreloadedVideoStream,
	} from '$lib/stores/video_session';
	import {
		cornerForKey,
		loadCorner,
		miniSize,
		nearestCorner,
		placeMini,
		saveCorner,
		type Bounds,
		type Corner,
	} from './mini_dock';
	import { WATCH_PATH } from '$lib/video/section';

	// The dock renders a single VideoPlayer that never unmounts while a session
	// is active, so audio keeps playing across route changes. Where it sits:
	//
	// - full: on /videos/watch, positioned over the watch page's stage.
	// - panel: the video queue panel is open in a side layout, so the video
	//   plays in that panel's artwork slot. No floating window to cover the
	//   queue, and no still image duplicating the video.
	// - mini: a floating window inside the content area (never over the side
	//   panel or the bottom player bar), snapped to a corner the listener can
	//   change by dragging or with the arrow keys, and shrinkable to a pill.
	//
	// Full mode keys off the published stage, not the path: only the watch
	// page publishes one, and a stage hidden under search results (zero size)
	// drops the player to the corner instead of shrinking it to nothing.
	let onWatchPage = $derived(page.url.pathname === WATCH_PATH);
	let active = $derived($videoSession.active && Boolean($videoSession.streamUrl));
	/** Set by the frame loop: the panel's artwork slot exists and is big enough
	 *  to watch in (the bottom layout's 96 px thumbnail is not). */
	let panelUsable = $state(false);
	/** Set by the frame loop: the watch page's stage exists and has a size. */
	let stageUsable = $state(false);
	let mode = $derived<'full' | 'panel' | 'mini'>(
		stageUsable ? 'full' : panelUsable ? 'panel' : 'mini'
	);
	const PANEL_MIN_WIDTH = 200;
	const MORPH_MS = 320;

	let qualityMode = $derived($audioSettings.settings?.video_quality_mode ?? 'MAX');
	let upNext = $derived($videoSessionUpcoming[0] ?? null);
	let hasNext = $derived($videoSessionUpcoming.length > 0 || ($videoSession.continuous && $videoSession.autoplay));
	let hasPrevious = $derived($videoSession.currentIndex > 0);

	// --- Anchor and bounds tracking ---
	type Rect = { top: number; left: number; width: number; height: number };
	let rect = $state<Rect | null>(null);
	let bounds = $state<Bounds | null>(null);
	let viewportWidth = $state(typeof window === 'undefined' ? 1280 : window.innerWidth);
	let viewportHeight = $state(typeof window === 'undefined' ? 800 : window.innerHeight);
	let rafId = 0;
	let workspace: HTMLElement | null = null;

	function sameRect(a: Rect | null, b: DOMRect) {
		return a !== null && a.top === b.top && a.left === b.left && a.width === b.width && a.height === b.height;
	}

	function track() {
		const panel = $videoPanelAnchor;
		const panelRect = panel?.isConnected ? panel.getBoundingClientRect() : null;
		const usable = Boolean(panelRect && panelRect.width >= PANEL_MIN_WIDTH && panelRect.height > 0);
		if (usable !== panelUsable) panelUsable = usable;
		const stage = $videoStageAnchor;
		const stageRect = stage?.isConnected ? stage.getBoundingClientRect() : null;
		const stageOk = Boolean(stageRect && stageRect.width > 0 && stageRect.height > 0);
		if (stageOk !== stageUsable) stageUsable = stageOk;

		const anchor = mode === 'full' ? stage : mode === 'panel' ? panel : null;
		if (active && anchor) {
			const r = anchor === panel && panelRect ? panelRect : anchor === stage && stageRect ? stageRect : anchor.getBoundingClientRect();
			if (!sameRect(rect, r)) rect = { top: r.top, left: r.left, width: r.width, height: r.height };
		} else if (rect !== null) {
			rect = null;
		}

		// Bounds are tracked in every mode, not just mini: the frame the stage
		// goes away must already know the corner, or the window shows for a
		// frame at the CSS fallback spot and then snaps (the "jerk").
		if (active) {
			if (!workspace?.isConnected) workspace = document.querySelector('main.workspace');
			const w = workspace?.getBoundingClientRect();
			if (w && (!bounds || bounds.left !== w.left || bounds.top !== w.top || bounds.right !== w.right || bounds.bottom !== w.bottom)) {
				bounds = { left: w.left, top: w.top, right: w.right, bottom: w.bottom };
			}
			if (window.innerWidth !== viewportWidth) viewportWidth = window.innerWidth;
			if (window.innerHeight !== viewportHeight) viewportHeight = window.innerHeight;
		}
		rafId = requestAnimationFrame(track);
	}
	rafId = requestAnimationFrame(track);

	// --- Floating window: corner, drag, keyboard, minimise ---
	let corner = $state<Corner>(loadCorner());
	let collapsed = $state(false);
	let drag = $state<{ left: number; top: number; dx: number; dy: number; pointerId: number } | null>(null);

	/** Phones keep the old CSS corner; the workspace there sits under a
	 *  bottom nav that the window must clear. */
	let placed = $derived(mode === 'mini' && bounds !== null && viewportWidth > 720);
	let size = $derived(miniSize(viewportWidth, collapsed));
	let position = $derived.by(() => {
		if (!placed || !bounds) return null;
		if (drag) return { left: drag.left, top: drag.top };
		return placeMini(bounds, corner, size);
	});

	function moveTo(next: Corner) {
		corner = next;
		saveCorner(next);
	}

	function startDrag(event: PointerEvent) {
		if (!position || event.button !== 0) return;
		const handle = event.currentTarget as HTMLElement;
		handle.setPointerCapture(event.pointerId);
		drag = {
			left: position.left,
			top: position.top,
			dx: event.clientX - position.left,
			dy: event.clientY - position.top,
			pointerId: event.pointerId,
		};
	}

	function moveDrag(event: PointerEvent) {
		if (!drag || event.pointerId !== drag.pointerId || !bounds) return;
		drag = {
			...drag,
			left: Math.min(Math.max(event.clientX - drag.dx, bounds.left), bounds.right - size.width),
			top: Math.min(Math.max(event.clientY - drag.dy, bounds.top), bounds.bottom - size.height),
		};
	}

	function endDrag(event: PointerEvent) {
		if (!drag || event.pointerId !== drag.pointerId) return;
		if (bounds) moveTo(nearestCorner(bounds, drag.left + size.width / 2, drag.top + size.height / 2));
		drag = null;
	}

	function moveWithKeys(event: KeyboardEvent) {
		const next = cornerForKey(corner, event.key);
		if (next === corner) return;
		event.preventDefault();
		moveTo(next);
	}

	// A new session opens as a window, not as a leftover pill.
	$effect(() => {
		if (!active) collapsed = false;
	});

	// --- Fullscreen: the same player grows to fill the window ---
	// Not native element fullscreen, which swaps the video into the top layer
	// in one hard cut. The dock glides from wherever it sits to cover the
	// window while the window itself goes fullscreen underneath, and glides
	// back on exit (button, double-click, F, or Esc).
	let expanded = $state(false);

	// One motion at a time. Asking for window fullscreen during the glide
	// resized the window mid-flight, so the glide's target moved under it and
	// the two fought (visible jank in Chrome and WebView2 alike). In: glide to
	// fill the current window, then go fullscreen - by then the dock is pinned
	// to the window edges and simply grows with it. Out: leave fullscreen
	// first, let the page settle at its normal size, then glide back.
	let fullscreenTimer: ReturnType<typeof setTimeout> | null = null;

	function toggleExpanded() {
		if (fullscreenTimer) {
			clearTimeout(fullscreenTimer);
			fullscreenTimer = null;
		}
		if (expanded) {
			if (document.fullscreenElement) {
				// fullscreenchange collapses once the window is back to size.
				void document.exitFullscreen().catch(() => (expanded = false));
			} else {
				expanded = false;
			}
		} else {
			expanded = true;
			fullscreenTimer = setTimeout(() => {
				fullscreenTimer = null;
				if (expanded && !document.fullscreenElement) {
					void document.documentElement.requestFullscreen?.().catch(() => {});
				}
			}, MORPH_MS);
		}
	}

	$effect(() => {
		if (!active) expanded = false;
	});

	$effect(() => {
		// Esc (or any other way out of window fullscreen) collapses too.
		const onFullscreenChange = () => {
			if (document.fullscreenElement || !expanded) return;
			// Two frames: the window has resized and the stage has re-laid out,
			// so the glide back aims at where the player really belongs.
			requestAnimationFrame(() => requestAnimationFrame(() => (expanded = false)));
		};
		// Esc with no window fullscreen (the request can be refused).
		const onKeydown = (event: KeyboardEvent) => {
			if (event.key === 'Escape' && expanded && !document.fullscreenElement) expanded = false;
		};
		document.addEventListener('fullscreenchange', onFullscreenChange);
		window.addEventListener('keydown', onKeydown);
		return () => {
			document.removeEventListener('fullscreenchange', onFullscreenChange);
			window.removeEventListener('keydown', onKeydown);
		};
	});

	// --- Moves between places glide ---
	// Stage, corner, queue panel and fullscreen are all the same element, so
	// every move animates left/top/width/height from where it was instead of
	// cutting (watch page -> corner, corner -> watch page, in and out of
	// fullscreen).
	type Place = 'full' | 'panel' | 'mini' | 'expanded';
	let place = $derived<Place>(expanded ? 'expanded' : mode);
	let box = $derived.by(() => {
		// Gliding in: explicit pixels so the size animates. Once there, no
		// inline box at all - CSS pins the dock to the window edges, so the
		// window going fullscreen resizes it in the same frame.
		if (expanded) return morphing ? { top: 0, left: 0, width: viewportWidth, height: viewportHeight } : null;
		if (mode !== 'mini' && rect) return rect;
		if (position) return { top: position.top, left: position.left, width: size.width, height: size.height };
		return null;
	});
	let morphing = $state(false);
	let previousPlace: Place | null = null;
	let morphTimer: ReturnType<typeof setTimeout> | null = null;
	$effect(() => {
		const next = place;
		if (!active) {
			previousPlace = null;
			return;
		}
		if (previousPlace !== null && previousPlace !== next) {
			morphing = true;
			if (morphTimer) clearTimeout(morphTimer);
			morphTimer = setTimeout(() => (morphing = false), MORPH_MS);
		}
		previousPlace = next;
	});

	// --- Prefetch the next stream for gapless autoplay ---
	let prefetched = $state<PreloadedVideoStream & { videoId: number } | null>(null);
	let prefetchSeq = 0;

	$effect(() => {
		const next = upNext;
		const autoplay = $videoSession.autoplay;
		if ($videoSession.continuous && autoplay && $videoSession.queue.length - $videoSession.currentIndex <= 5) {
			void refillVideoRadio();
		}
		if (!autoplay || !next) {
			prefetchSeq += 1;
			prefetched = null;
			return;
		}
		if (prefetched?.videoId === next.tidal_id) return;
		const seq = ++prefetchSeq;
		void api
			.getTidalVideoStream(next.tidal_id)
			.then((stream) => {
				if (seq !== prefetchSeq) return;
				prefetched = { videoId: next.tidal_id, url: stream.hls_url, expiresAt: stream.expires_at };
			})
			.catch(() => {
				if (seq === prefetchSeq) prefetched = null;
			});
	});

	// --- Music takes the device back: starting music stops the video ---
	let wasPlayingAudio = $isPlaying;
	$effect(() => {
		const nowPlaying = $isPlaying;
		if (nowPlaying && !wasPlayingAudio && $videoSession.active) {
			clearVideoSession();
		}
		wasPlayingAudio = nowPlaying;
	});

	async function handleEnded() {
		reportVideoEnded();
		const endedVideoId = $videoSession.current?.tidal_id;
		const wasRadio = $videoSession.continuous && $videoSession.autoplay;
		const preloaded = prefetched?.videoId === upNext?.tidal_id ? prefetched : null;
		const advanced = await advanceVideo({ preloaded });
		if (!advanced && wasRadio && endedVideoId != null) videoSession.radioExhausted(endedVideoId);
		else if (!advanced && $videoSession.current?.tidal_id === endedVideoId) videoSession.setAutoplay(false);
	}

	function handlePlay() {
		// Free the WASAPI exclusive endpoint so the WebView can output the
		// video's audio in shared mode. No-op server-side when exclusive is off.
		void api.releaseExclusivePlayback();
	}

	function toggleAutoplay() {
		videoSession.setAutoplay(!$videoSession.autoplay);
	}

	function returnToVideos() {
		// The watch page shows whatever is playing, so this is one hop back to
		// the big player from anywhere.
		void goto(WATCH_PATH);
	}

	/** The dock floats above main.workspace rather than inside it, so a wheel
	 *  over the docked video reached nothing and the page would not scroll.
	 *  Hand it to the workspace while the video sits in the page. */
	function forwardWheel(event: WheelEvent) {
		if (mode !== 'full' || expanded || !workspace) return;
		const scale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? workspace.clientHeight : 1;
		workspace.scrollBy({ top: event.deltaY * scale, left: event.deltaX * scale });
	}

	function closeDock() {
		clearVideoSession();
	}

	onDestroy(() => {
		if (rafId) cancelAnimationFrame(rafId);
		if (morphTimer) clearTimeout(morphTimer);
		if (fullscreenTimer) clearTimeout(fullscreenTimer);
	});
</script>

{#if active}
	<div
		class="video-dock"
		class:mini={place === 'mini'}
		class:panel={place === 'panel'}
		class:full={place === 'full'}
		class:expanded
		class:placed={placed && !expanded}
		class:collapsed={place === 'mini' && collapsed}
		class:dragging={drag !== null}
		class:morphing
		onwheel={forwardWheel}
		class:positioned={place !== 'mini' && box !== null}
		style:top={box ? `${box.top}px` : null}
		style:left={box ? `${box.left}px` : null}
		style:width={box ? `${box.width}px` : null}
		style:height={box ? `${box.height}px` : null}
	>
		<div class="player-surface" aria-hidden={mode === 'mini' && collapsed}>
			<VideoPlayer
				src={$videoSession.streamUrl!}
				poster={$videoSession.current?.artwork_url}
				title={$videoSession.current?.title ?? 'Video'}
				artist={$videoSession.current?.artist_name ?? null}
				qualityMode={qualityMode}
				variant={place === 'full' || expanded ? 'full' : 'mini'}
				fullscreenActive={expanded}
				onFullscreenToggle={toggleExpanded}
				onProgress={noteVideoProgress}
				autoplayNext={$videoSession.autoplay}
				hasNext={hasNext}
				hasPrevious={hasPrevious}
				upNextTitle={upNext?.title ?? null}
				upNextArtist={upNext?.artist_name ?? null}
				onEnded={handleEnded}
				onPrevious={() => void previousVideo()}
				onNext={() => void nextVideo()}
				onToggleAutoplay={toggleAutoplay}
				onPlay={handlePlay}
				refreshStream={refreshVideoStream}
			/>
		</div>

		{#if expanded}
			<!-- Fullscreen: the player's own controls only. -->
		{:else if mode === 'mini' && collapsed}
			<div class="pill">
				<span class="pill-title">{$videoSession.current?.title ?? 'Video'}</span>
				<button type="button" class="mini-btn" aria-label="Show video" title="Show video" onclick={() => (collapsed = false)}>&#x25A2;</button>
				<button type="button" class="mini-btn" aria-label="Close video" title="Close video" onclick={closeDock}>&#x2715;</button>
			</div>
		{:else if mode !== 'full'}
			<div class="mini-chrome">
				{#if mode === 'mini' && placed}
					<button
						type="button"
						class="mini-btn grip"
						aria-label="Move video window. Drag, or use the arrow keys to pick a corner."
						title="Drag to move"
						onpointerdown={startDrag}
						onpointermove={moveDrag}
						onpointerup={endDrag}
						onpointercancel={endDrag}
						onkeydown={moveWithKeys}>&#x283F;</button
					>
					<button type="button" class="mini-btn" aria-label="Minimise video" title="Minimise" onclick={() => (collapsed = true)}>&#x2212;</button>
				{/if}
				{#if !onWatchPage}
					<button
						type="button"
						class="mini-btn"
						aria-label="Open the player"
						title="Open the player"
						onclick={returnToVideos}>&#x2922;</button
					>
				{/if}
				<button type="button" class="mini-btn" aria-label="Close video" title="Close video" onclick={closeDock}>&#x2715;</button>
			</div>
		{/if}
	</div>
{/if}

<style>
	.video-dock {
		z-index: 60;
	}

	.player-surface {
		width: 100%;
		height: 100%;
	}

	/* Full and panel modes: a fixed box copied each frame onto an anchor's rect,
	   so it reads as inline while persisting across navigation. Hidden until
	   the first rect lands to avoid a flash at (0,0). */
	.video-dock.full,
	.video-dock.panel {
		position: fixed;
		opacity: 0;
		pointer-events: none;
	}

	.video-dock.full.positioned,
	.video-dock.panel.positioned {
		opacity: 1;
		pointer-events: auto;
	}

	/* Arriving on the watch page: the player settles into the stage instead
	   of snapping there from the corner. */
	.video-dock.full.positioned {
		animation: stage-in 0.28s cubic-bezier(0.22, 0.7, 0.2, 1) both;
	}

	@keyframes stage-in {
		from { opacity: 0; transform: translateY(10px) scale(0.985); }
		to { opacity: 1; transform: none; }
	}

	/* Matches the panel's artwork slot it covers. */
	.video-dock.panel {
		border-radius: 8px;
		overflow: hidden;
	}

	/* Mini mode: a floating window. Without workspace bounds (or on a phone)
	   it keeps the plain corner; with them, left/top come from placeMini. */
	.video-dock.mini {
		position: fixed;
		right: 18px;
		bottom: calc(18px + var(--safe-bottom, 0px));
		width: clamp(248px, 24vw, 340px);
		aspect-ratio: 16 / 9;
		border-radius: 10px;
		overflow: hidden;
		box-shadow: 0 18px 50px rgba(0, 0, 0, 0.5);
		border: 1px solid rgba(255, 255, 255, 0.12);
		animation: dock-in 0.22s ease both;
	}

	.video-dock.mini.placed {
		right: auto;
		bottom: auto;
		aspect-ratio: auto;
		transition:
			left 0.18s ease,
			top 0.18s ease,
			width 0.18s ease,
			height 0.18s ease;
	}


	.video-dock.mini.placed.dragging {
		transition: none;
		cursor: grabbing;
	}

	/* Minimised: the player keeps running (audio continues) but is hidden
	   behind a pill with the title and the ways back. */
	.video-dock.mini.collapsed {
		border-radius: 999px;
		background: var(--bg-raised);
		border-color: var(--border-strong);
	}

	.video-dock.collapsed .player-surface {
		position: absolute;
		inset: 0;
		visibility: hidden;
		pointer-events: none;
	}

	.pill {
		position: relative;
		z-index: 3;
		display: flex;
		align-items: center;
		gap: 6px;
		height: 100%;
		padding: 0 8px 0 16px;
	}

	.pill-title {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
	}

	.mini-chrome {
		position: absolute;
		top: 6px;
		right: 6px;
		z-index: 3;
		display: flex;
		gap: 5px;
		opacity: 0;
		transition: opacity 0.16s ease;
	}

	.video-dock.mini:hover .mini-chrome,
	.video-dock.panel:hover .mini-chrome,
	.mini-chrome:focus-within {
		opacity: 1;
	}

	.mini-btn {
		width: 26px;
		height: 26px;
		border-radius: 999px;
		display: grid;
		place-items: center;
		background: rgba(10, 10, 14, 0.72);
		color: rgba(255, 255, 255, 0.92);
		border: 1px solid rgba(255, 255, 255, 0.16);
		font-size: var(--font-size-xs);
		cursor: pointer;
	}

	.mini-btn:hover {
		background: rgba(20, 20, 26, 0.92);
	}

	.mini-btn:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	.grip {
		cursor: grab;
		touch-action: none;
	}

	@keyframes dock-in {
		from {
			opacity: 0;
			transform: translateY(14px) scale(0.96);
		}
		to {
			opacity: 1;
			transform: translateY(0) scale(1);
		}
	}

	@media (max-width: 720px) {
		.video-dock.mini {
			right: 10px;
			bottom: calc(76px + var(--safe-bottom, 0px));
			width: min(64vw, 240px);
		}
	}

	/* Fullscreen: covers the window, above the sidebar and player bar. */
	.video-dock.expanded {
		position: fixed;
		top: 0;
		left: 0;
		width: 100vw;
		height: 100vh;
		z-index: 1000;
		background: #000;
		border-radius: 0;
	}

	/* Every move between places glides. Last in the sheet and as specific as
	   .mini.placed, so it wins over that rule's short drag-snap transition.
	   Zero duration rather than animation: none - removing the animation when
	   the glide ends would restart dock-in or stage-in and pop a second time;
	   with the same name kept, it has already finished by then. */
	.video-dock.morphing:is(.mini, .panel, .full, .expanded) {
		animation-duration: 0s;
		transition:
			left 0.32s cubic-bezier(0.22, 0.7, 0.2, 1),
			top 0.32s cubic-bezier(0.22, 0.7, 0.2, 1),
			width 0.32s cubic-bezier(0.22, 0.7, 0.2, 1),
			height 0.32s cubic-bezier(0.22, 0.7, 0.2, 1),
			border-radius 0.32s ease;
	}

	@media (prefers-reduced-motion: reduce) {
		.video-dock.morphing:is(.mini, .panel, .full, .expanded) {
			transition: none;
		}
	}
</style>
