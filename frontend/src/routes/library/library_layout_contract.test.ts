import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+page.svelte'), 'utf8').replace(/\r\n/g, '\n');

function countOccurrences(haystack: string, needle: string): number {
	return haystack.split(needle).length - 1;
}

function cssBlock(selector: string): string {
	const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const match = source.match(new RegExp(`${escaped}\\s*\\{(?<body>[^}]*)\\}`));
	if (!match?.groups?.body) {
		throw new Error(`Missing CSS block for ${selector}`);
	}
	return match.groups.body;
}

describe('library layout contracts', () => {
	test('Songs follows the songs-scope setting; Liked is no longer a tab', () => {
		expect(source).toContain("let likedOnly = $derived($librarySongsScope === 'liked');");
		expect(source).not.toContain("switchTab('liked')");
		expect(source).toContain('restoreLibraryTab(saved.activeTab)');
	});

	test('the command header holds the field, the tabs and a toolbar on every tab', () => {
		const header = source.indexOf('<CommandHeader>');
		const tabs = source.indexOf('<ScopeTabs tabs={libraryTabs}');
		const toolbar = source.indexOf('<div class="library-toolbar">');
		expect(header).toBeGreaterThan(-1);
		expect(tabs).toBeGreaterThan(header);
		expect(toolbar).toBeGreaterThan(tabs);
		// The search status replaces the toolbar's start, so the header keeps
		// one height while searching.
		const status = source.indexOf('{#if searchBusy}', toolbar);
		expect(status).toBeGreaterThan(toolbar);
		const row = cssBlock('.library-toolbar');
		expect(row).toContain('justify-content: space-between');
		expect(row).toContain('flex-wrap: wrap');
	});

	test('album_cards_route_artwork_through_shared_fallback_component', () => {
		expect(source).toContain('class="album-art"');
		expect(countOccurrences(source, 'className="album-art-img"')).toBe(2);
		expect(countOccurrences(source, 'src={albumArt}')).toBe(2);
		expect(countOccurrences(source, 'size={320}')).toBeGreaterThanOrEqual(5);
		expect(countOccurrences(source, 'fallbackText={album.title.slice(0, 2).toUpperCase()}')).toBe(2);
		expect(source).toContain(':global(.album-art-img)');
		expect(source).not.toContain('<img src={albumArt}');
		expect(source).not.toContain('class="art-placeholder"');
	});

	test('artist_cards_route_artwork_through_shared_fallback_component', () => {
		expect(source).toContain('function artistImageSources(');
		expect(source).toContain('class="artist-photo"');
		expect(countOccurrences(source, 'className="artist-photo-img"')).toBe(2);
		expect(countOccurrences(source, 'src={artistImageSources(artist.photo_url, artistLazyArt[artist.id], fallbackSrc)}')).toBe(2);
		expect(countOccurrences(source, 'fallbackText={initials(artist.name)}')).toBe(2);
		expect(countOccurrences(source, 'enabled: !artistLazyArt[artist.id] && !fallbackSrc')).toBe(2);
		expect(source).toContain(':global(.artist-photo-img)');
		expect(source).not.toContain('failedArtistImages');
		expect(source).not.toContain('<img src={artistImg}');
		expect(source).not.toContain('class="artist-initial"');
	});
});
