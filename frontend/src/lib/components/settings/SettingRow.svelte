<script lang="ts">
	import type { Snippet } from 'svelte';
	let { label, id, hint = '', children }: { label: string; id?: string; hint?: string; children: Snippet } = $props();
</script>
<div class="setting-row" data-setting-id={id}>
	<div class="setting-label"><span>{label}</span>{#if hint}<p>{hint}</p>{/if}</div>
	<div class="setting-control">{@render children()}</div>
</div>
