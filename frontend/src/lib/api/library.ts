import { api } from './client';
export type { Track, Album, Artist, Genre, Playlist } from './client';

export const getTracks = api.getTracks.bind(api);
export const getAlbums = api.getAlbums.bind(api);
export const getArtists = api.getArtists.bind(api);
export const getArtistTracks = api.getArtistTracks.bind(api);
export const getAlbumTracks = api.getAlbumTracks.bind(api);
export const search = api.search.bind(api);
export const searchAudio = api.searchAudio.bind(api);
export const addTracksToPlaylist = api.addTracksToPlaylist.bind(api);
export const batchAddToPlaylist = api.batchAddToPlaylist.bind(api);
export const batchDelete = api.batchDelete.bind(api);
export const batchSetGenre = api.batchSetGenre.bind(api);
export const replacePlaybackQueue = api.replacePlaybackQueue.bind(api);
