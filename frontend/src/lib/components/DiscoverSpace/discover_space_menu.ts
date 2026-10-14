// The shared right-click menu for a Sound Space star, wherever it appears:
// canvas node, results row, or the track card. Library rows get the library
// track menu, TIDAL rows the TIDAL one; a Last.fm match that has not resolved
// to a track yet still gets its artist's menu.

import { openContextMenu } from '$lib/stores/context_menu';
import { buildArtistMenu } from '$lib/player/artist_menu';
import { buildTidalTrackMenu, buildTrackMenu } from '$lib/player/track_menu';
import type { DiscoverTrackNode } from './discover_space_types';

type MenuEvent = Parameters<typeof openContextMenu>[0];

export function openNodeMenu(event: MenuEvent, node: DiscoverTrackNode): void {
	const playable = node.playable;
	if (playable?.kind === 'library') {
		openContextMenu(event, buildTrackMenu(playable.track), node.title);
	} else if (playable?.kind === 'tidal') {
		openContextMenu(event, buildTidalTrackMenu(playable.track), node.title);
	} else if (node.artist) {
		openContextMenu(event, buildArtistMenu({ name: node.artist }), node.artist);
	}
}
