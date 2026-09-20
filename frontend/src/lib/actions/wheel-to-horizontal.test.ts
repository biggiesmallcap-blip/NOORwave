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
		node.dispatchEvent(new Event('mouseenter'));
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
		node.dispatchEvent(new Event('mouseenter'));

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
		node.dispatchEvent(new Event('mouseenter'));
		vi.setSystemTime(500);

		const event = dispatchWheel(node, { deltaY: 80 });

		expect(event.defaultPrevented).toBe(false);
		expect(node.scrollLeft).toBe(700);
		action.destroy();
	});

	it('supports explicit Shift+wheel navigation without enabling wheel browsing', () => {
		const node = new FakeRail();
		const action = wheelToHorizontal(node as unknown as HTMLElement);
		node.dispatchEvent(new Event('mouseenter'));
		vi.setSystemTime(10);

		const event = dispatchWheel(node, { deltaY: 2, deltaMode: 1, shiftKey: true });

		expect(event.defaultPrevented).toBe(true);
		expect(node.scrollLeft).toBe(164);
		action.destroy();
	});
});
