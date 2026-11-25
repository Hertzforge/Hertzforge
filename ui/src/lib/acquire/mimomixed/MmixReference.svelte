<script lang="ts">
	/**
	 * The floor's reference: the spectral density matrix the floor is controlled to, as MIMO Random
	 * makes it. Either each control's PSD as a breakpoint table (the single-output profile editor,
	 * with the control's own alarm and abort bands) and one coherence and phase for every pair, or a
	 * reference file read by the service by its path. The service builds and checks the matrix; the
	 * preview shows what it made. The floor's band is the test's band: every tone and narrowband
	 * plays inside it.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import ProfileEditor from '../ProfileEditor.svelte';
	import type { ProfileHost } from '../setup';
	import { mmixTitle } from './mmixSetup';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const d = $derived(mm.setup);
	const r = $derived(d.reference);
	const locked = $derived(mm.locked);
	const E = $derived(mm.fieldErrors);
	const W = $derived(mm.fieldWarnings);
	const labels = $derived(mm.controlLabels);
	const ref = $derived(mm.ref && mm.refKey === mm.refParamsKey ? mm.ref : null);

	let q = $state(0);
	const qi = $derived(Math.min(q, Math.max(0, labels.length - 1)));

	/** The profile editor's view of one control's table and its bands. */
	const host = $derived.by((): ProfileHost | null => {
		const c = d.controls[qi];
		if (!c) return null;
		const prefix = `controls.${qi}.`;
		const strip = (map: Record<string, string[]>) => {
			const out: Record<string, string[]> = {};
			for (const [k, v] of Object.entries(map)) {
				if (k.startsWith(`${prefix}rows.`)) out[`profile.breakpoints.${k.slice(`${prefix}rows.`.length)}`] = v;
				else if (k === `${prefix}rows`) out['profile.breakpoints'] = v;
				else if (k === `${prefix}alarm_db` || k === `${prefix}abort_db`) out[`profile.${k.slice(prefix.length)}`] = v;
			}
			return out;
		};
		return {
			setup: { get title() { return d.title; }, set title(v: string) { d.title = v; }, breakpoints: c.rows, alarm_db: c.alarm_db, abort_db: c.abort_db },
			locked,
			fieldErrors: strip(E),
			fieldWarnings: strip(W),
			tableFiles: mm.acq.tableFiles
		};
	});

	function copyToAll() {
		const src = d.controls[qi];
		if (!src || locked) return;
		for (const c of d.controls) if (c !== src) {
			c.rows = structuredClone($state.snapshot(src.rows));
			c.alarm_db = [...src.alarm_db] as typeof c.alarm_db;
			c.abort_db = [...src.abort_db] as typeof c.abort_db;
		}
	}
</script>

<div class="stack">
	<div class="aq-seg" role="radiogroup" aria-label="How the floor's reference is made">
		<button type="button" role="radio" aria-checked={r.method === 'breakpoints'} disabled={locked} onclick={() => (r.method = 'breakpoints')} data-testid="mmix-ref-breakpoints">Breakpoints</button>
		<button type="button" role="radio" aria-checked={r.method === 'import'} disabled={locked} onclick={() => (r.method = 'import')} data-testid="mmix-ref-import">Import</button>
	</div>

	{#if r.method === 'import'}
		<section class="aq-panel" aria-labelledby="mmix-ri" data-testid="mmix-ref-import-panel">
			<div class="aq-head"><h2 id="mmix-ri">Import a Reference</h2><span class="aq-sub">A reference folder, an .npz specification or a CSV table, read by its path on the computer that runs the test, as MIMO Random reads it.</span></div>
			<div class="path">
				<label for="mmix-ri-path" class="lbl">File or folder</label>
				<input id="mmix-ri-path" class="field mono" bind:value={r.importPath} disabled={locked} placeholder="Full path on the computer that runs the test" data-testid="mmix-ref-import-path" />
			</div>
			<div class="aq-actions">
				<button class="btn primary sm" disabled={locked || !r.importPath.trim() || mm.refBusy || !mm.connected} onclick={() => mm.importReference()} data-testid="mmix-ref-import-go">{mm.refBusy ? 'Reading' : 'Import'}</button>
			</div>
		</section>
	{:else if labels.length && host}
		<div class="quantity">
			<label for="mmix-rq" class="lbl">Control</label>
			<select id="mmix-rq" class="field" value={qi} onchange={(e) => (q = Number(e.currentTarget.value))} data-testid="mmix-ref-control">
				{#each labels as l, i (i)}<option value={i}>{l}</option>{/each}
			</select>
			<span class="aq-muted aq-small num">{qi + 1} of {labels.length}</span>
			<button class="btn sm ghost" disabled={locked || labels.length < 2} onclick={copyToAll} data-testid="mmix-ref-copy-all">Copy This Table to All</button>
		</div>
		{#key qi}
			<ProfileEditor acq={host} heading={`Floor PSD at ${labels[qi]}`} sub="Breakpoints at full level, with this control's alarm and abort bands." rowBands={false} unit={`${r.unit}²/Hz`} fileName="Floor Profile" titleHint={mmixTitle(d)} />
		{/key}
		<section class="aq-panel" aria-labelledby="mmix-pairs" data-testid="mmix-ref-pairs">
			<div class="aq-head"><h2 id="mmix-pairs">Between the Controls</h2><span class="aq-sub">One squared coherence and one phase for every pair. A coherence of 0 makes the controls independent.</span></div>
			<div class="aq-fields narrow">
				<NumberField id="mmix-ref-coh" label="Coherence" step={0.05} min={0} max={1} bind:value={r.coherence} errors={E['reference.coherence']} disabled={locked} testid="mmix-ref-coherence" />
				<NumberField id="mmix-ref-ph" label="Phase" unit="deg" step={5} bind:value={r.phase_deg} errors={E['reference.phase_deg']} disabled={locked || r.coherence === 0} testid="mmix-ref-phase" />
				<label class="aq-check"><input type="checkbox" bind:checked={r.acceptRepair} disabled={locked} /> Accept the service's repair if the matrix is not realizable</label>
			</div>
		</section>
	{:else}
		<p class="aq-panel aq-muted aq-small">Give channels the Control role under Channels; each control gets its breakpoint table here.</p>
	{/if}

	<section class="aq-panel" aria-labelledby="mmix-ref-pv" data-testid="mmix-ref-preview">
		<div class="aq-head"><h2 id="mmix-ref-pv">Reference</h2><span class="aq-sub">{ref?.ok && ref.band_hz ? `As the service built it: ${ref.lines} lines from ${ref.band_hz[0]} to ${ref.band_hz[1]} Hz.` : 'As the service builds it.'}</span></div>
		<Findings items={(E.reference ?? []).map((message) => ({ level: 'error' as const, message }))} testid="mmix-ref-findings" />
		{#if ref?.ok && ref.views.length}
			{#each ref.views as v, i (i)}<ViewRenderer view={v} minHeight={200} />{/each}
		{:else}
			<p class="aq-small aq-muted">{mm.refBusy ? 'Building the reference.' : 'Drawn once the tables are complete.'}</p>
		{/if}
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.quantity,
	.path {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}

	.path input {
		flex: 1;
		min-width: 240px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}
</style>
