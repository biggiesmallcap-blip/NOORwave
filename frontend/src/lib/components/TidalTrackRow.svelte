<script lang="ts">
	import { goto } from '$app/navigation';
	import type { TidalPlayable } from '$lib/api/client';
	import { buildTidalTrackMenu } from '$lib/player/track_menu';
	import {
		playTidalTrackNow,
		startTidalSongRadio,
		playTrackNow,
		startSongRadio,
	} from '$lib/stores/player';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { formatDuration } from '$lib/stores/library';
	import { api } from '$lib/api/client';

	interface Props {
		track: TidalPlayable;
		index?: number;
		showAlbum?: boolean;
	}
	let { track, index, showAlbum = true }: Props = $props();

	async function resolveAndPlay() {
		// If we already have a real Tidal id, play directly.
		if (track.tidal_id && track.tidal_id > 0) {
			void playTidalTrackNow(track);
			return;
		}
		// Last.fm-only entries: resolve to a Tidal track via search, then play.
		const q = `${track.artist_name ?? ''} ${track.title}`.trim();
		if (!q) return;
		try {
			const r = await api.searchTidal(q);
			const first = r?.tracks[0];
			if (first) {
				if (first.in_library && first.local_id != null) {
					void playTrackNow(first.local_id);
				} else {
					void playTidalTrackNow({
						tidal_id: first.tidal_id,
						title: first.title,
						artist_name: first.artist_name,
						album_title: first.album_title,
						artwork_url: first.artwork_url,
						duration_ms: first.duration_ms,
						artist_tidal_id: first.artist_id ?? null,
					});
				}
			}
		} catch {
			/* ignore */
		}
	}

	function onClick() {
		void resolveAndPlay();
	}

	function onKey(event: KeyboardEvent) {
		if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			void resolveAndPlay();
		}
	}

	function onContextMenu(event: MouseEvent) {
		event.preventDefault();
		// For unresolved (no real tidal_id) entries, build menu from a stub. The
		// menu actions will short-circuit if id is 0; works fine for "Song radio"
		// flows that resolve internally.
		const menu = buildTidalTrackMenu(track);
		openContextMenu(event, menu);
	}

	function onArtistClick(event: MouseEvent) {
		event.stopPropagation();
		if (track.artist_tidal_id != null) goto(`/tidal/artists/${track.artist_tidal_id}`);
	}

	function onRadio(event: MouseEvent) {
		event.stopPropagation();
		if (track.tidal_id && track.tidal_id > 0) {
			void startTidalSongRadio(track);
		} else if (track.track_id != null) {
			void startSongRadio(track.track_id);
		}
	}
</script>

<div
	class="track-row glass-tile"
	role="button"
	tabindex="0"
	onclick={onClick}
	onkeydown={onKey}
	oncontextmenu={onContextMenu}
>
	{#if index != null}
		<span class="track-index">{index + 1}</span>
	{/if}
	{#if track.artwork_url}
		<img class="track-art" src={track.artwork_url} alt="" />
	{:else}
		<div class="track-art placeholder">♫</div>
	{/if}
	<div class="track-meta">
		<p class="track-title">{track.title}</p>
		<span class="track-sub">
			{#if track.artist_tidal_id != null}
				<button type="button" class="link" onclick={onArtistClick}
					>{track.artist_name ?? 'Unknown artist'}</button
				>
			{:else}
				{track.artist_name ?? 'Unknown artist'}
			{/if}
			{#if showAlbum && track.album_title}
				<span class="dot">·</span>
				<span>{track.album_title}</span>
			{/if}
		</span>
	</div>
	<div class="track-actions">
		<button
			type="button"
			class="action-btn"
			title="Song radio"
			onclick={onRadio}
			aria-label="Song radio">◎</button
		>
		{#if track.duration_ms}
			<span class="track-duration">{formatDuration(track.duration_ms)}</span>
		{/if}
	</div>
</div>

<style>
	.track-row {
		display: grid;
		grid-template-columns: auto 40px 1fr auto;
		align-items: center;
		gap: 0.75rem;
		padding: 0.5rem 0.75rem;
		border-radius: 0.5rem;
		cursor: pointer;
		transition: background 0.15s ease;
	}
	.track-row:hover,
	.track-row:focus-visible {
		background: rgba(255, 255, 255, 0.05);
		outline: none;
	}
	.track-index {
		min-width: 1.5rem;
		text-align: right;
		color: var(--text-muted, #888);
		font-variant-numeric: tabular-nums;
		font-size: 0.875rem;
	}
	.track-art {
		width: 40px;
		height: 40px;
		border-radius: 0.25rem;
		object-fit: cover;
	}
	.track-art.placeholder {
		display: flex;
		align-items: center;
		justify-content: center;
		background: rgba(255, 255, 255, 0.06);
		font-size: 1.25rem;
	}
	.track-meta {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 0.125rem;
	}
	.track-title {
		margin: 0;
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.track-sub {
		font-size: 0.875rem;
		color: var(--text-muted, #888);
		display: flex;
		gap: 0.375rem;
		align-items: center;
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.link {
		background: none;
		border: none;
		padding: 0;
		color: inherit;
		cursor: pointer;
		font: inherit;
	}
	.link:hover {
		color: var(--text, #fff);
		text-decoration: underline;
	}
	.dot {
		opacity: 0.6;
	}
	.track-actions {
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}
	.action-btn {
		opacity: 0;
		background: none;
		border: none;
		color: inherit;
		cursor: pointer;
		font-size: 1rem;
		padding: 0.25rem 0.5rem;
		border-radius: 0.25rem;
		transition: background 0.15s ease;
	}
	.action-btn:hover {
		background: rgba(255, 255, 255, 0.08);
	}
	.track-row:hover .action-btn,
	.track-row:focus-visible .action-btn {
		opacity: 1;
	}
	.track-duration {
		font-variant-numeric: tabular-nums;
		font-size: 0.875rem;
		color: var(--text-muted, #888);
	}
</style>
