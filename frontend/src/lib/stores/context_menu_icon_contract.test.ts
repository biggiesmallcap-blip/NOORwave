import { readdirSync, readFileSync } from 'node:fs';
import { extname, join, relative } from 'node:path';
import { describe, expect, test } from 'vitest';
import { menuIconForDisplay } from './context_menu';

const sourceRoot = join(process.cwd(), 'src');

function sourceFiles(dir: string): string[] {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) return sourceFiles(path);
		if (entry.name.endsWith('.test.ts')) return [];
		return ['.svelte', '.ts'].includes(extname(entry.name)) ? [path] : [];
	});
}

describe('context menu icon contracts', () => {
	test('does not render multi-letter words as icons', () => {
		expect(menuIconForDisplay('Play')).toBeUndefined();
		expect(menuIconForDisplay('Song radio')).toBeUndefined();
		expect(menuIconForDisplay('Open')).toBeUndefined();
		expect(menuIconForDisplay('▶')).toBe('▶');
		expect(menuIconForDisplay('⤴')).toBe('⤴');
		expect(menuIconForDisplay('P')).toBe('P');
		expect(menuIconForDisplay('+')).toBe('+');
		expect(menuIconForDisplay(undefined)).toBeUndefined();
	});

	test('TIDAL playlist actions use a glyph icon', () => {
		const source = readFileSync(join(sourceRoot, 'lib/components/search/TidalDiscoverShelves.svelte'), 'utf8');

		expect(source).toContain("label: 'Play playlist'");
		expect(source).toContain("icon: '▶'");
		expect(source).not.toContain("icon: 'Play'");
	});

	test('source is free of UTF-8-as-Latin1 mojibake', () => {
		// A heart icon once shipped as 'â™¡' because the file was saved with the
		// UTF-8 bytes of ♡ reinterpreted as Latin-1. The lead byte 0xE2 ('â')
		// heads ♥ ♡ ™ … and smart quotes/dashes, so a stray 'â' in source is a
		// reliable mojibake tell. Intentional glyphs (♥ ▶ ⤴ → ◉ …) never contain it.
		const mojibake = /[âÃÂ]/;
		const offenders = sourceFiles(sourceRoot).flatMap((path) => {
			const source = readFileSync(path, 'utf8');
			return mojibake.test(source) ? [relative(sourceRoot, path)] : [];
		});

		expect(offenders).toEqual([]);
	});

	test('source does not assign visible words as context-menu icons', () => {
		const textIconPattern = /icon:\s*['"][A-Za-z][A-Za-z ]+['"]/g;
		const offenders = sourceFiles(sourceRoot).flatMap((path) => {
			const source = readFileSync(path, 'utf8');
			return [...source.matchAll(textIconPattern)].map((match) =>
				`${relative(sourceRoot, path)}: ${match[0]}`,
			);
		});

		expect(offenders).toEqual([]);
	});
});
