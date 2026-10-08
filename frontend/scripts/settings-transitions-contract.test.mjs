import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const settings = read('../src/routes/settings/+page.svelte');
const manifest = read('../src/lib/components/settings/settingsManifest.ts');
const automix = read('../src/lib/components/mix/AutomixPanel.svelte');

// Crossfade is a standing playback preference (it applies when DJ is off), so
// it lives in Settings. The DJ transition style is DJ policy and stays on the
// Mix page with mix intent and speed.
describe('Settings > Playback > Transitions', () => {
	test('crossfade lives in Playback', () => {
		expect(settings).toContain('data-setting-id="transitions"');
		expect(settings).toContain('<Segmented label="Crossfade"');
		expect(manifest).toContain("{ id: 'transitions', category: 'playback'");
	});

	test('the DJ style is not duplicated in Settings', () => {
		expect(settings).not.toContain('<Dropdown label="Default transition style"');
		expect(settings).toContain('href="/mix"');
	});

	test('Automix no longer owns a crossfade control', () => {
		expect(automix).not.toContain('crossfade-slider');
	});
});
