<script lang="ts">
	// The album quick view (STYLING.md "Album quick view and album page"). It is
	// the album page in a smaller container: the same DetailHero and ActionBar,
	// and the same rows (TrackRow for owned tracks, TidalTrackRow for the rest),
	// plus a link to the full page.
	import type { Album, TidalPlayable, Track } from '$lib/api/client';
	import { currentTrack, isPlaying, playAlbum, shuffleAlbum, togglePlayback } from '$lib/stores/player';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { formatTotalDuration } from '$lib/utils/format';
	import QuickViewPopup from '$lib/components/ui/QuickViewPopup.svelte';
	import DetailHero from '$lib/components/ui/DetailHero.svelte';
	import ActionBar from '$lib/components/ui/ActionBar.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import TidalTrackRow from '$lib/components/TidalTrackRow.svelte';

	let { album, tracks, loading, onClose, isLocal = true, onPlay, onShuffle, onPlayFrom, artistHref = null, albumHref = null }: {
		album: Album;
		tracks: Track[];
		loading: boolean;
		onClose: () => void;
		/**
		 * False when this album is not in the library, so `album.id` is not a
		 * real local id. Library always owns its albums and leaves this alone.
		 *
		 * Recommendations do not: a Last.fm album is usually TIDAL-only, and every
		 * default action here is keyed on a local id - `playAlbum(album.id)`,
		 * `shuffleAlbum(album.id)`, and the menu builders' `isLocal: true`. Passing
		 * false requires the caller to supply the play handlers, rather than
		 * silently calling playAlbum with an id that does not exist.
		 */
		isLocal?: boolean;
		onPlay?: () => void;
		onShuffle?: () => void;
		/** Play the album starting from this track. */
		onPlayFrom?: (track: Track) => void;
		/**
		 * Where the artist name goes. Only needed when the artist is not a library
		 * row: a TIDAL-only album has no `artist_id`, so the local href cannot be
		 * derived and the caller passes `/tidal/artists/{id}` instead.
		 */
		artistHref?: string | null;
		/**
		 * The full album page. Derived for library albums; a TIDAL-only album
		 * passes `/tidal/albums/{id}`. Null hides the link.
		 */
		albumHref?: string | null;
	} = $props();

	let resolvedArtistHref = $derived(
		artistHref ?? (isLocal && album.artist_id ? `/artists/${album.artist_id}` : null),
	);
	let resolvedAlbumHref = $derived(albumHref ?? (isLocal && album.id != null ? `/albums/${album.id}` : null));

	let songCount = $derived(loading ? (album.track_count ?? 0) : tracks.length);
	let totalMs = $derived(tracks.reduce((sum, track) => sum + (track.duration_ms ?? 0), 0));

	// Unowned TIDAL rows carry a negative placeholder id (album_detail.ts), so
	// "is this album playing" has to match on the TIDAL id as well.
	function isCurrent(track: Track) {
		const current = $currentTrack;
		if (!current) return false;
		return current.id === track.id || (track.tidal_id != null && current.tidal_id === track.tidal_id);
	}
	let playingFromHere = $derived(tracks.some(isCurrent));
	let albumPlaying = $derived($isPlaying && playingFromHere);

	// Clicking a track plays the album from that track, matching the album page.
	// playAlbum (not playTracksInContext over the popup's owned rows) so a
	// partially-owned album queues its TIDAL-only remainder too instead of a
	// short owned-only queue that automix pads with unrelated tracks.
	function playFromHere(track: Track) {
		if (onPlayFrom) return onPlayFrom(track);
		void playAlbum(album.id, track.id);
	}

	// Play pauses and resumes once this album is what is playing, as on the page.
	function playOrToggle() {
		if (playingFromHere) return void togglePlayback();
		if (onPlay) return onPlay();
		void playAlbum(album.id);
	}

	function shuffleWholeAlbum() {
		if (onShuffle) return onShuffle();
		void shuffleAlbum(album.id);
	}

	function albumMenu() {
		// The quick view has its own Open album page button.
		return buildAlbumMenu(album, { isLocal, hideOpen: resolvedAlbumHref != null });
	}

	function openAlbumContextMenu(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, albumMenu(), album.title);
	}

	// Stop the opening click before it reaches ContextMenu's window listener,
	// which would close the menu again on the same click.
	function openMoreMenu(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		openMenuAtElement(event.currentTarget as HTMLElement, albumMenu(), album.title);
	}

	function openAlbumArtistContextMenu(event: MouseEvent) {
		if (!album.artist_name) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildArtistMenu({ id: album.artist_id, name: album.artist_name, in_library: isLocal }, { isLocal }),
			album.artist_name,
		);
	}

	function tidalPlayable(track: Track): TidalPlayable & { track_number: number | null } {
		return {
			tidal_id: track.tidal_id ?? 0,
			title: track.title,
			artist_name: track.artist_name ?? null,
			album_title: track.album_title ?? album.title,
			artwork_url: track.artwork_url ?? album.artwork_url ?? null,
			duration_ms: track.duration_ms,
			is_favorite: track.is_favorite,
			track_number: track.track_number,
		};
	}
