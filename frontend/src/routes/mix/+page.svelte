<script lang="ts">
	// One page for what plays next (Automix) and how each track hands over to
	// the next (DJ). The header owns both switches and the page-level actions;
	// DjCockpit and AutomixPanel share one Diagnostics disclosure at the end.
	import { onMount } from 'svelte';
	import {
		automixEnabled,
		crossfadeMs,
		currentTrack,
		setPlayerAutomixEnabled,
		startSongRadio,
	} from '$lib/stores/player';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import AutomixPanel from '$lib/components/mix/AutomixPanel.svelte';
	import DjCockpit from '$lib/components/dj-cockpit/DjCockpit.svelte';
	import DjDiagnostics from '$lib/components/dj-cockpit/DjDiagnostics.svelte';
	import { djEnabled, djSaving, setDjEnabled, startDjPolling } from '$lib/components/dj-cockpit/dj_engine';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { showToast } from '$lib/stores/toast';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';
	import type { Snapshot } from './$types';

	const RADIO_GLYPH = '◉';
	const REFRESH_GLYPH = '⟳';

	let panel = $state<{ refresh: () => Promise<void> } | undefined>();
	let automixSaving = $state(false);

	onMount(() => startDjPolling());

	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: captureScroll() }),
		restore: (saved) => restoreScroll(saved.scrollY),
	};

	async function toggleAutomix() {
		if (automixSaving) return;
		automixSaving = true;
		try {
			await setPlayerAutomixEnabled(!$automixEnabled, $crossfadeMs);
		} catch {
			showToast('Could not change Automix.', 'error');
		} finally {
			automixSaving = false;
		}
	}

	async function startCurrentSongRadio() {
		const trackId = $currentTrack?.id;
		if (!trackId) return;
		try {
			await startSongRadio(trackId);
		} catch {
			showToast('Could not start radio.', 'error');
		}
	}

	function openMore(event: MouseEvent) {
		openContextMenu(
			event,
			[
				{ label: 'Start radio from this song', icon: RADIO_GLYPH, disabled: !$currentTrack, onSelect: () => void startCurrentSongRadio() },
				{ label: 'Refresh data', icon: REFRESH_GLYPH, onSelect: () => void panel?.refresh() },
			],
			'Mix',
		);
	}
</script>

<svelte:head>
	<title>Mix | NOOR</title>
</svelte:head>

<div class="page-shell mix-page">
	<PageHeader title="Mix" subtitle="What plays next, and how each track hands over to the next.">
		{#snippet actions()}
			<button
				type="button"
				class="mix-switch"
				role="switch"
				aria-label="Automix"
				aria-checked={$automixEnabled}
				disabled={automixSaving}
				onclick={() => void toggleAutomix()}
			>
				<span class="switch-track" aria-hidden="true"><span class="switch-thumb"></span></span>
				Automix
			</button>
			<button
				type="button"
				class="mix-switch"
				role="switch"
				aria-label="DJ transitions"
				aria-checked={$djEnabled ?? false}
				disabled={$djSaving || $djEnabled == null}
				onclick={() => void setDjEnabled(!$djEnabled)}
			>
				<span class="switch-track" aria-hidden="true"><span class="switch-thumb"></span></span>
				{$djEnabled == null ? 'DJ connecting' : 'DJ transitions'}
			</button>
			<button type="button" class="btn btn-glass mix-more" aria-label="More Mix actions" title="More Mix actions" aria-haspopup="menu" onclick={openMore}>
				<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><circle cx="5" cy="12" r="1.8" fill="currentColor" /><circle cx="12" cy="12" r="1.8" fill="currentColor" /><circle cx="19" cy="12" r="1.8" fill="currentColor" /></svg>
			</button>
		{/snippet}
	</PageHeader>

	<DjCockpit />

	<AutomixPanel bind:this={panel}>
		{#snippet session()}
			<DjCockpit part="controls" />
		{/snippet}
		{#snippet diagnostics()}
			<DjDiagnostics />
		{/snippet}
	</AutomixPanel>
</div>

<style>
	.mix-page {
		gap: var(--space-5);
	}

	.mix-switch {
		min-height: 2.5rem;
		padding: 0 var(--space-3) 0 var(--space-2);
		border: 1px solid var(--border-muted);
		border-radius: 999px;
		background: color-mix(in srgb, var(--bg-raised) 86%, transparent);
		color: var(--text-secondary);
		font: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.mix-switch[aria-checked='true'] {
		border-color: color-mix(in srgb, var(--state-success) 54%, var(--accent-line));
		background: color-mix(in srgb, var(--state-success) 14%, var(--accent-soft));
		color: var(--text-primary);
	}

	.mix-switch:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.mix-switch:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	.switch-track {
		width: 2.3rem;
		height: 1.25rem;
		padding: 0.15rem;
		border-radius: 999px;
		background: color-mix(in srgb, var(--text-tertiary) 32%, transparent);
		display: flex;
		align-items: center;
		transition: background var(--motion-fast);
	}

	.mix-switch[aria-checked='true'] .switch-track {
		background: color-mix(in srgb, var(--state-success) 68%, var(--accent-strong));
	}

	.switch-thumb {
		width: 0.95rem;
		height: 0.95rem;
		border-radius: 50%;
		background: var(--text-primary);
		box-shadow: 0 1px 4px rgba(0, 0, 0, 0.3);
		transition: transform var(--motion-fast);
	}

	.mix-switch[aria-checked='true'] .switch-thumb {
		transform: translateX(1.05rem);
	}

	.mix-more {
		width: 2.5rem;
		height: 2.5rem;
		padding: 0;
		display: inline-grid;
		place-items: center;
	}
</style>
