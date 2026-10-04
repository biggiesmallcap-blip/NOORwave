<script lang="ts">
	import type { Snippet } from 'svelte';
	import { isTauri, openExternal } from '$lib/util/external';
	let { href, children }: { href: string; children: Snippet } = $props();
	let error = $state(false);
	async function open(event: MouseEvent) {
		if (!isTauri()) return;
		event.preventDefault();
		error = !(await openExternal(href)).ok;
	}
</script>
<a class="external-link" {href} target="_blank" rel="noopener noreferrer" onclick={(event) => void open(event)}>{@render children()}<span aria-hidden="true"> ↗</span></a>
{#if error}<span class="external-link-error" role="alert">Could not open your browser. Open this address manually: <span>{href}</span></span>{/if}
<style>
	.external-link { color: var(--accent-strong); text-decoration: underline; text-underline-offset: 3px; font-size: var(--font-size-sm); }
	.external-link-error { display: block; color: var(--state-error); font-size: var(--font-size-sm); overflow-wrap: anywhere; }
</style>
