<script lang="ts">
	import { PALETTE_FAMILIES, PALETTES, rgbCss, type Palette, type PaletteId } from '$lib/components/wallpaper/palettes';
	import { WALLPAPERS } from '$lib/components/wallpaper/shaders';
	import type { SurfaceMode } from '$lib/stores/surfaceMode';
	import type { WallpaperId } from '$lib/components/wallpaper/shaders';
	import SettingRow from './SettingRow.svelte';
	export type AppearanceValues = { palette: PaletteId; theme: SurfaceMode; background: WallpaperId };
	let { values, onchange, showBackground = true, prefix = 'appearance' }: {
		values: AppearanceValues; onchange: (change: Partial<AppearanceValues>) => void;
		showBackground?: boolean; prefix?: string;
	} = $props();
	const groups = PALETTE_FAMILIES.map((family) => ({ ...family, palettes: PALETTES.filter((item) => item.family === family.id) }));
	let selectedPalette = $derived(PALETTES.find((item) => item.id === values.palette) ?? PALETTES[0]);
	function swatch(item: Palette): string {
		const [a, b, c, d] = [item.shader.c1, item.shader.c2, item.shader.c3, item.shader.c4].map(rgbCss);
		return `conic-gradient(from 45deg, ${a} 0 25%, ${b} 0 50%, ${c} 0 75%, ${d} 0)`;
	}
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
<div class="setting-row palette-row" data-setting-id="colour-scheme">
	<div class="setting-label">
		<span>Colour scheme</span>
		<p>{selectedPalette.label}: {selectedPalette.sublabel}</p>
	</div>
	<div class="palette-groups" role="radiogroup" aria-label="Colour scheme">
		{#each groups as group (group.id)}
			<div class="palette-group">
				<span class="palette-family">{group.label}</span>
				<div class="palette-chips">
					{#each group.palettes as item (item.id)}
						<label class="palette-chip" class:chosen={values.palette === item.id} title={item.sublabel}>
							<input type="radio" name={prefix + '-palette'} value={item.id} checked={values.palette === item.id} onchange={() => onchange({ palette: item.id })} />
							<span class="palette-swatch" style:background={swatch(item)} aria-hidden="true"></span>
							<span>{item.label}</span>
						</label>
					{/each}
				</div>
			</div>
		{/each}
	</div>
</div>
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
	.theme-options input, .palette-chip input { position: absolute; opacity: 0; width: 1px; height: 1px; pointer-events: none; }
	.theme-options label.chosen { background: var(--accent-soft); border-color: var(--accent-line); }
	.theme-options label:has(input:focus-visible), .palette-chip:has(input:focus-visible) { outline: 2px solid var(--accent-strong); outline-offset: 3px; }
	/* The scheme grid needs the full row, so its label sits above it. */
	:global(.settings-scope) .palette-row, .palette-row { grid-template-columns: minmax(0, 1fr); gap: var(--space-3); padding-block: var(--space-3); }
	.palette-groups { display: grid; gap: var(--space-3); min-width: 0; }
	.palette-group { display: grid; grid-template-columns: 72px minmax(0, 1fr); gap: var(--space-3); align-items: start; }
	.palette-family { padding-top: var(--space-2); font-size: var(--font-size-label); font-weight: var(--font-weight-semibold); letter-spacing: 0.12em; text-transform: uppercase; color: var(--text-tertiary); }
	.palette-chips { display: flex; flex-wrap: wrap; gap: var(--space-1); }
	.palette-chip { position: relative; display: inline-flex; align-items: center; gap: var(--space-2); min-height: 32px; padding: var(--space-1) var(--space-3) var(--space-1) var(--space-1); border: 1px solid transparent; border-radius: 999px; background: var(--bg-surface); color: var(--text-secondary); font-size: var(--font-size-sm); cursor: pointer; transition: background var(--motion-fast), color var(--motion-fast), border-color var(--motion-fast); }
	.palette-chip:hover { background: var(--bg-hover); color: var(--text-primary); }
	.palette-chip.chosen { background: var(--accent-soft); border-color: var(--accent-line); color: var(--text-primary); font-weight: var(--font-weight-medium); }
	.palette-swatch { width: 22px; height: 22px; flex-shrink: 0; border-radius: 50%; box-shadow: inset 0 0 0 1px var(--border-strong); }
	@media (max-width: 640px) {
		.palette-group { grid-template-columns: minmax(0, 1fr); gap: var(--space-1); }
		.palette-family { padding-top: 0; }
	}
</style>
