import { describe, expect, test, beforeEach, afterEach, vi } from 'vitest';
import {
	claimMoodThumbnailRefresh,
	clearCachedMoods,
	getCachedMoodCategories,
	moodCategoriesNeedThumbnails,
	putCachedMoodCategories,
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
		vi.useRealTimers();
		clearCachedMoods();
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	test('treats mood lists without thumbnails as provisional', () => {
		expect(moodCategoriesNeedThumbnails(incompleteCategories)).toBe(true);
		expect(moodCategoriesNeedThumbnails(completeCategories)).toBe(false);
	});

	test('caches incomplete mood categories so revisits render immediately', () => {
		putCachedMoodCategories(incompleteCategories);

		expect(getCachedMoodCategories()).toEqual(incompleteCategories);

		putCachedMoodCategories(completeCategories);

		expect(getCachedMoodCategories()).toEqual(completeCategories);
	});

	test('throttles thumbnail refresh claims across mood surfaces', () => {
		vi.useFakeTimers();
		vi.setSystemTime(1_000);

		expect(claimMoodThumbnailRefresh(incompleteCategories)).toBe(true);
		expect(claimMoodThumbnailRefresh(incompleteCategories)).toBe(false);

		vi.setSystemTime(62_000);
		expect(claimMoodThumbnailRefresh(incompleteCategories)).toBe(true);
		expect(claimMoodThumbnailRefresh(completeCategories)).toBe(false);
	});
});
