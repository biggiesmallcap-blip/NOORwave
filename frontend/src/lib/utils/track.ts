import type {
	MixedQueueItem,
	QueueItem,
	Track,
	TidalDiscographyTrack,
	TidalHomeItem,
	TidalPlayable,
	TidalSearchTrack,
} from '$lib/api/client';

/**
 * Convert an ephemeral `Track` (a now-playing or queue row) into a
 * `TidalPlayable` for `buildTidalTrackMenu` / `startTidalSongRadio`.
 *
 * Returns `null` for tracks that are NOT ephemeral Tidal entries.
 * Real `tidal_stream` library rows stay on the library path; enriched
 * `tidal_ephemeral` rows keep the Tidal path even after they gain a local id.
 * Callers fall back to `buildTrackMenu` when this returns null.
 *
 * Detection: `play_tidal_ephemeral` on the backend constructs a
 * synthetic `Track { id: -tidal_track_id, source: 'tidal_ephemeral', ... }`.
 * Store-side enrichment can later replace the id with a local id, so source is
 * also part of the signal.
 */
export function trackToTidalPlayable(track: Track): TidalPlayable | null {
	if (track.tidal_id == null) return null;
	if (track.id >= 0 && track.source !== 'tidal_ephemeral') return null;
	return trackWithTidalIdToPlayable(track);
}

function trackWithTidalIdToPlayable(track: Track): TidalPlayable | null {
	if (track.tidal_id == null || track.tidal_id <= 0) return null;
	const localId = track.id > 0 ? track.id : null;
	return {
		tidal_id: track.tidal_id,
		title: track.title,
		artist_name: track.artist_name,
		album_title: track.album_title,
		artwork_url: track.artwork_url,
		duration_ms: track.duration_ms,
		artist_tidal_id: track.artist_tidal_id ?? null,
		album_tidal_id: track.album_tidal_id ?? null,
		local_id: localId,
		is_in_library: localId != null,
		is_favorite: track.is_favorite ?? false,
	};
}

export function queueItemToTidalPlayable(item: QueueItem): TidalPlayable | null {
	const direct = trackToTidalPlayable(item.track);
	if (direct) return direct;
	if (item.id >= 0 || item.source !== 'tidal_mix') return null;
	return trackWithTidalIdToPlayable(item.track);
}

/**
 * Convert a `TidalSearchTrack` (from `/api/tidal/search` results)
 * into a `TidalPlayable`. Lifted from search/+page.svelte's local
 * helper so frontend has one place to do this conversion.
 */
export function tidalSearchTrackToPlayable(track: TidalSearchTrack): TidalPlayable {
	return {
		...track,
		artist_tidal_id: track.artist_id ?? null,
		album_tidal_id: track.album_tidal_id ?? null,
		local_id: track.local_id ?? null,
		is_in_library: track.in_library,
	};
}

export function tidalDiscographyTrackToPlayable(
	track: TidalDiscographyTrack,
	options: { artistTidalId?: number | null } = {},
): TidalPlayable {
	return {
		tidal_id: track.tidal_id,
		title: track.title,
		artist_name: track.artist_name ?? null,
		album_title: track.album_title ?? null,
		artwork_url: track.artwork_url ?? null,
		duration_ms: track.duration_ms,
		artist_tidal_id: track.artist_tidal_id ?? options.artistTidalId ?? null,
		album_tidal_id: track.album_tidal_id ?? null,
		track_id: track.track_id,
		local_id: track.track_id ?? null,
		is_in_library: track.is_in_library,
		is_favorite: track.is_favorite,
	};
}

/**
 * One row of an album's full track listing: an owned library row or a
 * TIDAL-only row the library is missing. The discriminated shape (instead of
 * a lossy conversion to one common type) keeps owned rows playing through the
 * local library pipeline and TIDAL-only rows on the streaming path.
 */
export type AlbumTrackEntry =
	| { kind: 'local'; local: Track }
	| { kind: 'tidal'; tidal: TidalDiscographyTrack };

/**
 * Merge an album's owned library rows with its TIDAL-only rows into one list
 * ordered by (disc, track) number so the album reads and plays 1..N even when
 * ownership is scattered. Owned rows are always kept - including ones with no
 * TIDAL id (local-only rips/bonus tracks) - because they play from the
 * library. Rows without a track number sort after numbered ones.
 */
export function mergeAlbumTracks(
	localTracks: readonly Track[],
	tidalOnlyTracks: readonly TidalDiscographyTrack[],
): AlbumTrackEntry[] {
	const ordered: { disc: number; track: number; entry: AlbumTrackEntry }[] = [];
	for (const t of localTracks) {
		ordered.push({
			disc: t.disc_number ?? 1,
			track: t.track_number ?? Number.MAX_SAFE_INTEGER,
			entry: { kind: 'local', local: t },
		});
	}
	for (const t of tidalOnlyTracks) {
		ordered.push({
			disc: t.disc_number ?? 1,
			track: t.track_number ?? Number.MAX_SAFE_INTEGER,
			entry: { kind: 'tidal', tidal: t },
		});
	}
	ordered.sort((a, b) => a.disc - b.disc || a.track - b.track);
	return ordered.map((item) => item.entry);
}

/**
 * Locate the start row for "play the album from here". `startId` is a local
 * track id when an owned row was clicked and a tidal id when a TIDAL-only row
 * was clicked; local ids are matched first so the two id spaces can't collide.
 * An unknown id starts from the top.
 */
export function albumEntryStartIndex(
	entries: readonly AlbumTrackEntry[],
	startId: number | undefined,
): number {
	if (startId == null) return 0;
	const byLocal = entries.findIndex((e) => e.kind === 'local' && e.local.id === startId);
	if (byLocal >= 0) return byLocal;
	const byTidal = entries.findIndex((e) => e.kind === 'tidal' && e.tidal.tidal_id === startId);
	return byTidal >= 0 ? byTidal : 0;
}

/** Map an album entry to a POST /api/playback/queue/mixed request row. */
export function albumEntryToMixedQueueItem(entry: AlbumTrackEntry): MixedQueueItem {
	if (entry.kind === 'local') {
		return {
			track_id: entry.local.id,
			artist: entry.local.artist_name,
			title: entry.local.title,
		};
	}
	return {
		tidal_id: entry.tidal.tidal_id,
		artist: entry.tidal.artist_name ?? null,
		title: entry.tidal.title,
	};
}

/**
 * True when the now-playing track belongs to the given page's track lists,
 * whether it's an owned library row (matched by local id) or a streamed row
 * (matched by tidal id). Shared by the album page and the artist page so the
 * "is this page's content playing" contract can't drift between surfaces.
 */
export function currentTrackMatchesTracks(
	current: Track | null,
	localTracks: readonly Track[],
	tidalTracks: readonly TidalDiscographyTrack[],
): boolean {
	if (!current) return false;
	if (localTracks.some((t) => t.id === current.id)) return true;
	return current.tidal_id != null && tidalTracks.some((t) => t.tidal_id === current.tidal_id);
}

export function tidalHomeItemToPlayable(item: TidalHomeItem): TidalPlayable {
	return {
		tidal_id: Number(item.id),
		title: item.title,
		artist_name: item.artist_name ?? null,
		album_title: item.album_title ?? null,
		artwork_url: item.artwork_url ?? null,
		duration_ms: item.duration != null ? item.duration * 1000 : null,
		artist_tidal_id: item.artist_id ?? null,
		album_tidal_id: item.album_id ?? null,
	};
}
