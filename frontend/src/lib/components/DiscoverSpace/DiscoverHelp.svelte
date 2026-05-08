<script lang="ts">
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
		onclick={close}
		role="presentation"
	></div>
	<div class="help-panel" role="dialog" aria-label="Sound Space help">
		<div class="panel-head">
			<div class="panel-title">
				<span class="panel-eyebrow">Sound Space</span>
				<h2>How the map works</h2>
			</div>
			<button class="close-btn" onclick={close} aria-label="Close">×</button>
		</div>

		<div class="panel-body">
			<section>
				<h3>What you're looking at</h3>
				<p>
					The map plots up to <strong>100 tracks</strong> in orbit around a
					seed — the song you're playing or a track you've locked. Edges
					are connections; brighter, shorter edges mean stronger ties.
					Click any node for actions, drag to pan, scroll to zoom.
				</p>
			</section>

			<section>
				<h3>Modes (top-left)</h3>
				<ul>
					<li><strong>Near Orbit</strong> — familiar territory, high confidence neighbors.</li>
					<li><strong>Open Current</strong> — balanced mix of known and adjacent.</li>
					<li><strong>Deep Signal</strong> — adventurous, more cold discoveries.</li>
				</ul>
			</section>

			<section>
				<h3>Lenses</h3>
				<ul>
					<li><strong>Energy</strong> — blue is calm, orange is intense.</li>
					<li><strong>Reason</strong> — color shows <em>why</em> a track connects.</li>
					<li><strong>Confidence</strong> — bright = well-supported, dim = cold start.</li>
					<li><strong>Source</strong> — ring color shows where the track came from.</li>
					<li><strong>Genre</strong> — sonic territories grouped by genre family.</li>
				</ul>
			</section>

			<section>
				<h3>Why tracks connect</h3>
				<ul class="reasons">
					<li><span class="dot harmonic"></span><strong>Harmonic</strong> — compatible key / tonal color.</li>
					<li><span class="dot behavioral"></span><strong>Behavioral</strong> — listeners who play one play the other.</li>
					<li><span class="dot bpm"></span><strong>Tempo</strong> — similar BPM.</li>
					<li><span class="dot artist"></span><strong>Artist / Album</strong> — same artist or release lineage.</li>
					<li><span class="dot genre"></span><strong>Genre / Energy</strong> — same family or intensity profile.</li>
					<li><span class="dot external"></span><strong>External</strong> — Last.fm, Discogs, scene graphs.</li>
				</ul>
			</section>

			<section>
				<h3>Limits</h3>
				<ul>
					<li>Each map shows at most <strong>100 nodes</strong> per seed — the graph pruner keeps the strongest edges and drops hairball noise.</li>
					<li>Per-source scores are normalized when a source returns ≥ 5 candidates; smaller sets use raw scores.</li>
					<li>Per-reason hit-rates need ≥ 20 impressions before they count — until then they show as <em>insufficient data</em>.</li>
					<li>Search jump (<em>Jump to…</em>) re-seeds the space around the prompt; it doesn't filter the existing map.</li>
				</ul>
			</section>

			<section>
				<h3>Why a song won't seed</h3>
				<ul>
					<li><strong>No embedding</strong> — the track was added after the model trained, or the audio failed analysis. The neighbor refresher will pick it up on the next pass.</li>
					<li><strong>No neighbors yet</strong> — newly imported tracks have no <code>track_neighbors</code> rows; play it once and the per-seed refresh kicks in.</li>
					<li><strong>Seed not found</strong> — track was deleted, hidden, or its id no longer resolves (rare, usually after a library rescan).</li>
					<li><strong>No model</strong> — if no embedding model has finished training, the engine has nothing to compare against.</li>
				</ul>
				<p class="hint">
					When auto-refresh runs, you'll see a progress pill in the top-right
					(loading embeddings → computing similarity → saving connections).
					The map reloads automatically when it finishes.
				</p>
			</section>

			<section>
				<h3>Tips &amp; tricks</h3>
				<ul>
					<li><strong>Lock the seed</strong> to keep the map fixed while you play through tracks — otherwise it follows what's playing.</li>
					<li><strong>Right-click anything</strong> — every node, row, and inline link has the universal context menu.</li>
					<li><strong>Hold a connection</strong> by hovering an edge — the tooltip explains which signals fired.</li>
					<li><strong>Cold (dim) nodes</strong> are the discovery edge — they're the most likely to surprise you.</li>
					<li><strong>Automix + Discover</strong> seeds the queue from this same graph; turn on <em>Learning</em> to feed your skips/likes back into the next refresh.</li>
					<li><strong>Use the training strip</strong> at the bottom to trigger a manual recompute after a big import or lots of new plays.</li>
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
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: rgba(10, 10, 30, 0.82);
		backdrop-filter: blur(6px);
		color: rgba(255, 255, 255, 0.7);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		transition: background 0.15s, color 0.15s, border-color 0.15s;
	}
	.help-btn:hover {
		background: rgba(124, 128, 255, 0.18);
		color: #fff;
		border-color: rgba(124, 128, 255, 0.5);
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
		background: rgba(14, 14, 26, 0.96);
		border: 1px solid rgba(124, 128, 255, 0.25);
		box-shadow: 0 20px 60px rgba(0, 0, 0, 0.6);
		z-index: 101;
		color: rgba(255, 255, 255, 0.85);
	}

	.panel-head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		padding: 18px 20px 10px;
		border-bottom: 1px solid var(--border-subtle);
	}
	.panel-title { display: flex; flex-direction: column; gap: 2px; }
	.panel-eyebrow {
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: rgba(124, 128, 255, 0.8);
	}
	h2 {
		margin: 0;
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
		color: rgba(255, 255, 255, 0.95);
	}
	.close-btn {
		background: transparent;
		border: none;
		color: rgba(255, 255, 255, 0.5);
		font-size: var(--font-size-xl);
		line-height: 1;
		cursor: pointer;
		padding: 0 6px;
	}
	.close-btn:hover { color: #fff; }

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
		color: rgba(124, 128, 255, 0.95);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}
	.panel-body p {
		margin: 0;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: rgba(255, 255, 255, 0.75);
	}
	.panel-body ul {
		margin: 0;
		padding-left: 18px;
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: rgba(255, 255, 255, 0.72);
	}
	.panel-body strong { color: rgba(255, 255, 255, 0.92); font-weight: var(--font-weight-semibold); }
	.panel-body em { color: rgba(255, 255, 255, 0.55); font-style: normal; }
	.panel-body code {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		padding: 1px 5px;
		border-radius: 4px;
		background: rgba(255, 255, 255, 0.06);
		color: rgba(180, 200, 255, 0.85);
	}
	.hint {
		margin-top: 4px;
		padding: 8px 10px;
		border-radius: 8px;
		background: rgba(124, 128, 255, 0.08);
		border: 1px solid rgba(124, 128, 255, 0.18);
		font-size: var(--font-size-xs) !important;
		color: rgba(200, 200, 255, 0.75) !important;
	}

	ul.reasons { padding-left: 0; list-style: none; }
	ul.reasons li { display: flex; align-items: center; gap: 8px; }
	.dot {
		width: 9px; height: 9px; border-radius: 50%;
		flex-shrink: 0;
		box-shadow: 0 0 6px currentColor;
	}
	.dot.harmonic   { background: rgba(180,160,255,0.9); color: rgba(180,160,255,0.5); }
	.dot.behavioral { background: rgba(100,180,255,0.9); color: rgba(100,180,255,0.5); }
	.dot.bpm        { background: rgba(255,220,80,0.9);  color: rgba(255,220,80,0.5);  }
	.dot.artist     { background: rgba(255,160,80,0.9);  color: rgba(255,160,80,0.5);  }
	.dot.genre      { background: rgba(80,220,120,0.9);  color: rgba(80,220,120,0.5);  }
	.dot.external   { background: rgba(120,100,220,0.9); color: rgba(120,100,220,0.5); }
</style>
