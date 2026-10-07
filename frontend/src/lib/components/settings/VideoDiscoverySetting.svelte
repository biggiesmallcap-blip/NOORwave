<script lang="ts">
	import { onMount } from 'svelte';
	import SettingRow from './SettingRow.svelte';
	import type { VideoDiscoverySetting } from '$lib/api/client';
	import { VIDEO_DISCOVERY_OPTIONS, VideoDiscoverySettings } from './videoDiscovery.svelte';
	const settings = new VideoDiscoverySettings();
	let hint = $derived(VIDEO_DISCOVERY_OPTIONS.find((option) => option.value === settings.setting)?.hint ?? '');
	onMount(() => void settings.load());
</script>
<SettingRow id="video-discovery" label="Video discovery" {hint}>
	<select
		class="audio-select"
		aria-label="Video discovery"
		value={settings.setting}
		disabled={!settings.known || settings.busy}
		onchange={(event) => void settings.save(event.currentTarget.value as VideoDiscoverySetting)}
	>
		{#each VIDEO_DISCOVERY_OPTIONS as option (option.value)}
			<option value={option.value}>{option.label}</option>
		{/each}
	</select>
</SettingRow>
{#if settings.status}
	<p class="setting-status">
		{settings.status.catalog_videos.toLocaleString()} videos from {settings.status.artists_with_videos.toLocaleString()} artists found so far. {settings.status.calls_today.toLocaleString()} TIDAL requests today.
	</p>
{/if}
{#if settings.error}
	<p class="error" role="alert">{settings.error}</p>
	<button class="btn btn-glass" disabled={settings.busy} onclick={() => void settings.load()}>Retry</button>
{/if}
