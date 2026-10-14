<script lang="ts">
	import SettingRow from './SettingRow.svelte';
	import { TUNE_MODES, setTuneMode, tuneMode, type TuneMode } from '$lib/video/onair/tune_mode';
	let hint = $derived(TUNE_MODES.find((option) => option.value === $tuneMode)?.hint ?? '');
</script>
<SettingRow id="onair-tune" label="On Air: tuning in" {hint}>
	<select
		class="audio-select"
		aria-label="On Air: tuning in"
		value={$tuneMode}
		onchange={(event) => setTuneMode(event.currentTarget.value as TuneMode)}
	>
		{#each TUNE_MODES as option (option.value)}
			<option value={option.value}>{option.label}</option>
		{/each}
	</select>
</SettingRow>
