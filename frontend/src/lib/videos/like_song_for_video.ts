// Settings > Library > "Saving a video likes its song": when on, saving a
// music video also likes the matching song, so the song lands in Library
// favorites too. Off by default because it writes to TIDAL favorites.
// Removing a saved video never unlikes the song.
import { api, type TidalSearchTrack, type TidalSearchVideo } from '$lib/api/client';
import { createPersistedStore, oneOf } from '$lib/stores/persisted';
import { toggleTidalTrackFavorite } from '$lib/stores/player';
import { tidalSearchTrackToPlayable } from '$lib/utils/track';

export const likeSongOnVideoSave = createPersistedStore<'on' | 'off'>('noor.videos.likeSongOnSave', 'off', {
	parse: oneOf(['on', 'off'] as const),
});

// Video-only decorations in a title: "(Official Video)", "[Live at ...]",
// "- Lyric Video". Featured artists and remix names stay, since the song has them too.
const VIDEO_SUFFIX = /\s*[([]\s*[^)\]]*\b(video|visuali[sz]er|lyrics?|live|performance|clip|mv)\b[^)\]]*[)\]]\s*|\s+-\s+.*\b(video|visuali[sz]er)\b.*$/gi;

function normalize(text: string): string {
	return text
		.toLowerCase()
		.normalize('NFKD')
		.replace(/[\u0300-\u036f]/g, '')
		.replace(/&/g, ' and ')
		.replace(/[^a-z0-9]+/g, ' ')
		.trim();
}

export function songTitleForVideo(title: string): string {
	return title.replace(VIDEO_SUFFIX, ' ').replace(/\s+/g, ' ').trim();
}

/** The search hit that is this video's song: same artist, same title. */
export function pickSongForVideo(video: TidalSearchVideo, tracks: TidalSearchTrack[]): TidalSearchTrack | null {
	const title = normalize(songTitleForVideo(video.title));
	const artist = normalize(video.artist_name ?? '');
	if (!title) return null;
	return (
		tracks.find((track) => {
			if (normalize(track.title) !== title) return false;
			if (!artist) return true;
			const trackArtist = normalize(track.artist_name ?? '');
			return (
				(video.artist_id != null && track.artist_id === video.artist_id) ||
				trackArtist === artist ||
				trackArtist.startsWith(`${artist} `) ||
				artist.startsWith(`${trackArtist} `)
			);
		}) ?? null
	);
}

/** Likes the song for a saved video. Resolves to the song title when one was
 *  liked, or null when no matching song was found. */
export async function likeSongForVideo(video: TidalSearchVideo): Promise<string | null> {
	const query = [video.artist_name, songTitleForVideo(video.title)].filter(Boolean).join(' ');
	const result = await api.searchTidal(query, 10);
	const track = pickSongForVideo(video, result.tracks);
	if (!track) return null;
	const liked = await toggleTidalTrackFavorite(tidalSearchTrackToPlayable(track), false);
	return liked ? track.title : null;
}
