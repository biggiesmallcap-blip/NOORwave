<script lang="ts">
	import { onDestroy, tick } from 'svelte';
	import { fade } from 'svelte/transition';

	// A control that asks in place before it acts: the button becomes the
	// question with its two answers, in the app's own buttons. It goes back on
	// its own after a few seconds (not while the pointer rests on it), on
	// Escape or on Cancel. Cancel takes focus, so an Enter pressed right after
	// the click never confirms by accident.
	let {
		label,
		question,
		confirmLabel = label,
		busy = false,
		timeoutMs = 6000,
		onconfirm,
	}: {
		label: string;
		question: string;
		confirmLabel?: string;
		busy?: boolean;
		timeoutMs?: number;
		onconfirm: () => void;
	} = $props();

	let asking = $state(false);
	let cancelButton = $state<HTMLButtonElement>();
	let timer: ReturnType<typeof setTimeout> | null = null;

	function stopTimer() {
		if (timer) clearTimeout(timer);
		timer = null;
	}

	function startTimer() {
		stopTimer();
		timer = setTimeout(() => (asking = false), timeoutMs);
	}

	async function ask() {
		asking = true;
		startTimer();
		await tick();
		cancelButton?.focus();
	}

	function cancel() {
		stopTimer();
		asking = false;
	}

	function confirm() {
		stopTimer();
		asking = false;
		onconfirm();
	}

	onDestroy(stopTimer);
</script>

<svelte:window
	onkeydown={(event) => {
		if (asking && event.key === 'Escape') cancel();
	}}
/>

{#if asking}
	<!-- Resting the pointer on the question only pauses its countdown; the
	     answers are the buttons inside. -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<div
		class="inline-confirm"
		role="group"
		aria-label={question}
		onpointerenter={stopTimer}
		onpointerleave={startTimer}
		in:fade={{ duration: 120 }}
	>
		<span class="question" aria-live="polite">{question}</span>
		<button type="button" class="btn btn-primary" onclick={confirm}>{confirmLabel}</button>
		<button type="button" class="btn btn-ghost" bind:this={cancelButton} onclick={cancel}>Cancel</button>
	</div>
{:else}
	<button type="button" class="btn btn-ghost" disabled={busy} onclick={() => void ask()}>{label}</button>
{/if}

<style>
	.inline-confirm {
		display: inline-flex;
		align-items: center;
		gap: 8px;
	}

	.question {
		margin-right: 4px;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		white-space: nowrap;
	}
</style>
