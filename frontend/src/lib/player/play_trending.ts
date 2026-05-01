import { api, type TidalPlayable } from '$lib/api/client';
import { playTidalTrackNow, playerError } from '$lib/stores/player';

/// Play a TidalPlayable from a chart entry. Last.fm-only entries arrive with
/// `tidal_id === 0` (the backend doesn't pre-search Tidal for them); we resolve
/// via Tidal search before handing off to the player.
export async function playChartTidalTrack(tp: TidalPlayable): Promise<void> {
	if (tp.tidal_id !== 0) {
		return playTidalTrackNow(tp);
	}
	const q = [tp.artist_name, tp.title].filter(Boolean).join(' ');
	if (!q) {
		playerError.set({ message: "Couldn't find that track on Tidal." });
		return;
	}
	try {
		const results = await api.searchTidal(q, 1);
		const hit = results.tracks[0];
		if (!hit) {
			playerError.set({ message: "Couldn't find that track on Tidal." });
			return;
		}
		return playTidalTrackNow({
			tidal_id: hit.tidal_id,
			title: hit.title,
			artist_name: hit.artist_name,
			album_title: hit.album_title,
			artwork_url: hit.artwork_url ?? tp.artwork_url,
			duration_ms: hit.duration_ms,
			artist_tidal_id: null,
		});
	} catch {
		playerError.set({ message: "Couldn't find that track on Tidal." });
	}
}
