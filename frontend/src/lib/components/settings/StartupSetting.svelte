<script lang="ts">
	import { onMount } from 'svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { isTauri } from '$lib/util/external';
	import { retainObservedStartupState, startupPresentation, type DesktopStartupState } from '$lib/desktop/startup';
	import SettingGroup from './SettingGroup.svelte';
	import SettingRow from './SettingRow.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	let startupState = $state<DesktopStartupState | null>(null);
	let busy = $state(false);
	let error = $state('');
	let presentation = $derived(startupPresentation(startupState, busy));
	onMount(() => {
		if (!isTauri()) return;
		void load();
	});
	async function load() {
		busy = true; error = '';
		try { startupState = await invoke<DesktopStartupState>('get_startup_state'); }
		catch { error = 'Could not read start-at-sign-in status.'; }
		finally { busy = false; }
	}
	async function save(enabled: boolean) {
		if (!startupState || busy) return;
		const previous = startupState;
		startupState = { ...previous, enabled };
		busy = true; error = '';
		try { startupState = await invoke<DesktopStartupState>('set_start_at_login', { enabled }); }
		catch (cause) { startupState = retainObservedStartupState(previous, cause); error = 'Start-at-sign-in could not be changed.'; }
		finally { busy = false; }
	}
</script>
<SettingGroup title="Startup">
	<SettingRow label="Start at sign-in" id="startup" hint={isTauri() ? presentation.message : 'Available in the desktop app. Starts hidden in the tray.'}>
		<Toggle label="Start at sign-in" checked={presentation.checked} disabled={!isTauri() || presentation.disabled} onchange={(event) => void save(event.currentTarget.checked)} />
	</SettingRow>
	{#if error}<p class="error" role="alert">{error}</p><button class="btn btn-glass" onclick={() => void load()} disabled={busy}>Retry status</button>{/if}
</SettingGroup>
