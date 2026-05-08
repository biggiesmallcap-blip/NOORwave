/**
 * Svelte action: convert vertical wheel events into horizontal scroll on a
 * horizontally-overflowing element (album rails, artist rails, video carousels).
 *
 * `delay` (default 200 ms) gives the user a grace period after their cursor
 * enters the rail before wheel events get hijacked. Without this, scrolling
 * the page past a rail accidentally captures the wheel and traps the user
 * inside the rail. The delay restores the "passing through" feeling.
 *
 * Pass `{ delay: 0 }` to opt out of the grace period (e.g. for rails the user
 * is expected to scroll directly without first scrolling past).
 */
type Options = { delay?: number };

export function wheelToHorizontal(node: HTMLElement, opts: Options = {}) {
	let { delay = 200 } = opts;
	let armed = delay <= 0;
	let timer: ReturnType<typeof setTimeout> | undefined;

	const onEnter = () => {
		if (delay <= 0) {
			armed = true;
			return;
		}
		timer = setTimeout(() => {
			armed = true;
			timer = undefined;
		}, delay);
	};

	const onLeave = () => {
		if (timer) {
			clearTimeout(timer);
			timer = undefined;
		}
		armed = delay <= 0;
	};

	const onWheel = (e: WheelEvent) => {
		if (!armed) return;
		if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
		// No horizontal overflow → let the page scroll vertically. Without this,
		// grid-mode containers (no x-overflow) silently swallow every wheel event
		// mouse-over them and the page appears frozen.
		if (node.scrollWidth <= node.clientWidth) return;
		e.preventDefault();
		node.scrollLeft += e.deltaY;
	};

	node.addEventListener('mouseenter', onEnter);
	node.addEventListener('mouseleave', onLeave);
	node.addEventListener('wheel', onWheel, { passive: false });

	return {
		update(next: Options = {}) {
			delay = next.delay ?? 200;
			armed = delay <= 0;
		},
		destroy() {
			if (timer) clearTimeout(timer);
			node.removeEventListener('mouseenter', onEnter);
			node.removeEventListener('mouseleave', onLeave);
			node.removeEventListener('wheel', onWheel);
		},
	};
}
