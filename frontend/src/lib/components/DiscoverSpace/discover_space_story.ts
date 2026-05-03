// All UI copy for the DiscoverSpace visualization. One place to tune language.

import type { DiscoverReason, RadioMode } from './discover_space_types';

export const PAGE_TITLE = 'Sound Space';
export const PAGE_SUBTITLE = 'Seed, branch, build.';
export const SEARCH_PLACEHOLDER = 'Jump to… dark ambient, 140bpm drum & bass…';

export const EMPTY_STATE = {
	noSeed: {
		title: 'Play something to seed the map',
		copy: 'Start playback or lock the current track.',
	},
	loading: {
		title: 'Mapping sound space',
		copy: 'Loading nearby tracks.',
	},
	noTracks: {
		title: 'No tracks found',
		copy: 'Try a different mode or seed track.',
	},
	loadFailed: {
		title: "Couldn't load the map",
		copy: 'Something went wrong fetching your sound space.',
	},
};

export const REASON_LABELS: Record<DiscoverReason, string> = {
	harmonic: 'Harmonic match',
	behavioral: 'Listening pattern',
	bpm: 'Tempo match',
	artist: 'Same artist',
	album: 'Same album',
	genre: 'Genre overlap',
	energy: 'Energy match',
	external: 'External signal',
	unknown: 'Related',
};

export const REASON_EXPLANATIONS: Record<DiscoverReason, string> = {
	harmonic: 'These tracks share a compatible key or tonal color.',
	behavioral: 'Listeners who play one often play the other.',
	bpm: 'These tracks run at a similar tempo.',
	artist: 'From the same artist or creative lineage.',
	album: 'From the same album or release.',
	genre: 'Rooted in the same genre or subgenre family.',
	energy: 'These tracks hit a similar physical intensity.',
	external: 'Connected via Last.fm, Discogs, or external taste graphs.',
	unknown: 'A deeper connection your model has detected.',
};

export const RADIO_MODE_NAMES: Record<string, string> = {
	Familiar: 'Near Orbit',
	Mixed: 'Open Current',
	Adventurous: 'Deep Signal',
};

export const TRAINING_PHASES = [
	'Tracing recent plays…',
	'Reading listening memory…',
	'Hashing behavioral trails…',
	'Finding tempo bridges…',
	'Measuring harmonic links…',
	'Detecting hub tracks…',
	'Rebalancing cold discoveries…',
	'Stabilizing constellations…',
];

export const TRAINING_COMPLETE_TOAST = (n: number) =>
	`Discovery map updated · ${n.toLocaleString()} relationships recalculated`;

export const LENS_LABELS = {
	energy: 'Energy',
	reason: 'Reason',
	confidence: 'Confidence',
	source: 'Source',
	genre: 'Genre',
};

export const SOURCE_LABELS = {
	library: 'Your library',
	lastfm: 'Last.fm',
	engine: 'Discovery engine',
	mixed: 'Mixed',
};

export const ERROR_TOASTS = {
	radioRouteFailed: "Couldn't generate route",
	hideFailed: "Couldn't save preference",
	searchFailed: 'Search jump failed',
};

export const HYPERSPACE_OVERLAY = (query: string) => `Jumping to: ${query}`;

export const SIDE_PANEL_ACTIONS = {
	playNow: 'Play now',
	playNext: 'Play next',
	startRadioHere: 'Start radio here',
	lockAsAnchor: 'Lock as anchor',
	addToPlaylist: 'Add to playlist',
	hideFromRadio: 'Hide from radio',
};
