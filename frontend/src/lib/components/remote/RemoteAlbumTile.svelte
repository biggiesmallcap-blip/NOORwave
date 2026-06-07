<script lang="ts">
	import { goto } from '$app/navigation';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
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
		/** Subtitle shown in the action-sheet header, defaults to year/releaseType. */
		menuArtistName?: string | null;
	} = $props();

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
		<ArtworkImage
			className="remote-album-tile-artwork"
			src={artworkUrl}
			size={320}
			fallbackText="NOOR"
			decorative={true}
		/>
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

	.remote-album-tile-art :global(.remote-album-tile-artwork) {
		width: 100%;
		height: 100%;
	}

	.remote-album-tile-art :global(.remote-album-tile-artwork:not(.fallback)) {
		object-fit: cover;
		display: block;
	}

	.remote-album-tile-art :global(.remote-album-tile-artwork.fallback) {
		display: grid;
		place-items: center;
		color: var(--text-muted);
		background: var(--surface-1);
	}

	.remote-album-tile-art :global(.remote-album-tile-artwork.fallback span) {
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
