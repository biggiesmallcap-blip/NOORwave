<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type TidalMoodCategory } from '$lib/api/client';
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { goto } from '$app/navigation';

	const PREVIEW_LIMIT = 8;

	let categories = $state<TidalMoodCategory[]>([]);
	let loaded = $state(false);
	let errored = $state(false);

	onMount(async () => {
		try {
			const data = await api.getTidalMoods();
			categories = (data.categories ?? []).slice(0, PREVIEW_LIMIT);
			loaded = true;
		} catch {
			errored = true;
		}
	});

	function menu(slug: string, title: string) {
		return [{ label: `Open ${title}`, onSelect: () => void goto(`/moods/${slug}`) }];
	}
</script>

{#if categories.length > 0}
	<section class="moods-rail" data-section="moods">
		<div class="header">
			<div class="title-group">
				<p class="eyebrow">TIDAL</p>
				<h2>Moods &amp; Activities</h2>
			</div>
			<a class="view-all" href="/moods">View all -&gt;</a>
		</div>
		<div class="rail" use:wheelToHorizontal>
			{#each categories as c (c.slug)}
				<a
					class="card"
					href={`/moods/${c.slug}`}
					oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, menu(c.slug, c.title), c.title); }}
				>
					<div class="art-wrap">
						{#if c.thumbnail}
							<div class="art" style="background-image:url('{c.thumbnail}')"></div>
						{:else}
							<div class="art fallback">~</div>
						{/if}
					</div>
					<p class="card-title">{c.title}</p>
				</a>
			{/each}
		</div>
	</section>
{:else if loaded && !errored}
	<!-- Empty list (e.g. TIDAL disconnected): hide the rail entirely. -->
{/if}

<style>
	.moods-rail { display: flex; flex-direction: column; gap: var(--gap); }
	.header { display: flex; align-items: center; justify-content: space-between; gap: var(--gap); }
	.title-group { display: flex; flex-direction: column; gap: var(--space-1); }
	.title-group h2 { font-size: var(--font-size-lg); font-weight: var(--font-weight-bold); margin: 0; }
	.eyebrow { font-size: var(--font-size-xs); letter-spacing: 0.08em; text-transform: uppercase; color: var(--text-secondary); margin: 0; font-weight: var(--font-weight-bold); }
	.view-all { font-size: var(--font-size-xs); font-weight: var(--font-weight-semibold); color: var(--text-secondary); text-decoration: none; transition: color var(--motion-fast) ease; }
	.view-all:hover, .view-all:focus-visible { color: var(--text-primary); outline: none; }
	.rail {
		display: flex;
		gap: var(--gap-sm);
		overflow-x: auto;
		padding-bottom: var(--space-2);
		scroll-snap-type: x mandatory;
		mask-image: linear-gradient(to right, transparent 0, black 16px, black calc(100% - 32px), transparent 100%);
		-webkit-mask-image: linear-gradient(to right, transparent 0, black 16px, black calc(100% - 32px), transparent 100%);
	}
	.rail::-webkit-scrollbar { height: 6px; }
	.rail::-webkit-scrollbar-track { background: var(--bg-surface); border-radius: var(--radius-xs); }
	.rail::-webkit-scrollbar-thumb { background: var(--border-subtle); border-radius: var(--radius-xs); }
	.rail::-webkit-scrollbar-thumb:hover { background: var(--text-muted); }
	.card {
		flex: 0 0 180px;
		width: 180px;
		min-width: 180px;
		max-width: 180px;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		background: none;
		border: 1px solid transparent;
		padding: var(--space-2);
		border-radius: var(--radius-md);
		text-decoration: none;
		color: inherit;
		cursor: pointer;
		transition: background var(--motion-base) ease, border-color var(--motion-base) ease;
		box-sizing: border-box;
		scroll-snap-align: start;
	}
	.card:hover, .card:focus-visible { background: var(--bg-hover); border-color: var(--panel-border); outline: none; }
	.card:focus-visible { border-color: var(--accent-line); }
	.art-wrap { position: relative; aspect-ratio: 1 / 1; width: 100%; border-radius: var(--radius-sm); overflow: hidden; background: var(--bg-hover); }
	.art { width: 100%; height: 100%; background-size: cover; background-position: center; transition: transform var(--motion-base) ease; }
	.card:hover .art { transform: scale(1.05); }
	.art.fallback { display: flex; align-items: center; justify-content: center; font-size: var(--font-size-4xl); color: var(--text-muted); }
	.card-title { margin: 0; font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; line-height: var(--line-height-snug); }
</style>
