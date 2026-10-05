<script lang="ts">
    import { onMount } from 'svelte';
    import SettingRow from './SettingRow.svelte';
    import Toggle from '$lib/components/ui/Toggle.svelte';
    import { wsMessages } from '$lib/api/ws';
    import { TidalContentSettings } from './tidalContent.svelte';
    const settings = new TidalContentSettings();
    onMount(() => {
        void settings.load();
        return wsMessages.subscribe(messages => {
            if (messages.at(-1)?.type === 'tidal_content_settings_changed') void settings.load();
        });
    });
</script>
<SettingRow id="tidal-content-preferences" label="Hide AI-generated tracks" hint="Uses TIDAL’s AI labels. Saved library items stay available.">
    <Toggle label="Hide AI-generated tracks" checked={settings.enabled} disabled={!settings.known || settings.busy} onchange={event => void settings.save(event.currentTarget.checked)} />
</SettingRow>
{#if settings.error}
    <p class="error" role="alert">{settings.error}</p>
    <button class="btn btn-glass" disabled={settings.busy} onclick={() => void settings.load()}>Retry filter status</button>
{/if}
