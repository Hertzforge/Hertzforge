<script lang="ts">
	/**
	 * The drive and plant page: what the controller is sending and what it is working through. The
	 * chosen drive's spectrum with every drive's rms against its ceiling; the coherence of two
	 * drives; the frequency response from a chosen drive to a chosen control, magnitude and phase;
	 * the plant's singular values per line and its condition number against the gate; each drive's
	 * cap factor this loop; and a diagnostic the control law declares, chosen by name.
	 */
	import type { MimoPageSpec } from '$lib/acquire/display/mimoLayout';
	import type { LawEntry } from '$lib/protocol/mimo';
	import type { View } from '$lib/protocol/types';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatFixed } from '$lib/viz/format';
	import type { MimoViewsState } from './mimoViews.svelte';
	import { plantSettings, plotId } from './pages';

	interface Props {
		views: MimoViewsState;
		page: MimoPageSpec;
		/** Each drive's voltage ceiling (V rms), in the setup's drive order. */
		ceilings?: number[] | null;
		/** The diagnostics the chosen law declares. */
		diagnostics?: LawEntry['diagnostics'];
	}
	let { views, page, ceilings = null, diagnostics = [] }: Props = $props();

	const s = $derived(plantSettings(page));
	const labels = $derived(views.labels);
	const drives = $derived(views.driveLabels);
	const status = $derived(views.status);
	const ceil = $derived(ceilings ?? status?.prediction?.ceiling_vrms ?? []);
	const rms = $derived(status?.drive_rms_v ?? []);
	const v = (part: string): View[] => views.plotData(plotId(page, part))?.views ?? [];
	const diag = $derived(diagnostics.find((d) => d.key === s.diag) ?? null);
	const biggest = $derived(Math.max(1e-12, ...ceil.filter((x) => Number.isFinite(x)), ...rms.filter((x) => Number.isFinite(x))));

	let box = $state<HTMLDivElement | null>(null);
	$effect(() => {
		if (!box) return;
		const el = box;
		const measure = () => views.setWidth(page.id, 'half', Math.max(64, el.clientWidth / 2 - 90));
		const obs = new ResizeObserver(measure);
		obs.observe(el);
		measure();
		return () => obs.disconnect();
	});

	const set = (patch: Record<string, unknown>) => views.patch(page.id, patch);
</script>

