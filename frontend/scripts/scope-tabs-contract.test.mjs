import { describe, expect, test } from 'vitest';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const path = (rel) => resolve(import.meta.dirname, rel);
const read = (rel) => readFileSync(path(rel), 'utf8');
const tabs = read('../src/lib/components/ui/ScopeTabs.svelte');
const videos = read('../src/routes/videos/+layout.svelte');

// STYLING.md "ScopeTabs": the one control for moving between views of a
// page. Links when each view is its own route, a tablist when the views
// live on one page. Arrow keys move between tabs either way.
describe('ScopeTabs', () => {
	test('route views are links that mark the current page', () => {
		expect(tabs).toContain("aria-current={current === tab.id ? 'page' : undefined}");
		expect(tabs).toContain('data-sveltekit-replacestate={replaceState || undefined}');
	});

	test('in-page views are a tablist with one tab stop', () => {
		expect(tabs).toContain('role="tablist"');
		expect(tabs).toContain('role="tab"');
		expect(tabs).toContain('aria-selected={current === tab.id}');
		expect(tabs).toContain('tabindex={current === tab.id || (current === null && index === 0) ? 0 : -1}');
	});

	test('arrow keys, Home and End move between tabs', () => {
		for (const key of ["'ArrowRight'", "'ArrowLeft'", "'Home'", "'End'"]) expect(tabs).toContain(key);
		expect(tabs).toContain('onkeydown={onKeydown}');
	});

	test('the active tab is the accent fill; counts are tertiary', () => {
		expect(tabs).toMatch(/\.scope-tab\.active\s*\{[^}]*background: var\(--accent\);[^}]*color: var\(--text-on-accent\);/);
		expect(tabs).toMatch(/\.count\s*\{[^}]*color: var\(--text-tertiary\);/);
		expect(tabs).toMatch(/\.scope-tab:hover\s*\{[^}]*background: var\(--bg-hover\);/);
	});
});

describe('Videos uses ScopeTabs for its sections', () => {
	test('the layout mounts ScopeTabs with the section list; the old navigation is gone', () => {
		expect(videos).toContain('<ScopeTabs tabs={VIDEO_TABS} current={tab} label="Video pages" replaceState />');
		expect(existsSync(path('../src/lib/components/video/VideoNavigation.svelte'))).toBe(false);
	});
});
