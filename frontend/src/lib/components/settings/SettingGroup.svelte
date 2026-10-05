<script lang="ts">
	import type { Snippet } from 'svelte';
	let { title, children }: { title: string; children: Snippet } = $props();
</script>
<section class="glass-tile setting-group">
	<h3 class="setting-group-title">{title}</h3>
	{@render children()}
</section>
