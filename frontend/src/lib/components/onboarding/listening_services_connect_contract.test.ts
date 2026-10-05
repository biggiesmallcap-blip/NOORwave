import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';
const source = readFileSync(new URL('./ListeningServicesConnect.svelte', import.meta.url), 'utf8');
const page = readFileSync(new URL('../../../routes/onboarding/+page.svelte', import.meta.url), 'utf8');
describe('optional listening service setup', () => {
	test('shares verified connection logic with Settings and retains skip/continue', () => {
		expect(source).toContain('<ListeningServicesPanel guided showHistory={false}');
		expect(source).toContain('onclick={oncontinue}');
		expect(source).toContain('onclick={onskip}');
		expect(page).toContain('ListeningServicesConnect');
	});
});
