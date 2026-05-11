import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

describe('Tauri external link capability', () => {
	it('allows the localhost UI to open external URLs through the opener plugin', () => {
		const capability = JSON.parse(readFileSync('../noor-app/capabilities/default.json', 'utf8'));

		expect(capability.permissions).toContain('opener:default');
	});
});
