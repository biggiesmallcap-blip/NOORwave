import type { Track, TidalPlayable } from '$lib/api/client';
import { buildAlbumMenu, type AlbumLike } from '$lib/player/album_menu';
import { buildArtistMenu, type ArtistLike } from '$lib/player/artist_menu';
import { buildTidalTrackMenu, buildTrackMenu, type MenuTrack } from '$lib/player/track_menu';
import type { MenuItem } from '$lib/stores/context_menu';
import { trackToTidalPlayable } from '$lib/utils/track';

export type MediaSource = 'local' | 'tidal';

export type TrackMediaRef =
	| { kind: 'track'; source: 'local'; label: string; track: MenuTrack }
	| { kind: 'track'; source: 'tidal'; label: string; track: TidalPlayable };

export type ArtistMediaRef = {
	kind: 'artist';
	source: MediaSource;
	label: string;
	artist: ArtistLike;
};

export type AlbumMediaRef = {
	kind: 'album';
	source: MediaSource;
	label: string;
	album: AlbumLike;
};

export type MediaRef = TrackMediaRef | ArtistMediaRef | AlbumMediaRef;

export function trackRefFromTrack(track: Track): TrackMediaRef {
	const tidal = trackToTidalPlayable(track);
	if (tidal) {
		return {
			kind: 'track',
			source: 'tidal',
			label: track.title,
			track: tidal,
		};
	}
	return {
		kind: 'track',
		source: 'local',
		label: track.title,
		track,
	};
}

export function artistRefFromTrack(track: Track): ArtistMediaRef | null {
	if (track.artist_id != null && track.artist_id > 0) {
		return {
			kind: 'artist',
			source: 'local',
			label: track.artist_name ?? 'Unknown artist',
			artist: {
				id: track.artist_id,
				name: track.artist_name ?? 'Unknown artist',
				in_library: true,
			},
		};
	}
	if (track.artist_tidal_id != null) {
		return {
			kind: 'artist',
			source: 'tidal',
			label: track.artist_name ?? 'Unknown artist',
			artist: {
				tidal_id: track.artist_tidal_id,
				name: track.artist_name ?? 'Unknown artist',
				in_library: false,
			},
		};
	}
	return null;
}

export function albumRefFromTrack(track: Track): AlbumMediaRef | null {
	if (track.album_id != null) {
		return {
			kind: 'album',
			source: 'local',
			label: track.album_title ?? 'Unknown album',
			album: {
				id: track.album_id,
				title: track.album_title ?? 'Unknown album',
				artist_id: track.artist_id > 0 ? track.artist_id : null,
				artist_name: track.artist_name,
				in_library: true,
			},
		};
	}
	if (track.album_tidal_id != null) {
		return {
			kind: 'album',
			source: 'tidal',
			label: track.album_title ?? 'Unknown album',
			album: {
				tidal_id: track.album_tidal_id,
				title: track.album_title ?? 'Unknown album',
				artist_name: track.artist_name,
				in_library: false,
			},
		};
	}
	return null;
}

export function mediaHref(ref: MediaRef | null): string | null {
	if (!ref) return null;
	if (ref.kind === 'track') {
		if (ref.source === 'local' && ref.track.album_id != null) return `/albums/${ref.track.album_id}`;
		return null;
	}
	if (ref.kind === 'artist') {
		if (ref.source === 'local' && ref.artist.id != null) return `/artists/${ref.artist.id}`;
		if (ref.source === 'tidal' && ref.artist.tidal_id != null) return `/tidal/artists/${ref.artist.tidal_id}`;
		return null;
	}
	if (ref.source === 'local' && ref.album.id != null) return `/albums/${ref.album.id}`;
	if (ref.source === 'tidal' && ref.album.tidal_id != null) return `/tidal/albums/${ref.album.tidal_id}`;
	return null;
}

export function buildMediaMenu(ref: MediaRef): MenuItem[] {
	if (ref.kind === 'track') {
		return ref.source === 'tidal' ? buildTidalTrackMenu(ref.track) : buildTrackMenu(ref.track);
	}
	if (ref.kind === 'artist') {
		return buildArtistMenu(ref.artist, { isLocal: ref.source === 'local' });
	}
	return buildAlbumMenu(ref.album, { isLocal: ref.source === 'local' });
}
