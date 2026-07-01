<script lang="ts">
	// Shared on/off switch. Extracted from the settings page so every toggle
	// shares one accessible, themeable control instead of duplicated markup and
	// CSS. Controlled input: the parent owns `checked` and updates it from
	// `onchange` (matching the existing settings handlers that read
	// `e.target.checked`).
	interface Props {
		checked: boolean;
		onchange?: (event: Event & { currentTarget: EventTarget & HTMLInputElement }) => void;
		disabled?: boolean;
		/** Accessible name for the switch; sets aria-label on the checkbox. */
		label?: string;
	}

	let { checked, onchange, disabled = false, label }: Props = $props();
</script>

<label class="toggle-switch">
	<input type="checkbox" {checked} {disabled} aria-label={label} {onchange} />
	<span class="toggle-slider"></span>
</label>

<style>
	.toggle-switch {
		position: relative;
		display: inline-block;
		width: 44px;
		height: 24px;
		cursor: pointer;
	}

	.toggle-switch input {
		opacity: 0;
		width: 0;
		height: 0;
	}

	.toggle-slider {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		bottom: 0;
		background: rgba(255, 255, 255, 0.12);
		border-radius: 999px;
		transition: background var(--motion-base);
	}

	.toggle-slider::before {
		content: '';
		position: absolute;
		height: 18px;
		width: 18px;
		left: 3px;
		bottom: 3px;
		background: white;
		border-radius: 50%;
		transition: transform var(--motion-base);
	}

	.toggle-switch input:checked + .toggle-slider {
		background: var(--accent);
	}

	.toggle-switch input:checked + .toggle-slider::before {
		transform: translateX(20px);
	}

	.toggle-switch input:disabled + .toggle-slider {
		opacity: 0.5;
		cursor: not-allowed;
	}
</style>
