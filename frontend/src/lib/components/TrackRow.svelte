<script lang="ts">
	import { goto } from '$app/navigation';
	import type { Track } from '$lib/api/client';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { playTrackNow, startSongRadio } from '$lib/stores/player';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { formatDuration } from '$lib/stores/library';

	interface Props {
		track: Track;
		index?: number;
		showAlbum?: boolean;
	}
	let { track, index, showAlbum = true }: Props = $props();

	function onClick() {
		void playTrackNow(track.id);
	}

	function onKey(event: KeyboardEvent) {
		if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			void playTrackNow(track.id);
		}
	}

	function onContextMenu(event: MouseEvent) {
		event.preventDefault();
		const menu = buildTrackMenu({
			id: track.id,
			title: track.title,
			artist_id: track.artist_id,
			artist_name: track.artist_name,
			album_id: track.album_id,
			album_title: track.album_title,
			is_favorite: track.is_favorite
		});
		openContextMenu(event, menu);
	}

	function onArtistClick(event: MouseEvent) {
		event.stopPropagation();
		if (track.artist_id != null) goto(`/artists/${track.artist_id}`);
	}

	function onAlbumClick(event: MouseEvent) {
		event.stopPropagation();
		if (track.album_id != null) goto(`/albums/${track.album_id}`);
	}

	function onRadio(event: MouseEvent) {
		event.stopPropagation();
		void startSongRadio(track.id);
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
			{#if track.artist_id != null}
				<button type="button" class="link" onclick={onArtistClick}
					>{track.artist_name ?? 'Unknown artist'}</button
				>
			{:else}
				{track.artist_name ?? 'Unknown artist'}
			{/if}
			{#if showAlbum && track.album_title}
				<span class="dot">·</span>
				{#if track.album_id != null}
					<button type="button" class="link" onclick={onAlbumClick}>{track.album_title}</button>
				{:else}
					<span>{track.album_title}</span>
				{/if}
			{/if}
		</span>
	</div>
	<div class="track-actions">
		<button type="button" class="action-btn" title="Song radio" onclick={onRadio} aria-label="Song radio">◎</button>
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
