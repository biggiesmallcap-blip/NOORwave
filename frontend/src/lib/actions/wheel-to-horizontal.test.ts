import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { wheelToHorizontal } from './wheel-to-horizontal';
import { horizontalShelfWheel } from '$lib/stores/shelf_scrolling';

class FakeRail extends EventTarget {
	scrollWidth = 1000;
	clientWidth = 300;
	scrollLeft = 100;

	scrollBy(options: ScrollToOptions) {
		this.scrollLeft += options.left ?? 0;
	}

	scrollTo(options: ScrollToOptions) {
		this.scrollLeft = options.left ?? this.scrollLeft;
	}
}

function dispatchWheel(
	node: FakeRail,
	options: Partial<Pick<WheelEvent, 'deltaX' | 'deltaY' | 'deltaMode' | 'shiftKey' | 'ctrlKey' | 'metaKey'>> = {},
): Event {
	const event = new Event('wheel', { cancelable: true });
	Object.defineProperties(event, {
		deltaX: { value: options.deltaX ?? 0 },
		deltaY: { value: options.deltaY ?? 100 },
		deltaMode: { value: options.deltaMode ?? 0 },
		shiftKey: { value: options.shiftKey ?? false },
		ctrlKey: { value: options.ctrlKey ?? false },
		metaKey: { value: options.metaKey ?? false },
	});
	node.dispatchEvent(event);
	return event;
}

/** A real pointer movement onto the rail (hover refreshes carry no movement). */
function pointAt(node: FakeRail, movement = 4) {
	const event = new Event('mousemove');
	Object.defineProperties(event, {
		movementX: { value: movement },
		movementY: { value: 0 },
	});
	node.dispatchEvent(event);
}

describe('wheelToHorizontal', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(0);
		horizontalShelfWheel.set(false);
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	it('leaves vertical gestures with the page in the default native mode', () => {
		const node = new FakeRail();
		const action = wheelToHorizontal(node as unknown as HTMLElement);
		pointAt(node);
		vi.setSystemTime(500);

		const event = dispatchWheel(node);

		expect(event.defaultPrevented).toBe(false);
		expect(node.scrollLeft).toBe(100);
		action.destroy();
	});

	it('does not capture momentum from a page gesture when the hover delay expires', () => {
		horizontalShelfWheel.set(true);
		const node = new FakeRail();
		const action = wheelToHorizontal(node as unknown as HTMLElement);
		pointAt(node);

		vi.setSystemTime(100);
		expect(dispatchWheel(node).defaultPrevented).toBe(false);
		vi.setSystemTime(240);
		expect(dispatchWheel(node).defaultPrevented).toBe(false);
		vi.setSystemTime(370);
		expect(dispatchWheel(node).defaultPrevented).toBe(false);
		expect(node.scrollLeft).toBe(100);

		// After the gesture goes idle, a deliberate new wheel gesture is allowed.
		vi.setSystemTime(600);
		expect(dispatchWheel(node).defaultPrevented).toBe(true);
		expect(node.scrollLeft).toBe(200);
		action.destroy();
	});

	it('hands the gesture back to the page at the shelf boundary', () => {
		horizontalShelfWheel.set(true);
		const node = new FakeRail();
		node.scrollLeft = 700;
		const action = wheelToHorizontal(node as unknown as HTMLElement);
		pointAt(node);
		vi.setSystemTime(500);

		const event = dispatchWheel(node, { deltaY: 80 });

		expect(event.defaultPrevented).toBe(false);
		expect(node.scrollLeft).toBe(700);
		action.destroy();
	});

	it('hands the gesture back to the page when the rail stops moving short of its end', () => {
		horizontalShelfWheel.set(true);
		const node = new FakeRail();
		node.scrollLeft = 600;
		const action = wheelToHorizontal(node as unknown as HTMLElement);
		pointAt(node);

		// Scroll snapping pulls the rail back to 600 after every step.
		const wheel = (at: number) => {
			vi.setSystemTime(at);
			const event = dispatchWheel(node);
			node.scrollLeft = 600;
			return event.defaultPrevented;
		};

		expect(wheel(500)).toBe(true);
		expect(wheel(550)).toBe(true);
		expect(wheel(600)).toBe(false);
		expect(wheel(650)).toBe(false);
		expect(wheel(700)).toBe(false);
		action.destroy();
	});

	it('keeps scrolling the rail while it makes progress', () => {
		horizontalShelfWheel.set(true);
		const node = new FakeRail();
		node.scrollLeft = 0;
		const action = wheelToHorizontal(node as unknown as HTMLElement);
		pointAt(node);

		for (const at of [500, 550, 600, 650, 700]) {
			vi.setSystemTime(at);
			expect(dispatchWheel(node).defaultPrevented).toBe(true);
		}
		expect(node.scrollLeft).toBe(500);
		action.destroy();
	});

	it('ignores a rail that scrolled under a parked cursor', () => {
		horizontalShelfWheel.set(true);
		const node = new FakeRail();
		const action = wheelToHorizontal(node as unknown as HTMLElement);
		// Chromium refreshes hover after a scroll with a zero-movement mousemove.
		pointAt(node, 0);
		vi.setSystemTime(1000);

		expect(dispatchWheel(node).defaultPrevented).toBe(false);
		expect(node.scrollLeft).toBe(100);
		action.destroy();
	});

	it('leaves the next gesture with the page after the rail hands one back', () => {
		horizontalShelfWheel.set(true);
		const node = new FakeRail();
		node.scrollLeft = 700;
		const action = wheelToHorizontal(node as unknown as HTMLElement);
		pointAt(node);

		// At the end of the rail: handed to the page.
		vi.setSystemTime(500);
		expect(dispatchWheel(node).defaultPrevented).toBe(false);
		// Wheeling back the other way, cursor parked: still the page.
		vi.setSystemTime(1000);
		expect(dispatchWheel(node, { deltaY: -100 }).defaultPrevented).toBe(false);
		expect(node.scrollLeft).toBe(700);

		// Moving the mouse and resting re-arms the rail.
		pointAt(node);
		vi.setSystemTime(1400);
		expect(dispatchWheel(node, { deltaY: -100 }).defaultPrevented).toBe(true);
		expect(node.scrollLeft).toBe(600);
		action.destroy();
	});

	it('supports explicit Shift+wheel navigation without enabling wheel browsing', () => {
		const node = new FakeRail();
		const action = wheelToHorizontal(node as unknown as HTMLElement);
		pointAt(node);
		vi.setSystemTime(10);

		const event = dispatchWheel(node, { deltaY: 2, deltaMode: 1, shiftKey: true });

		expect(event.defaultPrevented).toBe(true);
		expect(node.scrollLeft).toBe(164);
		action.destroy();
	});
});
