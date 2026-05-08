import { api } from './client';
export type {
	TidalSearchResults,
	TidalSearchAlbum,
	TidalSearchArtist,
	TidalSearchTrack,
	TidalSearchPlaylist,
	AudioSearchResult,
	AudioSearchParams,
	Genre,
	VibeTrack,
	BasicTrack,
	Playlist,
	SpotifyPlaylistSearchItem,
} from './client';

export const search = api.search.bind(api);
export const searchTidal = api.searchTidal.bind(api);
export const searchTidalPlaylists = api.searchTidalPlaylists.bind(api);
export const searchSpotifyPlaylists = api.searchSpotifyPlaylists.bind(api);
export const searchAudio = api.searchAudio.bind(api);
export const getRecentListens = api.getRecentListens.bind(api);
export const getVibeTracksForTrack = api.getVibeTracksForTrack.bind(api);
export const getUnderratedTracksForArtist = api.getUnderratedTracksForArtist.bind(api);
