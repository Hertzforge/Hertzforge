<script lang="ts">
	/**
	 * A matrix grid plot: a window of up to 8 controls drawn as an 8 by 8 grid (spectra on the
	 * diagonal, coherence above and phase below, or the cross-spectrum magnitude), the window
	 * stepped through the run's controls with pinned controls kept in place by name, and any cell
	 * opened full size. The achievable reference draws the same way from the prediction. What it
	 * draws off the diagonal and which controls are pinned are in the plot's options.
	 */
	import type { SdmView, SpectrumView } from '$lib/protocol/types';
	import MatrixGrid from '$lib/viz/MatrixGrid.svelte';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';
	import { cellKind, CELL_WORDS, GRID_MAX, gridGeometry, maxOffset, scrollWindow, togglePin, windowControls, type GridWindowSpec } from '$lib/viz/matrixGrid';
	import type { BodyPanel } from '../../screen/bodies';
	import { gridChoice, partId } from './mimoScreen';
	import type { MimoScreenState } from './mimoScreenState.svelte';

	interface Props {
		screen: MimoScreenState;
		panel: BodyPanel;
	}
	let { screen, panel }: Props = $props();

	const predicted = $derived(panel.plot.quantity === 'pred_sdm');
	const s = $derived(gridChoice(panel.plot.select));
	const labels = $derived(screen.labels);
	const win = $derived(windowControls(labels, s.window));
	const names = $derived(win.map((i) => labels[i]));
	const pinnedFlags = $derived(names.map((n) => s.window.pinned.includes(n)));
	const free = $derived(win.filter((i) => !s.window.pinned.includes(labels[i])));
	const sdm = $derived((screen.plotData(partId(panel.id, 'sdm'))?.views[0] as SdmView | undefined) ?? null);
	const spectra = $derived(predicted ? win.map(() => null) : win.map((i) => (screen.plotData(partId(panel.id, `psd:${i}`))?.views[0] as SpectrumView | undefined) ?? null));
	const unit = $derived(sdm?.unit ?? 'g²/Hz');
	const open = $derived(predicted ? null : s.open);
	const openView = $derived(open ? ((screen.plotData(partId(panel.id, 'open'))?.views[0] as SpectrumView | undefined) ?? null) : null);
	const openTitle = $derived(open ? `${CELL_WORDS[open.kind]}: ${open.row === open.col ? labels[open.row] : `${labels[open.row]} with ${labels[open.col]}`}` : '');
	let selected = $state<{ r: number; c: number } | null>(null);

	let box = $state<HTMLDivElement | null>(null);
	let full = $state<HTMLDivElement | null>(null);

	function setWindow(w: GridWindowSpec) {
		screen.choose(panel.id, { window: w });
	}

	function scroll(by: number) {
		setWindow(scrollWindow(labels, s.window, by * Math.max(1, free.length)));
	}

	function pin(name: string) {
		if (name) setWindow(togglePin(s.window, name));
	}

	function openCell(r: number, c: number) {
		if (!predicted) screen.choose(panel.id, { open: { row: win[r], col: win[c], kind: cellKind(r, c, s.off) } });
	}

	function close() {
		screen.choose(panel.id, { open: null });
	}

	// The cells' width, so the service decimates to what is drawn.
	$effect(() => {
		if (!box || typeof ResizeObserver === 'undefined') return;
		const el = box;
		const n = Math.max(1, win.length);
		const measure = () => {
			const g = gridGeometry(el.clientWidth, el.clientHeight, n, { left: 96, top: 26, bottom: 30 });
			screen.setWidth(panel.id, 'cell', Math.max(64, g.cell));
		};
		const obs = new ResizeObserver(measure);
		obs.observe(el);
		measure();
		return () => obs.disconnect();
	});

	$effect(() => {
		if (!full || typeof ResizeObserver === 'undefined') return;
		const el = full;
		const obs = new ResizeObserver(() => screen.setWidth(panel.id, 'full', el.clientWidth - 80));
		obs.observe(el);
		return () => obs.disconnect();
	});

	const first = $derived(free.length ? free[0] + 1 : 0);
	const last = $derived(free.length ? free[free.length - 1] + 1 : 0);
