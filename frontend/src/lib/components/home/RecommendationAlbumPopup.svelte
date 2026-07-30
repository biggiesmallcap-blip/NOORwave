<script lang="ts">
	import { onMount } from 'svelte';
	import type { ProviderRecommendationItem } from '$lib/api/client';
	import AlbumDetailPopup from '$lib/components/AlbumDetailPopup.svelte';
	import { playTidalAlbum, playAlbum, shuffleAlbum } from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import { playTidalTrackNow } from '$lib/stores/player';
	import { tidalSearchTrackToPlayable } from '$lib/utils/track';
	import {
		loadRecommendationAlbumDetail,
		type RecommendationAlbumDetail,
	} from './recommendation_album_detail';

	// The Library album popup, driven by a recommendation.
	//
	// Both the Home rail and the View all grid mount this, so opening a recommended
	// album feels the same wherever the card is. Resolution lives in
	// recommendation_album_detail.ts; this owns only the open/close lifecycle and
	// the play handlers, which differ from Library's because a recommended album
	// is usually not in the library.
	let { item, onClose }: { item: ProviderRecommendationItem; onClose: () => void } = $props();

	let detail = $state<RecommendationAlbumDetail | null>(null);
	let loading = $state(true);
	let failed = $state(false);

	// Loaded once, on mount, deliberately not in an `$effect`.
	//
	// The effect version reset `detail` and re-ran whenever the parent's item
	// reference changed identity - which it does, because the shelf re-derives
	// its item objects when the recommendation payload refreshes. The reload
	// raced its own predecessor and the popup sat on "loading" forever with a
	// fully resolved album in hand. One popup instance is about one album, so it
	// only ever needs to load once; the parent keys this component on the item,
	// so choosing a different album mounts a fresh one.
	onMount(() => {
		void start(item);
	});

	async function start(target: ProviderRecommendationItem) {
		const result = await loadRecommendationAlbumDetail(target);
		if (!result) {
			// Some Last.fm albums are not on TIDAL at all (old singles, regional
			// pressings), so there is no tracklist to show. This used to navigate to
			// /search for the title, which threw the user off Home for a click that
			// promised a popup. A toast says the same thing and leaves them where
			// they were; the context menu still offers "Search for this".
			//
			// showToast rather than playerError: the player error slot lives inside
			// PlayerBar, so nothing would have been shown at all with an idle player.
			failed = true;
			loading = false;
			showToast(`Couldn't find "${target.title}" on Tidal`, 'error');
			onClose();
			return;
		}
		detail = result;
		loading = false;
	}

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
</script>

{#if !failed && detail}
	<AlbumDetailPopup
		album={detail.album}
		tracks={detail.tracks}
		{loading}
		isLocal={detail.isLocal}
		onPlay={play}
		onShuffle={shuffle}
		onPlayFrom={(track) => {
			if (detail?.localAlbumId) return void playAlbum(detail.localAlbumId, track.id);
			// Unowned rows carry their TIDAL id; play that one track directly.
			const tidalId = (track as { tidal_id?: number | null }).tidal_id;
			if (tidalId) {
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
		}}
		{onClose}
	/>
{/if}
