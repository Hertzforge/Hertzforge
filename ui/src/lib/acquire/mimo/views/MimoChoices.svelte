<script lang="ts">
	/**
	 * A MIMO plot's own choices, in its options menu under what it shows: the control or drive (or
	 * every one), the two of a pair, the grid's pinned controls and what it draws off the diagonal,
	 * the heatmap's mode and row order, the law's diagnostic, the geometry's motion. Every change
	 * applies at once; positions are named as the run names them.
	 */
	import type { LawEntry } from '$lib/protocol/mimo';
	import { GRID_MAX, togglePin } from '$lib/viz/matrixGrid';
	import { needsDistinctPair, SELECTOR_KINDS } from '../../display/mimoLayout';
	import type { PlotPanelSpec, PlotSelect } from '../../plots/types';
	import { diagChoice, GEOMETRY, geometryChoice, gridChoice, heatmapChoice, positionName, selectorOf } from './mimoScreen';

	interface Props {
		spec: PlotPanelSpec;
		onchange: (spec: PlotPanelSpec) => void;
		/** The run's names for its controls and drives, in order. */
		labels: string[];
		drives: string[];
		/** The diagnostics the chosen law declares. */
		diagnostics?: LawEntry['diagnostics'];
		/** The service offers the coherence error per control. */
		coherence?: boolean;
		/** Control is in degrees of freedom. */
		virtual?: boolean;
	}
	let { spec, onchange, labels, drives, diagnostics = [], coherence = false, virtual = false }: Props = $props();

	const uid = `mc-${Math.random().toString(36).slice(2, 8)}`;
	const q = $derived(spec.quantity);
	const names = $derived({ labels, drive_labels: drives });
	const kinds = $derived(q === GEOMETRY || !(q in SELECTOR_KINDS) ? [] : SELECTOR_KINDS[q as keyof typeof SELECTOR_KINDS]);
	const sel = $derived(kinds.length && !kinds.includes('none') && q !== 'law_diag' ? selectorOf(q as keyof typeof SELECTOR_KINDS, spec.select) : null);
	/** How many controls, or drives, the plot can choose among; the chosen one at least, before the run names them. */
	const countOf = (drivesToo: boolean, at: number[]) => Math.max((drivesToo ? drives : labels).length, ...at.map((i) => i + 1));
	const choose = (patch: PlotSelect) => onchange({ ...spec, select: { ...(spec.select ?? {}), ...patch } });
	const setSel = (s: PlotSelect) => onchange({ ...spec, select: { ...s } });
	const waiting = $derived(!labels.length);
</script>

