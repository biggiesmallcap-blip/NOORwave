/**
 * Svelte action: convert vertical wheel events into horizontal scroll on a
 * horizontally-overflowing element (album rails, artist rails, video carousels).
 *
 * Native mode preserves the input axis: vertical gestures keep scrolling the
 * page and horizontal trackpad gestures are handled by the browser. People
 * using a conventional mouse wheel can opt into wheel browsing in Settings.
 *
 * Opt-in wheel browsing only claims a NEW gesture after the pointer has rested
 * over the rail. A gesture that began as page scrolling stays with the page,
 * including its momentum, and boundary events are released back to the page.
 */
import { horizontalShelfWheel } from '$lib/stores/shelf_scrolling';

type Options = { delay?: number };

const DEFAULT_DELAY = 350;
const GESTURE_GAP = 180;
const EDGE_EPSILON = 2;

type GestureOwner = 'page' | 'rail' | null;

function preferredScrollBehavior(): ScrollBehavior {
	return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
		? 'auto'
		: 'smooth';
}

export function wheelToHorizontal(node: HTMLElement, opts: Options = {}) {
	let { delay = DEFAULT_DELAY } = opts;
	let wheelBrowsing = false;
	let pointerEnteredAt = Number.NEGATIVE_INFINITY;
	let lastWheelAt = Number.NEGATIVE_INFINITY;
	let gestureOwner: GestureOwner = null;
	const unsubscribe = horizontalShelfWheel.subscribe((enabled) => {
		wheelBrowsing = enabled;
		gestureOwner = null;
	});

	const onEnter = () => {
		pointerEnteredAt = Date.now();
		gestureOwner = null;
	};

	const onLeave = () => {
		pointerEnteredAt = Number.NEGATIVE_INFINITY;
		gestureOwner = null;
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
			const restedLongEnough = now - pointerEnteredAt >= delay;
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
		if (atBoundary) {
			// Once the shelf is exhausted, hand the rest of this gesture back to
			// the page rather than leaving the user trapped over a dead rail.
			gestureOwner = 'page';
			return;
		}

		e.preventDefault();
		const scale = e.deltaMode === 1 // WheelEvent.DOM_DELTA_LINE
			? 32
			: e.deltaMode === 2 // WheelEvent.DOM_DELTA_PAGE
				? node.clientWidth
				: 1;
		node.scrollLeft += e.deltaY * scale;
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

	node.addEventListener('mouseenter', onEnter);
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
			node.removeEventListener('mouseenter', onEnter);
			node.removeEventListener('mouseleave', onLeave);
			node.removeEventListener('wheel', onWheel);
			node.removeEventListener('keydown', onKeyDown);
		},
	};
}
