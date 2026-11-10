<script lang="ts">
	/**
	 * The drawing area of one plot panel: the views the service sent for it, drawn with the panel's
	 * display options. (Placeholder until the display options land: draws spectrum and reading views
	 * as the run view did.)
	 */
	import type { SpectrumView, View } from '$lib/protocol/types';
	import ReadingPane from '$lib/viz/ReadingPane.svelte';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';
	import type { PlotPanelSpec, PlotType } from './types';

	interface Props {
		views: View[];
		type: PlotType | null;
		spec: PlotPanelSpec;
		/** What the plot says while it has nothing to draw. */
		empty: string;
		/** Live data: hold ranges steady across frames. */
		live?: boolean;
		/** An in-plot change to the spec (a zoom held, a note placed, a trace hidden from the legend). */
		onspec?: (spec: PlotPanelSpec) => void;
	}
	let { views, type, spec, empty, live = true }: Props = $props();

	const spectra = $derived(views.filter((v): v is SpectrumView => v.view === 'spectrum'));
	const reading = $derived(views.find((v) => v.view === 'reading') ?? null);
	const drawn = $derived(spectra.some((v) => v.series.length > 0) || reading !== null);

	export async function copyImage(): Promise<void> {}
	export async function saveImage(_name: string): Promise<void> {}
	export async function saveCsv(_name: string): Promise<void> {}
</script>

<div class="body" data-quantity={type?.id ?? spec.quantity}>
	{#if !drawn}
		<p class="empty">{empty}</p>
	{:else}
		{#each spectra as v, i (i)}
			<div class="pane"><SpectrumPane view={v} {live} followX={type?.time ?? false} showTitle={spectra.length > 1} minHeight={80} /></div>
		{/each}
		{#if reading}<ReadingPane view={reading} />{/if}
	{/if}
</div>

<style>
	.body {
		display: flex;
		flex-direction: column;
		min-height: 0;
		height: 100%;
	}
	.pane {
		flex: 1 1 0;
		min-height: 0;
	}
	.empty {
		margin: auto;
		color: var(--text-muted);
		font-size: 13px;
	}
</style>
