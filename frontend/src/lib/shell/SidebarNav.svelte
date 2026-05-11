<script lang="ts">
	import { NAVIGATION_ZONES } from '$lib/routes/navigation';

	let { pathname } = $props<{
		pathname: string;
	}>();

	function isNavItemActive(path: string) {
		if (path === '/') return pathname === '/';
		return pathname === path || pathname.startsWith(`${path}/`);
	}
</script>

<nav class="nav" aria-label="Primary">
	{#each NAVIGATION_ZONES as zone}
		<div class="nav-zone">
			<p class="nav-zone-label">{zone.label}</p>
			{#each zone.items as item}
				<a
					href={item.path}
					class="nav-item"
					class:special={item.id === 'genres'}
					class:active={isNavItemActive(item.path)}
					aria-current={isNavItemActive(item.path) ? 'page' : undefined}
				>
					<span class="nav-icon">{item.icon}</span>
					<span class="nav-label">{item.label}</span>
				</a>
			{/each}
		</div>
	{/each}
</nav>

<style>
	.nav {
		display: flex;
		flex-direction: column;
		gap: 14px;
		padding-top: 4px;
	}

	.nav-zone {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.nav-zone-label {
		padding: 0 10px 5px;
		color: var(--signal-text);
		font-size: var(--font-size-2xs);
		letter-spacing: 0.14em;
		text-transform: uppercase;
		font-weight: var(--font-weight-bold);
	}

	.nav-item {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 9px 10px;
		border-radius: var(--radius-sm);
		color: var(--text-secondary);
		position: relative;
		overflow: hidden;
		border: 1px solid transparent;
		transition:
			background var(--motion-fast),
			color var(--motion-fast),
			border-color var(--motion-fast),
			box-shadow var(--motion-fast);
	}

	.nav-item:hover {
		background: color-mix(in srgb, var(--instrument-surface) 75%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 48%, transparent);
		color: var(--text-primary);
	}

	.nav-item.active {
		background: color-mix(in srgb, var(--accent-soft) 76%, var(--instrument-surface));
		border-color: color-mix(in srgb, var(--accent-line) 72%, transparent);
		color: var(--text-primary);
		box-shadow:
			0 0 0 1px color-mix(in srgb, var(--accent-line) 42%, transparent),
			0 0 24px color-mix(in srgb, var(--accent-glow) 55%, transparent);
	}

	.nav-item.special.active {
		box-shadow:
			0 0 0 1px color-mix(in srgb, var(--accent-line) 52%, transparent),
			0 0 28px color-mix(in srgb, var(--accent-glow) 78%, transparent);
	}

	.nav-item.special.active .nav-icon {
		animation: galaxy-pulse 2.6s ease-in-out infinite;
	}

	.nav-item.active::before {
		content: '';
		position: absolute;
		left: 0;
		top: 7px;
		bottom: 7px;
		width: 2px;
		border-radius: 0 2px 2px 0;
		background: var(--accent);
		box-shadow: 0 0 14px color-mix(in srgb, var(--accent-glow) 88%, transparent);
	}

	.nav-icon {
		width: 18px;
		text-align: center;
		color: var(--text-tertiary);
	}

	.nav-item.active .nav-icon {
		color: var(--accent-strong);
	}

	.nav-label {
		white-space: nowrap;
		letter-spacing: 0.01em;
	}

	@keyframes galaxy-pulse {
		0%, 100% { transform: scale(1); opacity: 0.92; }
		50% { transform: scale(1.1); opacity: 1; }
	}
</style>
