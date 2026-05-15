import { hapticTap } from '$lib/remote/haptics';

/**
 * Long-press Svelte action. Fires `callback` after `delay` ms of a pointer
 * being held within `moveTolerance` px of the start position. Any larger
 * move, pointerup, or pointercancel before the timer expires aborts.
 *
 * Use as: `<div use:longPress={() => openSheet()}>`.
 * Pass an options object to override the defaults:
 *   `use:longPress={{ onLongPress: fn, delay: 600 }}`.
 *
 * iOS Safari still fires the browser's "callout" (text selection / image
 * preview) on long-press of an image; pair with `-webkit-touch-callout: none`
 * on the target element to suppress it.
 */
export interface LongPressOptions {
	onLongPress: (event: PointerEvent) => void;
	delay?: number;
	moveTolerance?: number;
}

export type LongPressParam = LongPressOptions | LongPressOptions['onLongPress'];

function normalize(param: LongPressParam): Required<Omit<LongPressOptions, 'onLongPress'>> & {
	onLongPress: LongPressOptions['onLongPress'];
} {
	if (typeof param === 'function') {
		return { onLongPress: param, delay: 480, moveTolerance: 10 };
	}
	return {
		onLongPress: param.onLongPress,
		delay: param.delay ?? 480,
		moveTolerance: param.moveTolerance ?? 10,
	};
}

export function longPress(node: HTMLElement, param: LongPressParam) {
	let config = normalize(param);
	let timer: ReturnType<typeof setTimeout> | null = null;
	let startX = 0;
	let startY = 0;
	let triggered = false;

	// iOS Safari fires a selection-highlight and a "Copy / Look Up" callout
	// during long-press by default. Suppress on every element that opts into
	// long-press so we don't have to remember per-call CSS at each site.
	// Capture previous inline values so destroy() can restore them and we
	// don't stomp on something a parent style was relying on.
	type IosStyle = CSSStyleDeclaration & {
		webkitUserSelect?: string;
		webkitTouchCallout?: string;
		webkitTapHighlightColor?: string;
	};
	const style = node.style as IosStyle;
	const previousStyle = {
		userSelect: style.userSelect,
		webkitUserSelect: style.webkitUserSelect ?? '',
		webkitTouchCallout: style.webkitTouchCallout ?? '',
		webkitTapHighlightColor: style.webkitTapHighlightColor ?? '',
	};
	style.userSelect = 'none';
	style.webkitUserSelect = 'none';
	style.webkitTouchCallout = 'none';
	style.webkitTapHighlightColor = 'transparent';

	function cancel() {
		if (timer) {
			clearTimeout(timer);
			timer = null;
		}
	}

	function onPointerDown(event: PointerEvent) {
		if (event.pointerType === 'mouse' && event.button !== 0) return;
		cancel();
		triggered = false;
		startX = event.clientX;
		startY = event.clientY;
		timer = setTimeout(() => {
			triggered = true;
			hapticTap();
			config.onLongPress(event);
		}, config.delay);
	}

	function onPointerMove(event: PointerEvent) {
		if (!timer) return;
		const dx = event.clientX - startX;
		const dy = event.clientY - startY;
		if (Math.hypot(dx, dy) > config.moveTolerance) cancel();
	}

	function onPointerUp() {
		cancel();
	}

	function suppressClick(event: MouseEvent) {
		// If the long-press fired, swallow the trailing click so the row's
		// onclick (play-now) doesn't run on top of the menu open.
		if (triggered) {
			event.stopPropagation();
			event.preventDefault();
			triggered = false;
		}
	}

	function suppressContextMenu(event: Event) {
		// iOS still fires `contextmenu` on long-press for images and links;
		// swallow it so the native callout doesn't steal the gesture.
		event.preventDefault();
	}

	function suppressSelectStart(event: Event) {
		// Some iOS versions begin a text selection mid-press even with
		// user-select: none on the element. Cancelling selectstart at the
		// element kills any in-flight selection before the timer fires.
		event.preventDefault();
	}

	node.addEventListener('pointerdown', onPointerDown);
	node.addEventListener('pointermove', onPointerMove);
	node.addEventListener('pointerup', onPointerUp);
	node.addEventListener('pointercancel', onPointerUp);
	node.addEventListener('click', suppressClick, true);
	node.addEventListener('contextmenu', suppressContextMenu);
	node.addEventListener('selectstart', suppressSelectStart);

	return {
		update(next: LongPressParam) {
			config = normalize(next);
		},
		destroy() {
			cancel();
			node.removeEventListener('pointerdown', onPointerDown);
			node.removeEventListener('pointermove', onPointerMove);
			node.removeEventListener('pointerup', onPointerUp);
			node.removeEventListener('pointercancel', onPointerUp);
			node.removeEventListener('click', suppressClick, true);
			node.removeEventListener('contextmenu', suppressContextMenu);
			node.removeEventListener('selectstart', suppressSelectStart);
			style.userSelect = previousStyle.userSelect;
			style.webkitUserSelect = previousStyle.webkitUserSelect;
			style.webkitTouchCallout = previousStyle.webkitTouchCallout;
			style.webkitTapHighlightColor = previousStyle.webkitTapHighlightColor;
		},
	};
}
