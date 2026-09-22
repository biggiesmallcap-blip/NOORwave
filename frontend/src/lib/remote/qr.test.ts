import { describe, expect, it } from 'vitest';
import jsQR from 'jsqr';
import { createCanvas, loadImage } from '@napi-rs/canvas';
import { defaultPairingAddress, OperationGeneration, pairingAddressOptions, renderPairingQr } from './qr';
import type { RemoteStatus } from '$lib/api/remote';

const status = (friendly: string | null): RemoteStatus => ({
	server_id: 's', control: 'desktop', configured_host_mode: true, effective_host_mode: true,
	restart_required: false, state: 'running', bind_address: '0.0.0.0:17600', port: 17600,
	discovery: { state: friendly ? 'advertised' : 'unavailable', hostname: friendly ? 'noorwave.local' : null, friendly_url: friendly },
	addresses: [
		{ id: 'wifi', label: 'Wi-Fi', url: 'http://192.168.1.4:17600/remote', kind: 'lan', recommended: true },
		{ id: 'vpn', label: 'VPN', url: 'http://10.0.0.2:17600/remote', kind: 'other', recommended: false },
	], remote_assets_available: true, phone_reachability: 'unverified', ticket: null, diagnostics: []
});

it('rejects stale asynchronous QR completions', () => {
	const operations = new OperationGeneration();
	const first = operations.begin();
	const second = operations.begin();
	expect(operations.isCurrent(first)).toBe(false);
	expect(operations.isCurrent(second)).toBe(true);
	operations.invalidate();
	expect(operations.isCurrent(second)).toBe(false);
});

describe('QR address selection', () => {
	it('prefers confirmed friendly and falls back to recommended direct IP', () => {
		expect(defaultPairingAddress(status('http://noorwave.local:17600/remote'))).toBe('friendly');
		expect(defaultPairingAddress(status(null))).toBe('wifi');
		expect(pairingAddressOptions(status(null))[1].label).toContain('VPN or virtual');
	});

	it('round-trips the generated image through an independent decoder', async () => {
		const payload = 'http://192.168.1.4:17600/remote#pair=independent-secret';
		const dataUrl = await renderPairingQr(payload);
		const image = await loadImage(dataUrl);
		const canvas = createCanvas(image.width, image.height);
		const context = canvas.getContext('2d');
		context.drawImage(image, 0, 0);
		const pixels = context.getImageData(0, 0, image.width, image.height);
		expect(jsQR(new Uint8ClampedArray(pixels.data), image.width, image.height)?.data).toBe(payload);
	});
});
