// Deep links into the Atlas. `/videos/atlas?artist=123` centres the map on an
// artist and opens its card; `scene=4` does the same for a scene; `trail=1,2,3`
// (oldest first) draws the path the listener took to get there, which is what
// the watch page's mini map sends.

export const ATLAS_PATH = '/videos/atlas';

/** The section search field's label on the Atlas tab (it finds artists on the map). */
export const ATLAS_SEARCH_LABEL = 'Find an artist on the map';
const MAX_TRAIL = 12;

export interface AtlasLinkParams {
	artist?: number | null;
	scene?: number | null;
	trail?: readonly number[] | null;
}

function positiveInt(value: string | null): number | null {
	if (value === null || value.trim() === '') return null;
	const n = Number(value);
	return Number.isSafeInteger(n) && n >= 0 ? n : null;
}

export function atlasUrl(params: AtlasLinkParams = {}): string {
	const query = new URLSearchParams();
	if (params.artist != null) query.set('artist', String(params.artist));
	if (params.scene != null) query.set('scene', String(params.scene));
	const trail = (params.trail ?? []).filter((id) => Number.isSafeInteger(id) && id >= 0).slice(-MAX_TRAIL);
	if (trail.length) query.set('trail', trail.join(','));
	const qs = query.toString();
	return qs ? `${ATLAS_PATH}?${qs}` : ATLAS_PATH;
}

export function parseAtlasParams(search: URLSearchParams): { artist: number | null; scene: number | null; trail: number[] } {
	const trail = (search.get('trail') ?? '')
		.split(',')
		.map((part) => positiveInt(part))
		.filter((id): id is number => id !== null)
		.slice(-MAX_TRAIL);
	return {
		artist: positiveInt(search.get('artist')),
		scene: positiveInt(search.get('scene')),
		trail,
	};
}

/** Links the scene card hands off to other tabs. */
export function sceneChannelUrl(sceneId: number): string {
	return `/videos?channel=scene:${sceneId}`;
}

export function sceneArchiveUrl(sceneId: number): string {
	return `/videos/archive?scene=${sceneId}`;
}
