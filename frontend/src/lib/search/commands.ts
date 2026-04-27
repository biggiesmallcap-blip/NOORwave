import { goto } from '$app/navigation';
import { get } from 'svelte/store';
import { currentTrack } from '$lib/stores/player';
import { api } from '$lib/api/client';

export interface SlashCommand {
	prefix: string;         // e.g. 'play', 'queue', 'radio', 'jump'
	description: string;    // shown in suggestions
	args?: string;          // optional hint, e.g. '<query>' or 'on|off'
	// execute receives the rest of the input after the command word (trimmed)
	execute: (arg: string) => Promise<void> | void;
}

export const SLASH_COMMANDS: SlashCommand[] = [
	{
		prefix: 'play',
		description: 'Play first search result',
		args: '<query>',
		execute: async (arg) => {
			if (!arg) return;
			try {
				const res = await api.searchTidal(arg);
				const first = res.tracks[0];
				if (first) {
					const { playTidalTrackNow } = await import('$lib/stores/player');
					await playTidalTrackNow({ ...first, artist_tidal_id: first.artist_id ?? null });
				}
			} catch { /* silent */ }
		},
	},
	{
		prefix: 'queue',
		description: 'Add first result to queue',
		args: '<query>',
		execute: async (arg) => {
			if (!arg) return;
			try {
				const res = await api.searchTidal(arg);
				const first = res.tracks[0];
				if (first) {
					const { addTidalTrackToQueue } = await import('$lib/stores/player');
					await addTidalTrackToQueue({ ...first, artist_tidal_id: first.artist_id ?? null });
				}
			} catch { /* silent */ }
		},
	},
	{
		prefix: 'radio',
		description: 'Start radio from current or query',
		args: '[query]',
		execute: async (arg) => {
			if (arg) {
				try {
					const res = await api.searchTidal(arg);
					const first = res.tracks[0];
					if (first) {
						const { startTidalSongRadio } = await import('$lib/stores/player');
						await startTidalSongRadio({ ...first, artist_tidal_id: first.artist_id ?? null });
					}
				} catch { /* silent */ }
			} else {
				const track = get(currentTrack);
				if (track && track.id > 0) {
					const { startSongRadio } = await import('$lib/stores/player');
					await startSongRadio(track.id);
				} else if (track?.tidal_id) {
					const { startTidalSongRadio } = await import('$lib/stores/player');
					await startTidalSongRadio({
						tidal_id: track.tidal_id,
						title: track.title,
						artist_name: track.artist_name,
						album_title: track.album_title,
						artwork_url: track.artwork_url,
						duration_ms: track.duration_ms,
					});
				}
			}
		},
	},
	{
		prefix: 'jump',
		description: 'Navigate to a page',
		args: 'library|genres|playlists|discover|settings|search',
		execute: (arg) => { if (arg) goto(`/${arg}`); },
	},
	{
		prefix: 'automix',
		description: 'Toggle automix',
		args: 'on|off',
		execute: async (arg) => {
			const enabled = arg === 'on';
			const { setPlayerAutomixEnabled } = await import('$lib/stores/player');
			await setPlayerAutomixEnabled(enabled);
		},
	},
];

export function matchCommands(input: string): SlashCommand[] {
	if (!input.startsWith('/')) return [];
	const typed = input.slice(1).toLowerCase();
	return SLASH_COMMANDS.filter(
		(c) => c.prefix.startsWith(typed) || (typed.includes(' ') && c.prefix === typed.split(' ')[0])
	);
}

export function parseSlashInput(input: string): { command: SlashCommand | null; arg: string } {
	const parts = input.slice(1).split(/\s+/);
	const prefix = parts[0]?.toLowerCase() ?? '';
	const arg = parts.slice(1).join(' ').trim();
	const command = SLASH_COMMANDS.find((c) => c.prefix === prefix) ?? null;
	return { command, arg };
}
