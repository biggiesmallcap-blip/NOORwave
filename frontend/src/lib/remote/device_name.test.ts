import { describe, expect, it } from 'vitest';
import { suggestDeviceName } from './device_name';

const IPHONE_SAFARI = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';
const IPHONE_CHROME = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/129.0 Mobile/15E148 Safari/604.1';
const IPADOS_DESKTOP_UA = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15';
const PIXEL_CHROME = 'Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Mobile Safari/537.36';
const SAMSUNG_TABLET = 'Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/26.0 Chrome/122.0 Safari/537.36';
const WINDOWS_EDGE = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36 Edg/129.0';

describe('suggestDeviceName', () => {
	it('names phones by platform and browser', () => {
		expect(suggestDeviceName({ userAgent: IPHONE_SAFARI })).toBe('iPhone - Safari');
		expect(suggestDeviceName({ userAgent: IPHONE_CHROME })).toBe('iPhone - Chrome');
		expect(suggestDeviceName({ userAgent: PIXEL_CHROME })).toBe('Android phone - Chrome');
		expect(suggestDeviceName({ userAgent: SAMSUNG_TABLET })).toBe('Android tablet - Samsung Internet');
		expect(suggestDeviceName({ userAgent: WINDOWS_EDGE })).toBe('Windows PC - Edge');
	});

	it('marks installed Home Screen apps', () => {
		expect(suggestDeviceName({ userAgent: IPHONE_SAFARI, standalone: true })).toBe('iPhone - Home Screen');
	});

	it('detects iPadOS behind its desktop user agent', () => {
		expect(suggestDeviceName({ userAgent: IPADOS_DESKTOP_UA, maxTouchPoints: 5 })).toBe('iPad - Safari');
		expect(suggestDeviceName({ userAgent: IPADOS_DESKTOP_UA, maxTouchPoints: 0 })).toBe('Mac - Safari');
	});

	it('falls back to the server default for unknown agents and stays within the 64-char limit', () => {
		expect(suggestDeviceName({ userAgent: '' })).toBe('Phone remote');
		expect(suggestDeviceName({ userAgent: SAMSUNG_TABLET, standalone: true }).length).toBeLessThanOrEqual(64);
	});
});
