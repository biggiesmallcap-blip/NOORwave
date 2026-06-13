<script lang="ts">
	import { page } from '$app/state';
	import ArtistDiscographySection from '../../../../../artists/ArtistDiscographySection.svelte';

	type Section = 'tracks' | 'albums' | 'singles' | 'compilations';

	function normalizeSection(raw: string | undefined): Section {
		if (raw === 'tracks' || raw === 'albums' || raw === 'singles' || raw === 'compilations') {
			return raw;
		}
		return 'tracks';
	}

	let tidalArtistId = $derived(Number(page.params.id));
	let section = $derived(normalizeSection(page.params.section));
</script>

<ArtistDiscographySection source={{ kind: 'tidal', tidalArtistId }} {section} />
