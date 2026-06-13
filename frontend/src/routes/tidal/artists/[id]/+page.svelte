<script lang="ts">
	import { page } from '$app/state';
	import type { Snapshot } from './$types';
	import ArtistDetail from '../../../artists/ArtistDetail.svelte';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';

	let tidalArtistId = $derived(Number(page.params.id));

	// Non-library artist found via search. Renders the same ArtistDetail view as
	// a library artist, sourced from the TIDAL profile endpoint instead of a
	// local artist row.
	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: captureScroll() }),
		restore: (saved) => {
			restoreScroll(saved.scrollY);
		}
	};
</script>

<ArtistDetail source={{ kind: 'tidal', tidalArtistId }} />
