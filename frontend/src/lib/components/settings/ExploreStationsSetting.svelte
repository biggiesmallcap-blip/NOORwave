<script lang="ts">
	import { onMount } from 'svelte';
	import SettingRow from './SettingRow.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import { ExploreStationSettings } from './exploreStations.svelte';
	const settings = new ExploreStationSettings();
	onMount(() => void settings.load());
</script>
<SettingRow id="explore-stations" label="Explore stations" hint="Genre stations on the Stations tab beyond your own genres. Scenes you switch off are not built or shown.">
	<Toggle label="Explore stations" checked={settings.enabled} disabled={!settings.known || settings.busy} onchange={(event) => void settings.setEnabled(event.currentTarget.checked)} />
</SettingRow>
{#if settings.known && settings.enabled && settings.scenes.length > 0}
	<div class="scene-chips" role="group" aria-label="Explore scenes">
		{#each settings.scenes as scene (scene.slug)}
			<button
				type="button"
				class="scene-chip"
				aria-pressed={settings.isOn(scene.slug)}
				title={scene.subtitle}
				disabled={settings.busy}
				onclick={() => void settings.toggleScene(scene.slug)}
			>{scene.title}</button>
		{/each}
	</div>
{/if}
{#if settings.error}
	<p class="error" role="alert">{settings.error}</p>
	<button class="btn btn-glass" disabled={settings.busy} onclick={() => void settings.load()}>Retry</button>
{/if}

<style>
	.scene-chips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin: 2px 0 var(--space-2);
	}
	.scene-chip {
		height: 30px;
		padding: 0 12px;
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		background: transparent;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		cursor: pointer;
	}
	.scene-chip[aria-pressed='true'] {
		border-color: var(--accent-line);
		background: var(--accent-soft);
		color: var(--text-primary);
	}
	.scene-chip:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
	.scene-chip:disabled { opacity: 0.6; cursor: default; }
</style>
