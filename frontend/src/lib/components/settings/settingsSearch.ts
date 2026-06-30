// Search index for the settings page. Each entry maps to a section's
// data-setting-id anchor so a match can switch to the right category and
// scroll the section into view. Keywords carry synonyms and the names of
// notable controls inside each section so a search like "exclusive" or
// "scrobble" finds the section that hosts it.

export type SettingsCategoryId = 'appearance' | 'sources' | 'audio' | 'account';

export interface SettingsSearchEntry {
	/** Matches the data-setting-id attribute on the section element. */
	id: string;
	category: SettingsCategoryId;
	label: string;
	/** Space-separated synonyms and notable control names for matching. */
	keywords: string;
}

export const SETTINGS_SEARCH_INDEX: SettingsSearchEntry[] = [
	{ id: 'colour-scheme', category: 'appearance', label: 'Colour scheme', keywords: 'palette color colour accent theme swatch' },
	{ id: 'interface-size', category: 'appearance', label: 'Interface size', keywords: 'zoom ui scale text size bigger smaller' },
	{ id: 'background', category: 'appearance', label: 'Background', keywords: 'wallpaper shader blur fps animation' },
	{ id: 'access-pin', category: 'account', label: 'Access PIN', keywords: 'token pin password remote pair device regenerate' },
	{ id: 'app-updates', category: 'account', label: 'App updates', keywords: 'version update install mode upgrade release check' },
	{ id: 'closing-the-window', category: 'account', label: 'Closing the window', keywords: 'tray minimize close quit exit window behaviour' },
	{ id: 'connect-tidal', category: 'sources', label: 'Connect TIDAL', keywords: 'tidal login auth sync library auto-sync streaming' },
	{ id: 'musicbrainz-enrichment', category: 'sources', label: 'MusicBrainz enrichment', keywords: 'musicbrainz genre metadata enrich tags' },
	{ id: 'spotify-tags', category: 'sources', label: 'Spotify tags', keywords: 'spotify genre metadata enrich' },
	{ id: 'last-fm-tags', category: 'sources', label: 'Last.fm tags', keywords: 'lastfm last.fm scrobble tags api key listenbrainz' },
	{ id: 'playback-output', category: 'audio', label: 'Playback output', keywords: 'quality device bit-perfect exclusive wasapi sample rate follow latency crossfade lossless output dac' },
	{ id: 'portable-snapshot', category: 'sources', label: 'Portable snapshot', keywords: 'export import backup snapshot transfer enrichment' },
	{ id: 'clear-non-library-entries', category: 'sources', label: 'Clear non-library entries', keywords: 'clear remove non-library cleanup purge' },
	{ id: 'downloads', category: 'audio', label: 'Downloads', keywords: 'download folder flac mp3 format quality save disk' },
	{ id: 'discovery-engine', category: 'audio', label: 'Discovery engine', keywords: 'discovery training learning radio intensity safety engine model' },
	{ id: 'radio-similarity-index', category: 'audio', label: 'Radio similarity index', keywords: 'radio similarity neighbours index coverage' },
	{ id: 'now-playing-path', category: 'audio', label: 'Now playing path', keywords: 'runtime device format now playing output path' },
	{ id: 'additional-services', category: 'sources', label: 'Additional services', keywords: 'planned services future sources soundcloud youtube' },
	{ id: 'library-audio-data', category: 'audio', label: 'Library audio data', keywords: 'analysis bpm key energy dsp passive audio data' },
	{ id: 'acrcloud', category: 'sources', label: 'ACRCloud', keywords: 'acrcloud recognition fingerprint sample cover detection' },
];

/**
 * Returns the settings sections matching every whitespace-separated term in
 * the query. Empty/whitespace queries return an empty list.
 */
export function searchSettings(query: string): SettingsSearchEntry[] {
	const q = query.trim().toLowerCase();
	if (!q) return [];
	const terms = q.split(/\s+/);
	return SETTINGS_SEARCH_INDEX.filter((entry) => {
		const haystack = `${entry.label} ${entry.keywords} ${entry.category}`.toLowerCase();
		return terms.every((term) => haystack.includes(term));
	});
}
