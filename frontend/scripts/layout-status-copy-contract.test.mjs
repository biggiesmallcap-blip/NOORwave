import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(import.meta.dirname, '../src/routes/+layout.svelte'), 'utf8');

describe('layout status copy', () => {
	test('sidebar connection status uses plain websocket meaning', () => {
		expect(source).toContain("'Server connected'");
		expect(source).toContain("'Server offline'");
		expect(source).toContain("'Realtime updates active'");
		expect(source).toContain("'Waiting for realtime updates'");
		expect(source).not.toContain('Observatory live');
		expect(source).not.toContain('Realtime stream is locked in');
		expect(source).not.toContain('Waiting for websocket relay');
	});

	test('shuffle sidebar status is labeled as state, not a standalone notification', () => {
		expect(source).toContain('Shuffle mode: {shuffleStatusLabels[$shuffleMode]}');
		expect(source).toContain("true: 'True random'");
		expect(source).not.toContain("<p class=\"status-line\">{shuffleLabels[$shuffleMode]}</p>");
	});
});
