<script lang="ts" module>
	export type TimeRange = '24h' | '7d' | '14d' | '30d' | 'all';

	export const TIME_RANGE_DAYS: Record<TimeRange, number> = {
		'24h': 1,
		'7d': 7,
		'14d': 14,
		'30d': 30,
		// "All" maps to 36500 days — 100 years, well beyond any real listen history.
		// The backend `/dashboard` clamp was relaxed to [1, 36500] to accept this.
		all: 36500,
	};

	const STORAGE_KEY = 'noor:analytics:days';
</script>

<script lang="ts">
	/**
	 * TimeRangePills — five-pill row that drives the analytics page's time window.
	 *
	 * Persists to localStorage[noor:analytics:days] so the choice survives page reload.
	 * Default 30d if nothing stored.
	 *
	 * The `value` prop is bindable so the page can read it directly via $bindable;
	 * `onchange` fires after the user picks a new pill.
	 */

	import { onMount } from 'svelte';

	interface Props {
		value?: TimeRange;
		onchange?: (value: TimeRange) => void;
	}

	let { value = $bindable('30d'), onchange }: Props = $props();

	const RANGES: TimeRange[] = ['24h', '7d', '14d', '30d', 'all'];
	const LABELS: Record<TimeRange, string> = {
		'24h': '24h',
		'7d': '7d',
		'14d': '14d',
		'30d': '30d',
		all: 'All',
	};

	onMount(() => {
		const stored = localStorage.getItem(STORAGE_KEY) as TimeRange | null;
		if (stored && RANGES.includes(stored)) {
			value = stored;
		}
	});

	function pick(next: TimeRange) {
		value = next;
		try {
			localStorage.setItem(STORAGE_KEY, next);
		} catch {
			// localStorage may be unavailable (private mode, etc.); choice still works in-session.
		}
		onchange?.(next);
	}
</script>

<div class="pills" role="radiogroup" aria-label="Analytics time range">
	{#each RANGES as range (range)}
		<button
			type="button"
			class="pill"
			class:active={value === range}
			role="radio"
			aria-checked={value === range}
			onclick={() => pick(range)}
		>
			{LABELS[range]}
		</button>
	{/each}
</div>

<style>
	.pills {
		display: inline-flex;
		gap: 2px;
		padding: 2px;
		background: var(--input-bg);
		border: 1px solid var(--input-border);
		border-radius: var(--radius-xs);
	}

	.pill {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		letter-spacing: 0.04em;
		padding: 6px 12px;
		min-width: 36px;
		background: transparent;
		border: none;
		color: var(--text-secondary);
		border-radius: 4px;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			color var(--motion-fast);
	}

	.pill:hover {
		color: var(--text-primary);
		background: var(--bg-hover);
	}

	.pill.active {
		color: var(--text-primary);
		background: var(--accent-soft);
		box-shadow: inset 0 0 0 1px var(--accent-line);
	}

	.pill:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	@media (prefers-reduced-motion: reduce) {
		.pill {
			transition: none;
		}
	}
</style>
