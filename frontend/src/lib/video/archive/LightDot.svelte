<script lang="ts">
	import type { Light } from '$lib/api/video_explore_types';
	import { LIGHT_HINTS, LIGHT_NAMES } from './versions';

	// Light is knowledge, in one dot: lit (watched, liked, saved) glows in the
	// accent, warm is a softer accent, dim is a quiet grey, uncharted is an
	// empty ring. Same vocabulary as the Atlas.
	let { light }: { light: Light } = $props();
</script>

<span class="light-dot" data-light={LIGHT_NAMES[light]} title={LIGHT_HINTS[light]} role="img" aria-label={LIGHT_HINTS[light]}></span>

<style>
	.light-dot {
		flex: 0 0 auto;
		display: inline-block;
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--text-tertiary);
		opacity: 0.55;
	}

	.light-dot[data-light='lit'] {
		background: var(--accent);
		opacity: 1;
		box-shadow: 0 0 0 2px var(--accent-soft), 0 0 8px var(--accent-glow);
	}

	.light-dot[data-light='warm'] {
		background: color-mix(in srgb, var(--accent) 60%, var(--text-tertiary));
		opacity: 0.9;
	}

	.light-dot[data-light='uncharted'] {
		background: transparent;
		box-shadow: inset 0 0 0 1.5px var(--text-tertiary);
	}
</style>
