<script lang="ts">
	/**
	 * The matrix grid page: a window of up to 8 controls drawn as an 8 by 8 grid (spectra on the
	 * diagonal, coherence above, phase below, or the cross-spectrum magnitude), the window scrolled
	 * through the run's controls with pinned controls kept in place by name, and any cell opened
	 * full size with the single-output plot's tools.
	 */
	import type { MimoPageSpec } from '$lib/acquire/display/mimoLayout';
	import type { SdmView, SpectrumView } from '$lib/protocol/types';
	import MatrixGrid from '$lib/viz/MatrixGrid.svelte';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';
	import { cellKind, CELL_WORDS, GRID_MAX, gridGeometry, maxOffset, scrollWindow, togglePin, windowControls, type GridWindowSpec } from '$lib/viz/matrixGrid';
	import type { MimoViewsState } from './mimoViews.svelte';
	import { gridSettings, plotId } from './pages';

	interface Props {
		views: MimoViewsState;
		page: MimoPageSpec;
	}
	let { views, page }: Props = $props();

	const s = $derived(gridSettings(page));
	const labels = $derived(views.labels);
	const win = $derived(windowControls(labels, s.window));
	const names = $derived(win.map((i) => labels[i]));
	const pinnedFlags = $derived(names.map((n) => s.window.pinned.includes(n)));
	const free = $derived(win.filter((i) => !s.window.pinned.includes(labels[i])));
	const sdm = $derived((views.plotData(plotId(page, 'sdm'))?.views[0] as SdmView | undefined) ?? null);
	const spectra = $derived(win.map((i) => (views.plotData(plotId(page, `psd:${i}`))?.views[0] as SpectrumView | undefined) ?? null));
	const unit = $derived(sdm?.unit ?? 'g²/Hz');
	const openView = $derived(s.open ? ((views.plotData(plotId(page, 'open'))?.views[0] as SpectrumView | undefined) ?? null) : null);
	const openTitle = $derived(
		s.open ? `${CELL_WORDS[s.open.kind]}: ${s.open.row === s.open.col ? labels[s.open.row] : `${labels[s.open.row]} with ${labels[s.open.col]}`}` : ''
	);
	let selected = $state<{ r: number; c: number } | null>(null);
	let pinChoice = $state('');

	let box = $state<HTMLDivElement | null>(null);
	let full = $state<HTMLDivElement | null>(null);

	function setWindow(w: GridWindowSpec) {
		views.patch(page.id, { window: w });
	}

	function scroll(by: number) {
		setWindow(scrollWindow(labels, s.window, by * Math.max(1, free.length)));
	}

	function pin(name: string) {
		if (name) setWindow(togglePin(s.window, name));
	}

	function open(r: number, c: number) {
		views.patch(page.id, { open: { row: win[r], col: win[c], kind: cellKind(r, c, s.off) } });
	}

	function close() {
		views.patch(page.id, { open: null });
	}

	// The cells' width, so the service decimates to what is drawn.
	$effect(() => {
		if (!box) return;
		const el = box;
		const n = Math.max(1, win.length);
		const measure = () => {
			const g = gridGeometry(el.clientWidth - 16, el.clientHeight - 70, n, { left: 96, top: 26, bottom: 30 });
			views.setWidth(page.id, 'cell', Math.max(64, g.cell));
		};
		const obs = new ResizeObserver(measure);
		obs.observe(el);
		measure();
		return () => obs.disconnect();
	});

	$effect(() => {
		if (!full) return;
		const el = full;
		const obs = new ResizeObserver(() => views.setWidth(page.id, 'full', el.clientWidth - 80));
		obs.observe(el);
		return () => obs.disconnect();
	});

	const first = $derived(free.length ? free[0] + 1 : 0);
	const last = $derived(free.length ? free[free.length - 1] + 1 : 0);
</script>

