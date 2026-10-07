export type SettingsCategoryId = 'appearance' | 'playback' | 'library' | 'services' | 'remote' | 'app';
export const SETTINGS_CATEGORIES: { id: SettingsCategoryId; label: string }[] = [
	{ id: 'appearance', label: 'Appearance' }, { id: 'playback', label: 'Playback' },
	{ id: 'library', label: 'Library' }, { id: 'services', label: 'Services' },
	{ id: 'remote', label: 'Remote' }, { id: 'app', label: 'App' },
];
export interface SettingsSearchEntry {
	id: string; category: SettingsCategoryId; label: string; keywords: string;
	target?: string; focus?: string;
}
export const SETTINGS_SEARCH_INDEX: SettingsSearchEntry[] = [
	{ id: 'surface-mode', category: 'appearance', label: 'Theme', keywords: 'dark light system automatic surface mode' },
	{ id: 'colour-scheme', category: 'appearance', label: 'Colour scheme', keywords: 'palette color accent swatch' },
	{ id: 'interface-size', category: 'appearance', label: 'Interface size', keywords: 'zoom scale text bigger smaller reset shortcuts' },
	{ id: 'player-position', category: 'appearance', label: 'Player position', keywords: 'layout right left bottom horizontal sidebar queue dock' },
	{ id: 'player-artwork', category: 'appearance', label: 'Side artwork', keywords: 'album art cover square banner image' },
	{ id: 'player-information', category: 'appearance', label: 'Player quality display', keywords: 'side bottom icon details bitrate sample rate lossless hires streaming quality' },
	{ id: 'horizontal-shelves', category: 'appearance', label: 'Scroll shelves with mouse wheel', keywords: 'horizontal scrolling trackpad carousel rail navigation sideways' },
	{ id: 'background', category: 'appearance', label: 'Background', keywords: 'wallpaper shader off gallery animation' },
	{ id: 'background-fps', target: 'background', focus: '[aria-label="Frame rate"]', category: 'appearance', label: 'Frame rate', keywords: 'fps background wallpaper rendering' },
	{ id: 'background-motion', target: 'background', focus: '[aria-label="Reduce motion"]', category: 'appearance', label: 'Reduce motion', keywords: 'background animation system motion accessibility' },
	{ id: 'background-blur', target: 'background', focus: '[aria-label="Background blur"]', category: 'appearance', label: 'Background blur', keywords: 'wallpaper rendering blur' },
	{ id: 'playback-output', category: 'playback', label: 'Audio output', keywords: 'quality device bit-perfect exclusive wasapi sample rate follow latency crossfade lossless output dac' },
	{ id: 'advanced-output', target: 'playback-output', focus: '.audio-advanced', category: 'playback', label: 'Advanced output', keywords: 'exclusive buffer idle release pause sample rate latency grace' },
	{ id: 'video-quality', target: 'playback-output', focus: '[aria-label="Video quality"]', category: 'playback', label: 'Video quality', keywords: 'max highest auto video' },
	{ id: 'now-playing-path', category: 'playback', label: 'Output details', keywords: 'runtime device format now playing track path diagnostics' },
	{ id: 'library-audio-data', category: 'playback', label: 'Analyse while playing', keywords: 'analysis bpm key energy dsp passive audio data' },
	{ id: 'library-sync', category: 'library', label: 'Sync library', keywords: 'tidal sync daily auto-sync full resync cancel favourite favorite albums' },
	{ id: 'musicbrainz-enrichment', category: 'library', label: 'MusicBrainz genres', keywords: 'genre metadata enrich tags galaxy resume' },
	{ id: 'last-fm-tags', category: 'library', label: 'Last.fm tags', keywords: 'lastfm last.fm enrichment genres context retry untagged recheck' },
	{ id: 'downloads', category: 'library', label: 'Downloads', keywords: 'download folder flac mp3 aac m4a format quality save disk source' },
	{ id: 'discovery-engine', category: 'library', label: 'Discovery', keywords: 'training learning intensity safety engine model refresh retrain stop cost watchdog' },
	{ id: 'radio-similarity-index', category: 'library', label: 'Radio index', keywords: 'radio similarity neighbours pairs index build rebuild last built' },
	{ id: 'portable-snapshot', category: 'library', label: 'Enrichment transfer', keywords: 'export import snapshot portable transfer backup metadata' },
	{ id: 'database-size', category: 'library', label: 'Database storage', keywords: 'database size disk space vacuum compact shrink storage big large noor.db' },
	{ id: 'clear-non-library-entries', category: 'library', label: 'Remove unused recommendations', keywords: 'clear non-library cleanup purge orphan' },
	{ id: 'library-maintenance', category: 'library', label: 'Library management', keywords: 'maintenance clean reclean merge duplicates reset last.fm tags clear audio analysis' },
	{ id: 'connect-tidal', category: 'services', label: 'TIDAL', keywords: 'connect tidal login auth authentication streaming disconnect' },
	{ id: 'tidal-content-preferences', category: 'services', label: 'Hide AI-generated tracks', keywords: 'tidal ai artificial intelligence generated recordings songs music filter block allow preferences' },
	{ id: 'video-discovery', category: 'services', label: 'Video discovery', keywords: 'music videos video radio crawler crawl index indexing background discovery tidal requests limited off' },
	{ id: 'explore-stations', category: 'services', label: 'Explore stations', keywords: 'video stations explore scenes genres latin reggaeton reggae metal punk classical jazz country disco funk afrobeats k-pop hide' },
	{ id: 'lastfm-service', category: 'services', label: 'Last.fm connection', keywords: 'lastfm last.fm scrobble scrobbling profile recommendations api key secret credentials account approval auth disconnect' },
	{ id: 'listenbrainz-service', category: 'services', label: 'ListenBrainz connection', keywords: 'listenbrainz scrobble scrobbling profile recommendations token connect disconnect' },
	{ id: 'listening-history', category: 'services', label: 'Listening history', keywords: 'backfill upload last 30 days pending failed status submissions scrobbles' },
	{ id: 'phone-remote', category: 'remote', label: 'Phone remote', keywords: 'qr network pair pairing device wifi local address access token pin password regenerate reset recovery' },
	{ id: 'access-pin', category: 'remote', label: 'Recovery PIN', keywords: 'master pin password token regenerate reset recovery' },
	{ id: 'startup', category: 'app', label: 'Start at sign-in', keywords: 'startup autostart tray sign in login' },
	{ id: 'closing-the-window', category: 'app', label: 'Close to tray', keywords: 'tray minimize close quit exit window behaviour keep running' },
	{ id: 'app-updates', category: 'app', label: 'Updates', keywords: 'version update install mode upgrade release check patch info' },
];
const CATEGORY_ALIASES: Record<string, SettingsCategoryId> = { sources: 'library', audio: 'playback', account: 'remote' };
const ANCHOR_ALIASES: Record<string, string> = { 'sources-tidal': 'connect-tidal', 'integrations-listening': 'lastfm-service' };
export function categoryLabel(category: SettingsCategoryId): string {
	return SETTINGS_CATEGORIES.find((entry) => entry.id === category)?.label ?? category;
}
export function resolveSettingsLocation(url: URL): { category: SettingsCategoryId; entry?: SettingsSearchEntry } {
	let anchor = url.hash.slice(1);
	try { anchor = decodeURIComponent(anchor); } catch { /* Malformed external anchor: ignore it. */ }
	const requested = url.searchParams.get('setting') ?? anchor;
	const entry = SETTINGS_SEARCH_INDEX.find((item) => item.id === (ANCHOR_ALIASES[requested] ?? requested));
	if (entry) return { category: entry.category, entry };
	const category = url.searchParams.get('category') ?? 'appearance';
	return { category: SETTINGS_CATEGORIES.some((item) => item.id === category) ? category as SettingsCategoryId : CATEGORY_ALIASES[category] ?? 'appearance' };
}
export function settingsHref(category: SettingsCategoryId, setting?: string): string {
	return '/settings?category=' + category + (setting ? '&setting=' + encodeURIComponent(setting) : '');
}
export function searchSettings(query: string): SettingsSearchEntry[] {
	const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
	if (!terms.length) return [];
	return SETTINGS_SEARCH_INDEX.filter((entry) => {
		const text = (entry.label + ' ' + entry.keywords + ' ' + categoryLabel(entry.category)).toLowerCase();
		return terms.every((term) => text.includes(term));
	});
}
