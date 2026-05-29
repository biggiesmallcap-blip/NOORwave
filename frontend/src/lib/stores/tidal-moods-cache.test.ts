import { describe, expect, test, beforeEach } from 'vitest';
import {
	clearCachedMoods,
	getCachedMoodCategories,
	moodCategoriesNeedThumbnails,
	putCompleteMoodCategories,
} from './tidal-moods-cache';
import type { TidalMoodCategory } from '$lib/api/client';

const completeCategories: TidalMoodCategory[] = [
	{
		slug: 'mood_party',
		title: 'Party',
		icon: null,
		imageId: null,
		thumbnail: 'https://resources.tidal.com/images/party/320x320.jpg',
	},
];

const incompleteCategories: TidalMoodCategory[] = [
	{
		slug: 'mood_party',
		title: 'Party',
		icon: null,
		imageId: null,
		thumbnail: null,
	},
];

describe('TIDAL moods cache', () => {
	beforeEach(() => {
		clearCachedMoods();
	});

	test('treats mood lists without thumbnails as provisional', () => {
		expect(moodCategoriesNeedThumbnails(incompleteCategories)).toBe(true);
		expect(moodCategoriesNeedThumbnails(completeCategories)).toBe(false);
	});

	test('does not cache incomplete mood thumbnail probes for six hours', () => {
		putCompleteMoodCategories(incompleteCategories);

		expect(getCachedMoodCategories()).toBeNull();

		putCompleteMoodCategories(completeCategories);

		expect(getCachedMoodCategories()).toEqual(completeCategories);
	});
});