</script>

<div class="gridbody" data-testid="mimo-grid" data-quantity={panel.plot.quantity}>
	{#if labels.length > GRID_MAX || s.window.pinned.length}
		<div class="tools" role="toolbar" aria-label="Controls in the grid">
			<button class="btn ghost icon sm" aria-label="Previous controls" title="Previous controls (Page Up)" disabled={s.window.offset <= 0} onclick={() => scroll(-1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<span class="num where" data-testid="grid-window">{labels.length <= GRID_MAX ? `All ${labels.length} controls` : `Controls ${first} to ${last} of ${labels.length}`}</span>
			<button class="btn ghost icon sm" aria-label="Next controls" title="Next controls (Page Down)" disabled={s.window.offset >= maxOffset(labels, s.window)} onclick={() => scroll(1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			{#each s.window.pinned as p (p)}
				<button class="chip" title="Unpin {p}" aria-label="Unpin {p}" onclick={() => pin(p)}>
					<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M6 1.5v5M3.5 6.5h5M6 6.5v4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
					{p}
				</button>
			{/each}
		</div>
	{/if}

	<div class="body" bind:this={box}>
		{#if labels.length && (!predicted || sdm)}
			<MatrixGrid labels={names} pinned={pinnedFlags} {sdm} {spectra} off={predicted ? 'coh_phase' : s.off} {unit} bind:selected onopen={predicted ? undefined : openCell} onpin={(k) => pin(names[k])} onscroll={scroll} />
		{:else}
			<div class="waiting">
				{#if !labels.length}
					The grid fills once the run has named its controls.
				{:else}
					Appears after identification when the controls outnumber the drives.
				{/if}
			</div>
		{/if}

		{#if open}
			<div class="full" bind:this={full} role="dialog" aria-label={openTitle} data-testid="grid-open">
				<div class="fullhead">
					<h4>{openTitle}</h4>
					<button class="btn sm" data-testid="grid-close" onclick={close}>Close</button>
				</div>
				<div class="fullplot">
					{#if openView}
						<SpectrumPane view={openView} live minHeight={160} showTitle={false} yFixed={open.kind === 'coh' ? [0, 1.05] : open.kind === 'phase' ? [-180, 180] : null} />
					{:else}
						<div class="waiting">Waiting for the run.</div>
					{/if}
				</div>
			</div>
		{/if}
	</div>
</div>

<svelte:window onkeydown={(e) => open && e.key === 'Escape' && close()} />

<style>
	.gridbody {
		display: flex;
		flex-direction: column;
		gap: 4px;
		height: 100%;
		min-height: 0;
	}

	.tools {
		display: flex;
		flex: none;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px;
		padding: 0 2px;
	}

	.where {
		min-width: 12em;
		font-size: var(--text-xs);
		color: var(--text-2);
		text-align: center;
	}

	.chip {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 1px 8px;
		font-size: var(--text-xs);
		color: var(--text-1);
		background: var(--bg-3);
		border: 1px solid var(--line-2);
		border-radius: 999px;
		cursor: pointer;
	}

	.body {
		position: relative;
		flex: 1;
		min-height: 0;
	}

	.full {
		position: absolute;
		inset: 12px 16px;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg-1);
		border: 1px solid var(--line-3);
		border-radius: var(--radius-lg);
		box-shadow: var(--shadow-pop);
	}

	.fullhead {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.fullhead h4 {
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.fullplot {
		flex: 1;
		min-height: 0;
	}

	.waiting {
		display: grid;
		place-items: center;
		height: 100%;
		padding: var(--sp-3);
		color: var(--text-3);
		font-size: var(--text-sm);
		text-align: center;
	}
</style>