<div class="gridpage" data-testid="mimo-grid-page">
	<div class="tools" role="toolbar" aria-label="Matrix grid">
		<div class="group">
			<button class="btn ghost icon sm" aria-label="Previous controls" title="Previous controls (Page Up)" disabled={s.window.offset <= 0} onclick={() => scroll(-1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<span class="num where" data-testid="grid-window">
				{#if labels.length}
					{labels.length <= GRID_MAX ? `All ${labels.length} controls` : `Controls ${first} to ${last} of ${labels.length}`}
				{:else}
					No controls yet
				{/if}
			</span>
			<button class="btn ghost icon sm" aria-label="Next controls" title="Next controls (Page Down)" disabled={s.window.offset >= maxOffset(labels, s.window)} onclick={() => scroll(1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
		</div>

		<label class="group">
			<span class="eyebrow">Pin</span>
			<select class="field" data-testid="grid-pin" bind:value={pinChoice} onchange={() => ((pin(pinChoice), (pinChoice = '')))} disabled={!labels.length}>
				<option value="">Keep a control in view</option>
				{#each labels as l (l)}<option value={l} disabled={s.window.pinned.length >= s.window.size && !s.window.pinned.includes(l)}>{s.window.pinned.includes(l) ? `${l} (pinned)` : l}</option>{/each}
			</select>
		</label>
		{#if s.window.pinned.length}
			<div class="pins" aria-label="Pinned controls">
				{#each s.window.pinned as p (p)}
					<button class="chip" title="Unpin {p}" onclick={() => pin(p)}>
						{p}
						<svg viewBox="0 0 10 10" width="9" height="9" aria-hidden="true"><path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
					</button>
				{/each}
			</div>
		{/if}

		<div class="seg" role="radiogroup" aria-label="Off the diagonal">
			<button role="radio" aria-checked={s.off === 'coh_phase'} class:on={s.off === 'coh_phase'} data-testid="grid-coh" onclick={() => views.patch(page.id, { off: 'coh_phase', open: null })}>Coherence and Phase</button>
			<button role="radio" aria-checked={s.off === 'csd'} class:on={s.off === 'csd'} data-testid="grid-csd" onclick={() => views.patch(page.id, { off: 'csd', open: null })}>Cross Spectrum</button>
		</div>
	</div>

	<div class="body" bind:this={box}>
		{#if labels.length}
			<MatrixGrid labels={names} pinned={pinnedFlags} {sdm} {spectra} off={s.off} {unit} bind:selected onopen={open} onpin={(k) => pin(names[k])} onscroll={scroll} />
		{:else}
			<div class="waiting">The grid fills once the run has named its controls.</div>
		{/if}

		{#if s.open}
			<div class="full" bind:this={full} role="dialog" aria-label={openTitle} data-testid="grid-open">
				<div class="fullhead">
					<h3>{openTitle}</h3>
					<button class="btn sm" data-testid="grid-close" onclick={close}>Close</button>
				</div>
				<div class="fullplot">
					{#if openView}
						<SpectrumPane view={openView} live minHeight={260} showTitle={false} yFixed={s.open.kind === 'coh' ? [0, 1.05] : s.open.kind === 'phase' ? [-180, 180] : null} />
					{:else}
						<div class="waiting">Waiting for the run.</div>
					{/if}
				</div>
			</div>
		{/if}
	</div>
</div>

<svelte:window onkeydown={(e) => s.open && e.key === 'Escape' && close()} />

<style>
	.gridpage {
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

	.group {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.where {
		min-width: 13em;
		font-size: var(--text-sm);
		text-align: center;
	}

	.eyebrow {
		margin-right: 4px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.06em;
		color: var(--text-3);
		text-transform: uppercase;
	}

	.pins {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
	}

	.chip {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		padding: 2px 8px;
		font-size: var(--text-xs);
		color: var(--text-1);
		background: var(--bg-3);
		border: 1px solid var(--line-2);
		border-radius: 999px;
		cursor: pointer;
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

	.body {
		position: relative;
		flex: 1;
		min-height: 360px;
	}

	.full {
		position: absolute;
		inset: 24px 32px;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
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

	.fullhead h3 {
		font-size: var(--text-md);
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
		min-height: 200px;
		color: var(--text-3);
		font-size: var(--text-sm);
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}
</style>
