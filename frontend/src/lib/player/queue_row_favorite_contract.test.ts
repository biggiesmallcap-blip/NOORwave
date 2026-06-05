import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, '../../routes/+layout.svelte'), 'utf8');

describe('queue row favorite contract', () => {
	test('row favorite actions require a local track id', () => {
		expect(source).toContain('function canFavoriteQueueRow(item: QueueItemType): boolean');
		expect(source).toContain('return item.is_pending !== true && item.track.id > 0;');
		expect(source).toContain('disabled={!canFavoriteQueueRow(item)}');
		expect(source).toContain('Favorite unavailable for this queue row');
	});
});
