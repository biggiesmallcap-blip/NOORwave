import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const detail = readFileSync(join(here, '[id]/+page.svelte'), 'utf8');
const root = join(here, '../..');
const read = (relative: string) => readFileSync(join(root, relative), 'utf8');

describe('playlist detail page contracts', () => {
	test('renders the track list through the shared TrackRow', () => {
		expect(detail).toContain("import TrackRow from '$lib/components/TrackRow.svelte';");
		expect(detail).toContain('variant="numbered"');
		// Not a hand-rolled table: TrackRow already owns the row menu, the
		// favourite toggle, artist/album links, and the play/pause affordance.
		expect(detail).not.toContain('<table');
	});

	test('track removal is addressed by position, not track id', () => {
		// playlist_tracks permits the same track at two positions, so a track id
		// would be ambiguous about which copy to drop.
		expect(detail).toContain('removePositions');
		expect(detail).toContain('api.removePlaylistTracks(playlistId, positions)');
		expect(read('lib/api/client.ts')).toContain('removePlaylistTracks(id: number, positions: number[])');
	});

	test('reorder goes through the shared drag action, not a second copy', () => {
		expect(detail).toContain("import { createDragReorder } from '$lib/actions/drag_reorder';");
		expect(detail).toContain('use:drag.row=');
		expect(detail).toContain('api.movePlaylistTrack(playlistId, from, to)');
		// The queue must still be driven by the same action after the extraction.
		expect(read('routes/+layout.svelte')).toContain('createDragReorder({');
	});

	test('exactly one surface handles Alt+Arrow reorder per list', () => {
		// The action provides keyboard reorder by default. The queue keeps its own
		// handler because it also guards the play head and binds Alt+Shift+Up, so
		// it opts out - if both ran, one keypress would move the row twice.
		expect(read('routes/+layout.svelte')).toContain('keyboardReorder: false');
		expect(detail).not.toContain('keyboardReorder: false');
		expect(read('lib/actions/drag_reorder.ts')).toContain("event.key === 'ArrowUp'");
	});

	test('multi-select is scoped to this page rather than sharing library state', () => {
		expect(detail).toContain("import { createSelection } from '$lib/stores/selection';");
		expect(detail).toContain('createSelection()');
		expect(detail).toContain("import SelectionBar from '$lib/components/ui/SelectionBar.svelte';");
		// Sharing library's singletons would surface a playlist selection in the
		// library batch bar.
		expect(detail).not.toContain("selectedTrackIds } from '$lib/stores/library'");
	});

	test('smart playlists are read-only here', () => {
		// Their contents come from rules; the server rejects edits too.
		expect(detail).toContain('let editable = $derived(playlist !== null && !isSmart);');
		expect(detail).toContain('canDrag: () => editable');
	});

	test('menus come from the shared playlist builder', () => {
		expect(detail).toContain(
			"import { buildPlaylistMenu, buildAddToPlaylistSubmenu } from '$lib/player/playlist_menu';",
		);
		expect(detail).toContain('buildPlaylistMenu(playlist, {');
		// downloadPlaylist existed but was reachable from nothing before this.
		expect(detail).toContain('downloadPlaylist(playlistId)');
	});

	test('artwork uses ArtworkImage at approved TIDAL sizes', () => {
		expect(detail).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		// 320 for the mosaic tiles, 640 for the single hero cover.
		expect(detail).toContain('size={320}');
		expect(detail).toContain('size={640}');
	});
});
