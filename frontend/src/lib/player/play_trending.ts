import { api, type TidalPlayable } from '$lib/api/client';
import { playTidalTrackNow, playTidalTracksNow, playerError } from '$lib/stores/player';

/// Play a TidalPlayable from a chart entry. Last.fm-only entries arrive with
/// `tidal_id === 0` (the backend doesn't pre-search Tidal for them); we resolve
/// via Tidal search before handing off to the player.
export async function resolveChartTidalTrack(tp: TidalPlayable): Promise<TidalPlayable | null> {
	if (tp.tidal_id !== 0) {
		return tp;
	}
	const q = [tp.artist_name, tp.title].filter(Boolean).join(' ');
	if (!q) {
		return null;
	}
	try {
		const results = await api.searchTidal(q, 1);
		const hit = results.tracks.find((track) => track.stream_ready !== false);
		if (!hit) {
			return null;
		}
		return {
			tidal_id: hit.tidal_id,
			title: hit.title,
			artist_name: hit.artist_name,
			album_title: hit.album_title,
			artwork_url: hit.artwork_url ?? tp.artwork_url,
			duration_ms: hit.duration_ms,
			artist_tidal_id: null,
			album_tidal_id: hit.album_tidal_id,
			local_id: hit.local_id ?? null,
			is_in_library: hit.in_library,
		};
	} catch {
		return null;
	}
}

export async function playChartTidalTrack(tp: TidalPlayable): Promise<void> {
	const playable = await resolveChartTidalTrack(tp);
	if (!playable) {
		playerError.set({ message: "Couldn't find that track on Tidal." });
		return;
	}
	return playTidalTrackNow(playable);
}

export async function playChartTidalTracks(tracks: TidalPlayable[], label = 'recommendations'): Promise<void> {
	const playable: TidalPlayable[] = [];
	for (const track of tracks) {
		const resolved = await resolveChartTidalTrack(track);
		if (resolved) playable.push(resolved);
	}
	if (!playable.length) {
		playerError.set({ message: 'No playable tracks ready yet.' });
		return;
	}
	return playTidalTracksNow(playable, label);
}
