import QRCode from 'qrcode';
import type { RemoteAddress, RemoteStatus } from '$lib/api/remote';

export interface PairingAddressOption { id: string; label: string; url: string; friendly: boolean; recommended: boolean }

export function pairingAddressOptions(status: RemoteStatus): PairingAddressOption[] {
	const friendly = status.discovery.friendly_url
		? [{ id: 'friendly', label: `${status.discovery.hostname ?? 'Friendly address'} (.local)`, url: status.discovery.friendly_url, friendly: true, recommended: true }]
		: [];
	return [...friendly, ...status.addresses.map((address: RemoteAddress) => ({
		id: address.id, label: `${address.label}${address.kind === 'other' ? ' (VPN or virtual)' : ''}`,
		url: address.url, friendly: false, recommended: address.recommended && !friendly.length,
	}))];
}

export function defaultPairingAddress(status: RemoteStatus): string | undefined {
	const options = pairingAddressOptions(status);
	return options.find((option) => option.recommended)?.id ?? options[0]?.id;
}

export function renderPairingQr(url: string): Promise<string> {
	return QRCode.toDataURL(url, { errorCorrectionLevel: 'M', margin: 4, width: 320, type: 'image/png' });
}

export class OperationGeneration {
	private current = 0;
	begin(): number { return ++this.current; }
	isCurrent(generation: number): boolean { return generation === this.current; }
	invalidate(): void { this.current += 1; }
}
