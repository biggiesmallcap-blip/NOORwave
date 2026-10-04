import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';
const source = readFileSync(new URL('./ListeningServicesPanel.svelte', import.meta.url), 'utf8');
const wrapper = readFileSync(new URL('./IntegrationsPanel.svelte', import.meta.url), 'utf8');
describe('listening service form contract', () => {
	test('uses shared setup and labels every credential input', () => {
		expect(wrapper).toContain('ListeningServicesPanel');
		for (const label of ['Last.fm API key', 'Last.fm shared secret', 'ListenBrainz user token']) expect(source).toContain('aria-label="' + label + '"');
		expect(source).not.toContain('type="text"');
	});
	test('keeps privacy information, credential removal and manual history upload available', () => {
		expect(source).toContain('profiles may be public');
		expect(source).toContain('avoid duplicates');
		expect(source).toContain('Remove Last.fm credentials');
		expect(source).toContain('Upload last 30 days');
		expect(source).toContain('api.backfillScrobbles()');
		expect(source).toContain("result.status === 'up_to_date'");
	});
});
