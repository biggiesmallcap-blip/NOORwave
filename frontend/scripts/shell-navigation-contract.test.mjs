import { readFileSync } from 'node:fs';

import { describe, expect, test } from 'vitest';

describe('shell navigation extraction', () => {
	test('keeps desktop sidebar navigation in the shell component', () => {
		const layout = readFileSync('src/routes/+layout.svelte', 'utf8');
		const sidebar = readFileSync('src/lib/shell/SidebarNav.svelte', 'utf8');

		expect(layout).toContain("import SidebarNav from '$lib/shell/SidebarNav.svelte'");
		expect(layout).toContain('<SidebarNav pathname={page.url.pathname} />');
		expect(layout).not.toContain('class="nav-zone"');
		expect(layout).not.toContain('class:special={item.id ===');
		expect(sidebar).toContain("import { NAVIGATION_ZONES } from '$lib/routes/navigation'");
		expect(sidebar).toContain('function isNavItemActive');
		expect(sidebar).toContain('aria-current={isNavItemActive(item.path) ?');
	});
});
