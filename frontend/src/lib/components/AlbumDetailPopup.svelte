<script lang="ts">
	import type { Album, Track } from '$lib/api/client';
	import { currentTrack, playTrackNow, playAlbum, shuffleAlbum } from '$lib/stores/player';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { formatTrackDuration } from '$lib/utils/format';

	let { album, tracks, loading, onClose }: {
		album: Album;
		tracks: Track[];
		loading: boolean;
		onClose: () => void;
	} = $props();

	function handleKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			onClose();
		}
	}

	function trackMenu(track: Track) {
		return buildTrackMenu(track);
	}

	function openAlbumContextMenu(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildAlbumMenu(album, { isLocal: true }), album.title);
	}

	function openAlbumArtistContextMenu(event: MouseEvent) {
		if (!album.artist_name) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildArtistMenu({ id: album.artist_id, name: album.artist_name, in_library: true }, { isLocal: true }),
			album.artist_name
		);
	}

	function openTrackArtistContextMenu(event: MouseEvent, track: Track) {
		if (!track.artist_name || !track.artist_id) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildArtistMenu({ id: track.artist_id, name: track.artist_name, in_library: true }, { isLocal: true }),
			track.artist_name
		);
	}
</script>

<svelte:window onkeydown={handleKey} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="popup-backdrop" role="presentation" onclick={onClose}>
	<div
		class="popup-panel"
		role="dialog"
		tabindex="-1"
		aria-modal="true"
		aria-label={album.title}
		onclick={(e) => e.stopPropagation()}
	>
		<div class="popup-topbar">
			<button class="popup-close" aria-label="Close" onclick={onClose}>✕</button>
		</div>

		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="popup-hero" oncontextmenu={openAlbumContextMenu}>
			{#if album.artwork_url}
				<img class="popup-art" src={album.artwork_url} alt={album.title} />
			{:else}
				<div class="popup-art placeholder">♫</div>
			{/if}
			<div class="popup-info">
				<h2>{album.title}</h2>
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<p
					class="popup-artist"
					oncontextmenu={openAlbumArtistContextMenu}
				>{album.artist_name ?? 'Unknown Artist'}</p>
				<div class="popup-meta-row">
					{#if album.year}<span class="popup-chip">{album.year}</span>{/if}
					{#if album.release_type}<span class="popup-chip">{album.release_type}</span>{/if}
					{#if album.track_count}<span class="popup-chip">{album.track_count} tracks</span>{/if}
					<span class="popup-chip">{album.source}</span>
				</div>
				<div class="popup-actions">
					<button class="btn btn-primary" onclick={() => void playAlbum(album.id)}>▶ Play All</button>
					<button class="btn btn-glass" onclick={() => void shuffleAlbum(album.id)}>⤮ Shuffle</button>
				</div>
			</div>
		</div>

		{#if loading}
			<div class="popup-loading"><div class="spinner spinner-sm"></div><span>Loading tracks…</span></div>
		{:else if tracks.length === 0}
			<div class="popup-empty">No tracks synced yet.</div>
		{:else}
			<div class="popup-track-list">
				{#each tracks as track, i (track.id)}
					<div
						class="popup-track-row"
						class:playing={$currentTrack?.id === track.id}
						role="button"
						tabindex="0"
						onclick={() => void playTrackNow(track.id)}
						onkeydown={(e) => e.key === 'Enter' && void playTrackNow(track.id)}
						oncontextmenu={(e) => {
							e.preventDefault();
							e.stopPropagation();
							openContextMenu(e, trackMenu(track), track.title);
						}}
					>
						<span class="popup-track-num">{i + 1}</span>
						<span class="popup-track-title">{track.title}</span>
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<span
							class="popup-track-artist"
							oncontextmenu={(e) => openTrackArtistContextMenu(e, track)}
						>{track.artist_name ?? ''}</span>
						<span class="popup-track-duration">{formatTrackDuration(track.duration_ms)}</span>
						<button
							class="popup-track-menu"
							aria-label="Track actions"
							onclick={(e) => {
								e.preventDefault();
								e.stopPropagation();
								openMenuAtElement(e.currentTarget, trackMenu(track), track.title);
							}}
						>⋯</button>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>

<style>
	.popup-backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.55);
		backdrop-filter: blur(3px) brightness(0.72);
		-webkit-backdrop-filter: blur(3px) brightness(0.72);
		z-index: 80;
		display: flex;
		align-items: center;
		justify-content: center;
		animation: backdrop-fade 180ms ease-out both;
	}

	@keyframes backdrop-fade {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.popup-panel {
		width: min(880px, 92vw);
		max-height: 88vh;
		display: flex;
		flex-direction: column;
		gap: 16px;
		padding: 22px;
		border-radius: var(--radius-lg, 18px);
		border: 1px solid rgba(255, 255, 255, 0.09);
		background: linear-gradient(160deg,
			rgba(20, 20, 32, 0.72) 0%,
			rgba(12, 12, 20, 0.68) 100%);
		backdrop-filter: var(--blur-modal);
		-webkit-backdrop-filter: var(--blur-modal);
		box-shadow:
			0 32px 64px -16px rgba(0, 0, 0, 0.72),
			inset 0 1px 0 rgba(255, 255, 255, 0.07);
		animation: popup-bloom 240ms cubic-bezier(0.22, 1, 0.36, 1) both;
		overflow: hidden;
	}

	@keyframes popup-bloom {
		from {
			opacity: 0;
			transform: scale(0.97) translateY(8px);
			filter: blur(4px);
		}
		to {
			opacity: 1;
			transform: scale(1) translateY(0);
			filter: blur(0);
		}
	}

	.popup-topbar {
		display: flex;
		justify-content: flex-end;
		margin: -8px -8px 0 0;
	}

	.popup-close {
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.09);
		border-radius: 999px;
		width: 32px;
		height: 32px;
		color: var(--text-secondary, rgba(255,255,255,0.7));
		cursor: pointer;
		font-size: var(--font-size-md);
		transition: background 120ms ease, color 120ms ease;
	}
	.popup-close:hover {
		background: rgba(255, 255, 255, 0.13);
		color: var(--text-primary, #fff);
	}

	.popup-hero {
		display: grid;
		grid-template-columns: 160px 1fr;
		gap: 20px;
		align-items: start;
	}

	.popup-art {
		width: 160px;
		height: 160px;
		border-radius: 12px;
		object-fit: cover;
		box-shadow: 0 12px 32px -8px rgba(0,0,0,0.55);
	}
	.popup-art.placeholder {
		display: grid;
		place-items: center;
		font-size: var(--font-size-4xl);
		color: rgba(255,255,255,0.35);
		background: rgba(255,255,255,0.04);
	}

	.popup-info {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-width: 0;
	}

	.popup-info h2 {
		margin: 0;
		font-size: var(--font-size-xl);
		line-height: var(--line-height-snug);
		color: var(--text-primary, #fff);
	}

	.popup-artist {
		margin: 0;
		color: var(--text-secondary, rgba(255,255,255,0.7));
		font-size: var(--font-size-md);
	}

	.popup-meta-row {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 4px;
	}

	.popup-chip {
		padding: 2px 9px;
		border-radius: 999px;
		background: var(--bg-hover);
		border: 1px solid var(--panel-border);
		color: var(--text-secondary, rgba(255,255,255,0.7));
		font-size: var(--font-size-xs);
	}

	.popup-actions {
		display: flex;
		gap: 8px;
		margin-top: 8px;
	}

	.popup-track-list {
		max-height: 400px;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding-right: 2px;
		scrollbar-width: thin;
		scrollbar-color: var(--scrollbar-thumb, rgba(255,255,255,0.18)) transparent;
	}

	.popup-track-list::-webkit-scrollbar {
		width: 5px;
	}
	.popup-track-list::-webkit-scrollbar-track {
		background: transparent;
	}
	.popup-track-list::-webkit-scrollbar-thumb {
		background: var(--scrollbar-thumb, rgba(255,255,255,0.18));
		border-radius: 99px;
	}
	.popup-track-list::-webkit-scrollbar-thumb:hover {
		background: var(--scrollbar-thumb-hover, rgba(255,255,255,0.28));
	}

	.popup-track-row {
		display: grid;
		grid-template-columns: 28px 1fr 1fr 60px 32px;
		gap: 12px;
		align-items: center;
		padding: 6px 10px;
		border-radius: 8px;
		cursor: pointer;
		transition: background 120ms ease;
		min-width: 0;
	}
	.popup-track-row:hover {
		background: rgba(255,255,255,0.05);
	}
	.popup-track-row.playing {
		background: rgba(125, 99, 255, 0.10);
	}

	.popup-track-num {
		color: var(--text-tertiary, rgba(255,255,255,0.45));
		font-variant-numeric: tabular-nums;
		font-size: var(--font-size-sm);
	}

	.popup-track-title {
		color: var(--text-primary, #fff);
		font-size: var(--font-size-sm);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.popup-track-artist {
		color: var(--text-secondary, rgba(255,255,255,0.7));
		font-size: var(--font-size-sm);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.popup-track-duration {
		color: var(--text-tertiary, rgba(255,255,255,0.45));
		font-variant-numeric: tabular-nums;
		font-size: var(--font-size-sm);
		text-align: right;
	}

	.popup-track-menu {
		padding: 3px 10px;
		border-radius: 999px;
		background: rgba(255,255,255,0.06);
		border: 1px solid rgba(255,255,255,0.09);
		color: var(--text-secondary, rgba(255,255,255,0.7));
		font-size: var(--font-size-sm);
		cursor: pointer;
		transition: background 120ms ease, border-color 120ms ease, color 120ms ease;
	}
	.popup-track-menu:hover {
		background: rgba(255,255,255,0.11);
		border-color: rgba(255,255,255,0.16);
		color: var(--text-primary, #fff);
	}

	.popup-loading,
	.popup-empty {
		display: flex;
		align-items: center;
		gap: 10px;
		justify-content: center;
		padding: 24px;
		color: var(--text-secondary, rgba(255,255,255,0.7));
	}
</style>
