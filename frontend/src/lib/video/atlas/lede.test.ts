import { describe, expect, test } from 'vitest';
import { KNOWN_LINE, genreWords, hoursWords } from './lede';

describe('artist card lede', () => {
	test('hours read as words', () => {
		expect(hoursWords(0)).toBe('');
		expect(hoursWords(0.01)).toBe('1 minute');
		expect(hoursWords(0.66)).toBe('40 minutes');
		expect(hoursWords(1)).toBe('1 hour');
		expect(hoursWords(1.84)).toBe('1.8 hours');
		expect(hoursWords(12.4)).toBe('12 hours');
	});

	test('genres join like a sentence, three at most', () => {
		expect(genreWords([])).toBe('');
		expect(genreWords(['house'])).toBe('house');
		expect(genreWords(['house', 'pop'])).toBe('house and pop');
		expect(genreWords(['house', 'pop', 'electronic', 'r&b'])).toBe('house, pop and electronic');
	});

	test('knowing an artist is said, not coloured', () => {
		expect(KNOWN_LINE[3]).toBe('You have watched, liked or saved them.');
		expect(KNOWN_LINE[0]).toBe('');
	});
});
