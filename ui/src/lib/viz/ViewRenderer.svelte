<script lang="ts">
	/**
	 * The one way the shell draws a view: dispatch on `view.view`. No workspace draws its own
	 * charts. Unknown kinds render a short note instead of failing the page.
	 */
	import type { View } from '$lib/protocol/types';
	import ReadingPane from './ReadingPane.svelte';
	import SpectrumPane from './SpectrumPane.svelte';

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
	}

	let { view, live = false, layout = 'list', showTitle = true, minHeight = 180, mode = $bindable(0) }: Props = $props();

	// three.js is most of the bundle; load it only when a shape view first appears.
	const shape3d = () => import('./Shape3DPane.svelte');
</script>

{#if view.view === 'spectrum'}
	<SpectrumPane {view} {live} {showTitle} {minHeight} />
{:else if view.view === 'reading'}
	<ReadingPane {view} {layout} {showTitle} />
{:else if view.view === 'shape3d'}
	{#await shape3d()}
		<div class="loading">Loading the 3-D view.</div>
	{:then { default: Shape3DPane }}
		{#key view}
			<Shape3DPane {view} bind:mode />
		{/key}
	{:catch err}
		<p class="unknown">The 3-D view failed to load: {err.message}</p>
	{/await}
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