</script>

<!-- Links rather than goto(): middle-click and the router both work, and
     requestClose plays the exit while the page changes underneath. -->
<QuickViewPopup title={album.title} {onClose}>
	{#snippet hero(requestClose)}
		<DetailHero
			title={album.title}
			eyebrow={album.release_type ?? 'Album'}
			artwork={album.artwork_url}
			fallbackText={album.title.slice(0, 1)}
			oncontextmenu={openAlbumContextMenu}
		>
			{#snippet meta()}
				{#if album.artist_name && resolvedArtistHref}
					<a class="hero-link" href={resolvedArtistHref} title={`Open ${album.artist_name}`}
						onclick={requestClose} oncontextmenu={openAlbumArtistContextMenu}>{album.artist_name}</a>
				{:else}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<span class="hero-artist" oncontextmenu={openAlbumArtistContextMenu}>{album.artist_name ?? 'Unknown Artist'}</span>
				{/if}
				{#if album.year}<span class="meta-item">{album.year}</span>{/if}
				{#if songCount > 0}<span class="meta-item">{songCount} {songCount === 1 ? 'song' : 'songs'}</span>{/if}
				{#if totalMs > 0}<span class="meta-item duration">{formatTotalDuration(totalMs)}</span>{/if}
			{/snippet}
			{#snippet actions()}
				<ActionBar
					disabled={loading || tracks.length === 0}
					playing={albumPlaying}
					onplay={playOrToggle}
					onshuffle={shuffleWholeAlbum}
					shuffleHint="Play this album in random order"
					onmore={openMoreMenu}
					moreLabel="More album actions"
				>
					{#if resolvedAlbumHref}
						<a class="btn btn-ghost" href={resolvedAlbumHref} onclick={requestClose}>Open album page</a>
					{/if}
				</ActionBar>
			{/snippet}
		</DetailHero>
	{/snippet}

	{#if loading}
		<Skeleton rows={6} label="Loading tracks" />
	{:else if tracks.length === 0}
		<EmptyState title="No tracks yet" copy="This album has no synced tracks." />
	{:else}
		<ol class="track-list">
			{#each tracks as track, index (track.id)}
				{#if track.id > 0}
					<TrackRow
						{track}
						{index}
						variant="indexed"
						isCurrent={isCurrent(track)}
						isPlaying={$isPlaying}
						showAlbum={false}
						showArtist={track.artist_name !== album.artist_name}
						onRowClick={() => playFromHere(track)}
						menuOptions={{ hideAlbumActions: true }}
					/>
				{:else}
					<TidalTrackRow
						track={tidalPlayable(track)}
						{index}
						variant="indexed"
						isCurrent={isCurrent(track)}
						isPlaying={$isPlaying}
						showAlbum={false}
						showArtist={track.artist_name !== album.artist_name}
						onRowClick={() => playFromHere(track)}
					/>
				{/if}
			{/each}
		</ol>
	{/if}
</QuickViewPopup>

<style>
	.hero-link {
		color: var(--text-primary);
		font-weight: var(--font-weight-semibold);
		text-decoration: none;
	}

	.hero-link:hover {
		text-decoration: underline;
	}

	.hero-artist {
		color: var(--text-primary);
		font-weight: var(--font-weight-semibold);
	}

	/* A middle dot before every item after the artist. */
	.meta-item::before {
		content: '\00b7';
		margin-right: var(--space-2);
		color: var(--text-tertiary);
	}

	.duration {
		font-variant-numeric: tabular-nums;
	}

	.track-list {
		list-style: none;
		display: flex;
		flex-direction: column;
		margin: 0 calc(-1 * var(--space-2));
		padding: 0;
	}
</style>
