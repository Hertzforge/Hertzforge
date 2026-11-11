<script lang="ts">
	/**
	 * An error heatmap plot: every control (or a chosen set) down the side, frequency across, each
	 * cell the control's error against its reference; the overview that shows a 72-control test
	 * converging at a glance. A row opens its control in the matrix grid. When the service offers the
	 * coherence error per control, the plot's options switch the map to it. The scale is ordered by
	 * lightness, cells beyond abort are hatched and each row carries a mark and a count, so nothing
	 * rests on color.
	 */
	import type { HeatmapView } from '$lib/protocol/types';
	import Heatmap from '$lib/viz/Heatmap.svelte';
	import { summaries } from '$lib/viz/heatmap';
	import type { RowBand } from '$lib/viz/scales';
	import type { BodyPanel } from '../../screen/bodies';
	import { coherenceModeOffered, heatmapChoice } from './mimoScreen';
	import type { MimoScreenState } from './mimoScreenState.svelte';

	interface Props {
		screen: MimoScreenState;
		panel: BodyPanel;
	}
	let { screen, panel }: Props = $props();

	const s = $derived(heatmapChoice(panel.plot.select));
	const mode = $derived(s.mode === 'coherence' && coherenceModeOffered(screen.quantities) ? 'coherence' : 'error');
	const view = $derived((screen.plotData(panel.id)?.views[0] as HeatmapView | undefined) ?? null);
	const beyond = $derived(view && mode === 'error' ? countBeyond(view) : null);

	function countBeyond(v: HeatmapView): { alarm: number; abort: number } {
		const sums = summaries({ rows: v.rows.ticks.length, cols: v.x_values.length, values: v.values, bands: v.row_bands as RowBand[] }, 'error');
		return { alarm: sums.filter((x) => x.alarm > 0).length, abort: sums.filter((x) => x.abort > 0).length };
	}

	let box = $state<HTMLDivElement | null>(null);
	$effect(() => {
		if (!box || typeof ResizeObserver === 'undefined') return;
		const el = box;
		// the map's columns: the body less its row names and its rms column
		const measure = () => screen.setWidth(panel.id, 'main', Math.max(64, el.clientWidth - 92 - 132 - 18));
		const obs = new ResizeObserver(measure);
		obs.observe(el);
		measure();
		return () => obs.disconnect();
	});

	function openRow(_row: number, label: string) {
		const pos = screen.labels.indexOf(label);
		if (pos >= 0) screen.openInGrid(pos);
	}
</script>

<div class="heatbody" data-testid="mimo-heatmap">
	<div class="body" bind:this={box}>
		<Heatmap {view} {mode} order={s.order} onopen={openRow} empty={screen.labels.length ? 'Waiting for the first loop.' : 'Waiting for the run.'} />
	</div>
	{#if beyond}
		<p class="counts num" data-testid="heat-counts">
			<span><b>{beyond.alarm}</b> of {view?.rows.ticks.length} controls with lines beyond alarm</span>
			<span><b>{beyond.abort}</b> beyond abort</span>
		</p>
	{/if}
</div>

<style>
	.heatbody {
		display: flex;
		flex-direction: column;
		gap: 2px;
		height: 100%;
		min-height: 0;
	}

	.body {
		flex: 1;
		min-height: 0;
	}

	.counts {
		display: flex;
		flex: none;
		flex-wrap: wrap;
		gap: 2px var(--sp-4);
		margin: 0;
		padding: 0 4px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.counts b {
		color: var(--text-1);
	}
</style>
