<script lang="ts">
	import { onDestroy } from 'svelte';
	import { get } from 'svelte/store';
	import { goto } from '$app/navigation';
	import { navigating, page } from '$app/state';
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
	import { hasNativeVideoFullscreen, setNativeVideoFullscreen } from '$lib/tauri/video_fullscreen';
	import { videoFullscreenDimMs, videoFullscreenGrowMs, videoFullscreenStyle } from '$lib/stores/video_fullscreen_style';

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
	let dockEl = $state<HTMLDivElement | null>(null);
	/** Where the dock was drawn at the start of this frame: the "from" of a glide. */
	let lastDockRect: DOMRect | null = null;
	let glide: Animation | null = null;

	let workspace: HTMLElement | null = null;

	function sameRect(a: Rect | null, b: DOMRect) {
		return a !== null && a.top === b.top && a.left === b.left && a.width === b.width && a.height === b.height;
	}

	function track() {
		if (dockEl?.isConnected) lastDockRect = dockEl.getBoundingClientRect();
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
			// Hold still on the stage while a page change is in flight: the
			// outgoing page reflows for a frame (the stage grew ~4%), and
			// following it showed as a twitch right before the glide.
			// (navigating.to also clears on a cancelled navigation, unlike a
			// before/after pair, so the hold can never stick.)
			const holdStill = (dimming || navigating.to !== null) && mode === 'full' && rect !== null;
			if (!holdStill && !sameRect(rect, r)) rect = { top: r.top, left: r.left, width: r.width, height: r.height };
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
	/** The window's size even while it is a pill: the player keeps it, so the
	 *  pill morph only uncovers or covers the video and never resizes it. */
	let windowSize = $derived(miniSize(viewportWidth, false));
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

	// Window <-> pill: the box animates its size (CSS) around a player that
	// stays at window size, pinned to the corner the two share. Resizing the
	// playing video with the box re-laid it out every frame, and the video
	// lagged the box edges. The player's chrome sits the morph out.
	const PILL_MS = 240;
	let unfolding = $state(false);
	let unfoldTimer: ReturnType<typeof setTimeout> | null = null;

	function setCollapsed(next: boolean) {
		if (next === collapsed) return;
		collapsed = next;
		unfolding = true;
		if (unfoldTimer) clearTimeout(unfoldTimer);
		unfoldTimer = setTimeout(() => {
			unfoldTimer = null;
			unfolding = false;
		}, PILL_MS);
	}

	// --- Fullscreen: the same player grows to fill the window ---
	// Not native element fullscreen, which swaps the video into the top layer
	// in one hard cut. The dock glides from wherever it sits to cover the
	// window while the window itself goes fullscreen underneath, and glides
	// back on exit (button, double-click, F, or Esc).
	let expanded = $state(false);

	// One motion at a time. Asking for window fullscreen during the glide
	// resized the window mid-flight, so the glide's target moved under it and
	// the two fought (visible jank in Chrome and WebView2 alike). In: go
	// fullscreen first, wait for the window to reach its final size, then
	// glide to fill it. Gliding first filled the still-windowed app for the
	// whole glide, so the native title bar (minimize / maximize / close) sat
	// over a black frame before the window went fullscreen. Out: leave
	// fullscreen first, let the page settle at its normal size, then glide back.
	let enteringFullscreen = false;

	// Inside the desktop app the window switch is native instead (see
	// $lib/tauri/video_fullscreen), in the same order: the window goes
	// fullscreen in one step, then the dock grows into it; out, the window
	// comes back, then the dock glides home. The command returns once the
	// window has switched, so no settle polling is needed. The listener picks
	// the style in Settings (Classic keeps the Fullscreen API path).
	const nativeSwitch = hasNativeVideoFullscreen();
	let nativeOn = false;
	let nativePending = false;

	/** "Dim, then grow": lights down around the video before the window
	 *  switches. The page is laid out at fullscreen size a few frames before
	 *  the window grows, and anything on the right edge (the queue) can
	 *  shift and be cut off for a frame. */
	let dimming = $state(false);

	function enterNativeFullscreen() {
		if (nativeOn || nativePending) return;
		nativePending = true;
		const dim = get(videoFullscreenStyle) === 'dim';
		if (dim) {
			// Out of the stage first: the dock rides above the dim as a fixed
			// layer, held still where it is while the page re-lays out.
			moveHome();
			dimming = true;
		}
		setTimeout(() => {
			void setNativeVideoFullscreen(true).then((ok) => {
				nativePending = false;
				nativeOn = ok;
				if (!active) {
					dimming = false;
					void leaveNativeFullscreen();
					return;
				}
				// Refused: still fill the window. Switched: two frames so the dock
				// measures the page at its fullscreen size, then glide.
				if (!ok) expanded = true;
				else requestAnimationFrame(() => requestAnimationFrame(() => (expanded = nativeOn && active)));
			});
		}, dim ? get(videoFullscreenDimMs) : 0);
	}

	async function leaveNativeFullscreen() {
		if (!nativeOn) return;
		nativeOn = false;
		await setNativeVideoFullscreen(false);
	}

	/** Out of fullscreen by any route (button, double-click, F, Esc). */
	function collapse() {
		dimming = false;
		if (nativeOn) {
			// Two frames: the stage has re-laid out at the restored size, so
			// the glide back aims at where the player really belongs.
			void leaveNativeFullscreen().then(() =>
				requestAnimationFrame(() => requestAnimationFrame(() => (expanded = false)))
			);
			return;
		}
		expanded = false;
	}

	function toggleExpanded() {
		// Already native (the setting may have changed since): leave natively.
		if (nativeOn || nativePending) {
			if (expanded) collapse();
			return;
		}
		if (nativeSwitch && $videoFullscreenStyle !== 'classic') {
			if (expanded) collapse();
			else enterNativeFullscreen();
			return;
		}
		if (expanded) {
			if (document.fullscreenElement) {
				// fullscreenchange collapses once the window is back to size.
				void document.exitFullscreen().catch(() => (expanded = false));
			} else {
				expanded = false;
			}
			return;
		}
		if (enteringFullscreen) return;
		if (document.fullscreenElement) {
			expanded = true;
			return;
		}
		const request = document.documentElement.requestFullscreen?.();
		if (!request) {
			expanded = true;
			return;
		}
		enteringFullscreen = true;
		// Refused (no user activation, policy): still fill the window.
		request.catch(() => {
			enteringFullscreen = false;
			if (active) expanded = true;
		});
	}

	/** Runs once the window has finished growing. WebView2 resizes the host
	 *  window after fullscreenchange; browsers usually before it. Done when
	 *  the viewport covers the screen, or has grown and then held still (UI
	 *  zoom keeps it below screen.width in CSS px). A viewport that has not
	 *  grown after a short beat was already full size; capped either way. */
	function afterWindowSettles(done: () => void) {
		const start = performance.now();
		const fromW = window.innerWidth;
		const fromH = window.innerHeight;
		let lastW = fromW;
		let lastH = fromH;
		let still = 0;
		const tick = () => {
			const w = window.innerWidth;
			const h = window.innerHeight;
			const elapsed = performance.now() - start;
			const fills = w >= screen.width - 2 && h >= screen.height - 2;
			const grew = w !== fromW || h !== fromH;
			still = w === lastW && h === lastH ? still + 1 : 0;
			lastW = w;
			lastH = h;
			if (fills || (grew && still >= 3) || (!grew && elapsed > 250) || elapsed > 800) {
				// One more frame so the dock measures the re-laid-out page.
				requestAnimationFrame(done);
				return;
			}
			requestAnimationFrame(tick);
		};
		requestAnimationFrame(tick);
	}

	$effect(() => {
		if (active) return;
		dimming = false;
		void leaveNativeFullscreen();
		expanded = false;
	});

	$effect(() => {
		// Esc (or any other way out of window fullscreen) collapses too.
		const onFullscreenChange = () => {
			if (document.fullscreenElement) {
				if (!enteringFullscreen) return;
				enteringFullscreen = false;
				afterWindowSettles(() => {
					if (!document.fullscreenElement) return;
					if (active) expanded = true;
					else void document.exitFullscreen().catch(() => {});
				});
				return;
			}
			enteringFullscreen = false;
			if (!expanded) return;
			// Two frames: the window has resized and the stage has re-laid out,
			// so the glide back aims at where the player really belongs.
			requestAnimationFrame(() => requestAnimationFrame(() => (expanded = false)));
		};
		// Esc with no window fullscreen (the request can be refused).
		const onKeydown = (event: KeyboardEvent) => {
			if (event.key === 'Escape' && expanded && !document.fullscreenElement) collapse();
		};
		document.addEventListener('fullscreenchange', onFullscreenChange);
		window.addEventListener('keydown', onKeydown);
		return () => {
			document.removeEventListener('fullscreenchange', onFullscreenChange);
			window.removeEventListener('keydown', onKeydown);
		};
	});

	// --- On the watch page the player lives inside the stage ---
	// A fixed layer that copies the stage's rect each frame trails the page
	// by a frame whenever Chrome scrolls on its compositor thread (smooth
	// wheel), so the video slid out of its frame while scrolling. While the
	// stage is the place, the whole dock (host) is moved into the stage
	// element and scrolls natively with the page; it moves back out for the
	// corner, the queue panel and fullscreen. Moving a playing <video>
	// within the document in one step does not pause or reload it.
	let host: HTMLDivElement | null = $state(null);
	let homeMarker: Comment | null = null;
	let inStage = false;

	function moveIntoStage(stage: HTMLElement) {
		if (!host || host.parentNode === stage) return;
		if (!homeMarker) {
			homeMarker = document.createComment('video-dock-home');
			host.before(homeMarker);
		}
		stage.appendChild(host);
		inStage = true;
		settleRestartedAnimations();
	}

	/** Re-inserting an element restarts its CSS animations, so a move
	 *  replayed stage-in or dock-in (a 10-14px nudge, and a skewed start box
	 *  for the next glide). Finish them on the spot. Moves home happen as a
	 *  glide starts, so by the time its 0s duration override comes off they
	 *  are past their real length and stay finished. The move into the stage
	 *  happens as a glide ends, so there the dock has no animation at all
	 *  (see the in-stage CSS). */
	function settleRestartedAnimations() {
		for (const animation of dockEl?.getAnimations() ?? []) {
			if (animation !== glide) animation.finish();
		}
	}

	function moveHome() {
		if (!host || !homeMarker?.parentNode) return;
		if (host.previousSibling !== homeMarker) {
			homeMarker.after(host);
			settleRestartedAnimations();
		}
		inStage = false;
	}

	// Synchronous, not an effect: the watch page tears its stage down during
	// navigation, and the host must be back home within that same task so
	// the video never leaves the document long enough to pause.
	const unsubscribeStage = videoStageAnchor.subscribe((stage) => {
		if (!inStage) return;
		if (stage?.isConnected) moveIntoStage(stage);
		else moveHome();
	});

	// --- Moves between places glide ---
	// Stage, corner, queue panel and fullscreen are all the same element, so
	// every move animates from where it was instead of cutting.
	//
	// Page moves (watch page <-> corner <-> queue panel) are a FLIP transform:
	// the dock takes its new box at once and a compositor animation carries
	// it there from the old one. Animating left/top/width/height instead
	// re-laid-out and re-scaled the playing video every frame while the next
	// page was rendering, which stuttered. Controls fade for the glide so the
	// scale never shows stretched text.
	//
	// Fullscreen glides position and size instead: the window is not 16:9,
	// so a scale would stretch the picture, and nothing else renders during
	// it. Both kinds are driven from the measured start box, never from CSS
	// class timing, so neither can lose its starting point.
	type Place = 'full' | 'panel' | 'mini' | 'expanded';
	let place = $derived<Place>(expanded ? 'expanded' : mode);
	let box = $derived.by(() => {
		// No inline box: CSS pins the dock to the window edges, so the window
		// going fullscreen resizes it in the same frame.
		if (expanded) return null;
		if (mode !== 'mini' && rect) return rect;
		if (position) return { top: position.top, left: position.left, width: size.width, height: size.height };
		return null;
	});
	let morphing = $state(false);
	let previousPlace: Place | null = null;
	let morphTimer: ReturnType<typeof setTimeout> | null = null;
	const GLIDE_EASING = 'cubic-bezier(0.22, 0.7, 0.2, 1)';

	function endGlide() {
		morphing = false;
		// The dock covers the window now; the dim fades out unseen.
		if (place === 'expanded') dimming = false;
		const stage = get(videoStageAnchor);
		if (place === 'full' && stage?.isConnected) moveIntoStage(stage);
		if (dockEl) {
			dockEl.style.transition = '';
			dockEl.style.animationDuration = '';
			dockEl.style.transform = '';
			dockEl.style.transformOrigin = '';
		}
	}

	/** Where the dock really lands. The class that switches transitions off
	 *  lands a beat after the new box, so the corner's drag-snap transition
	 *  has already started and would hand us a half-moved box; cancel it
	 *  inline first (jumps to the real target). Same for the arrival
	 *  animation: dock-in had started (scale 0.96, 14px down) and skewed the
	 *  measurement, so a glide began ~30px off and corrected itself
	 *  mid-flight. Both are cleared again in endGlide. */
	function measureLanding(): DOMRect | null {
		if (!dockEl) return null;
		dockEl.style.transition = 'none';
		dockEl.style.animationDuration = '0s';
		const to = dockEl.getBoundingClientRect();
		return to.width > 0 && to.height > 0 ? to : null;
	}

	/** Fullscreen in and out: animate position and size from the old box. */
	function sizeGlideFrom(from: DOMRect | null): Animation | null {
		if (!dockEl || !from || from.width <= 0 || from.height <= 0) return null;
		const to = measureLanding();
		if (!to) return null;
		glide?.cancel();
		const px = (r: { top: number; left: number; width: number; height: number }) => ({
			top: `${r.top}px`, left: `${r.left}px`, width: `${r.width}px`, height: `${r.height}px`,
		});
		glide = dockEl.animate([px(from), px(to)], { duration: get(videoFullscreenGrowMs), easing: GLIDE_EASING });
		if (document.timeline.currentTime != null) glide.startTime = document.timeline.currentTime;
		return glide;
	}

	/** Invert the jump: draw the dock at its old box via a transform, then let
	 *  that transform run out. Runs after the DOM took the new box. */
	function flipFrom(from: DOMRect | null): Animation | null {
		if (!dockEl || !from || from.width <= 0 || from.height <= 0) return null;
		const to = measureLanding();
		if (!to) return null;
		const dx = from.left - to.left;
		const dy = from.top - to.top;
		const sx = from.width / to.width;
		const sy = from.height / to.height;
		const start = `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`;
		glide?.cancel();
		// Pin the start inline too: a new animation only takes over on the
		// next frame, and without this the dock painted once at its new box.
		const el = dockEl;
		el.style.transformOrigin = '0 0';
		el.style.transform = start;
		glide = el.animate(
			[
				{ transformOrigin: '0 0', transform: start },
				{ transformOrigin: '0 0', transform: 'translate(0, 0) scale(1, 1)' },
			],
			{ duration: MORPH_MS, easing: GLIDE_EASING }
		);
		// Left pending on purpose: it starts when the compositor first runs
		// it, and the inline pin holds the start pose until then. Pinning the
		// start time to this frame skipped the first ~40% of the path whenever
		// the page change kept the main thread busy before the next frame.
		// Once the animation is driving, the inline pin must go, or it would
		// show again when the animation ends.
		void glide.ready.then(() => {
			el.style.transform = '';
			el.style.transformOrigin = '';
		}).catch(() => {});
		return glide;
	}

	$effect(() => {
		const next = place;
		if (!active) {
			previousPlace = null;
			moveHome();
			return;
		}
		// Host placement first, so a FLIP measures the dock where it lands.
		// Gliding onto the stage it stays a fixed layer until the glide ends
		// (endGlide moves it in): inside the stage it was clipped by the
		// stage's overflow and faded with the arriving page, so the video
		// grew out from behind the frame.
		const moving = previousPlace !== null && previousPlace !== next;
		const reducedMotion = moving && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
		if (next !== 'full') moveHome();
		else if (!dimming && (!moving || reducedMotion) && $videoStageAnchor) moveIntoStage($videoStageAnchor);
		if (reducedMotion && next === 'expanded') dimming = false;
		if (moving) {
			const fullscreenMove = previousPlace === 'expanded' || next === 'expanded';
			if (!reducedMotion) {
				morphing = true;
				const started = fullscreenMove ? sizeGlideFrom(lastDockRect) : flipFrom(lastDockRect);
				// Land when the glide really ends (it may start a frame late);
				// the timer only covers a glide that never started or never
				// reports back.
				if (morphTimer) clearTimeout(morphTimer);
				morphTimer = setTimeout(endGlide, started ? Math.max(MORPH_MS, get(videoFullscreenGrowMs)) * 2 : MORPH_MS);
				started?.finished.then(() => {
					if (glide !== started) return;
					if (morphTimer) clearTimeout(morphTimer);
					morphTimer = null;
					endGlide();
				}, () => {});
			}
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

	function closeDock() {
		clearVideoSession();
	}

	onDestroy(() => {
		if (rafId) cancelAnimationFrame(rafId);
		if (morphTimer) clearTimeout(morphTimer);
		if (unfoldTimer) clearTimeout(unfoldTimer);
		glide?.cancel();
		void leaveNativeFullscreen();
		unsubscribeStage();
		// Never moveHome() here: by teardown Svelte may already have removed
		// the host, and re-inserting it would resurrect a dead dock (a second
		// player). Svelte removes the host wherever it sits; drop the marker.
		inStage = false;
		homeMarker?.remove();
		homeMarker = null;
	});
</script>

<div class="video-dock-host" bind:this={host}>
{#if active}
	<div class="fullscreen-dim" class:on={dimming} style:transition-duration={`${$videoFullscreenDimMs}ms`} aria-hidden="true"></div>
	<div
		bind:this={dockEl}
		class="video-dock"
		class:mini={place === 'mini'}
		class:panel={place === 'panel'}
		class:full={place === 'full'}
		class:expanded
		class:placed={placed && !expanded}
		class:collapsed={place === 'mini' && collapsed}
		class:dragging={drag !== null}
		class:morphing
		class:unfolding
		class:dimming
		class:positioned={place !== 'mini' && box !== null}
		data-corner={corner}
		style:--window-w={`${windowSize.width}px`}
		style:--window-h={`${windowSize.height}px`}
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
				<button type="button" class="mini-btn" aria-label="Show video" title="Show video" onclick={() => setCollapsed(false)}>&#x25A2;</button>
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
					<button type="button" class="mini-btn" aria-label="Minimise video" title="Minimise" onclick={() => setCollapsed(true)}>&#x2212;</button>
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
</div>

<style>
	/* At home the host adds no box; inside the stage it fills it. */
	.video-dock-host {
		display: contents;
	}

	:global(.stage-anchor) > .video-dock-host {
		display: block;
		position: absolute;
		inset: 0;
	}

	/* In the stage the dock is part of the page: it fills the stage and
	   scrolls with it. The fixed-mode rect the frame loop keeps writing
	   inline is overridden here; it is what the dock falls back to the
	   instant it leaves the stage, so it never flashes elsewhere. No arrival
	   animation here: it glided in as a fixed layer already, and stage-in
	   restarted by the move would replay (fade + 10px drop) on landing. */
	:global(.stage-anchor) > .video-dock-host > .video-dock {
		position: absolute !important;
		top: 0 !important;
		left: 0 !important;
		width: 100% !important;
		height: 100% !important;
		opacity: 1;
		pointer-events: auto;
		animation: none !important;
	}
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
		/* backwards, not both: a held last frame (transform: none) would beat
		   the inline start pin of a FLIP glide and flash the end box. */
		animation: stage-in 0.28s cubic-bezier(0.22, 0.7, 0.2, 1) backwards;
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
		animation: dock-in 0.22s ease backwards;
	}

	.video-dock.mini.placed {
		right: auto;
		bottom: auto;
		aspect-ratio: auto;
		transition-property: left, top, width, height, border-radius, border-color;
		transition-duration: 0.24s;
		transition-timing-function: cubic-bezier(0.22, 0.7, 0.2, 1);
	}

	/* Placed, the player keeps the window's size (border-box minus the 1px
	   border) pinned to the corner the window and the pill share, so folding
	   into the pill and back only covers and uncovers the video. */
	.video-dock.mini.placed .player-surface {
		position: absolute;
		width: calc(var(--window-w) - 2px);
		height: calc(var(--window-h) - 2px);
	}
	.video-dock.mini.placed[data-corner^='t'] .player-surface { top: 0; }
	.video-dock.mini.placed[data-corner^='b'] .player-surface { bottom: 0; }
	.video-dock.mini.placed[data-corner$='l'] .player-surface { left: 0; }
	.video-dock.mini.placed[data-corner$='r'] .player-surface { right: 0; }


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

	/* Half the pill's height: fully round, and unlike 999px it eases
	   smoothly to and from the window's 10px. */
	.video-dock.mini.placed.collapsed {
		border-radius: 22px;
	}

	.video-dock.collapsed .player-surface {
		visibility: hidden;
		pointer-events: none;
	}

	.video-dock.collapsed:not(.placed) .player-surface {
		position: absolute;
		inset: 0;
	}

	/* Folding into the pill the video stays until the pill has faded in
	   over it; unfolding it shows at once. */
	.video-dock.mini.placed.collapsed .player-surface {
		transition: visibility 0s linear 0.24s;
	}

	.pill {
		position: relative;
		z-index: 3;
		display: flex;
		align-items: center;
		gap: 6px;
		height: 100%;
		padding: 0 8px 0 16px;
		background: var(--bg-raised);
		border-radius: inherit;
	}

	.video-dock.placed .pill {
		animation: pill-in 0.24s ease both;
	}

	@keyframes pill-in {
		from { opacity: 0; }
		to { opacity: 1; }
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

	/* The window's buttons take the top edge while they show; the title
	   steps aside instead of running under them. */
	.video-dock.mini:hover :global(.top-meta),
	.video-dock.panel:hover :global(.top-meta),
	.video-dock:has(.mini-chrome:focus-within) :global(.top-meta) {
		opacity: 0;
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
	/* Lights down while the desktop app switches the window to fullscreen;
	   the dock rides above it. */
	.fullscreen-dim {
		position: fixed;
		inset: 0;
		z-index: 999;
		background: #000;
		opacity: 0;
		pointer-events: none;
		transition: opacity 90ms ease;
	}

	.fullscreen-dim.on {
		opacity: 1;
	}

	.video-dock.dimming {
		z-index: 1000;
	}

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

	/* During any glide. Last in the sheet and as specific as .mini.placed, so
	   it wins over that rule's drag-snap transition, which would otherwise
	   slide left/top underneath the transform glide. Zero duration rather
	   than animation: none - removing the animation when the glide ends would
	   restart dock-in or stage-in and pop a second time; with the same name
	   kept, it has already finished by then. */
	.video-dock.morphing:is(.mini, .panel, .full, .expanded) {
		animation-duration: 0s;
		transition: none;
		will-change: transform;
	}

	/* Controls sit out the glide, so the transform's scale never shows
	   stretched buttons or text, and the pill morph, so they never pile up
	   in a half-open window. */
	.video-dock:is(.morphing, .unfolding) :global(:is(.controls, .top-meta, .up-next-pill)),
	.video-dock:is(.morphing, .unfolding) .mini-chrome {
		/* Beats the hover reveal: the pointer is usually on the dock. */
		opacity: 0 !important;
		transition: none;
	}
</style>
