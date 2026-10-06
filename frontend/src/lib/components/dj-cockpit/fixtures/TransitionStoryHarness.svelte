<script module lang="ts">
	export { mount, tick, unmount } from 'svelte';
	export { writable } from 'svelte/store';
	export { default as Cockpit } from '../DjCockpit.svelte';
</script>

<script lang="ts">
	import type { Readable } from 'svelte/store';
	import type { DjStatusResponse } from '$lib/api/client';
	import TransitionStory from '../TransitionStory.svelte';
	let { statuses }: { statuses: Readable<DjStatusResponse> } = $props();
</script>

<TransitionStory status={$statuses} />
