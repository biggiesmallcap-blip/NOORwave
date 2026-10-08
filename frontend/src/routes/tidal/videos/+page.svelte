<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';

	// TIDAL editorial videos moved into the video section as one of its tabs.
	onMount(() => void goto('/videos/editorial', { replaceState: true }));
</script>
