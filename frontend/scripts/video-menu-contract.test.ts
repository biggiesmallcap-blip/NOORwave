import { describe, expect, test, vi } from 'vitest';

const { goto } = vi.hoisted(() => ({ goto: vi.fn() }));

vi.mock('$app/navigation', () => ({ goto }));

import { buildVideoMenu } from '../src/lib/player/video_menu';

describe('video menu contracts', () => {
	test('video open action resolves inside NOORwave instead of opening TIDAL externally', () => {
		const open = vi.fn();
		vi.stubGlobal('window', { open });
		vi.stubGlobal('location', { origin: 'http://localhost:5173' });

		const items = buildVideoMenu({
			tidal_id: 12345,
			title: 'Live Clip',
			duration_ms: 1000,
			artist_id: 678,
			artist_name: 'Video Artist',
			album_tidal_id: null,
			artwork_url: null,
			quality: null,
			explicit: null,
			type: 'video',
		});

		const openItem = items.find((item) => item.label === 'Open video');
		expect(openItem).toBeDefined();
		openItem?.onSelect?.();

		expect(goto).toHaveBeenCalledWith('/videos?videoId=12345');
		expect(open).not.toHaveBeenCalled();
	});
});
