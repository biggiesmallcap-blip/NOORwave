// Labels and glyphs for the shuffle and repeat modes, shared by every
// transport (desktop player bar, mobile now-playing sheet).

export const SHUFFLE_LABELS: Record<string, string> = {
	off: 'Shuffle off',
	genre: 'Genre mix',
	weighted: 'Smart shuffle',
	true: 'True random',
};

export const SHUFFLE_ICONS: Record<string, string> = {
	off: '⇄',
	genre: '◆',
	weighted: '◉',
	true: '⤮',
};

export const REPEAT_LABELS: Record<string, string> = {
	off: 'Repeat off',
	all: 'Repeat all',
	one: 'Repeat one',
};

export const REPEAT_ICONS: Record<string, string> = {
	off: '↻',
	all: '↺',
	one: '⊙',
};
