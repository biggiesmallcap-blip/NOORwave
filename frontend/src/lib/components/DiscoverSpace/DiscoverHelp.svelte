<script lang="ts">
	import { EDGE_COLORS } from './discover_space_renderer';
	import { portal } from '$lib/actions/portal';
	let open = $state(false);

	function close() { open = false; }
	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') close();
	}
</script>

<svelte:window onkeydown={onKey} />

<button
	class="help-btn"
	onclick={() => (open = !open)}
	aria-label="Sound Space help"
	title="Sound Space help"
>
	?
</button>

{#if open}
	<div
		class="help-backdrop"
		use:portal
		onclick={close}
		role="presentation"
	></div>
	<div class="help-panel" use:portal role="dialog" aria-label="Sound Space help">
		<div class="panel-head">
			<div class="panel-title">
				<h2>How the map works</h2>
			</div>
			<button class="close-btn" onclick={close} aria-label="Close">×</button>
		</div>

		<div class="panel-body">
			<section>
				<h3>What you're looking at</h3>
				<p>
					Up to <strong>100 tracks</strong> around an anchor: the song you're
					playing, or one you've locked. Hover a star to inspect it, click it for
					actions, right-click for the usual menu. Drag to pan, scroll to zoom,
					<em>Fit</em> frames the whole space again.
				</p>
			</section>

			<section>
				<h3>Reading the map</h3>
				<ul>
					<li><strong>Distance</strong> is rank: the closer a star sits to the anchor, the higher it ranks. It is the same order as the results list, and the dotted rings mark the top 25%, 50% and 75%.</li>
					<li><strong>Direction</strong> is genre family: each family gets its own slice of the circle. Tracks with no genre yet share the last slice.</li>
					<li><strong>Size</strong> is confidence: bigger stars rest on more evidence.</li>
					<li><strong>Color</strong> follows the lens you pick. Grey always means "no data for this lens yet", never a guess.</li>
				</ul>
			</section>

			<section>
				<h3>Familiar to Adventurous</h3>
				<p>
					The slider on the map trades closeness for discovery. Familiar keeps to
					well-supported neighbors; Adventurous reaches further and lets more cold
					discoveries in. Filters narrow by tempo, energy, era, key and what you
					already own.
				</p>
			</section>

			<section>
				<h3>Lenses</h3>
				<ul>
					<li><strong>Energy</strong>: blue is calm, orange is intense.</li>
					<li><strong>Reason</strong>: why a track connects (below).</li>
					<li><strong>Confidence</strong>: bright is well supported, dim is a cold start.</li>
					<li><strong>Source</strong>: your library, Last.fm, or the discovery engine.</li>
					<li><strong>Genre</strong>: the genre family, matching the direction.</li>
				</ul>
			</section>

			<section>
				<h3>Why tracks connect</h3>
				<ul class="reasons">
					<li><span class="dot" style:background={EDGE_COLORS.similar}></span><strong>Sounds similar</strong>: your listening model places them close together.</li>
					<li><span class="dot" style:background={EDGE_COLORS.behavioral}></span><strong>Listening pattern</strong>: listeners who play one play the other.</li>
					<li><span class="dot" style:background={EDGE_COLORS.harmonic}></span><strong>Harmonic</strong>: compatible key or tonal color.</li>
					<li><span class="dot" style:background={EDGE_COLORS.bpm}></span><strong>Tempo</strong>: a similar BPM.</li>
					<li><span class="dot" style:background={EDGE_COLORS.artist}></span><strong>Artist / Album</strong>: the same artist or release.</li>
					<li><span class="dot" style:background={EDGE_COLORS.genre}></span><strong>Genre</strong>: the same genre family.</li>
					<li><span class="dot" style:background={EDGE_COLORS.energy}></span><strong>Energy</strong>: a similar intensity.</li>
					<li><span class="dot" style:background={EDGE_COLORS.external}></span><strong>External</strong>: matched by Last.fm or TIDAL. Their link to the anchor is drawn faintly, since it only says "matched", not how.</li>
				</ul>
			</section>

			<section>
				<h3>New tracks</h3>
				<p>
					A track added since the listening model last trained has no neighbors of
					its own yet. Until it does, its map borrows from its artist, its genre and
					the tracks that sound closest to it, alongside Last.fm.
				</p>
				<p class="hint">
					While neighbors refresh you'll see a progress pill at the bottom left of
					the map. It reloads automatically when it finishes.
				</p>
			</section>

			<section>
				<h3>Tips</h3>
				<ul>
					<li><strong>Lock the seed</strong> to keep the map while you play through tracks; otherwise it follows what's playing.</li>
					<li><strong>Branch here</strong> re-centers the map on a star and remembers the path, so you can walk back.</li>
					<li><strong>More like this / less like this</strong> in the results list re-rank the map for this session; stars move to their new rank.</li>
					<li><strong>Jump to</strong> re-seeds the space from a description instead of a track.</li>
				</ul>
			</section>
		</div>
	</div>
{/if}

<style>
	.help-btn {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		border: 1px solid var(--border-muted);
		background: var(--bg-surface);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		transition: background var(--motion-fast), color var(--motion-fast), border-color var(--motion-fast);
	}
	.help-btn:hover {
		background: var(--accent-soft);
		color: var(--text-primary);
		border-color: var(--accent-line);
	}

	.help-backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		backdrop-filter: blur(2px);
		z-index: 100;
	}
	.help-panel {
		position: fixed;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		width: min(640px, 90vw);
		max-height: 85vh;
		display: flex;
		flex-direction: column;
		border-radius: var(--radius-md);
		background: var(--bg-surface-strong);
		border: 1px solid var(--panel-border);
		box-shadow: var(--panel-shadow);
		z-index: 101;
		color: var(--text-primary);
	}

	.panel-head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		padding: 18px 20px 10px;
		border-bottom: 1px solid var(--border-subtle);
	}
	.panel-title { display: flex; flex-direction: column; gap: 2px; }
	h2 {
		margin: 0;
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
	}
	.close-btn {
		background: transparent;
		border: none;
		color: var(--text-tertiary);
		font-size: var(--font-size-xl);
		line-height: 1;
		cursor: pointer;
		padding: 0 6px;
	}
	.close-btn:hover { color: var(--text-primary); }

	.panel-body {
		overflow-y: auto;
		padding: 14px 20px 20px;
		display: flex;
		flex-direction: column;
		gap: 18px;
	}
	.panel-body section { display: flex; flex-direction: column; gap: 6px; }
	h3 {
		margin: 0;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: var(--accent-strong);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}
	.panel-body p {
		margin: 0;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: var(--text-secondary);
	}
	.panel-body ul {
		margin: 0;
		padding-left: 18px;
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: var(--text-secondary);
	}
	.panel-body strong { color: var(--text-primary); font-weight: var(--font-weight-semibold); }
	.panel-body em { color: var(--text-secondary); font-style: normal; }
	.hint {
		margin-top: 4px;
		padding: 8px 10px;
		border-radius: 8px;
		background: var(--accent-soft);
		border: 1px solid var(--accent-line);
		font-size: var(--font-size-xs) !important;
		color: var(--accent-strong) !important;
	}

	ul.reasons { padding-left: 0; list-style: none; }
	ul.reasons li { display: flex; align-items: center; gap: 8px; }
	.dot {
		width: 9px; height: 9px; border-radius: 50%;
		flex-shrink: 0;
	}
</style>
