<!-- Empty state for the track and video queues: a headline and a hint line. -->
<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		title: string;
		children: Snippet;
	}

	let { title, children }: Props = $props();
</script>

<div class="queue-empty">
	<p>{title}</p>
	<span>{@render children()}</span>
</div>

<style>
	.queue-empty {
		padding: 18px 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.queue-empty p {
		font-weight: var(--font-weight-semibold);
	}

	.queue-empty span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}
</style>
