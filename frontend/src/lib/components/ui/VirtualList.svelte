<script lang="ts" module>
    export interface WindowArgs {
        scrollTop: number;
        itemHeight: number;
        viewportH: number;
        total: number;
        overscan: number;
    }
    export function computeWindow(a: WindowArgs) {
        if (a.itemHeight <= 0 || a.total <= 0) {
            return { start: 0, end: 0, padTop: 0, padBottom: 0 };
        }
        const visible = Math.ceil(a.viewportH / a.itemHeight);
        const firstVisible = Math.max(0, Math.floor(a.scrollTop / a.itemHeight));
        const start = Math.max(0, firstVisible - a.overscan);
        const end = Math.min(a.total, firstVisible + visible + a.overscan);
        return {
            start,
            end,
            padTop: start * a.itemHeight,
            padBottom: Math.max(0, (a.total - end) * a.itemHeight),
        };
    }
</script>

<script lang="ts" generics="T">
    import { onMount } from 'svelte';
    import type { Snippet } from 'svelte';

    interface Props {
        items: T[];
        itemHeight: number;
        overscan?: number;
        /**
         * Optional key extractor. If provided, used as the `{#each}` key so DOM
         * nodes are reused as the window slides — preserving selection, focus,
         * and any per-row state. If omitted, items are keyed by their position
         * in the slice (Svelte default), which is fine for purely visual lists.
         */
        key?: (item: T) => string | number;
        children: Snippet<[T, number]>;
    }
    let { items, itemHeight, overscan = 5, key, children }: Props = $props();

    let viewport: HTMLDivElement | null = $state(null);
    let scrollTop = $state(0);
    let viewportH = $state(0);

    onMount(() => {
        if (!viewport) return;
        viewportH = viewport.clientHeight;
        const ro = new ResizeObserver(() => {
            if (viewport) viewportH = viewport.clientHeight;
        });
        ro.observe(viewport);
        return () => ro.disconnect();
    });

    // TODO(perf-phase-4): rAF-throttle this to coalesce wheel/touch bursts.
    function onScroll(e: Event) {
        scrollTop = (e.target as HTMLDivElement).scrollTop;
    }

    let win = $derived(computeWindow({
        scrollTop, itemHeight, viewportH, total: items.length, overscan,
    }));
</script>

<div class="vl-viewport" bind:this={viewport} onscroll={onScroll}>
    <div class="vl-pad" style:height="{win.padTop}px"></div>
    {#each items.slice(win.start, win.end) as item, i (key ? key(item) : win.start + i)}
        <div class="vl-item" style:height="{itemHeight}px">
            {@render children(item, win.start + i)}
        </div>
    {/each}
    <div class="vl-pad" style:height="{win.padBottom}px"></div>
</div>

<style>
    .vl-viewport {
        height: 100%;
        overflow-y: auto;
        contain: strict;
    }
    .vl-item {
        contain: layout style paint;
    }
</style>
