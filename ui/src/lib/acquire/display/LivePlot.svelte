<script lang="ts">
	/**
	 * One plot on a page: its title, its settings and its removal, and the views the service sent
	 * for it. Several channels are drawn one pane each, or all on one pane, as the operator chose.
	 */
	import type { DataPlot, LiveQuantity, SpectrumView } from '$lib/protocol/types';
	import ReadingPane from '$lib/viz/ReadingPane.svelte';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';
	import { hasChannels, plotTitle, type PlotSpec } from './layout';
	import PlotEditor from './PlotEditor.svelte';
	import { fixedY, presented } from './views';

	interface Props {
		plot: PlotSpec;
		data: DataPlot | null;
		/** The run has streamed: an empty plot is waiting on the run, not on the service. */
		streaming: boolean;
		editing: boolean;
		inputs: { index: number; name: string }[];
		quantities: LiveQuantity[];
		onedit: () => void;
		onclose: () => void;
		onchange: (patch: Partial<Omit<PlotSpec, 'id'>>) => void;
		onremove: () => void;
		onmove: (by: -1 | 1) => void;
		first: boolean;
		last: boolean;
	}
	let { plot, data, streaming, editing, inputs, quantities, onedit, onclose, onchange, onremove, onmove, first, last }: Props = $props();

	const views = $derived(presented(plot, data));
	const spectra = $derived(views.filter((v): v is SpectrumView => v.view === 'spectrum'));
	const reading = $derived(views.find((v) => v.view === 'reading') ?? null);
	const title = $derived(plotTitle(plot));
	const cols = $derived(spectra.length <= 1 ? 1 : spectra.length <= 4 ? 2 : 3);
	/** A pane names its channel whenever the plot is of channels: the plot title says only which set. */
	const named = $derived(spectra.length > 1 || hasChannels(plot.quantity));
	const drawn = $derived(spectra.some((v) => v.series.length > 0) || reading !== null);

	const empty = $derived.by(() => {
		if (!streaming) return 'Appears when a run starts.';
		if (!data) return 'Waiting for the control service.';
		if (data.channels && data.channels.length === 0) {
			if (plot.channels === 'limited') return 'No channel has a limit in this setup.';
			if (plot.channels === 'control') return 'The setup has no control channel.';
			return 'None of the chosen channels is in the run.';
		}
		if (plot.quantity === 'coherence') return 'Appears when system identification has measured it.';
		return 'Appears with the first control loop.';
	});
</script>

<section class="plot" class:editing data-testid="plot-{plot.id}" data-plot={plot.id} data-quantity={plot.quantity} aria-label={title}>
	<header>
		<h3 title={title}>{title}</h3>
		<div class="tools">
			<button class="btn ghost icon sm" aria-label="Move {title} earlier" title="Move earlier" disabled={first} onclick={() => onmove(-1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<button class="btn ghost icon sm" aria-label="Move {title} later" title="Move later" disabled={last} onclick={() => onmove(1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<button class="btn ghost sm" data-testid="plot-edit" aria-expanded={editing} onclick={() => (editing ? onclose() : onedit())}>
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M2 4h10M2 10h10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /><circle cx="5" cy="4" r="1.7" fill="var(--bg-1)" stroke="currentColor" stroke-width="1.3" /><circle cx="9.5" cy="10" r="1.7" fill="var(--bg-1)" stroke="currentColor" stroke-width="1.3" /></svg>
				Settings
			</button>
			<button class="btn ghost icon sm" data-testid="plot-remove" aria-label="Remove {title}" title="Remove this plot" onclick={onremove}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
			</button>
		</div>
	</header>

	{#if editing}
		<div class="edit">
			<PlotEditor {plot} {inputs} {quantities} {onchange} {onclose} />
		</div>
	{/if}

	<div class="body">
		{#if !drawn}
			<p class="empty" data-testid="plot-empty">{empty}</p>
		{:else if reading}
			<div class="reading"><ReadingPane view={reading} layout="list" showTitle={false} /></div>
		{:else}
			<div class="panes" style:--cols={cols} data-panes={spectra.length}>
				{#each spectra as v, i (i)}
					<div class="pane" data-testid="plot-pane">
						<SpectrumPane view={v} live showTitle={named} minHeight={spectra.length > 2 ? 110 : 150} yFixed={fixedY(plot, v)} />
					</div>
				{/each}
			</div>
		{/if}
	</div>
</section>

<style>
	.plot {
		position: relative;
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
		padding: var(--sp-2) var(--sp-3) var(--sp-3);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	header {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-height: 28px;
	}

	h3 {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.tools {
		display: flex;
		align-items: center;
		gap: 2px;
		flex-shrink: 0;
	}

	/* The settings float over the page, whole, whatever the plot's size; the plot keeps drawing
	   underneath, so every change is seen as it is made. */
	.edit {
		position: fixed;
		top: 50%;
		left: 50%;
		z-index: 50;
		width: min(580px, calc(100vw - 32px));
		max-height: calc(100vh - 48px);
		overflow-y: auto;
		transform: translate(-50%, -50%);
		border-radius: var(--radius-md);
	}

	.body {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-height: 0;
	}

	.panes {
		display: grid;
		flex: 1;
		grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
		grid-auto-rows: minmax(150px, 1fr);
		gap: var(--sp-2);
		min-height: 0;
	}

	.pane {
		min-width: 0;
		min-height: 0;
	}

	.reading {
		overflow-y: auto;
	}

	.empty {
		display: grid;
		flex: 1;
		place-items: center;
		min-height: 120px;
		padding: var(--sp-4);
		font-size: var(--text-sm);
		color: var(--text-3);
		text-align: center;
		background: var(--bg-plot);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-md);
	}
</style>
