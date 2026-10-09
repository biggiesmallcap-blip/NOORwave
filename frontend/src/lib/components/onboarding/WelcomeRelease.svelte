<script lang="ts">
	// One-time welcome for a milestone release. Shown once per install when the
	// server reports RELEASE or later; the flag lives in localStorage, so a
	// cleared store only means the listener sees it again.
	import { tick } from 'svelte';
	import { goto } from '$app/navigation';
	import ExternalLink from '$lib/components/ui/ExternalLink.svelte';

	const RELEASE = { major: 1, minor: 0, label: '1.0', tag: 'v1.0.0' };
	const SEEN_KEY = 'noor.welcome.v1-0.seen';
	const NOTES_URL = `https://github.com/biggiesmallcap-blip/NOORwave/releases/tag/${RELEASE.tag}`;

	let { enabled, version }: { enabled: boolean; version: string } = $props();

	let visible = $state(false);
	let startButton = $state<HTMLButtonElement | null>(null);

	const HIGHLIGHTS = [
		{
			title: 'One design, everywhere',
			body: 'Every page shares one layout, one title size and one action bar.',
			href: null,
			icon: 'M4 5h16M4 12h10M4 19h16'
		},
		{
			title: 'Search that plays',
			body: 'The top result sits beside the five best songs. Enter plays it.',
			href: '/search',
			icon: 'm20 20-4.5-4.5M17 10.5a6.5 6.5 0 1 1-13 0 6.5 6.5 0 0 1 13 0Z'
		},
		{
			title: 'Videos remember',
			body: 'Recently watched, a History tab, and skips that teach your stations.',
			href: '/videos',
			icon: 'M4 6h12v12H4zM16 10l4-2.5v9L16 14'
		},
		{
			title: 'One Mix page',
			body: 'Automix and DJ transitions live together now, under Tools.',
			href: '/mix',
			icon: 'M3 17c3 0 4-10 7-10s3 10 6 10 3-6 5-6'
		}
	];

	function reached(v: string): boolean {
		const m = /^v?(\d+)\.(\d+)/.exec(v.trim());
		if (!m) return false;
		const [major, minor] = [Number(m[1]), Number(m[2])];
		return major > RELEASE.major || (major === RELEASE.major && minor >= RELEASE.minor);
	}

	function seen(): boolean {
		try {
			return localStorage.getItem(SEEN_KEY) === '1';
		} catch {
			return true;
		}
	}

	function markSeen() {
		try {
			localStorage.setItem(SEEN_KEY, '1');
		} catch {
			// A full store only means the welcome may show again next launch.
		}
	}

	$effect(() => {
		if (visible || !enabled || !reached(version) || seen()) return;
		// Never stack on another dialog (patch notes, discovery setup).
		if (document.querySelector('[aria-modal="true"], .modal-backdrop, .patch-info-backdrop')) return;
		visible = true;
		markSeen();
		void tick().then(() => startButton?.focus());
	});

	function close() {
		visible = false;
	}

	function open(href: string) {
		close();
		void goto(href);
	}

	function onKeydown(event: KeyboardEvent) {
		if (visible && event.key === 'Escape') {
			event.preventDefault();
			close();
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

{#if visible}
	<div class="welcome-backdrop">
		<button type="button" class="welcome-dismiss" aria-label="Close" tabindex="-1" onclick={close}></button>
		<div
			class="welcome-dialog"
			role="dialog"
			aria-modal="true"
			aria-labelledby="welcome-title"
			aria-describedby="welcome-summary"
		>
			<div class="welcome-hero">
				<img class="welcome-mark on-dark" src="/noor-logo-centered-transparent.svg" alt="" />
				<img class="welcome-mark on-light" src="/noor-logo-centered-transparent-dark.svg" alt="" />
				<h2 id="welcome-title" class="welcome-version">
					<span class="visually-hidden">Welcome to NOORwave </span>{RELEASE.label.split('.')[0]}<span class="dot">.</span>{RELEASE.label.split('.')[1]}
				</h2>
				<p class="welcome-label">The hundredth release</p>
				<p id="welcome-summary" class="welcome-summary">
					Six months, a hundred releases, and one design across the whole app. Thank you for listening along.
				</p>
			</div>

			<ul class="welcome-highlights">
				{#each HIGHLIGHTS as item, i (item.title)}
					<li style:--rise-index={i}>
						{#if item.href}
							<button type="button" class="welcome-item" onclick={() => open(item.href!)}>
								{@render highlight(item)}
							</button>
						{:else}
							<div class="welcome-item static">{@render highlight(item)}</div>
						{/if}
					</li>
				{/each}
			</ul>

			<footer class="welcome-actions">
				<ExternalLink href={NOTES_URL}>Read the release notes</ExternalLink>
				<button type="button" class="btn btn-primary" bind:this={startButton} onclick={close}>Start listening</button>
			</footer>
		</div>
	</div>
{/if}

{#snippet highlight(item: { title: string; body: string; icon: string })}
	<span class="welcome-icon" aria-hidden="true">
		<svg viewBox="0 0 24 24" focusable="false"><path d={item.icon} /></svg>
	</span>
	<span class="welcome-copy">
		<span class="t-row-title">{item.title}</span>
		<span class="welcome-body">{item.body}</span>
	</span>
{/snippet}

<style>
	.welcome-backdrop {
		position: fixed;
		inset: 0;
		z-index: var(--z-modal);
		display: grid;
		place-items: center;
		padding: var(--space-5);
		background: rgba(0, 0, 0, 0.62);
		backdrop-filter: var(--blur-overlay);
		animation: welcome-fade var(--motion-base) both;
	}

	.welcome-dismiss {
		position: absolute;
		inset: 0;
		border: 0;
		background: transparent;
		cursor: default;
	}

	.welcome-dialog {
		position: relative;
		width: min(100%, 620px);
		max-height: calc(100svh - 2 * var(--space-5));
		overflow: auto;
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
		padding: var(--space-6) var(--space-6) var(--space-5);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-lg);
		background:
			radial-gradient(120% 70% at 50% 0%, var(--accent-soft), transparent 70%),
			color-mix(in srgb, var(--bg-elevated) 96%, transparent);
		backdrop-filter: var(--blur-modal);
		box-shadow: var(--panel-shadow);
		animation: welcome-rise var(--motion-slow) both;
	}

	.welcome-hero {
		display: grid;
		justify-items: center;
		text-align: center;
		gap: var(--space-2);
	}

	.welcome-mark {
		width: min(300px, 70%);
		height: auto;
	}
	.on-light {
		display: none;
	}
	:global([data-theme='light']) .welcome-dialog .on-light {
		display: block;
	}
	:global([data-theme='light']) .welcome-dialog .on-dark {
		display: none;
	}

	.welcome-version {
		margin: 0;
		font-family: var(--font-body);
		font-weight: 800; /* the milestone number is the one display weight here */
		font-size: calc(var(--font-size-4xl) * 1.9);
		line-height: 1;
		letter-spacing: -0.04em;
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
	}
	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}
	.welcome-version .dot {
		color: var(--accent);
	}

	.welcome-label {
		margin: 0;
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--accent-strong);
	}

	.welcome-summary {
		margin: var(--space-1) 0 0;
		max-width: 44ch;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: var(--text-secondary);
	}

	.welcome-highlights {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-2);
	}
	.welcome-highlights li {
		animation: welcome-rise var(--motion-slow) both;
		animation-delay: calc(120ms + var(--rise-index) * 60ms);
	}

	.welcome-item {
		width: 100%;
		height: 100%;
		display: flex;
		gap: var(--space-3);
		align-items: flex-start;
		padding: var(--space-3);
		border: 0;
		border-radius: var(--radius-sm);
		background: var(--bg-surface);
		color: inherit;
		font: inherit;
		text-align: left;
		transition: background var(--motion-fast);
	}
	button.welcome-item {
		cursor: pointer;
	}
	button.welcome-item:hover {
		background: var(--bg-hover);
	}
	button.welcome-item:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.welcome-icon {
		flex: none;
		display: grid;
		place-items: center;
		width: 32px;
		height: 32px;
		border-radius: 50%;
		background: var(--accent-soft);
		color: var(--accent-strong);
	}
	.welcome-icon svg {
		width: 18px;
		height: 18px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.welcome-copy {
		display: grid;
		gap: 2px;
		min-width: 0;
	}
	.welcome-copy .t-row-title {
		white-space: normal;
	}
	.welcome-body {
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
		color: var(--text-secondary);
	}

	.welcome-actions {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		flex-wrap: wrap;
	}

	@keyframes welcome-fade {
		from {
			opacity: 0;
		}
	}
	@keyframes welcome-rise {
		from {
			opacity: 0;
			transform: translateY(12px);
		}
	}

	@media (max-width: 560px) {
		.welcome-dialog {
			padding: var(--space-5) var(--space-4) var(--space-4);
		}
		.welcome-highlights {
			grid-template-columns: 1fr;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.welcome-dialog,
		.welcome-highlights li {
			animation: welcome-fade var(--motion-fast) both;
		}
	}
	:global([data-motion='reduce']) .welcome-dialog,
	:global([data-motion='reduce']) .welcome-highlights li {
		animation: welcome-fade var(--motion-fast) both;
	}
</style>
