import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'IntegrationsPanel.svelte'), 'utf8');
const client = readFileSync(join(here, '../../api/client.ts'), 'utf8');
const settingsPage = readFileSync(join(here, '../../../routes/settings/+page.svelte'), 'utf8');

describe('integrations settings contract', () => {
	test('wires Last.fm and ListenBrainz status and config APIs', () => {
		expect(source).toContain('api.getLastfmStatus()');
		expect(source).toContain('api.saveLastfmConfig');
		expect(source).toContain('api.lastfmAuthStart()');
		expect(source).toContain('api.lastfmAuthComplete()');
		expect(source).toContain('api.getListenBrainzStatus()');
		expect(source).toContain('api.saveListenBrainzConfig');
		expect(source).toContain('api.clearListenBrainzConfig');
		expect(client).toContain('/api/lastfm/config');
		expect(client).toContain('/api/listenbrainz/config');
	});

	test('keeps privacy copy and manual backfill visible before provider enablement', () => {
		expect(source).toContain('Opt-in only.');
		expect(source).toContain('avoid duplicates');
		expect(source).toContain('api.backfillScrobbles()');
		expect(source).toContain('Backfill 30 days');
		expect(source).toContain('No provider is connected for scrobbling yet.');
		expect(source).toContain("result.status === 'up_to_date'");
		expect(source).toContain('connectedProviderCount() > 0');
		expect(source).toContain('result.providers ?? 0');
		expect(source).toContain('already queued or submitted');
		expect(client).toContain('/api/scrobbling/backfill');
	});

	test('surfaces upload progress from outbox status', () => {
		expect(source).toContain('Upload status');
		expect(source).toContain('Uploading');
		expect(source).toContain('queued scrobbles in the background');
		expect(source).toContain('Scrobble queue is clear');
		expect(source).toContain('Refresh status');
	});

	test('distinguishes Last.fm credential setup from account auth', () => {
		expect(source).toContain('Needs secret');
		expect(source).toContain('Needs auth');
		expect(source).toContain('status?.scrobbling && status.user');
		expect(source).toContain('API key saved. Add the Last.fm API secret');
		expect(source).toContain('Credentials saved. Start account auth');
		expect(source).toContain('canSaveLastfmConfig');
		expect(source).toContain('if (lastfmApiKey.trim()) return true');
		expect(source).toContain('Save secret');
	});

	test('uses the extracted panel instead of adding more settings page bulk', () => {
		expect(settingsPage).toContain('IntegrationsPanel');
		expect(settingsPage).toContain("activeCategory === 'account'");
		expect(settingsPage).not.toContain("id: 'integrations'");
	});
});
