import { api, type Playlist, type TidalPlayable } from '$lib/api/client';
import { invalidatePlaylistCaches } from '$lib/cache/ws_events';

/** Preserve the displayed snapshot rather than re-fetching a changing mix. */
export async function saveTidalPlaylist(name: string, tracks: TidalPlayable[]): Promise<Playlist> {
	const title = name.trim();
	if (!title || !tracks.length || tracks.some((track) => track.tidal_id <= 0)) {
		throw new Error('A playlist needs a name and playable TIDAL tracks.');
	}
	const imported = new Map<number, number>();
	const trackIds: number[] = [];
	for (const track of tracks) {
		let id = imported.get(track.tidal_id);
		if (id === undefined) {
			const result = await api.importTidalTrackForRadio(track);
			id = result.local_id;
			imported.set(track.tidal_id, id);
		}
		trackIds.push(id);
	}
	const { playlist } = await api.createTidalPlaylist(title, trackIds, tracks.map((track) => track.tidal_id));
	invalidatePlaylistCaches();
	return playlist;
}
