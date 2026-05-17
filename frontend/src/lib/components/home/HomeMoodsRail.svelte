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
					{#if c.thumbnail}
						<div class="art" style="background-image:url('{c.thumbnail}')"></div>
					{:else}
						<div class="art fallback">~</div>
					{/if}
					<span class="card-title">{c.title}</span>
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
	.rail { display: flex; gap: var(--gap-sm); overflow-x: auto; padding-bottom: var(--space-2); scroll-snap-type: x mandatory; }
	.rail::-webkit-scrollbar { height: 6px; }
	.rail::-webkit-scrollbar-track { background: var(--bg-surface); border-radius: var(--radius-xs); }
	.rail::-webkit-scrollbar-thumb { background: var(--border-subtle); border-radius: var(--radius-xs); }
	.card { --card-w: clamp(120px, 11vw, 168px); flex: 0 0 var(--card-w); width: var(--card-w); display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-2); border-radius: var(--radius-md); text-decoration: none; color: inherit; cursor: pointer; transition: background var(--motion-fast) ease; scroll-snap-align: start; }
	.card:hover, .card:focus-visible { background: var(--bg-hover); outline: none; }
	.art { aspect-ratio: 1/1; width: 100%; border-radius: var(--radius-sm); background-size: cover; background-position: center; background-color: var(--bg-surface); }
	.art.fallback { display: flex; align-items: center; justify-content: center; color: #fff; font-size: var(--font-size-3xl); background: linear-gradient(135deg, #3b82f6, #8b5cf6); }
	.card-title { font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; text-align: center; }
</style>
