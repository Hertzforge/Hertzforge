<script lang="ts">
	/**
	 * The reference: the spectral density matrix the test controls to, made one of four ways.
	 *
	 * - Import: a reference file on the computer that runs the test, read by the service by path.
	 * - Breakpoints: each controlled quantity's PSD as a breakpoint table (the single-output
	 *   profile editor), and the pairs' coherence and phase.
	 * - Rigid body: the control sensors' positions and directions, whose fit gives the six
	 *   rigid-body degrees of freedom; the reference is then entered on those six.
	 * - From field data: built from measured records in the analysis workspace, then imported.
	 *
	 * Whichever way it is made, the control service builds and checks it; the preview beside the
	 * editor shows what it made.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import ProfileEditor from '../ProfileEditor.svelte';
	import type { ProfileHost } from '../setup';
	import { defaultTable, rowName, type ReferenceMethod } from './mimoSetup';
	import type { MimoState } from './mimoStore.svelte';
	import MimoReferencePreview from './MimoReferencePreview.svelte';
	import PairTable from './PairTable.svelte';

	interface Props {
		mimo: MimoState;
		/** Open the analysis workspace. */
		onanalyze: () => void;
	}
	let { mimo, onanalyze }: Props = $props();

	const d = $derived(mimo.setup);
	const r = $derived(d.reference);
	const locked = $derived(mimo.locked);
	const E = $derived(mimo.fieldErrors);
	const W = $derived(mimo.fieldWarnings);
	const labels = $derived(mimo.labels);
	const physical = $derived(mimo.space.kind === 'physical');

	const TABS: { id: ReferenceMethod; label: string }[] = [
		{ id: 'import', label: 'Import' },
		{ id: 'breakpoints', label: 'Breakpoints' },
		{ id: 'rigid', label: 'Rigid Body' },
		{ id: 'field', label: 'From Field Data' }
	];

	let q = $state(0);
	let tablist = $state<HTMLElement | null>(null);
	const qi = $derived(Math.min(q, Math.max(0, labels.length - 1)));
	const label = $derived(labels[qi] ?? '');

	// Every quantity has its table before an editor reads it.
	$effect(() => {
		if (locked) return;
		for (const l of labels) if (!d.reference.tables[l]) d.reference.tables[l] = defaultTable();
	});

	/** The profile editor's view of one quantity's table: its rows, its bands, its findings. */
	const host = $derived.by((): ProfileHost | null => {
		const rows = d.reference.tables[label];
		if (!rows) return null;
		const ch = physical ? mimo.channels.controls[qi] : null;
		const bands = ch ? d.controls[String(ch.index)] : null;
		const prefix = `reference.tables.${qi}.`;
		const strip = (map: Record<string, string[]>) => {
			const out: Record<string, string[]> = {};
			for (const [k, v] of Object.entries(map)) {
				if (k.startsWith(prefix)) out[k.slice(prefix.length)] = v;
				if (ch && k.startsWith(`control.${ch.index}.`)) out[`profile.${k.slice(`control.${ch.index}.`.length)}`] = v;
			}
			return out;
		};
		return {
			setup: { title: d.title, breakpoints: rows, alarm_db: bands?.alarm_db ?? r.alarm_db, abort_db: bands?.abort_db ?? r.abort_db },
			locked,
			fieldErrors: strip(E),
			fieldWarnings: strip(W),
			tableFiles: mimo.acq.tableFiles
		};
	});

	function copyToAll() {
		const src = d.reference.tables[label];
		if (!src || locked) return;
		for (const l of labels) if (l !== label) d.reference.tables[l] = structuredClone($state.snapshot(src));
	}

	function onTabKey(e: KeyboardEvent) {
		const ids = TABS.map((t) => t.id);
		const i = ids.indexOf(r.method);
		let next = -1;
		if (e.key === 'ArrowRight') next = (i + 1) % ids.length;
		else if (e.key === 'ArrowLeft') next = (i - 1 + ids.length) % ids.length;
		else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = ids.length - 1;
		if (next < 0) return;
		e.preventDefault();
		mimo.setMethod(ids[next]);
		tablist?.querySelector<HTMLButtonElement>(`[data-ref-tab="${ids[next]}"]`)?.focus();
	}

	// ---- rigid body ----------------------------------------------------------------------
	const sensors = $derived(mimo.channels.controls);
	$effect(() => {
		if (locked) return;
		for (const s of sensors) d.reference.rigid.sensors[String(s.index)] ??= { position: [0, 0, 0], direction: [0, 0, 1] };
	});
	const fit = $derived(r.method === 'rigid' ? mimo.rigid : null);
	const share = (label: string) => fit?.sensors.find((s) => s.label === label)?.check_share ?? null;
	const AXES = ['x', 'y', 'z'] as const;
	const importing = $derived(mimo.refBusy && r.method === 'import');
