import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'ListeningServicesConnect.svelte'), 'utf8');
const onboardingPage = readFileSync(join(here, '../../../routes/onboarding/+page.svelte'), 'utf8');

describe('listening services onboarding contract', () => {
	test('offers optional Last.fm and ListenBrainz cards with skip and continue paths', () => {
		expect(onboardingPage).toContain('ListeningServicesConnect');
		expect(source).toContain('Last.fm');
		expect(source).toContain('ListenBrainz');
		expect(source).toContain('onskip');
		expect(source).toContain('oncontinue');
		expect(source).toContain('Skip for now');
	});

	test('does not enable provider submission without explicit credentials', () => {
		expect(source).toContain('api.saveLastfmConfig');
		expect(source).toContain('api.lastfmAuthStart');
		expect(source).toContain('api.saveListenBrainzConfig');
		expect(source).toContain('API key');
		expect(source).toContain('Shared secret');
		expect(source).toContain('User token');
	});

	test('shows duplicate and privacy expectations during setup', () => {
		expect(source).toContain('Optional scrobbling sends eligible plays');
		expect(source).toContain('does not use a shared');
		expect(source).toContain('provider failures never stop local playback');
	});
});
