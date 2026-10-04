<script lang="ts">
	import ListeningServicesPanel from "$lib/components/settings/ListeningServicesPanel.svelte";
	let { oncontinue, onskip }: { oncontinue?: () => void; onskip?: () => void } = $props();
</script>
<div class="listening-setup">
	<h2 class="onboarding-title">Connect listening history</h2>
	<ListeningServicesPanel guided showHistory={false} />
	<div class="onboarding-actions">
		<button class="btn btn-primary onboarding-action" onclick={oncontinue}>Continue</button>
		<button class="btn btn-glass onboarding-action" onclick={onskip}>Skip for now</button>
	</div>
</div>
<style>
	.listening-setup { width: 100%; display: flex; flex-direction: column; align-items: center; gap: var(--space-5); }
</style>
