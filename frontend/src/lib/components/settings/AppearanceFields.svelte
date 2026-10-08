<script lang="ts">
	import { PALETTES, rgbCss, type PaletteId } from '$lib/components/wallpaper/palettes';
	import { WALLPAPERS } from '$lib/components/wallpaper/shaders';
	import type { SurfaceMode } from '$lib/stores/surfaceMode';
	import type { WallpaperId } from '$lib/components/wallpaper/shaders';
	import SettingRow from './SettingRow.svelte';
	export type AppearanceValues = { palette: PaletteId; theme: SurfaceMode; background: WallpaperId };
	let { values, onchange, showBackground = true, prefix = 'appearance' }: {
		values: AppearanceValues; onchange: (change: Partial<AppearanceValues>) => void;
		showBackground?: boolean; prefix?: string;
	} = $props();
	let selectedPalette = $derived(PALETTES.find((item) => item.id === values.palette) ?? PALETTES[0]);
</script>
<SettingRow label="Theme" id="surface-mode">
	<div class="theme-options" role="radiogroup" aria-label="Theme">
		{#each ['light', 'dark', 'system'] as mode}
			<label class:chosen={values.theme === mode}>
				<input type="radio" name={prefix + '-theme'} checked={values.theme === mode} onchange={() => onchange({ theme: mode as SurfaceMode })} />
				<span>{mode === 'light' ? 'Light' : mode === 'dark' ? 'Dark' : 'System'}</span>
			</label>
		{/each}
	</div>
</SettingRow>
<SettingRow label="Colour scheme" id="colour-scheme">
	<div class="palette-field">
		<div class="swatches" aria-hidden="true">
			{#each [selectedPalette.shader.c1, selectedPalette.shader.c2, selectedPalette.shader.c3, selectedPalette.shader.c4] as colour}<span style:background={rgbCss(colour)}></span>{/each}
		</div>
		<select aria-label="Colour scheme" value={values.palette} onchange={(event) => onchange({ palette: event.currentTarget.value as PaletteId })}>
			{#each PALETTES as item}<option value={item.id}>{item.label}</option>{/each}
		</select>
	</div>
</SettingRow>
{#if showBackground}
	<SettingRow label="Background" id="background">
		<select aria-label="Background" value={values.background} onchange={(event) => onchange({ background: event.currentTarget.value as WallpaperId })}>
			{#each WALLPAPERS as item}<option value={item.id}>{item.id === 'none' ? 'Off' : item.label}</option>{/each}
		</select>
	</SettingRow>
{/if}
<style>
	.theme-options { display: flex; gap: var(--space-1); }
	.theme-options label { position: relative; cursor: pointer; padding: var(--space-2) var(--space-3); min-height: var(--settings-control-height, 40px); display: flex; align-items: center; border: 1px solid var(--border-muted); border-radius: var(--radius-sm); background: var(--bg-elevated); font-size: var(--font-size-sm); }
	.theme-options input { position: absolute; opacity: 0; width: 1px; height: 1px; pointer-events: none; }
	.theme-options label.chosen { background: var(--accent-soft); border-color: var(--accent-line); }
	.theme-options label:has(input:focus-visible) { outline: 2px solid var(--accent-strong); outline-offset: 3px; }
	.palette-field { display: flex; align-items: center; gap: var(--space-2); min-width: 0; width: 100%; }
	.swatches { display: flex; flex-shrink: 0; gap: 2px; }
	.swatches span { width: 10px; height: 20px; border-radius: 2px; }
</style>
