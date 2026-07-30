<script lang="ts">
	import { onMount } from 'svelte';
	import type { Track } from '$lib/api/client';
	import AlbumDetailPopup from '$lib/components/AlbumDetailPopup.svelte';
	import { loadAlbumDetail, type AlbumDetail, type AlbumHints } from '$lib/album/album_detail';
	import {
		playAlbum,
		playTidalAlbum,
		playTidalTrackNow,
		shuffleAlbum,
	} from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import { tidalSearchTrackToPlayable } from '$lib/utils/track';

	// The Library album popup, driven by an album id from anywhere.
	//
	// Every album card in the app opens this: Library, the TIDAL discover shelves
	// and their View all pages, and the Last.fm recommendation rails. Loading
	// lives in album_detail.ts; this owns the open/close lifecycle and the play
	// handlers, which differ from Library's because most of these albums are not
	// in the library.
	let {
		localAlbumId = null,
		tidalAlbumId = null,
		artistTidalId = null,
		hints,
		onClose,
	}: {
		localAlbumId?: number | null;
		tidalAlbumId?: number | null;
		/** Makes the popup's artist name a link for a TIDAL-only album. */
		artistTidalId?: number | null;
		hints: AlbumHints;
		onClose: () => void;
	} = $props();

	let detail = $state<AlbumDetail | null>(null);
	let loading = $state(true);
	let failed = $state(false);

	// Loaded once, on mount, deliberately not in an `$effect`.
	//
	// The effect version reset `detail` and re-ran whenever a prop changed
	// identity - which it does, because a shelf re-derives its item objects when
	// its payload refreshes. The reload raced its own predecessor and the popup
	// sat on "loading" forever with a fully resolved album in hand. One popup
	// instance is about one album, so it only ever needs to load once; callers key
	// this component on the album, so picking a different one mounts a fresh copy.
	onMount(() => {
		void start();
	});

	async function start() {
		const result = await loadAlbumDetail({ localAlbumId, tidalAlbumId }, hints);
		if (!result) {
			// No tracklist to show. A toast says so and leaves the user where they
			// were, rather than a dead popup or a surprise navigation.
			failed = true;
			loading = false;
			showToast(`Couldn't load "${hints.title}"`, 'error');
			onClose();
			return;
		}
		detail = result;
		loading = false;
	}

	const artistHref = $derived(
		detail?.isLocal && detail.album.artist_id
			? `/artists/${detail.album.artist_id}`
			: artistTidalId
				? `/tidal/artists/${artistTidalId}`
				: null,
	);

	function play() {
		if (!detail) return;
		if (detail.localAlbumId) return void playAlbum(detail.localAlbumId);
		if (detail.tidalAlbumId) return void playTidalAlbum(detail.tidalAlbumId);
	}

	function shuffle() {
		if (!detail) return;
		// shuffleAlbum is local-only; a TIDAL album queues in order and the
		// player's own shuffle takes over from there.
		if (detail.localAlbumId) return void shuffleAlbum(detail.localAlbumId);
		if (detail.tidalAlbumId) return void playTidalAlbum(detail.tidalAlbumId);
	}

	function playFrom(track: Track) {
		if (detail?.localAlbumId) return void playAlbum(detail.localAlbumId, track.id);
		// Unowned rows carry their TIDAL id; play that one track directly.
		const tidalId = (track as { tidal_id?: number | null }).tidal_id;
		if (!tidalId) return;
		void playTidalTrackNow(
			tidalSearchTrackToPlayable({
				tidal_id: tidalId,
				title: track.title,
				artist_name: track.artist_name,
				album_title: track.album_title,
				artwork_url: track.artwork_url,
				duration_ms: track.duration_ms,
			} as never),
		);
	}
</script>

{#if !failed && detail}
	<AlbumDetailPopup
		album={detail.album}
		tracks={detail.tracks}
		{loading}
		isLocal={detail.isLocal}
		{artistHref}
		onPlay={play}
		onShuffle={shuffle}
		onPlayFrom={playFrom}
		{onClose}
	/>
{/if}
