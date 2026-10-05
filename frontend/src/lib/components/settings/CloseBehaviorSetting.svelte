<script lang="ts">
	import { onMount } from 'svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { isTauri } from '$lib/util/external';
	import SettingGroup from './SettingGroup.svelte';
	import SettingRow from './SettingRow.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	let enabled = $state(false);
	let known = $state(false);
	let busy = $state(false);
	let error = $state('');
	onMount(() => { if (isTauri()) void load(); });
	async function load() {
		busy = true; error = '';
		try { enabled = await invoke<boolean>('get_minimize_to_tray'); known = true; }
		catch { error = 'Could not read the close-button behavior.'; }
		finally { busy = false; }
	}
	async function save(next: boolean) {
		if (!known || busy || !isTauri()) return;
		const previous = enabled;
		enabled = next;
		busy = true; error = '';
		try { await invoke('set_minimize_to_tray', { value: next }); }
		catch { enabled = previous; error = 'Could not save the close-button behavior.'; }
		finally { busy = false; }
	}
</script>
<SettingGroup title="Closing the window">
	<SettingRow id="closing-the-window" label="Close to tray" hint={isTauri()
		? known ? 'On: keep music and phone remote running after closing the window. Off: quit NOORwave.' : 'Checking close-button behavior…'
		: 'Available in the desktop app. Keeps music and phone remote running when the window closes.'}>
		<Toggle label="Close to tray" checked={enabled} disabled={!isTauri() || !known || busy} onchange={(event) => void save(event.currentTarget.checked)} />
	</SettingRow>
	{#if error}<p class="error" role="alert">{error}</p><button class="btn btn-glass" onclick={() => void load()} disabled={busy}>Retry status</button>{/if}
</SettingGroup>
