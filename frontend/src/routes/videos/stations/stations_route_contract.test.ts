import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const page = readFileSync(join(here, '+page.svelte'), 'utf8');
const row = readFileSync(join(here, '../../../lib/components/video/StationRow.svelte'), 'utf8');
const spotlight = readFileSync(join(here, '../../../lib/components/video/StationSpotlight.svelte'), 'utf8');
const guideRow = readFileSync(join(here, '../../../lib/components/video/GuideRow.svelte'), 'utf8');
const guideFeature = readFileSync(join(here, '../../../lib/components/video/GuideFeature.svelte'), 'utf8');

describe('stations channel guide contract', () => {
	test('renders numbered rows and the spotlight, not rails of cards', () => {
		expect(page).toContain('<StationSpotlight');
		expect(page).toContain('<StationRow');
		expect(page).toContain('numberStations(grouped.spotlight, grouped.rows)');
		expect(page).not.toContain('MediaRail');
		expect(page).not.toContain('StationCard.svelte');
	});

	test('a frame starts its station from that video', () => {
		expect(page).toContain('playVideoStation({ id: card.id, title: card.title }, { startWith })');
		expect(row).toContain('onplay={(startWith) => onplay(card, startWith)}');
		expect(spotlight).toContain('onpick={(video) => onplay(card, video)}');
		expect(guideRow).toContain('onclick={() => onplay(video)}');
		expect(guideFeature).toContain('onclick={() => onpick(video)}');
		for (const source of [guideRow, guideFeature]) expect(source).toContain('buildVideoMenu(video)');
	});

	test('marks the station on air from the session', () => {
		expect(page).toContain('$videoSession.continuous && $videoStationOnAir === card.id');
	});

	test('the spotlight bio is cleaned and never blocks the row', () => {
		expect(spotlight).toContain('api.getTidalArtistProfile(id, true)');
		expect(spotlight).toContain('cleanBio(profile.bio?.summary) ?? cleanBio(profile.bio?.text)');
		expect(spotlight).toContain('.catch(() => {})');
	});
});
