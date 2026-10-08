<script lang="ts">
	// A failed load (STYLING.md "States"): what happened in plain words, a
	// Retry, and the raw error behind Details. Never print an exception as copy.
	import type { Snippet } from 'svelte';
	import { describeError } from '$lib/utils/describe_error';

	let {
		title,
		error,
		onretry,
		actions,
	}: {
		title: string;
		error: unknown;
		onretry?: () => void;
		/** Extra ways out, after Retry (e.g. "Back to library"). */
		actions?: Snippet;
	} = $props();

	let described = $derived(describeError(error));
</script>

<div class="error-state" role="alert">
	<h3>{title}</h3>
	<p>{described.message}</p>
	<div class="actions">
		{#if onretry}
			<button type="button" class="btn btn-primary" onclick={onretry}>Retry</button>
		{/if}
		{@render actions?.()}
	</div>
	{#if described.detail}
		<details>
			<summary>Details</summary>
			<p class="detail">{described.detail}</p>
		</details>
	{/if}
</div>

<style>
	.error-state {
		padding: var(--space-5) 0;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 10px;
	}

	h3,
	p {
		margin: 0;
	}

	p {
		color: var(--text-secondary);
		max-width: 52ch;
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	details {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	summary {
		cursor: pointer;
	}

	summary:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.detail {
		margin-top: var(--space-1);
		color: var(--text-tertiary);
		overflow-wrap: anywhere;
	}
</style>
