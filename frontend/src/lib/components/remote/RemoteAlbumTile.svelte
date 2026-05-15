<script lang="ts">
	import { goto } from '$app/navigation';
	import { upscaleTidalArtwork } from '$lib/utils/artwork';
	import { hapticTap } from '$lib/remote/haptics';
	import { longPress } from '$lib/remote/long_press';
	import { openActionSheet } from '$lib/remote/action_sheet';
	import { buildAlbumMenu, type AlbumLike } from '$lib/player/album_menu';

	let {
		title,
		artworkUrl = null,
		year = null,
		releaseType = null,
		href,
		albumForMenu = null,
		menuArtistName = null
	}: {
		title: string;
		artworkUrl?: string | null;
		year?: number | string | null;
		releaseType?: string | null;
		href: string;
		/** When set, long-press opens the shared album action sheet. */
		albumForMenu?: AlbumLike | null;
		/** Subtitle shown in the action-sheet header — defaults to year/releaseType. */
		menuArtistName?: string | null;
	} = $props();

	let cover = $derived(upscaleTidalArtwork(artworkUrl, 320));
	// Some TIDAL records return AccessDenied for specific sizes on specific
	// covers — we can't predict which, so swap to the placeholder on first
	// load failure rather than leaving a broken-image glyph on screen.
	let failed = $state(false);
	$effect(() => {
		cover;
		failed = false;
	});
	let meta = $derived.by(() => {
		const parts: string[] = [];
		if (year) parts.push(String(year));
		if (releaseType && releaseType.toLowerCase() !== 'album') parts.push(releaseType);
		return parts.join(' · ');
	});

	function onClick() {
		hapticTap();
		void goto(href);
	}

	function onLongPress() {
		if (!albumForMenu) return;
		openActionSheet({
			title,
			subtitle: menuArtistName ?? meta,
			items: buildAlbumMenu(albumForMenu, { remoteRoutes: true })
		});
	}
</script>

<button
	type="button"
	class="remote-album-tile"
	aria-label="Open {title}"
	onclick={onClick}
	use:longPress={onLongPress}
>
	<span class="remote-album-tile-art">
		{#if cover && !failed}
			<img src={cover} alt="" onerror={() => (failed = true)} />
		{:else}
			<span class="remote-album-tile-empty" aria-hidden="true">NOOR</span>
		{/if}
	</span>
	<span class="remote-album-tile-copy">
		<strong>{title}</strong>
		{#if meta}
			<small>{meta}</small>
		{/if}
	</span>
</button>

<style>
	.remote-album-tile {
		flex: 0 0 auto;
		width: 132px;
		display: grid;
		gap: 6px;
		padding: 4px;
		background: transparent;
		color: var(--text-primary);
		text-align: left;
		border-radius: 12px;
	}

	.remote-album-tile:active {
		background: var(--surface-1);
	}

	.remote-album-tile-art {
		display: block;
		width: 124px;
		height: 124px;
		border-radius: 10px;
		overflow: hidden;
		background: var(--surface-1);
	}

	.remote-album-tile-art img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.remote-album-tile-empty {
		display: grid;
		place-items: center;
		width: 100%;
		height: 100%;
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-album-tile-copy {
		min-width: 0;
		display: grid;
		gap: 1px;
	}

	.remote-album-tile-copy strong,
	.remote-album-tile-copy small {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-album-tile-copy strong {
		font-size: var(--font-size-sm);
	}

	.remote-album-tile-copy small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}
</style>
