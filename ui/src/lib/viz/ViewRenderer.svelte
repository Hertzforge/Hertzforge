<script lang="ts">
	/**
	 * The one way the shell draws a view: dispatch on `view.view`. No workspace draws its own
	 * charts. Unknown kinds render a short note instead of failing the page.
	 */
	import type { ParamValues } from '$lib/forms/params';
	import type { View } from '$lib/protocol/types';
	import MatrixPane from './MatrixPane.svelte';
	import ReadingPane from './ReadingPane.svelte';
	import SpectrumPane from './SpectrumPane.svelte';
	import TablePane from './TablePane.svelte';

	interface Props {
		view: View;
		/** Live data: hold axes steady between updates. */
		live?: boolean;
		/** Reading layout. */
		layout?: 'list' | 'strip';
		showTitle?: boolean;
		minHeight?: number;
		/** Selected mode for shape3d views. */
		mode?: number;
		/** Run the view's operation again with these params (an `in_view` choice made in the view). */
		onparams?: (patch: ParamValues) => void;
		/** A matrix that selects a shape view: the cell chosen, and how to choose another. */
		selected?: { row: number; column: number } | null;
		onselect?: (cell: { row: number; column: number }) => void;
		/** Index into a shape3d view's overlays, drawn over its mode. */
		overlay?: number | null;
		/** The result the view belongs to: a playback `link` ties views within it only. */
		scope?: string;
		/** A draggable region released at new edges. */
		onspan?: (region: number, x0: number, x1: number) => void;
		/** An editable table's cell typed into; resolves to the refusal's words, or null. */
		onedit?: (row: number, column: string, value: string) => Promise<string | null>;
		/** A click in a 3-D view with `pick`: what it chose, to run the pick's operation with. */
		onpick?: (patch: Record<string, unknown>) => void;
	}

	let { view, live = false, layout = 'list', showTitle = true, minHeight = 180, mode = $bindable(0), onparams, selected = null, onselect, overlay = null, scope, onedit, onspan, onpick }: Props = $props();

	// three.js is most of the bundle; load it only when a shape view first appears.
	const shape3d = () => import('./Shape3DPane.svelte');
	const waterfall = () => import('./WaterfallPane.svelte');
</script>

{#if view.view === 'spectrum'}
	<SpectrumPane {view} {live} {showTitle} {minHeight} {scope} {onspan} />
{:else if view.view === 'reading'}
	<ReadingPane {view} {layout} {showTitle} />
{:else if view.view === 'shape3d'}
	{#await shape3d()}
		<div class="loading">Loading the 3-D view.</div>
	{:then { default: Shape3DPane }}
		{#key view}
			<Shape3DPane {view} bind:mode {overlay} {scope} {onpick} />
		{/key}
	{:catch err}
		<p class="unknown">The 3-D view failed to load: {err.message}</p>
	{/await}
{:else if view.view === 'waterfall'}
	{#await waterfall()}
		<div class="loading">Loading the 3-D view.</div>
	{:then { default: WaterfallPane }}
		{#key view}
			<WaterfallPane {view} {onparams} />
		{/key}
	{:catch err}
		<p class="unknown">The 3-D view failed to load: {err.message}</p>
	{/await}
{:else if view.view === 'matrix'}
	<MatrixPane {view} {showTitle} {selected} {onselect} />
{:else if view.view === 'table'}
	<TablePane {view} {onedit} />
{:else}
	<p class="unknown" data-testid="view-unknown">This shell has no renderer for the view "{(view as { view: string }).view}".</p>
{/if}

<style>
	.loading {
		display: grid;
		place-items: center;
		height: 100%;
		color: var(--text-3);
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.unknown {
		padding: var(--sp-4);
		color: var(--text-3);
		font-size: var(--text-sm);
	}
</style>
