import { prefersReducedMotion } from '$lib/stores/motion';
/**
 * Svelte action: convert vertical wheel events into horizontal scroll on a
 * horizontally-overflowing element (album rails, artist rails, video carousels).
 *
 * Native mode preserves the input axis: vertical gestures keep scrolling the
 * page and horizontal trackpad gestures are handled by the browser. People
 * using a conventional mouse wheel can opt into wheel browsing in Settings.
 *
 * Opt-in wheel browsing only claims a NEW gesture after the pointer has moved
 * onto the rail and rested there. A rail that scrolled under a parked cursor
 * never claims the wheel (Chromium's hover-refresh mousemoves carry no
 * movement), so scrolling the page past shelves cannot get hijacked. A gesture
 * that began as page scrolling stays with the page, including its momentum,
 * and boundary events are released back to the page.
 */
import { horizontalShelfWheel } from '$lib/stores/shelf_scrolling';

type Options = { delay?: number };

const DEFAULT_DELAY = 350;
const GESTURE_GAP = 180;
const EDGE_EPSILON = 2;

type GestureOwner = 'page' | 'rail' | null;

function preferredScrollBehavior(): ScrollBehavior {
	return prefersReducedMotion() ? 'auto' : 'smooth';
}

export function wheelToHorizontal(node: HTMLElement, opts: Options = {}) {
	let { delay = DEFAULT_DELAY } = opts;
	let wheelBrowsing = false;
	// When the pointer was deliberately moved onto the rail. Cleared when the
	// rail hands a gesture back, so only a fresh mouse movement re-arms it.
	let armedAt = Number.NEGATIVE_INFINITY;
	let lastWheelAt = Number.NEGATIVE_INFINITY;
	let gestureOwner: GestureOwner = null;
	// Where the previous rail-owned event started and how far it asked to move.
	// Scroll snapping can pull the rail back short of its true end, so the
	// boundary check alone never fires and the rail swallows the wheel forever.
	let lastStart: number | null = null;
	let lastRequested = 0;
	let stalls = 0;
	const resetProgress = () => {
		lastStart = null;
		lastRequested = 0;
		stalls = 0;
	};
	const unsubscribe = horizontalShelfWheel.subscribe((enabled) => {
		wheelBrowsing = enabled;
		gestureOwner = null;
	});

	const disarm = () => {
		armedAt = Number.NEGATIVE_INFINITY;
	};

	const onMove = (e: MouseEvent) => {
		if (e.movementX === 0 && e.movementY === 0) return;
		if (armedAt === Number.NEGATIVE_INFINITY) armedAt = Date.now();
	};

	const onLeave = () => {
		disarm();
		gestureOwner = null;
	};

	// The rail ran out (or stopped moving): the page takes the rest of the
	// gesture, and the rail stays out of the way until the mouse moves, so
	// wheeling back the other way scrolls the page, not the rail.
	const releaseToPage = () => {
		gestureOwner = 'page';
		disarm();
		resetProgress();
	};

	const onWheel = (e: WheelEvent) => {
		if (e.ctrlKey || e.metaKey) return; // yield to global UI-zoom handler
		// Horizontal trackpad gestures already have the right semantics. Keeping
		// them native also preserves browser momentum and platform tuning.
		if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
		// No horizontal overflow → let the page scroll vertically. Without this,
		// grid-mode containers (no x-overflow) silently swallow every wheel event
		// mouse-over them and the page appears frozen.
		if (node.scrollWidth <= node.clientWidth) return;

		const now = Date.now();
		const isNewGesture = now - lastWheelAt > GESTURE_GAP;
		lastWheelAt = now;
		const explicitHorizontalIntent = e.shiftKey;

		if (isNewGesture) {
			const restedLongEnough = Number.isFinite(armedAt) && now - armedAt >= delay;
			resetProgress();
			gestureOwner = explicitHorizontalIntent || (wheelBrowsing && restedLongEnough)
				? 'rail'
				: 'page';
		}

		if (gestureOwner !== 'rail') return;

		const maxScrollLeft = Math.max(0, node.scrollWidth - node.clientWidth);
		const movingForward = e.deltaY > 0;
		const atBoundary = movingForward
			? node.scrollLeft >= maxScrollLeft - EDGE_EPSILON
			: node.scrollLeft <= EDGE_EPSILON;
		const scale = e.deltaMode === 1 // WheelEvent.DOM_DELTA_LINE
			? 32
			: e.deltaMode === 2 // WheelEvent.DOM_DELTA_PAGE
				? node.clientWidth
				: 1;
		const requested = e.deltaY * scale;
		const start = node.scrollLeft;
		if (lastStart !== null && Math.sign(requested) === Math.sign(lastRequested)) {
			const progress = (start - lastStart) * Math.sign(lastRequested);
			stalls = progress < Math.abs(lastRequested) / 2 ? stalls + 1 : 0;
		}
		if (atBoundary || stalls >= 2) {
			// Once the shelf is exhausted (or stops moving), hand the rest of this
			// gesture back to the page rather than leaving the user trapped over a
			// dead rail.
			releaseToPage();
			return;
		}

		e.preventDefault();
		lastStart = start;
		lastRequested = requested;
		node.scrollLeft = start + requested;
	};

	const onKeyDown = (e: KeyboardEvent) => {
		// Cards and links inside the shelf own their own keyboard interaction.
		if (e.target !== node) return;
		const amount = Math.max(160, node.clientWidth * 0.85);
		if (e.key === 'ArrowLeft') {
			e.preventDefault();
			node.scrollBy({ left: -amount, behavior: preferredScrollBehavior() });
		} else if (e.key === 'ArrowRight') {
			e.preventDefault();
			node.scrollBy({ left: amount, behavior: preferredScrollBehavior() });
		} else if (e.key === 'Home') {
			e.preventDefault();
			node.scrollTo({ left: 0, behavior: preferredScrollBehavior() });
		} else if (e.key === 'End') {
			e.preventDefault();
			node.scrollTo({ left: node.scrollWidth, behavior: preferredScrollBehavior() });
		}
	};

	node.addEventListener('mousemove', onMove);
	node.addEventListener('mouseleave', onLeave);
	node.addEventListener('wheel', onWheel, { passive: false });
	node.addEventListener('keydown', onKeyDown);

	return {
		update(next: Options = {}) {
			delay = next.delay ?? DEFAULT_DELAY;
			gestureOwner = null;
		},
		destroy() {
			unsubscribe();
			node.removeEventListener('mousemove', onMove);
			node.removeEventListener('mouseleave', onLeave);
			node.removeEventListener('wheel', onWheel);
			node.removeEventListener('keydown', onKeyDown);
		},
	};
}
