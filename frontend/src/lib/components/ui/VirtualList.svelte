<script lang="ts" module>
    export interface WindowArgs {
        scrollTop: number;
        itemHeight: number;
        viewportH: number;
        total: number;
        overscan: number;
    }
    export function computeWindow(a: WindowArgs) {
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
        children: Snippet<[T, number]>;
    }
    let { items, itemHeight, overscan = 5, children }: Props = $props();

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

    function onScroll(e: Event) {
        scrollTop = (e.target as HTMLDivElement).scrollTop;
    }

    let win = $derived(computeWindow({
        scrollTop, itemHeight, viewportH, total: items.length, overscan,
    }));
</script>

<div class="vl-viewport" bind:this={viewport} onscroll={onScroll}>
    <div class="vl-pad" style:height="{win.padTop}px"></div>
    {#each items.slice(win.start, win.end) as item, i (win.start + i)}
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
