<script lang="ts">
	/**
	 * The reference as the control service built or read it: every PSD on one plot, the smallest
	 * eigenvalue of each line (below zero, no signal has those spectra), and the chosen pair's
	 * coherence and phase; then the checks: lines and band, positive semi-definiteness with the
	 * lines that fail and the repair the service would make (each pair's coherence before and
	 * after), and the provenance. The shell computes none of it.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import type { MimoState } from './mimoStore.svelte';

	interface Props {
		mimo: MimoState;
	}
	let { mimo }: Props = $props();

	const r = $derived(mimo.setup.reference);
	const res = $derived(mimo.ref);
	const current = $derived(!!res && mimo.refKey === mimo.refInputKey);
	const psd = $derived(res?.psd ?? null);
	const labels = $derived(res?.labels ?? mimo.labels);
	const inSetup = $derived(mimo.check && mimo.checkedKey === mimo.key ? mimo.check.reference : null);

	function eig(x: number | null | undefined): string {
		if (x === null || x === undefined) return '-';
		if (x === 0) return '0';
		return x.toExponential(2);
	}

	const METHOD: Record<string, string> = {
		breakpoints: 'Breakpoint tables',
		rigid_body: 'Rigid body',
		specification_npz: 'Specification file (.npz)',
		specification_mat: 'Specification file (.mat)',
		csv: 'CSV table',
		hforge: 'Run file',
		field_data: 'Field data',
		direct: 'Entered matrices'
	};
</script>

<section class="aq-panel preview" aria-labelledby="rp-title" data-testid="mimo-reference-preview" data-state={!res ? 'none' : !current ? 'stale' : res.ok ? 'ok' : 'refused'}>
	<div class="aq-head">
		<h2 id="rp-title">Reference Preview</h2>
		<span class="aq-spacer"></span>
		<span class="state aq-small" aria-live="polite">
			{#if mimo.refBusy}
				Building
			{:else if res && current && res.ok}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
				Built and checked by the control service
			{:else if res && current}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="var(--amber-2)" /></svg>
				Not accepted yet
			{/if}
		</span>
	</div>

	{#if res && res.views.length}
		<div class="pair-pick">
			<label for="rp-pair" class="lbl">Pair shown</label>
			<select id="rp-pair" class="field" value={`${r.previewPair[0]}-${r.previewPair[1]}`} onchange={(e) => (r.previewPair = e.currentTarget.value.split('-').map(Number) as [number, number])} disabled={labels.length < 2} data-testid="preview-pair">
				{#each labels as a, i (i)}
					{#each labels as b, j (j)}
						{#if j > i}<option value="{i}-{j}">{a} and {b}</option>{/if}
					{/each}
				{/each}
			</select>
			{#if !current}<span class="aq-muted aq-small">Updating to the latest edits.</span>{/if}
		</div>
		<div class="plots" class:stale={!current}>
			{#each res.views as v, i (i)}
				<div class="plot" data-testid="preview-view-{i}"><ViewRenderer view={v} minHeight={170} /></div>
			{/each}
		</div>
	{:else}
		<p class="aq-muted aq-small empty">
			{#if mimo.refBusy}
				The control service is building the reference.
			{:else if !mimo.connected}
				The reference is drawn by the control service once it is connected.
			{:else if r.method === 'import'}
				Import a reference to see it here.
			{:else if r.method === 'field'}
				A reference built from field data is imported like any other; it is drawn here then.
			{:else}
				The reference is drawn once every table and pair holds numbers.
			{/if}
		</p>
	{/if}

	{#if res && current}
		<Findings items={[...res.errors.map((e) => ({ level: 'error' as const, message: e.message })), ...res.warnings.map((w) => ({ level: 'warning' as const, message: w.message }))]} testid="reference-findings" />

		<dl class="facts">
			<div><dt>Quantities</dt><dd class="num">{labels.length}</dd></div>
			<div><dt>Lines in band</dt><dd class="num">{res.lines || '-'}</dd></div>
			<div><dt>Band</dt><dd class="num">{res.band_hz ? `${formatNumber(res.band_hz[0])} to ${formatNumber(res.band_hz[1])} Hz` : '-'}</dd></div>
			<div><dt>Line spacing</dt><dd class="num">{formatNumber(res.df_hz, 4)} Hz</dd></div>
			<div>
				<dt>Smallest eigenvalue</dt>
				<dd class="num" data-testid="reference-min-eig">{eig(psd?.min_eigenvalue_ratio)}<span class="aq-muted"> of the line's largest</span></dd>
			</div>
			<div><dt>Carried</dt><dd>{res.reference ? (res.reference.kind === 'inline' ? 'In the setup' : 'As a reference folder') : '-'}</dd></div>
		</dl>

		{#if psd && psd.violations > 0}
			<div class="psd" data-testid="psd-report" data-accepted={psd.repair_accepted}>
				<h3>Lines No Signal Can Have</h3>
				<p class="aq-small">
					On <span class="num">{psd.violations}</span> {psd.violations === 1 ? 'line' : 'lines'} the coherences and phases cannot all hold at once; the worst is at <span class="num">{formatNumber(psd.worst_hz ?? 0)} Hz</span>, smallest eigenvalue <span class="num">{eig(psd.min_eigenvalue_ratio)}</span> of the line's largest.
					{#if psd.lines_hz.length}First lines: <span class="num">{psd.lines_hz.map((f) => formatNumber(f)).join(', ')} Hz</span>.{/if}
				</p>
				{#if psd.repair}
					<p class="aq-small">
						The repair sets each negative eigenvalue to zero and scales the result back to the PSDs entered, so every PSD stays as given and the coherences give way. On {psd.repair.lines} {psd.repair.lines === 1 ? 'line' : 'lines'} it would change these pairs most:
					</p>
					<table class="aq-table changes" data-testid="repair-changes">
						<thead><tr><th>Pair</th><th class="num">Coherence Entered</th><th class="num">After Repair</th><th class="num">At</th></tr></thead>
						<tbody>
							{#each psd.repair.pairs as p, i (i)}
								<tr><td>{p.labels[0]} and {p.labels[1]}</td><td class="num">{formatNumber(p.before, 3)}</td><td class="num">{formatNumber(p.after, 3)}</td><td class="num">{formatNumber(p.freq_hz)} Hz</td></tr>
							{/each}
						</tbody>
					</table>
					<div class="aq-actions">
						{#if psd.repair_accepted}
							<span class="accepted aq-small"><svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg> Repair accepted: the reference is the repaired one.</span>
							<button class="btn sm ghost" disabled={mimo.locked} onclick={() => (r.acceptRepair = false)}>Undo</button>
						{:else}
							<button class="btn sm primary" disabled={mimo.locked} onclick={() => (r.acceptRepair = true)} data-testid="accept-repair">Accept the Repair</button>
							<span class="aq-muted aq-small">Or change the coherence or phase of the pairs listed.</span>
						{/if}
					</div>
				{/if}
			</div>
		{/if}

		{#if res.provenance}
			<div class="prov" data-testid="reference-provenance">
				<h3>Provenance</h3>
				<dl class="facts">
					<div><dt>Made from</dt><dd>{METHOD[res.provenance.method] ?? res.provenance.method}</dd></div>
					<div><dt>Source</dt><dd>{res.provenance.source}</dd></div>
					<div><dt>Made by</dt><dd>{res.provenance.tool}</dd></div>
					{#if res.provenance.conjugated_on_import !== null}<div><dt>Cross terms</dt><dd>{res.provenance.conjugated_on_import ? 'Conjugated on import' : 'As in the file'}</dd></div>{/if}
				</dl>
				{#if res.provenance.assumptions.length}
					<ul class="assume aq-small">{#each res.provenance.assumptions as a, i (i)}<li>{a}</li>{/each}</ul>
				{/if}
			</div>
		{/if}
	{/if}

	{#if inSetup}
		<div class="insetup aq-small" data-testid="reference-in-setup">
			<h3>In the Setup's Check</h3>
			<p>
				{inSetup.n} by {inSetup.n} on <span class="num">{inSetup.lines}</span> lines{#if inSetup.band_hz}, {formatNumber(inSetup.band_hz[0])} to {formatNumber(inSetup.band_hz[1])} Hz{/if}.
				{#if inSetup.rank_min !== null}Rank {inSetup.rank_min === inSetup.rank_max ? inSetup.rank_min : `${inSetup.rank_min} to ${inSetup.rank_max}`}.{/if}
				{#if inSetup.repaired_lines}The run repairs {inSetup.repaired_lines} {inSetup.repaired_lines === 1 ? 'line' : 'lines'} at rounding level.{/if}
			</p>
		</div>
	{/if}
</section>

<style>
	.state {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		color: var(--text-2);
	}

	.pair-pick {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px var(--sp-3);
		margin-bottom: var(--sp-2);
	}

	.pair-pick .field {
		max-width: 280px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.plots {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--sp-2);
	}

	.plots.stale {
		opacity: 0.55;
	}

	.plot {
		min-width: 0;
	}

	.empty {
		display: grid;
		place-items: center;
		min-height: 160px;
		margin: 0;
		padding: var(--sp-3);
		text-align: center;
		background: var(--bg-plot);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-md);
	}

	.facts {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
		gap: var(--sp-2) var(--sp-3);
		margin: var(--sp-3) 0 0;
	}

	.facts dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.facts dd {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	h3 {
		margin: var(--sp-4) 0 var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.psd {
		margin-top: var(--sp-3);
		padding: var(--sp-3);
		border: 1.5px solid var(--amber-2);
		border-radius: var(--radius-md);
	}

	.psd h3 {
		margin-top: 0;
	}

	.psd p {
		margin: 0 0 var(--sp-2);
	}

	.changes {
		margin-bottom: var(--sp-2);
	}

	.accepted {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-weight: var(--weight-strong);
	}

	.assume {
		margin: var(--sp-2) 0 0;
		padding-left: 1.2em;
		color: var(--text-2);
	}

	.insetup p {
		color: var(--text-2);
	}
</style>