{#snippet seg<T extends string>(label: string, options: [T, string][], value: T, pick: (v: T) => void)}
	<div class="row">
		<span class="lbl">{label}</span>
		<div class="seg" role="radiogroup" aria-label={label}>
			{#each options as [v, words] (v)}
				<button type="button" role="radio" aria-checked={value === v} onclick={() => pick(v)}>{words}</button>
			{/each}
		</div>
	</div>
{/snippet}

{#snippet position(label: string, id: string, value: number, count: number, drivesToo: boolean, pick: (i: number) => void, all: boolean = false, testid = '')}
	<div class="row">
		<label class="lbl" for="{uid}-{id}">{label}</label>
		<select id="{uid}-{id}" class="field sm" data-testid={testid || undefined} value={String(value)} onchange={(e) => pick(Number(e.currentTarget.value))}>
			{#if all}<option value="-1">All {drivesToo ? 'Drives' : 'Controls'}</option>{/if}
			{#each { length: count } as _, i (i)}<option value={String(i)}>{positionName(drivesToo ? 'drive_psd' : 'psd', i, names)}</option>{/each}
		</select>
	</div>
{/snippet}

<div class="choices" data-testid="mimo-choices" data-quantity={q}>
	{#if q === 'sdm' || q === 'pred_sdm'}
		{@const g = gridChoice(spec.select)}
		{#if q === 'sdm'}
			<div class="row">
				<span class="lbl">Off Diagonal</span>
				<div class="seg" role="radiogroup" aria-label="Off the diagonal">
					<button type="button" role="radio" aria-checked={g.off === 'coh_phase'} data-testid="grid-coh" onclick={() => choose({ off: 'coh_phase', open: null })}>Coherence and Phase</button>
					<button type="button" role="radio" aria-checked={g.off === 'csd'} data-testid="grid-csd" onclick={() => choose({ off: 'csd', open: null })}>Cross Spectrum</button>
				</div>
			</div>
		{/if}
		<div class="row">
			<label class="lbl" for="{uid}-pin">Pin</label>
			<select id="{uid}-pin" class="field sm" data-testid="grid-pin" value="" disabled={waiting} onchange={(e) => (e.currentTarget.value && choose({ window: togglePin(g.window, e.currentTarget.value) }), (e.currentTarget.value = ''))}>
				<option value="">{waiting ? 'The run names its controls' : 'Keep a control in view'}</option>
				{#each labels as l (l)}<option value={l} disabled={g.window.pinned.length >= GRID_MAX && !g.window.pinned.includes(l)}>{g.window.pinned.includes(l) ? `${l} (pinned)` : l}</option>{/each}
			</select>
		</div>
		{#if g.window.pinned.length}
			<div class="row">
				<span class="lbl"></span>
				<ul class="pins" aria-label="Pinned controls">
					{#each g.window.pinned as p (p)}
						<li>
							<button type="button" class="chip" title="Unpin {p}" aria-label="Unpin {p}" onclick={() => choose({ window: togglePin(g.window, p) })}>
								{p}
								<svg viewBox="0 0 10 10" width="9" height="9" aria-hidden="true"><path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
							</button>
						</li>
					{/each}
				</ul>
			</div>
		{/if}
	{:else if q === 'error_db'}
		{@const h = heatmapChoice(spec.select)}
		{#if coherence}
			{@render seg(
				'Shows',
				[
					['error', 'Spectrum Error'],
					['coherence', 'Coherence Error']
				],
				h.mode,
				(v) => choose({ mode: v })
			)}
		{/if}
		<div class="row">
			<span class="lbl">Rows</span>
			<div class="seg" role="radiogroup" aria-label="Row order">
				<button type="button" role="radio" aria-checked={h.order === 'setup'} data-testid="heat-order-setup" onclick={() => choose({ order: 'setup' })}>Setup Order</button>
				<button type="button" role="radio" aria-checked={h.order === 'worst'} data-testid="heat-order-worst" onclick={() => choose({ order: 'worst' })}>Largest Error First</button>
			</div>
		</div>
	{:else if q === GEOMETRY}
		{#if virtual}
			<div class="row">
				<span class="lbl">Motion</span>
				<label class="check"><input type="checkbox" checked={geometryChoice(spec.select).motion} onchange={(e) => choose({ motion: e.currentTarget.checked })} /> Animate the rigid-body motion</label>
			</div>
		{/if}
	{:else if q === 'law_diag'}
		{@const d = diagChoice(spec.select)}
		{@const decl = diagnostics.find((x) => x.key === d.key) ?? null}
		<div class="row">
			<label class="lbl" for="{uid}-diag">Diagnostic</label>
			<select id="{uid}-diag" class="field sm" data-testid="plant-diag" value={d.key ?? ''} disabled={!diagnostics.length && !d.key} onchange={(e) => setSel({ kind: 'key', key: e.currentTarget.value, index: null })}>
				<option value="">{diagnostics.length ? 'Choose one' : 'This law declares none'}</option>
				{#each diagnostics as x (x.key)}<option value={x.key}>{x.label}{x.unit ? ` (${x.unit})` : ''}</option>{/each}
				{#if d.key && !decl}<option value={d.key}>{d.key}</option>{/if}
			</select>
		</div>
		{#if decl?.help}<p class="help">{decl.help}</p>{/if}
		{#if decl && (decl.layout === 'per_control_line' || decl.layout === 'per_drive_line')}
			{@const onDrives = decl.layout === 'per_drive_line'}
			{@render position(onDrives ? 'Drive' : 'Control', 'diag-index', d.index ?? 0, countOf(onDrives, [d.index ?? 0]), onDrives, (i) => setSel({ kind: 'key', key: d.key, index: i }), false, 'plant-diag-index')}
		{/if}
	{:else if sel}
		{@const onDrives = q === 'drive_psd' || q === 'drive_coh' || q === 'cap' || q === 'pred_drive_rms' || q === 'pred_drive_accel'}
		{#if sel.kind === 'index' || sel.kind === 'diagonal'}
			{@const at = sel.kind === 'index' ? sel.index : -1}
			{@render position(onDrives ? 'Drive' : 'Control', 'pos', at, countOf(onDrives, [Math.max(0, at)]), onDrives, (i) => setSel(i < 0 ? { kind: 'diagonal' } : { kind: 'index', index: i }), kinds.includes('diagonal'), onDrives ? 'plant-drive' : '')}
		{:else if sel.kind === 'pair' && q === 'frf'}
			{@render position('Control', 'frf-c', sel.pair[0], countOf(false, [sel.pair[0]]), false, (i) => setSel({ kind: 'pair', pair: [i, sel.pair[1]] }), false, 'plant-frf-control')}
			{@render position('From Drive', 'frf-d', sel.pair[1], countOf(true, [sel.pair[1]]), true, (i) => setSel({ kind: 'pair', pair: [sel.pair[0], i] }), false, 'plant-frf-drive')}
		{:else if sel.kind === 'pair'}
			{@const distinct = needsDistinctPair(q as 'coh')}
			{@const n = countOf(onDrives, [sel.pair[0], sel.pair[1]])}
			{@render position(onDrives ? 'Drive' : 'Control', 'pa', sel.pair[0], n, onDrives, (i) => setSel({ kind: 'pair', pair: [i, distinct && i === sel.pair[1] ? sel.pair[0] : sel.pair[1]] }), false, onDrives ? 'plant-dcoh-a' : '')}
			{@render position('With', 'pb', sel.pair[1], n, onDrives, (i) => setSel({ kind: 'pair', pair: [distinct && i === sel.pair[0] ? sel.pair[1] : sel.pair[0], i] }), false, onDrives ? 'plant-dcoh-b' : '')}
		{/if}
		{#if waiting}<p class="help">The run names its controls and drives when it starts.</p>{/if}
	{/if}
</div>

<style>
	.choices {
		display: contents;
	}

	.row {
		display: grid;
		grid-template-columns: 70px minmax(0, 1fr);
		align-items: center;
		gap: var(--sp-2);
		min-height: 24px;
	}

	.lbl {
		color: var(--text-3);
	}

	.help {
		margin: 0 0 0 78px;
		color: var(--text-3);
		line-height: 1.35;
	}

	.field.sm {
		height: 24px;
		font-size: var(--text-xs);
	}

	.seg {
		display: inline-flex;
		flex-wrap: wrap;
		justify-self: start;
		padding: 2px;
		background: var(--bg-1);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.seg button {
		height: 20px;
		padding: 0 7px;
		font-size: var(--text-xs);
		color: var(--text-2);
		background: transparent;
		border: 0;
		border-radius: 4px;
		white-space: nowrap;
		cursor: pointer;
	}

	.seg button[aria-checked='true'] {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		background: var(--bg-4);
		box-shadow: inset 0 0 0 1px var(--line-3);
	}

	.seg button:focus-visible {
		outline: 1px solid var(--focus);
	}

	.check {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		cursor: pointer;
	}

	.pins {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.chip {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		padding: 1px 8px;
		font-size: var(--text-xs);
		color: var(--text-1);
		background: var(--bg-3);
		border: 1px solid var(--line-2);
		border-radius: 999px;
		cursor: pointer;
	}
</style>