</script>

<div class="refwrap">
	<div class="tabs" role="tablist" aria-label="How the reference is made" bind:this={tablist} tabindex="-1" onkeydown={onTabKey}>
		{#each TABS as t (t.id)}
			<button role="tab" data-ref-tab={t.id} data-testid="ref-tab-{t.id}" aria-selected={r.method === t.id} tabindex={r.method === t.id ? 0 : -1} class:on={r.method === t.id} disabled={locked && r.method !== t.id} onclick={() => mimo.setMethod(t.id)}>{t.label}</button>
		{/each}
	</div>

	<div class="cols">
		<div class="stack" role="tabpanel" aria-label={TABS.find((t) => t.id === r.method)?.label}>
			{#if r.method === 'import'}
				<section class="aq-panel" aria-labelledby="ri-title" data-testid="ref-import">
					<div class="aq-head"><h2 id="ri-title">Import a Reference</h2></div>
					<div class="path">
						<label for="ri-path" class="lbl">File or folder</label>
						<input id="ri-path" class="field mono" bind:value={r.importPath} disabled={locked} placeholder="Full path on the computer that runs the test" data-testid="ref-import-path" />
						<label for="ri-pairs" class="lbl">Pair table (CSV only)</label>
						<input id="ri-pairs" class="field mono" bind:value={r.importPairPath} disabled={locked} placeholder="Optional: the coherence and phase table" />
					</div>
					<div class="aq-fields narrow">
						<div class="sel">
							<label for="ri-unit" class="lbl">Control unit</label>
							<input id="ri-unit" class="field" bind:value={r.unit} disabled={locked} />
						</div>
					</div>
					<div class="aq-actions go">
						<button class="btn primary sm" disabled={locked || !r.importPath.trim() || importing || !mimo.connected} onclick={() => mimo.importReference()} data-testid="ref-import-go">{importing ? 'Reading' : 'Import'}</button>
						<Findings items={(E['reference.import'] ?? []).map((message) => ({ level: 'error' as const, message }))} />
					</div>
					<div class="accepts aq-small">
						<h3>What It Reads</h3>
						<ul>
							<li><strong>A reference folder</strong> saved by this program: the matrix, its tolerances, any transformation and how it was made, checked by its content hash.</li>
							<li><strong>An .npz specification file</strong> with the frequencies and the spectral matrix, placed on this test's lines (its spacing must match the frame under Control).</li>
							<li><strong>A CSV table</strong>: a header of <span class="mono">frequency_hz</span> then one column per control, one row per frequency, PSDs in the control unit squared per hertz. A second CSV may give pairs: <span class="mono">frequency_hz, row, column, coherence, phase_deg</span>.</li>
						</ul>
						<p class="aq-muted">Anything else: save it as the CSV table above and import that. The file stays where it is; the control service reads it by its path, so give the path as the computer running the test sees it.</p>
					</div>
				</section>
			{:else if r.method === 'breakpoints' || r.method === 'rigid'}
				{#if r.method === 'rigid'}
					<section class="aq-panel" aria-labelledby="rr-title" data-testid="ref-rigid">
						<div class="aq-head">
							<h2 id="rr-title">Sensor Geometry</h2>
							<span class="aq-sub">Each control sensor's position and sensing direction; the reference point the six degrees of freedom are written at.</span>
						</div>
						{#if sensors.length}
							<div class="aq-scroll">
								<table class="aq-table geo">
									<thead>
										<tr><th>Sensor</th>{#each AXES as a (a)}<th class="num">Position {a}</th>{/each}{#each AXES as a (a)}<th class="num">Direction {a}</th>{/each}<th class="num">Checked by Others</th></tr>
									</thead>
									<tbody>
										{#each sensors as s, i (s.index)}
											{@const g = d.reference.rigid.sensors[String(s.index)]}
											{@const c = share(rowName(s))}
											{#if g}
												<tr data-testid="rigid-row-{i}">
													<td>{rowName(s)}</td>
													{#each AXES as a, k (a)}<td class="num"><NumberField id="rg-{i}-p{k}" label="{rowName(s)} position {a}" compact unit={d.reference.rigid.length_unit} bind:value={g.position[k]} disabled={locked} /></td>{/each}
													{#each AXES as a, k (a)}<td class="num"><NumberField id="rg-{i}-d{k}" label="{rowName(s)} direction {a}" compact bind:value={g.direction[k]} disabled={locked} /></td>{/each}
													<td class="num share" class:low={c !== null && c < 0.2}>
														{#if c === null}-{:else}{c.toFixed(2)}{#if c < 0.2}<span class="flag" title="The other sensors barely check this one: a sensor here on a flexible part would not show.">Unchecked</span>{/if}{/if}
													</td>
												</tr>
											{/if}
										{/each}
									</tbody>
								</table>
							</div>
						{:else}
							<p class="aq-muted aq-small">Give at least six channels the Control role, under Channels.</p>
						{/if}
						<div class="aq-fields refpt">
							{#each AXES as a, k (a)}
								<NumberField id="rg-o{k}" label="Reference point {a}" unit={d.reference.rigid.length_unit} bind:value={d.reference.rigid.reference_point[k]} disabled={locked} />
							{/each}
							<div class="sel">
								<label for="rg-len" class="lbl">Length unit</label>
								<select id="rg-len" class="field" bind:value={d.reference.rigid.length_unit} disabled={locked}>
									{#each ['m', 'mm', 'in'] as u (u)}<option value={u}>{u}</option>{/each}
								</select>
							</div>
						</div>
						{#if fit}
							<div class="fit aq-small" data-testid="rigid-fit" data-ok={fit.ok}>
								{#if fit.ok}
									<span><svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg> The sensors resolve all six degrees of freedom.</span>
									<span>Condition <span class="num">{fit.condition?.toFixed(1)}</span></span>
									{#if fit.dof_units}<span>Translations in {fit.dof_units[0]}, rotations in {fit.dof_units[3]}</span>{/if}
								{/if}
								<Findings items={[...fit.errors.map((e) => ({ level: 'error' as const, message: e.message })), ...(fit.warnings ?? []).map((w) => ({ level: 'warning' as const, message: w.message }))]} />
							</div>
						{/if}
						<p class="aq-muted aq-small">
							"Checked by others" is how much of each sensor's reading the other sensors can check, from 0 to 1. A sensor near 0 is the only one in its direction: if it sits on a part that does not move rigidly, the fit cannot show it.
						</p>
					</section>
				{/if}

				{#if labels.length && host}
					<div class="quantity">
						<label for="rq" class="lbl">{r.method === 'rigid' ? 'Degree of freedom' : physical ? 'Control' : 'Degree of freedom'}</label>
						<div class="qpick">
							<button class="btn sm icon" aria-label="Previous" disabled={qi === 0} onclick={() => (q = qi - 1)}>
								<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
							</button>
							<select id="rq" class="field" value={qi} onchange={(e) => (q = Number(e.currentTarget.value))} data-testid="ref-quantity">
								{#each labels as l, i (i)}<option value={i}>{l}</option>{/each}
							</select>
							<button class="btn sm icon" aria-label="Next" disabled={qi >= labels.length - 1} onclick={() => (q = qi + 1)}>
								<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
							</button>
							<span class="aq-muted aq-small num">{qi + 1} of {labels.length}</span>
						</div>
						<button class="btn sm ghost" disabled={locked || labels.length < 2} onclick={copyToAll} data-testid="ref-copy-all">Copy This Table to All</button>
					</div>
					{#key label}
						<ProfileEditor
							acq={host}
							heading={`PSD of ${label}`}
							sub={physical ? 'Breakpoints at full level. The bands are this control channel\'s, the same as under Channels.' : 'Breakpoints at full level, in degree-of-freedom space.'}
							showTitle={false}
							rowBands={false}
							showBands={physical}
							unit={r.method === 'rigid' && qi >= 3 ? `(${r.unit}/${d.reference.rigid.length_unit})²/Hz` : `${r.unit}²/Hz`}
							fileName="Reference Profile"
						/>
					{/key}
					<PairTable {mimo} {labels} />
				{:else if r.method === 'breakpoints'}
					<p class="aq-panel aq-muted aq-small">Give channels the Control role under Channels; each control gets its breakpoint table here.</p>
				{/if}
			{:else}
				<section class="aq-panel" aria-labelledby="rf-title" data-testid="ref-field">
					<div class="aq-head"><h2 id="rf-title">A Reference From Field Data</h2></div>
					<p class="aq-small body">
						A reference can be built from measured records, from the field or from flight: the spectral matrix across the control sensors is estimated from the records, several records are combined line by line (averaged, enveloped, or taken at a percentile), made realizable, and given its tolerance bands.
					</p>
					<ol class="steps aq-small">
						<li>Open the records in the analysis workspace and build the reference across the control sensors, in the same order as the controls here.</li>
						<li>Save it as a reference.</li>
						<li>Import it here, under Import.</li>
					</ol>
					<div class="aq-actions">
						<button class="btn sm" onclick={onanalyze} data-testid="ref-field-analyze">Open the Signal Processing Module</button>
						<button class="btn sm ghost" disabled={locked} onclick={() => mimo.setMethod('import')}>Go to Import</button>
					</div>
				</section>
			{/if}
		</div>
		<div class="stack">
			<MimoReferencePreview {mimo} />
		</div>
	</div>
</div>

<style>
	.refwrap {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}

	.tabs {
		display: inline-flex;
		align-self: flex-start;
		gap: 2px;
		padding: 2px;
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.tabs button {
		height: 28px;
		padding: 0 14px;
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
		color: var(--text-2);
		background: transparent;
		border: 1px solid transparent;
		border-radius: calc(var(--radius-md) - 2px);
	}

	.tabs button.on {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		background: var(--bg-1);
		border-color: var(--line-2);
		box-shadow: inset 0 -2px 0 var(--blue-2);
	}

	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1.2fr) minmax(360px, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.path {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		margin-bottom: var(--sp-3);
	}

	.mono {
		font-family: var(--font-mono);
	}

	.narrow {
		max-width: 260px;
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.go {
		margin-top: var(--sp-3);
		align-items: center;
	}

	.accepts h3 {
		margin: var(--sp-4) 0 var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.accepts ul,
	.steps {
		margin: 0 0 var(--sp-2);
		padding-left: 1.2em;
		color: var(--text-2);
		line-height: 1.5;
	}

	.accepts li + li,
	.steps li + li {
		margin-top: 4px;
	}

	.body {
		margin: 0 0 var(--sp-3);
		color: var(--text-2);
		max-width: 72ch;
	}

	.quantity {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px var(--sp-3);
	}

	.qpick {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.qpick .field {
		min-width: 180px;
	}

	.qpick span {
		white-space: nowrap;
	}

	.geo td.num :global(.nf) {
		min-width: 92px;
	}

	.share {
		white-space: nowrap;
	}

	.share.low {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.flag {
		margin-left: 6px;
		padding: 0 5px;
		font-size: var(--text-2xs);
		border: 1px solid currentColor;
		border-radius: 3px;
	}

	.refpt {
		margin-top: var(--sp-3);
	}

	.fit {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px var(--sp-4);
		margin: var(--sp-3) 0;
	}

	.fit span {
		display: inline-flex;
		align-items: center;
		gap: 5px;
	}

	@media (max-width: 1180px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
