<script lang="ts">
	import { hapticCommit } from '$lib/remote/haptics';

	let {
		onPlay,
		onShuffle,
		onRadio = null,
		disabled = false
	}: {
		onPlay: () => void | Promise<void>;
		onShuffle: () => void | Promise<void>;
		onRadio?: (() => void | Promise<void>) | null;
		disabled?: boolean;
	} = $props();

	function wrap(fn: () => void | Promise<void>) {
		return () => {
			hapticCommit();
			void fn();
		};
	}
</script>

<div class="remote-actionbar" role="group" aria-label="Playback actions">
	<button type="button" class="primary" disabled={disabled} onclick={wrap(onPlay)}>
		<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
			<path d="M7 5l12 7-12 7z" fill="currentColor" />
		</svg>
		Play
	</button>
	<button type="button" disabled={disabled} onclick={wrap(onShuffle)}>
		<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
			<path
				d="M4 7h3l3 5-3 5H4M4 17h3l3-5-3-5H4M14 7h6m0 0l-3-3m3 3l-3 3M14 17h6m0 0l-3-3m3 3l-3 3"
				fill="none"
				stroke="currentColor"
				stroke-width="1.8"
				stroke-linecap="round"
				stroke-linejoin="round"
			/>
		</svg>
		Shuffle
	</button>
	{#if onRadio}
		<button type="button" disabled={disabled} onclick={wrap(onRadio)}>
			<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
				<circle cx="12" cy="12" r="2" fill="currentColor" />
				<path
					d="M7 12a5 5 0 0110 0M4 12a8 8 0 0116 0"
					fill="none"
					stroke="currentColor"
					stroke-width="1.8"
					stroke-linecap="round"
				/>
			</svg>
			Radio
		</button>
	{/if}
</div>

<style>
	.remote-actionbar {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.remote-actionbar button {
		flex: 1;
		min-height: 48px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		padding: 0 14px;
		border-radius: 12px;
		background: var(--surface-1);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.remote-actionbar button:active {
		background: var(--surface-2);
	}

	.remote-actionbar button:disabled {
		opacity: 0.4;
	}

	.remote-actionbar .primary {
		flex: 1.4;
		background: var(--accent);
		color: var(--surface-0);
	}

	.remote-actionbar svg {
		width: 18px;
		height: 18px;
	}
</style>
