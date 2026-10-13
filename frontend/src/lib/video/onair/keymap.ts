// The remote control. While a channel is on air and its stage is on screen
// (watch page or fullscreen), these keys run the TV: Up/Down flip, digits
// dial, G opens the guide, +/- teach the channel, Esc closes what is open.
//
// Collisions with the video shortcuts in $lib/video/controls.ts: Up and Down
// are volume there; on air they flip channels (a remote's channel rocker),
// and Shift+Up/Down keep volume. Space, arrows left/right, F, M and Q are
// untouched. Ctrl/Cmd +/- stay the app's zoom: anything with a modifier other
// than Shift is ignored here.

export type OnAirAction =
	| { kind: 'flip'; step: 1 | -1 }
	| { kind: 'digit'; digit: number }
	| { kind: 'guide' }
	| { kind: 'taste'; more: boolean }
	| { kind: 'volume'; up: boolean }
	| { kind: 'close' };

export interface KeyLike {
	key: string;
	shiftKey?: boolean;
	ctrlKey?: boolean;
	metaKey?: boolean;
	altKey?: boolean;
	defaultPrevented?: boolean;
	target?: EventTarget | null;
}

export interface KeyContext {
	/** The guide grid is open: it takes arrows, digits and Enter itself. */
	guideOpen: boolean;
	/** Anything Esc should close first (guide, a half-typed number). */
	overlayOpen: boolean;
}

/** True for fields where keys type text. Duck-typed so it also runs outside
 *  a DOM (tests). */
export function isTypingTarget(target: EventTarget | null | undefined): boolean {
	const el = target as { tagName?: unknown; isContentEditable?: unknown } | null | undefined;
	if (!el || typeof el.tagName !== 'string') return false;
	const tag = el.tagName.toUpperCase();
	return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable === true;
}

export function onAirKeyAction(event: KeyLike, ctx: KeyContext): OnAirAction | null {
	if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return null;
	if (isTypingTarget(event.target)) return null;
	const key = event.key;
	if (key === 'Escape') return ctx.overlayOpen ? { kind: 'close' } : null;
	if (key === 'g' || key === 'G') return { kind: 'guide' };
	if (ctx.guideOpen) return null;
	switch (key) {
		case 'ArrowUp':
			return event.shiftKey ? { kind: 'volume', up: true } : { kind: 'flip', step: 1 };
		case 'ArrowDown':
			return event.shiftKey ? { kind: 'volume', up: false } : { kind: 'flip', step: -1 };
		case 'PageUp':
		case 'ChannelUp':
			return { kind: 'flip', step: 1 };
		case 'PageDown':
		case 'ChannelDown':
			return { kind: 'flip', step: -1 };
		case '+':
		case '=':
			return { kind: 'taste', more: true };
		case '-':
		case '_':
			return { kind: 'taste', more: false };
	}
	if (key.length === 1 && key >= '0' && key <= '9') return { kind: 'digit', digit: Number(key) };
	return null;
}
