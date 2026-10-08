<script lang="ts" generics="T extends string">
	// Choosing one value (STYLING.md "Segmented"): a sunken track with a
	// neutral thumb that slides to the chosen option. Never accent; the accent
	// means "where you are" (ScopeTabs) or "on" (FilterChip), not "which value".
	// A radio group with one tab stop: arrows, Home and End choose.

	let {
		options,
		value,
		label,
		onchange,
		disabled = false,
		full = false,
	}: {
		options: readonly { value: T; label: string }[];
		value: T;
		/** Accessible name of the group. */
		label: string;
		onchange: (value: T) => void;
		disabled?: boolean;
		/** Stretch to the container instead of hugging the labels. */
		full?: boolean;
	} = $props();

	let selected = $derived(options.findIndex((option) => option.value === value));
	let root = $state<HTMLElement>();

	function onKeydown(event: KeyboardEvent) {
		const from = selected < 0 ? 0 : selected;
		const last = options.length - 1;
		let to = -1;
		if (event.key === 'ArrowRight' || event.key === 'ArrowDown') to = from === last ? 0 : from + 1;
		else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') to = from === 0 ? last : from - 1;
		else if (event.key === 'Home') to = 0;
		else if (event.key === 'End') to = last;
		if (to < 0) return;
		// The window-level player shortcuts read arrows as seek and volume.
		event.preventDefault();
		event.stopPropagation();
		onchange(options[to].value);
		root?.querySelectorAll<HTMLElement>('[role="radio"]')[to]?.focus();
	}
</script>

<div
	class="segmented"
	class:full
	role="radiogroup"
	aria-label={label}
	aria-disabled={disabled || undefined}
	style:--count={options.length}
	style:--index={selected}
	bind:this={root}
>
	{#if selected >= 0}
		<span class="thumb" aria-hidden="true"></span>
	{/if}
	{#each options as option, index (option.value)}
		<button
			type="button"
			role="radio"
			aria-checked={value === option.value}
			tabindex={value === option.value || (selected < 0 && index === 0) ? 0 : -1}
			{disabled}
			onclick={() => onchange(option.value)}
			onkeydown={onKeydown}
		>{option.label}</button>
	{/each}
</div>

<style>
	.segmented {
		position: relative;
		isolation: isolate;
		display: inline-grid;
		grid-auto-flow: column;
		grid-auto-columns: minmax(0, 1fr);
		padding: 3px;
		border-radius: 999px;
		background: var(--bg-surface);
	}

	.segmented.full {
		display: grid;
		width: 100%;
	}

	/* One column wide, moved by whole columns: the transition retargets from
	   wherever it is when the value changes mid-slide. */
	.thumb {
		position: absolute;
		z-index: -1;
		top: 3px;
		bottom: 3px;
		left: 3px;
		width: calc((100% - 6px) / var(--count));
		border-radius: 999px;
		background: var(--bg-raised);
		/* The hairline keeps the thumb legible where --bg-raised sits close to
		   the track (Clay dark). */
		box-shadow:
			inset 0 0 0 1px var(--border-subtle),
			0 1px 2px var(--player-art-shadow);
		transform: translateX(calc(var(--index) * 100%));
		transition: transform var(--motion-base);
	}

	button {
		height: var(--control-h);
		padding: 0 14px;
		border: 0;
		border-radius: 999px;
		background: transparent;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		white-space: nowrap;
		cursor: pointer;
		transition: color var(--motion-fast);
	}

	button:hover:not(:disabled),
	button[aria-checked='true'] {
		color: var(--text-primary);
	}

	button[aria-checked='true'] {
		font-weight: var(--font-weight-semibold);
	}

	button:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	@media (prefers-reduced-motion: reduce) {
		.thumb {
			transition: none;
		}
	}
</style>
