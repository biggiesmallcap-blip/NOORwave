<script lang="ts">
	type Shortcut = {
		keys: string[];
		action: string;
	};

	type ShortcutGroup = {
		id: string;
		title: string;
		shortcuts: Shortcut[];
	};

	let {
		open,
		onClose
	}: {
		open: boolean;
		onClose: () => void;
	} = $props();

	const groups: ShortcutGroup[] = [
		{
			id: 'playback',
			title: 'Playback',
			shortcuts: [
				{ keys: ['Space'], action: 'Play or pause' },
				{ keys: ['Left'], action: 'Seek back 5 seconds' },
				{ keys: ['Right'], action: 'Seek forward 5 seconds' },
				{ keys: ['Shift', 'Left'], action: 'Previous track' },
				{ keys: ['Shift', 'Right'], action: 'Next track' },
				{ keys: ['M'], action: 'Mute or unmute' },
				{ keys: ['L'], action: 'Like the current track' },
				{ keys: ['S'], action: 'Cycle shuffle mode' },
				{ keys: ['R'], action: 'Cycle repeat mode' }
			]
		},
		{
			id: 'navigation',
			title: 'Navigation',
			shortcuts: [
				{ keys: ['Ctrl/Cmd', 'K'], action: 'Open command palette' },
				{ keys: ['Q'], action: 'Expand or collapse the queue' },
				{ keys: ['Up'], action: 'Raise volume' },
				{ keys: ['Down'], action: 'Lower volume' },
				{ keys: ['?'], action: 'Open keyboard shortcuts' },
				{ keys: ['Esc'], action: 'Close this help' }
			]
		},
		{
			id: 'queue-focus',
			title: 'Queue Focus',
			shortcuts: [
				{ keys: ['Up'], action: 'Expand the collapsed queue when focused inside it' },
				{ keys: ['Down'], action: 'Collapse the expanded queue when focused inside it' }
			]
		}
	];

	function handleWindowKeydown(event: KeyboardEvent) {
		if (!open || event.key !== 'Escape') return;
		event.preventDefault();
		onClose();
	}

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) onClose();
	}
</script>

<svelte:window onkeydown={handleWindowKeydown} />

{#if open}
	<div class="shortcut-backdrop" onclick={handleBackdropClick} role="presentation">
		<div
			class="shortcut-panel"
			role="dialog"
			aria-modal="true"
			aria-labelledby="shortcut-help-title"
			tabindex="-1"
		>
			<header class="shortcut-header">
				<div>
					<p class="shortcut-eyebrow">Player</p>
					<h2 id="shortcut-help-title">Keyboard shortcuts</h2>
				</div>
				<button
					type="button"
					class="shortcut-close"
					aria-label="Close keyboard shortcuts"
					title="Close"
					onclick={onClose}
				>
					x
				</button>
			</header>

			<div class="shortcut-groups">
				{#each groups as group (group.title)}
					<section class="shortcut-group" aria-labelledby={`shortcut-group-${group.id}`}>
						<h3 id={`shortcut-group-${group.id}`}>{group.title}</h3>
						<ul>
							{#each group.shortcuts as shortcut (shortcut.action)}
								<li>
									<span class="shortcut-keys">
										{#each shortcut.keys as key, index (key)}
											<kbd>{key}</kbd>
											{#if index < shortcut.keys.length - 1}
												<span class="shortcut-plus">+</span>
											{/if}
										{/each}
									</span>
									<span class="shortcut-action">{shortcut.action}</span>
								</li>
							{/each}
						</ul>
					</section>
				{/each}
			</div>
		</div>
	</div>
{/if}

<style>
	.shortcut-backdrop {
		position: fixed;
		inset: 0;
		z-index: var(--z-modal);
		display: flex;
		align-items: flex-end;
		justify-content: center;
		padding: 24px;
		background: rgba(0, 0, 0, 0.36);
		backdrop-filter: var(--blur-overlay);
		-webkit-backdrop-filter: var(--blur-overlay);
	}

	.shortcut-panel {
		width: min(520px, 100%);
		max-height: min(720px, calc(100vh - 48px));
		overflow: auto;
		padding: 18px;
		border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.1));
		border-radius: 16px;
		background: color-mix(in srgb, var(--bg-surface-strong, #14162a) 95%, transparent);
		box-shadow: 0 24px 64px rgba(0, 0, 0, 0.42);
		color: var(--text-primary, #f8fafc);
	}

	.shortcut-header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
		padding-bottom: 14px;
		border-bottom: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.08));
	}

	.shortcut-eyebrow {
		margin: 0 0 4px;
		font-size: 0.68rem;
		font-weight: 700;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--text-tertiary, rgba(255, 255, 255, 0.5));
	}

	h2,
	h3 {
		margin: 0;
	}

	h2 {
		font-size: 1.1rem;
		line-height: 1.2;
	}

	h3 {
		font-size: 0.78rem;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-secondary, rgba(255, 255, 255, 0.74));
	}

	.shortcut-close {
		width: 34px;
		height: 34px;
		border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.1));
		border-radius: 50%;
		background: rgba(255, 255, 255, 0.06);
		color: var(--text-primary, #fff);
		font-size: 1rem;
		line-height: 1;
		cursor: pointer;
	}

	.shortcut-close:hover {
		background: rgba(255, 255, 255, 0.12);
	}

	.shortcut-groups {
		display: grid;
		gap: 18px;
		padding-top: 16px;
	}

	.shortcut-group ul {
		display: grid;
		gap: 8px;
		padding: 0;
		margin: 10px 0 0;
		list-style: none;
	}

	.shortcut-group li {
		display: grid;
		grid-template-columns: minmax(150px, 0.75fr) 1fr;
		align-items: center;
		gap: 14px;
		min-height: 34px;
	}

	.shortcut-keys {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 5px;
	}

	kbd {
		min-width: 30px;
		padding: 5px 8px;
		border: 1px solid rgba(255, 255, 255, 0.14);
		border-radius: 7px;
		background: rgba(255, 255, 255, 0.08);
		box-shadow: inset 0 -1px 0 rgba(0, 0, 0, 0.2);
		color: var(--text-primary, #fff);
		font-family: inherit;
		font-size: 0.72rem;
		font-weight: 700;
		text-align: center;
		white-space: nowrap;
	}

	.shortcut-plus {
		color: var(--text-tertiary, rgba(255, 255, 255, 0.42));
		font-size: 0.72rem;
		font-weight: 700;
	}

	.shortcut-action {
		color: var(--text-secondary, rgba(255, 255, 255, 0.72));
		font-size: 0.84rem;
		line-height: 1.35;
	}

	@media (max-width: 640px) {
		.shortcut-backdrop {
			padding: 12px;
		}

		.shortcut-panel {
			max-height: calc(100vh - 24px);
			padding: 16px;
			border-radius: var(--radius-md);
		}

		.shortcut-group li {
			grid-template-columns: 1fr;
			gap: 5px;
			padding: 4px 0;
		}
	}
</style>
