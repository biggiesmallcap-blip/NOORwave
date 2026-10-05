import { describe, expect, test } from 'vitest';
import { SETTINGS_CATEGORIES, resolveSettingsLocation } from '../src/lib/components/settings/settingsManifest';
describe('settings category navigation', () => {
	test('provides six focused categories', () => {
		expect(SETTINGS_CATEGORIES.map(item => item.id)).toEqual(['appearance', 'playback', 'library', 'services', 'remote', 'app']);
	});
	test.each([
		['?category=audio', 'playback'], ['?category=sources', 'library'], ['?category=account', 'remote'],
		['#sources-tidal', 'services'], ['#integrations-listening', 'services'],
		['?category=audio&setting=discovery-engine', 'library'],
		['?category=app&setting=background-fps', 'appearance'],
		['?category=unknown#%E0%A4%A', 'appearance'],
	])('resolves old and current URLs: %s', (query, category) => {
		expect(resolveSettingsLocation(new URL('http://localhost/settings' + query)).category).toBe(category);
	});
});
