<script lang="ts">
	/**
	 * The plots of the page on screen, laid out to fill the space: one column for one or two plots,
	 * two for up to four, three beyond. Adding or removing a plot changes only what this window asks
	 * the service to send; the run goes on untouched.
	 */
	import type { DisplayState } from './display.svelte';
	import LivePlot from './LivePlot.svelte';

	interface Props {
		display: DisplayState;
		/** The run has streamed, so an empty plot is waiting on the run. */
		streaming: boolean;
		/** The channel table's enabled inputs, for choosing a plot's channels. */
		inputs: { index: number; name: string }[];
	}
	let { display, streaming, inputs }: Props = $props();

	const plots = $derived(display.page.plots);
	const cols = $derived(plots.length <= 2 ? 1 : plots.length <= 4 ? 2 : 3);
</script>

<div class="pageview" data-testid="page-view" data-page={display.page.id} data-plots={plots.length} style:--cols={cols}>
	{#each plots as p, i (p.id)}
		<LivePlot
			plot={p}
			data={display.plotData(p.id)}
			{streaming}
			{inputs}
			quantities={display.quantities}
			editing={display.editing === p.id}
			first={i === 0}
			last={i === plots.length - 1}
			onedit={() => (display.editing = p.id)}
			onclose={() => (display.editing = null)}
			onchange={(patch) => display.updatePlot(p.id, patch)}
			onremove={() => display.removePlot(p.id)}
			onmove={(by) => display.movePlot(p.id, by)}
		/>
	{:else}
		<div class="none" data-testid="page-empty">
			<p>This page has no plots yet.</p>
			<button class="btn primary sm" onclick={() => display.addPlot()} disabled={!display.canAddPlot}>Add a Plot</button>
		</div>
	{/each}
</div>

<style>
	.pageview {
		display: grid;
		flex: 1;
		grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
		grid-auto-rows: minmax(240px, 1fr);
		gap: var(--sp-3);
		min-height: 0;
		overflow-y: auto;
	}

	/* A Control page reads as it always has: the control spectrum large, the drive below it. */
	.pageview[data-plots='2'] {
		grid-template-rows: minmax(260px, 2fr) minmax(180px, 1fr);
	}

	.none {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: var(--sp-3);
		min-height: 240px;
		color: var(--text-3);
		background: var(--bg-plot);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-lg);
	}
</style>
