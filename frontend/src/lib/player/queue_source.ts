/**
 * Queue source labels, slugs and the legend that explains them.
 *
 * Playback rows carry the raw source string the backend wrote (`radio_pending`,
 * `lastfm_similar`, `user_play_next`, ...). Those strings are internal, so
 * every surface that shows a source to a human goes through
 * `formatQueueSource`. The final fallback prettifies whatever it is handed
 * rather than echoing it, which is what used to leak `radio_pending` into the
 * now-playing panel.
 */

export type LegendEntry = { slug: string; label: string };

/** Labels that are also legend keys - keep the two lists in agreement. */
const LABEL_LIBRARY = 'Library';
const LABEL_PLAYLIST = 'Playlist';
const LABEL_GENRE = 'Genre';
const LABEL_AUTOMIX = 'Automix';
const LABEL_DISCOVER = 'Discover';
const LABEL_RADIO = 'Song radio';
const LABEL_LASTFM = 'Last.fm';
const LABEL_TIDAL = 'TIDAL';
const LABEL_SPOTIFY = 'Spotify';
const LABEL_DJ = 'DJ';
const LABEL_MANUAL = 'Manual';

/**
 * Sources the now-playing card does not bother naming: the user put the track
 * there, so "Manual" tells them nothing they don't know.
 */
export const SILENT_SOURCE_LABELS = new Set([LABEL_MANUAL, 'Queued']);

/** Turn an unknown slug into something presentable: `foo_bar` -> `Foo bar`. */
function prettifySlug(source: string): string {
	const words = source
		.trim()
		.split(/[_\-\s]+/)
		.filter(Boolean);
	if (words.length === 0) return 'Queued';
	const [first, ...rest] = words;
	return [first.charAt(0).toUpperCase() + first.slice(1).toLowerCase(), ...rest.map((w) => w.toLowerCase())].join(' ');
}

export function formatQueueSource(source: string): string {
	const normalized = (source ?? '').trim().toLowerCase();
	if (!normalized) return 'Queued';
	// Hand-queued rows land here as user / user_queue / user_play_next /
	// manual_*. All of them mean "you put it there", so they share one label
	// that the now-playing card then declines to print.
	if (normalized.startsWith('user') || normalized.includes('manual') || normalized.includes('queue')) {
		return LABEL_MANUAL;
	}
	if (normalized.includes('automix')) return LABEL_AUTOMIX;
	// The radio orchestrator's own picks are automix in everything but name.
	if (normalized === 'engine') return LABEL_AUTOMIX;
	if (normalized.startsWith('radio')) return LABEL_RADIO;
	if (normalized.includes('lastfm')) return LABEL_LASTFM;
	if (normalized.includes('tidal')) return LABEL_TIDAL;
	if (normalized.includes('spotify')) return LABEL_SPOTIFY;
	if (normalized === 'dj' || normalized.startsWith('dj_')) return LABEL_DJ;
	if (normalized.includes('genre')) return LABEL_GENRE;
	if (normalized.includes('discover') || normalized.includes('blend')) return LABEL_DISCOVER;
	if (normalized.includes('playlist')) return LABEL_PLAYLIST;
	if (normalized.includes('library') || normalized === 'local') return LABEL_LIBRARY;
	if (normalized === 'external') return 'Outside library';
	return prettifySlug(source);
}

/**
 * Slug behind the `.source-*` class that paints the dot on queue artwork.
 * Colours live in app.css so both the queue rows and the legend can use them.
 * The one special case is `automix-new` (discover-injected automix), which
 * keeps its own slug so the dot can wear a distinguishing ring.
 */
export function queueSourceSlug(source: string): string {
	const normalized = (source ?? '').trim().toLowerCase();
	if (normalized === 'automix-new') return 'automix-new';
	return formatQueueSource(source).toLowerCase().replace(/[^a-z0-9]+/g, '-');
}

export const SOURCE_LEGEND: LegendEntry[] = [
	{ slug: 'library', label: LABEL_LIBRARY },
	{ slug: 'playlist', label: LABEL_PLAYLIST },
	{ slug: 'genre', label: LABEL_GENRE },
	{ slug: 'automix', label: LABEL_AUTOMIX },
	{ slug: 'automix-new', label: 'Discover automix' },
	{ slug: 'discover', label: LABEL_DISCOVER },
	{ slug: 'song-radio', label: LABEL_RADIO },
	{ slug: 'last-fm', label: LABEL_LASTFM },
	{ slug: 'tidal', label: LABEL_TIDAL },
	{ slug: 'dj', label: LABEL_DJ },
	{ slug: 'manual', label: LABEL_MANUAL },
];
