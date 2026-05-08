<script lang="ts">
	type Tone = 'default' | 'active' | 'success' | 'warning' | 'error' | 'muted';

	let {
		label,
		tone = 'default',
		compact = false
	}: {
		label: string;
		tone?: Tone;
		compact?: boolean;
	} = $props();
</script>

<span class:compact class={`state-badge ${tone}`}>
	<span class="dot"></span>
	{label}
</span>

<style>
	.state-badge {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		padding: 7px 11px;
		border-radius: 999px;
		border: 1px solid var(--panel-border);
		background: rgba(255, 255, 255, 0.04);
		color: var(--text-secondary);
		font-size: 0.76rem;
		white-space: nowrap;
	}

	.dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: rgba(255, 255, 255, 0.2);
	}

	.compact {
		padding: 5px 9px;
		font-size: 0.72rem;
		gap: 7px;
	}

	.active,
	.default {
		color: var(--text-primary);
	}

	.active .dot,
	.default .dot {
		background: var(--accent);
	}

	.success {
		color: var(--state-success);
	}

	.success .dot {
		background: var(--state-success);
	}

	.warning {
		color: var(--state-warning);
	}

	.warning .dot {
		background: var(--state-warning);
	}

	.error {
		color: var(--state-error);
	}

	.error .dot {
		background: var(--state-error);
	}

	.muted .dot {
		background: rgba(255, 255, 255, 0.16);
	}
</style>
