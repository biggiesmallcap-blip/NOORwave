<script lang="ts">
	import TidalEditorialPage from '$lib/components/tidal/TidalEditorialPage.svelte';
</script>

<TidalEditorialPage
	pagePath="explore"
	title="Explore"
	subtitle="Browse TIDAL editorial shelves and recommendations."
	emptyText="TIDAL returned no explore modules right now."
	disconnectedText="Connect TIDAL to browse Explore."
	errorText="Could not load Explore."
/>
