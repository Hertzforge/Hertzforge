<script lang="ts">
	/** The page on screen, drawn as its view. Switching page changes only what this window asks for. */
	import type { LawEntry } from '$lib/protocol/mimo';
	import type { MimoGeometry } from '$lib/viz/geometry';
	import type { RowBand } from '$lib/viz/scales';
	import GeometryPage from './GeometryPage.svelte';
	import GridPage from './GridPage.svelte';
	import HeatmapPage from './HeatmapPage.svelte';
	import type { MimoViewsState } from './mimoViews.svelte';
	import PlantPage from './PlantPage.svelte';

	interface Props {
		views: MimoViewsState;
		geometry?: MimoGeometry | null;
		bands?: RowBand[];
		ceilings?: number[] | null;
		diagnostics?: LawEntry['diagnostics'];
	}
	let { views, geometry = null, bands = [], ceilings = null, diagnostics = [] }: Props = $props();

	const page = $derived(views.page);
</script>

<div class="pageview" data-testid="mimo-page-view" data-page={page.id} data-kind={page.kind}>
	{#key page.id}
		{#if page.kind === 'grid'}
			<GridPage {views} {page} />
		{:else if page.kind === 'heatmap'}
			<HeatmapPage {views} {page} />
		{:else if page.kind === 'plant'}
			<PlantPage {views} {page} {ceilings} {diagnostics} />
		{:else}
			<GeometryPage {views} {page} {geometry} {bands} />
		{/if}
	{/key}
</div>

<style>
	.pageview {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
	}

	.pageview > :global(*) {
		flex: 1;
	}
</style>
