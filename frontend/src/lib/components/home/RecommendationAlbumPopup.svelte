<script lang="ts">
	import { onMount } from 'svelte';
	import type { ProviderRecommendationItem } from '$lib/api/client';
	import AlbumPopup from '$lib/components/album/AlbumPopup.svelte';
	import { resolveRecommendationAlbum } from '$lib/player/play_recommendations';
	import { showToast } from '$lib/stores/toast';

	// The shared album popup, driven by a recommendation.
	//
	// A recommendation carries names rather than ids, so this resolves it first and
	// then hands the ids to AlbumPopup, which is the same component every other
	// album card in the app opens. Since the server started resolving album ids
	// into the payload the resolve is normally a no-op.
	let { item, onClose }: { item: ProviderRecommendationItem; onClose: () => void } = $props();

	let ids = $state<{ localAlbumId: number | null; tidalAlbumId: number | null } | null>(null);
	let failed = $state(false);

	onMount(() => {
		void start(item);
	});

	async function start(target: ProviderRecommendationItem) {
		const resolved = await resolveRecommendationAlbum(target);
		if (!resolved) {
			// Some Last.fm albums are not on TIDAL at all (old singles, regional
			// pressings), so there is no tracklist to show. A toast says so and
			// leaves the user where they were; the context menu still offers
			// "Search for this".
			//
			// showToast rather than playerError: the player error slot lives inside
			// PlayerBar, so nothing would have been shown at all with an idle player.
			failed = true;
			showToast(`Couldn't find "${target.title}" on Tidal`, 'error');
			onClose();
			return;
		}
		ids = { localAlbumId: resolved.localId, tidalAlbumId: resolved.tidalId };
	}
</script>

{#if !failed && ids}
	<AlbumPopup
		localAlbumId={ids.localAlbumId}
		tidalAlbumId={ids.tidalAlbumId}
		artistTidalId={item.tidal_artist_id ?? null}
		hints={{
			title: item.title,
			artistName: item.artist_name,
			artworkUrl: item.artwork_url,
			artistId: item.local_artist_id ?? null,
		}}
		{onClose}
	/>
{/if}