{#snippet plot(part: string, empty: string, minHeight = 190)}
	{@const vs = v(part)}
	{#if vs.length}
		{#each vs as view, i (i)}
			<div class="pane"><ViewRenderer {view} live showTitle={false} {minHeight} /></div>
		{/each}
	{:else}
		<div class="waiting" style:min-height="{minHeight}px">{empty}</div>
	{/if}
{/snippet}

{#snippet pick(label: string, value: number, names: string[], onpick: (i: number) => void, test: string)}
	<label class="pick">
		<span class="eyebrow">{label}</span>
		<select class="field" data-testid={test} value={String(Math.min(value, Math.max(0, names.length - 1)))} onchange={(e) => onpick(Number(e.currentTarget.value))} disabled={!names.length}>
			{#each names as n, i (i)}<option value={String(i)}>{n}</option>{/each}
		</select>
	</label>
{/snippet}

<div class="plant" bind:this={box} data-testid="mimo-plant-page">
	<section class="card">
		<header>
			<h3>Drive</h3>
			{@render pick('Drive', s.drive, drives, (i) => set({ drive: i }), 'plant-drive')}
		</header>
		{@render plot('drive', 'Waiting for the run.')}
		{#if drives.length}
			<table class="rmsbars" aria-label="Drive rms against its ceiling" data-testid="plant-drive-rms">
				<thead><tr><th>Drive</th><th class="bar">rms against ceiling</th><th class="r">V rms</th><th class="r">Ceiling</th><th></th></tr></thead>
				<tbody>
					{#each drives as d, k (k)}
						{@const r = rms[k]}
						{@const c = ceil[k]}
						{@const over = r !== undefined && c !== undefined && r > c}
						<tr class:sel={k === s.drive} class:over onclick={() => set({ drive: k })}>
							<td>{d}</td>
							<td class="bar">
								<span class="track">
									<span class="fill" style:width="{r === undefined ? 0 : Math.min(100, (r / biggest) * 100)}%"></span>
									{#if c !== undefined}<span class="ceil" style:left="{Math.min(100, (c / biggest) * 100)}%" title="Ceiling"></span>{/if}
								</span>
							</td>
							<td class="r num">{r === undefined ? '-' : formatFixed(r, 3)}</td>
							<td class="r num">{c === undefined ? '-' : formatFixed(c, 3)}</td>
							<td>{#if over}<StatusMark status="fail" />{/if}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>

	<section class="card">
		<header>
			<h3>Frequency Response</h3>
			{@render pick('Control', s.frf[0], labels, (i) => set({ frf: [i, s.frf[1]] }), 'plant-frf-control')}
			{@render pick('from Drive', s.frf[1], drives, (i) => set({ frf: [s.frf[0], i] }), 'plant-frf-drive')}
		</header>
		{@render plot('frf', 'Waiting for identification.', 150)}
	</section>

	<section class="card">
		<header><h3>Singular Values</h3></header>
		{@render plot('sv', 'Waiting for identification.', 170)}
		<header><h3>Condition Number</h3></header>
		{@render plot('cond', 'Waiting for identification.', 150)}
	</section>

	<section class="card">
		{#if drives.length >= 2}
			<header>
				<h3>Drive Coherence</h3>
				{@render pick('Drive', s.drivePair[0], drives, (i) => set({ drivePair: [i, i === s.drivePair[1] ? s.drivePair[0] : s.drivePair[1]] }), 'plant-dcoh-a')}
				{@render pick('with', s.drivePair[1], drives, (i) => set({ drivePair: [i === s.drivePair[0] ? s.drivePair[1] : s.drivePair[0], i] }), 'plant-dcoh-b')}
			</header>
			{@render plot('dcoh', 'Waiting for the run.', 150)}
		{/if}
		<header><h3>Drive Caps This Loop</h3></header>
		{@render plot('cap', 'Waiting for the run.', 80)}
		<header>
			<h3>Law Diagnostic</h3>
			<label class="pick">
				<span class="eyebrow">Show</span>
				<select class="field" data-testid="plant-diag" value={s.diag ?? ''} onchange={(e) => set({ diag: e.currentTarget.value || null, diagIndex: null })} disabled={!diagnostics.length}>
					<option value="">{diagnostics.length ? 'None' : 'This law declares none'}</option>
					{#each diagnostics as d (d.key)}<option value={d.key}>{d.label}{d.unit ? ` (${d.unit})` : ''}</option>{/each}
				</select>
			</label>
			{#if diag && (diag.layout === 'per_control_line' || diag.layout === 'per_drive_line')}
				{@const names = diag.layout === 'per_control_line' ? labels : drives}
				{@render pick(diag.layout === 'per_control_line' ? 'Control' : 'Drive', s.diagIndex ?? 0, names, (i) => set({ diagIndex: i }), 'plant-diag-index')}
			{/if}
		</header>
		{#if s.diag}
			{#if diag?.help}<p class="help">{diag.help}</p>{/if}
			{@render plot('diag', 'Waiting for the law.', 150)}
		{/if}
	</section>
</div>

<style>
	.plant {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-3);
		height: 100%;
		min-height: 0;
		overflow-y: auto;
	}

	.card {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
		padding: var(--sp-3);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	header {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px var(--sp-3);
	}

	h3 {
		margin-right: auto;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.pick {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.eyebrow {
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.06em;
		color: var(--text-3);
		text-transform: uppercase;
	}

	.pane {
		min-height: 0;
	}

	.waiting {
		display: grid;
		place-items: center;
		color: var(--text-3);
		font-size: var(--text-sm);
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.help {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.rmsbars {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--text-xs);
	}

	.rmsbars th {
		padding: 3px 4px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		color: var(--text-3);
		text-align: left;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		border-bottom: 1px solid var(--line-2);
	}

	.rmsbars td {
		padding: 3px 4px;
		border-bottom: 1px solid var(--line-1);
	}

	.rmsbars tr {
		cursor: pointer;
	}

	.rmsbars tr.sel td {
		background: var(--accent-weak);
	}

	.rmsbars tr.over td {
		font-weight: var(--weight-strong);
	}

	.r {
		text-align: right;
	}

	.bar {
		width: 45%;
	}

	.track {
		position: relative;
		display: block;
		height: 8px;
		background: var(--bg-3);
		border-radius: 2px;
	}

	.fill {
		position: absolute;
		inset: 0 auto 0 0;
		background: var(--series-drive);
		border-radius: 2px;
	}

	.over .fill {
		background: var(--status-fail);
	}

	.ceil {
		position: absolute;
		top: -3px;
		bottom: -3px;
		width: 2px;
		margin-left: -1px;
		background: var(--text-1);
	}

	@media (max-width: 1000px) {
		.plant {
			grid-template-columns: 1fr;
		}
	}
</style>
