<script lang="ts">
	import TidalEditorialPage from '$lib/components/tidal/TidalEditorialPage.svelte';
</script>

<TidalEditorialPage
	pagePath="new-releases"
	title="New Releases"
	subtitle="New music modules from TIDAL's latest editorial page."
	emptyText="TIDAL returned no new-release modules right now."
	disconnectedText="Connect TIDAL to browse new releases."
	errorText="Could not load new releases."
/>
