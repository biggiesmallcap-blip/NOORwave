import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const settings = read('../src/routes/settings/+page.svelte');
const manifest = read('../src/lib/components/settings/settingsManifest.ts');
const automix = read('../src/routes/automix/+page.svelte');
const mixIntent = read('../src/lib/components/dj-cockpit/MixIntentControl.svelte');

describe('Settings > Playback > Transitions', () => {
	test('crossfade and the default transition style live in Playback', () => {
		expect(settings).toContain('data-setting-id="transitions"');
		expect(settings).toContain('<Segmented label="Crossfade"');
		expect(settings).toContain('<Dropdown label="Default transition style"');
		expect(manifest).toContain("{ id: 'transitions', category: 'playback'");
	});

	test('the old homes no longer own them', () => {
		expect(automix).not.toContain('crossfade-slider');
		expect(mixIntent).not.toContain('Transition style');
	});
});
