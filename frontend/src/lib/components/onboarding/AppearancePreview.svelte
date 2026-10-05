<script lang="ts">
	import { onMount } from 'svelte';
	import type { AppearanceValues } from '$lib/components/settings/AppearanceFields.svelte';
	import { applyPaletteTheme } from '$lib/components/wallpaper/paletteTheme';
	import { resolveSurfaceMode } from '$lib/stores/surfaceMode';
	import { wallpaperById } from '$lib/components/wallpaper/shaders';
	import ShaderWallpaper from '$lib/components/wallpaper/ShaderWallpaper.svelte';
	let { values }: { values: AppearanceValues } = $props();
	let frame = $state<HTMLDivElement>();
	let systemLight = $state(false);
	let mode = $derived(resolveSurfaceMode(values.theme, systemLight));
	let background = $derived(wallpaperById(values.background));
	onMount(() => {
		const media = matchMedia('(prefers-color-scheme: light)');
		const update = () => systemLight = media.matches;
		update(); media.addEventListener('change', update);
		return () => media.removeEventListener('change', update);
	});
	$effect(() => { if (frame) applyPaletteTheme(frame, values.palette, mode); });
</script>
<div bind:this={frame} class="appearance-preview" data-theme={mode} aria-label="App appearance preview">
	{#if background.shader}<div class="preview-background"><ShaderWallpaper shader={background.shader} paletteOverride={values.palette} colorSourceOverride="palette" maxDpr={1} targetFps={24} /></div>{/if}
	<div class="preview-surface"><span class="preview-sidebar">NOORwave</span><div><strong>Your library</strong><p>Your music, your look.</p><span class="preview-progress"></span><button class="btn btn-primary" type="button" disabled>Play</button></div></div>
</div>
<style>
	.appearance-preview { position: relative; isolation: isolate; overflow: hidden; min-height: 160px; padding: var(--space-4); width: 100%; background: var(--bg-base); color: var(--text-primary); border-radius: var(--radius-md); }
	.preview-background { position: absolute; inset: 0; z-index: -1; }
	.preview-surface { display: flex; gap: var(--space-5); text-align: left; padding: var(--space-3); background: var(--panel-bg); border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); }
	.preview-sidebar { font-weight: var(--font-weight-semibold); color: var(--accent-strong); }
	p { font-size: var(--font-size-sm); color: var(--text-secondary); margin: var(--space-2) 0; }
	.preview-progress { display: block; width: 80px; height: 3px; background: var(--accent); margin-bottom: var(--space-3); }
</style>
