<script lang="ts">
	/**
	 * The error heatmap page: every control (or a chosen set) down the side, frequency across,
	 * each cell the control's error against its reference; the overview that shows a 72-control
	 * test converging at a glance. A row opens its control in the matrix grid. When the service
	 * offers the coherence error per control, a second mode shows it the same way.
	 */
	import type { MimoPageSpec } from '$lib/acquire/display/mimoLayout';
	import type { HeatmapView } from '$lib/protocol/types';
	import Heatmap from '$lib/viz/Heatmap.svelte';
	import { summaries } from '$lib/viz/heatmap';
	import type { RowBand } from '$lib/viz/scales';
	import type { MimoViewsState } from './mimoViews.svelte';
	import { coherenceModeOffered, heatmapSettings, plotId } from './pages';

	interface Props {
		views: MimoViewsState;
		page: MimoPageSpec;
	}
	let { views, page }: Props = $props();

	const s = $derived(heatmapSettings(page));
	const offered = $derived(coherenceModeOffered(views.quantities));
	const mode = $derived(s.mode === 'coherence' && offered ? 'coherence' : 'error');
	const view = $derived((views.plotData(plotId(page, 'heat'))?.views[0] as HeatmapView | undefined) ?? null);
	const worstCount = $derived(view ? countBeyond(view) : null);

	function countBeyond(v: HeatmapView): { alarm: number; abort: number } {
		const sums = summaries({ rows: v.rows.ticks.length, cols: v.x_values.length, values: v.values, bands: v.row_bands as RowBand[] }, 'error');
		return { alarm: sums.filter((x) => x.alarm > 0).length, abort: sums.filter((x) => x.abort > 0).length };
	}

	let box = $state<HTMLDivElement | null>(null);
	$effect(() => {
		if (!box) return;
		const el = box;
		const measure = () => views.setWidth(page.id, 'main', Math.max(64, el.clientWidth - 92 - 132 - 18));
		const obs = new ResizeObserver(measure);
		obs.observe(el);
		measure();
		return () => obs.disconnect();
	});

	function openRow(_row: number, label: string) {
		const pos = views.labels.indexOf(label);
		if (pos >= 0) views.openInGrid(pos);
	}
</script>

<div class="heatpage" data-testid="mimo-heatmap-page">
	<div class="tools" role="toolbar" aria-label="Error heatmap">
		{#if offered}
			<div class="seg" role="radiogroup" aria-label="What the map shows">
				<button role="radio" aria-checked={mode === 'error'} class:on={mode === 'error'} onclick={() => views.patch(page.id, { mode: 'error' })}>Spectrum Error</button>
				<button role="radio" aria-checked={mode === 'coherence'} class:on={mode === 'coherence'} onclick={() => views.patch(page.id, { mode: 'coherence' })}>Coherence Error</button>
			</div>
		{/if}
		<div class="seg" role="radiogroup" aria-label="Row order">
			<button role="radio" aria-checked={s.order === 'setup'} class:on={s.order === 'setup'} data-testid="heat-order-setup" onclick={() => views.patch(page.id, { order: 'setup' })}>Setup Order</button>
			<button role="radio" aria-checked={s.order === 'worst'} class:on={s.order === 'worst'} data-testid="heat-order-worst" onclick={() => views.patch(page.id, { order: 'worst' })}>Largest Error First</button>
		</div>
		{#if worstCount && mode === 'error'}
			<span class="counts num" data-testid="heat-counts">
				<span><b>{worstCount.alarm}</b> of {view?.rows.ticks.length} controls with lines beyond alarm</span>
				<span><b>{worstCount.abort}</b> beyond abort</span>
			</span>
		{/if}
	</div>
	<div class="body" bind:this={box}>
		<Heatmap {view} {mode} order={s.order} onopen={openRow} empty={views.labels.length ? 'Waiting for the first loop.' : 'Waiting for the run.'} />
	</div>
</div>

<style>
	.heatpage {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		height: 100%;
		min-height: 0;
	}

	.tools {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-4);
	}

	.seg {
		display: inline-flex;
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.seg button {
		padding: 4px 10px;
		font-size: var(--text-xs);
		color: var(--text-2);
		background: transparent;
		border: 0;
		cursor: pointer;
	}

	.seg button.on {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		background: var(--accent-weak);
		box-shadow: inset 0 -2px 0 var(--accent);
	}

	.counts {
		display: inline-flex;
		gap: var(--sp-4);
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.counts b {
		color: var(--text-1);
	}

	.body {
		flex: 1;
		min-height: 320px;
	}
</style>
