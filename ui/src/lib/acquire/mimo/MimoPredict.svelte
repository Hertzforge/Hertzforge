<script lang="ts">
	/**
	 * The prediction, after identification and before anything drives at test level: what the
	 * chosen law would ask of each drive against its ceiling, and the error it would leave on
	 * each control against the tolerances, computed by the service from the identified plant.
	 * The operator can predict again (the predicted response fed back as if measured, so a
	 * closed-loop law shows where it would settle), choose another law or other parameters, and
	 * then engage; after a failed gate, only an override in the operator's own words engages.
	 */
	import ParamForm from '$lib/forms/ParamForm.svelte';
	import type { ParamValues } from '$lib/forms/params';
	import type { SdmView } from '$lib/protocol/types';
	import { formatFixed, formatNumber } from '$lib/viz/format';
	import MatrixGrid from '$lib/viz/MatrixGrid.svelte';
	import { pointsForWidth } from '../display/mimoLayout';
	import Findings from '../Findings.svelte';
	import OverrideDialog from '../OverrideDialog.svelte';
	import { db } from '../words';
	import { controlOf, lawParamDescriptor } from './mimoSetup';
	import type { MimoState } from './mimoStore.svelte';

	interface Props {
		mimo: MimoState;
	}
	let { mimo }: Props = $props();

	const s = $derived(mimo.status);
	const p = $derived(s?.prediction ?? null);
	const gates = $derived(s?.gates ?? null);
	const review = $derived(s?.state === 'review');
	const gateFailed = $derived(s?.phase === 'gate_failed');
	const predictionExceeded = $derived(s?.phase === 'prediction_exceeded');
	const labels = $derived(s?.shape?.labels ?? mimo.labels);
	const driveLabels = $derived(s?.shape?.drive_labels ?? mimo.channels.drives.map((r) => r.description));
	const physical = $derived((s?.shape?.space ?? mimo.space.kind) === 'physical');
	const moreControls = $derived(s?.shape?.shape === 'more_controls_than_drives');
	const law = $derived(mimo.law);
	const descriptors = $derived((law?.params ?? []).map(lawParamDescriptor).filter((x) => x !== null));
	const canEdit = $derived(review && mimo.isController);
	let overrideOpen = $state(false);
	/** Controls marked to exclude at a failed gate, and the operator's reason. */
	let excludeSet = $state<number[]>([]);
	let excludeWhy = $state('');
	const excluded = $derived(s?.excluded ?? []);
	function toggleExclude(i: number) {
		excludeSet = excludeSet.includes(i) ? excludeSet.filter((x) => x !== i) : [...excludeSet, i];
	}
	async function exclude() {
		if (!excludeSet.length || !excludeWhy.trim()) return;
		if (await mimo.exclude([...excludeSet].sort((a, b) => a - b), excludeWhy.trim())) {
			excludeSet = [];
			excludeWhy = '';
		}
	}

	/**
	 * The tolerances a controlled quantity is judged by: the run's own (a control row's, or a
	 * degree of freedom's under virtual control), or the draft's while there is no run.
	 */
	function bands(i: number): { alarm: [number, number]; abort: [number, number] } | null {
		const own = s?.shape?.bands?.[i];
		if (own) return { alarm: own.alarm_db, abort: own.abort_db };
		if (!physical) return null;
		const r = mimo.channels.controls[i];
		if (!r) return null;
		const c = controlOf(mimo.setup, r.index);
		return { alarm: c.alarm_db, abort: c.abort_db };
	}

	// The achievable reference, when controls outnumber drives: a window of up to eight
	// controls drawn as the run's matrix grid draws the measured matrix.
	$effect(() => {
		mimo.predictShown = true;
		const stop = mimo.predictFeed.watch();
		return () => {
			stop();
			mimo.predictShown = false;
		};
	});
	let gridWidth = $state(0);
	$effect(() => {
		if (gridWidth > 0) mimo.predictPoints = pointsForWidth(gridWidth / 3, Math.min(2, globalThis.devicePixelRatio || 1));
	});
	const achView = $derived((mimo.predictFeed.plotData('predict:achievable')?.views[0] as SdmView | undefined) ?? null);
	const achFirst = $derived(Math.min(mimo.predictFirst, Math.max(0, labels.length - 8)));
	const achNames = $derived(labels.slice(achFirst, achFirst + 8));
	const achSpectra = $derived(achNames.map(() => null));
	const unit = $derived(achView?.unit ?? 'g²/Hz');
	const controlNames = $derived(mimo.channels.controls.map((r) => r.description || `Channel ${r.index}`));

	const VERDICT_WORDS: Record<string, { word: string; tone: 'pass' | 'warn' | 'fail' | 'info' }> = {
		pass: { word: 'Within rating', tone: 'pass' },
		advisory: { word: 'Advisory', tone: 'warn' },
		exceeded: { word: 'Exceeded', tone: 'fail' },
		not_evaluated: { word: 'No shaker record', tone: 'info' }
	};

	function errorVerdict(e: number, b: ReturnType<typeof bands>): { word: string; tone: 'pass' | 'warn' | 'fail' | 'info' } {
		if (!Number.isFinite(e)) return { word: '-', tone: 'info' };
		if (!b) return { word: 'Shown', tone: 'info' };
		if (e > b.abort[1] || e < -b.abort[0]) return { word: 'Outside abort', tone: 'fail' };
		if (e > b.alarm[1] || e < -b.alarm[0]) return { word: 'Outside alarm', tone: 'warn' };
		return { word: 'Within alarm', tone: 'pass' };
	}

	/** Where a value sits on a 0-to-`max` bar, clamped, as a percentage. */
	const pct = (v: number, max: number) => `${Math.max(0, Math.min(100, (v / max) * 100))}%`;
	const ERR_SPAN = 12;
	const errPos = (e: number) => `${50 + Math.max(-50, Math.min(50, (e / ERR_SPAN) * 50))}%`;
	const iterating = $derived(mimo.busy === 'Predict Again');
</script>

<div class="predict" data-testid="mimo-predict" data-state={s?.state ?? 'none'} data-phase={s?.phase ?? ''}>
	{#if !p}
		<section class="aq-panel">
			<div class="aq-head"><h2>Prediction</h2></div>
			<p class="aq-muted aq-small">
				{#if s?.state === 'running' && (s.phase === 'sysid' || s.phase === 'sysid_noise')}
					Identification is running; the prediction appears here when it is done.
				{:else}
					After identification the service predicts what the law would ask of each drive and leave on each control, and holds here until you engage. Start the run from the run bar to identify the system.
				{/if}
			</p>
		</section>
	{:else}
		<div class="cols">
			<div class="stack">
				<section class="aq-panel" aria-labelledby="pg-title" data-testid="predict-gates" data-passed={gates?.passed ?? ''}>
					<div class="aq-head">
						<h2 id="pg-title">Identification Gates</h2>
						<span class="aq-spacer"></span>
						{#if gates}
							<span class="verdict {gates.passed ? 'pass' : 'fail'}">
								{#if gates.passed}
									<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg> Passed
								{:else}
									<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /></svg> Failed
								{/if}
							</span>
						{/if}
					</div>
					{#if gates}
						<dl class="facts">
							<div><dt>Smallest multiple coherence</dt><dd class="num">{formatFixed(gates.coherence_min, 3)} <span class="aq-muted">floor {formatFixed(gates.coherence_floor, 2)}</span></dd></div>
							<div><dt>Largest condition number</dt><dd class="num">{formatNumber(gates.condition_max, 4)} <span class="aq-muted">limit {formatNumber(gates.condition_limit, 4)}</span></dd></div>
						</dl>
					{/if}
					{#if gateFailed}
						<p class="aq-small warnline">A gate failed: some controls are not explained by the drives, or the plant is nearly singular on some lines. Exclude a control (it stays measured, as a monitor), override in your own words, or change the setup and run again.</p>
					{/if}
					{#if gates?.coherence?.length}
						<table class="aq-table compact" data-testid="predict-gate-controls">
							<thead><tr><th>{physical ? 'Control' : 'Degree of freedom'}</th><th class="num">Multiple coherence</th><th>Against the floor</th>{#if gateFailed && physical}<th>Exclude</th>{/if}</tr></thead>
							<tbody>
								{#each gates.coherence as c, i (i)}
									{@const low = c !== null && c < gates.coherence_floor}
									<tr data-testid="gate-control-{i}" data-low={low}>
										<td>{labels[i] ?? `Control ${i + 1}`}</td>
										<td class="num">{c === null ? '-' : formatFixed(c, 4)}</td>
										<td class="verdict {excluded.includes(i) ? 'info' : low ? 'fail' : 'pass'}">{excluded.includes(i) ? 'Excluded, kept as a monitor' : low ? 'Below the floor' : 'Above the floor'}</td>
										{#if gateFailed && physical}
											<td><input type="checkbox" aria-label="Exclude {labels[i] ?? `control ${i + 1}`}" checked={excludeSet.includes(i)} disabled={!mimo.isController || excluded.includes(i)} onchange={() => toggleExclude(i)} data-testid="gate-exclude-{i}" /></td>
										{/if}
									</tr>
								{/each}
							</tbody>
						</table>
						{#if gateFailed && physical && excludeSet.length}
							<div class="excl">
								<label class="lbl" for="pg-why">Why these controls are excluded</label>
								<input id="pg-why" class="field" bind:value={excludeWhy} placeholder="For example: sits on a fixture mode the shakers cannot reach." data-testid="gate-exclude-why" />
								<button class="btn" disabled={!excludeWhy.trim() || mimo.busy !== null} onclick={exclude} data-testid="gate-exclude-go">Exclude {excludeSet.length === 1 ? 'This Control' : `${excludeSet.length} Controls`}</button>
							</div>
							<p class="aq-muted aq-small">The reference is reduced to the controls kept and the gate is judged again on them; the reason is kept with the run.</p>
						{/if}
					{/if}
				</section>

				<section class="aq-panel" aria-labelledby="pd-title" data-testid="predict-drives" data-pass={p.drives_pass}>
					<div class="aq-head">
						<h2 id="pd-title">Drive per Output</h2>
						<span class="aq-sub">Predicted rms against each ceiling</span>
						<span class="aq-spacer"></span>
						<span class="verdict {p.drives_pass ? 'pass' : 'fail'}">{p.drives_pass ? 'Every drive within its ceiling' : 'A drive over its ceiling'}</span>
					</div>
					<table class="aq-table bars">
						<thead><tr><th>Drive</th><th class="num">Predicted</th><th class="num">Ceiling</th><th class="barcol">Of the Ceiling</th></tr></thead>
						<tbody>
							{#each p.drive_rms_v as v, k (k)}
								{@const ceil = p.ceiling_vrms[k]}
								{@const over = v > ceil}
								<tr data-testid="predict-drive-{k}" data-over={over}>
									<td>{driveLabels[k] ?? `Drive ${k + 1}`}</td>
									<td class="num">{formatFixed(v, 3)} V</td>
									<td class="num">{formatFixed(ceil, 2)} V</td>
									<td class="barcol">
										<span class="bar" aria-hidden="true"><span class="fill" class:over style:width={pct(v, ceil * 1.25)}></span><span class="tick" style:left="80%"></span></span>
										<span class="word" class:over>{over ? `Over by ${Math.round((v / ceil - 1) * 100)}%` : `${Math.round((v / ceil) * 100)}%`}</span>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
					<p class="aq-muted aq-small">The bar's tick is the ceiling. The core caps a drive at its ceiling during the run, so a drive over it would leave its controls short of the reference.</p>
				</section>

				{#if p.drive_accel?.length}
					<section class="aq-panel" aria-labelledby="pa-title" data-testid="predict-accel">
						<div class="aq-head">
							<h2 id="pa-title">Shaker Acceleration</h2>
							<span class="aq-sub">Predicted at full level from the identified plant and the predicted drive</span>
						</div>
						<table class="aq-table">
							<thead><tr><th>Drive</th><th>Read at</th><th class="num">Predicted</th><th>Shaker</th></tr></thead>
							<tbody>
								{#each p.drive_accel as a, k (k)}
									{@const v = VERDICT_WORDS[p.feasibility?.[k]?.verdict ?? 'not_evaluated']}
									<tr data-testid="predict-accel-{k}" data-verdict={p.feasibility?.[k]?.verdict ?? 'not_evaluated'}>
										<td>{driveLabels[k] ?? `Drive ${k + 1}`}</td>
										<td>{controlNames[p.accel_control?.[k] ?? -1] ?? '-'}</td>
										<td class="num">{formatNumber(a, 3)} {unit.split('²')[0]} rms</td>
										<td class="verdict {v.tone}" title={p.feasibility?.[k]?.message || undefined}>{v.word}{#if p.feasibility?.[k]?.message && v.tone !== 'pass' && v.tone !== 'info'}<span class="aq-muted">: {p.feasibility[k].message}</span>{/if}</td>
									</tr>
								{/each}
							</tbody>
						</table>
						<p class="aq-muted aq-small">Each drive is read at the control it moves most. Where a drive has a shaker record, its ratings are checked against this acceleration rather than the estimate the run armed on.</p>
					</section>
				{/if}

				<section class="aq-panel" aria-labelledby="pc-title" data-testid="predict-controls">
					<div class="aq-head">
						<h2 id="pc-title">Error per Control</h2>
						<span class="aq-sub">Predicted rms error over the band, against the reference</span>
					</div>
					<table class="aq-table bars">
						<thead><tr><th>{physical ? 'Control' : 'Degree of freedom'}</th><th class="num">Error</th><th class="barcol">Against the Bands</th><th>Verdict</th></tr></thead>
						<tbody>
							{#each p.error_db as e, i (i)}
								{@const b = bands(i)}
								{@const v = errorVerdict(e, b)}
								<tr data-testid="predict-control-{i}" data-verdict={v.tone}>
									<td>{labels[i] ?? `Control ${i + 1}`}</td>
									<td class="num">{db(e, 2)}</td>
									<td class="barcol">
										<span class="ebar" aria-hidden="true">
											{#if b}
												<span class="band abort" style:left={errPos(-b.abort[0])} style:right={`calc(100% - ${errPos(b.abort[1])})`}></span>
												<span class="band alarm" style:left={errPos(-b.alarm[0])} style:right={`calc(100% - ${errPos(b.alarm[1])})`}></span>
											{/if}
											<span class="zero"></span>
											<span class="mark" style:left={errPos(e)}></span>
										</span>
									</td>
									<td class="verdict {v.tone}">{v.word}</td>
								</tr>
							{/each}
						</tbody>
					</table>
					<p class="aq-muted aq-small">
						The darker band is the alarm band, the lighter the abort band; the bar spans plus and minus {ERR_SPAN} dB.{#if !physical} Under virtual control each degree of freedom is judged by its own bands.{/if}
					</p>
					{#if moreControls && !p.has_achievable}
						<p class="aq-small note" data-testid="predict-achievable">
							There are more controls than drives, so the shakers can reach only part of the reference: what the drives can produce at the controls, through the identified plant. The error above is measured against the whole reference, so a control the drives cannot reach shows the part left over. Coherence and phase between controls follow from the drives, and may not match the reference's.
						</p>
					{/if}
				</section>

				{#if p.has_achievable}
					<section class="aq-panel" aria-labelledby="ar-title" data-testid="predict-achievable">
						<div class="aq-head">
							<h2 id="ar-title">Achievable Reference</h2>
							<span class="aq-sub">What the drives can reach of the reference, through the identified plant</span>
							<span class="aq-spacer"></span>
							{#if labels.length > 8}
								<button class="btn ghost" disabled={achFirst === 0} onclick={() => (mimo.predictFirst = Math.max(0, achFirst - 8))}>Previous Eight</button>
								<span class="aq-small num">{achFirst + 1} to {achFirst + achNames.length} of {labels.length}</span>
								<button class="btn ghost" disabled={achFirst + 8 >= labels.length} onclick={() => (mimo.predictFirst = Math.min(labels.length - 1, achFirst + 8))}>Next Eight</button>
							{/if}
						</div>
						<p class="aq-small aq-muted">
							There are more controls than drives, so only part of the reference can be reached. Solid lines are the achievable reference, dashed the reference: the diagonal is each control's spectrum, above it the coherence and below it the phase between two controls. A cross term the drives cannot make shows here before the test, not as an error during it.
						</p>
						<div class="achgrid" bind:clientWidth={gridWidth}>
							{#if achView}
								<MatrixGrid labels={achNames} sdm={achView} spectra={achSpectra} {unit} />
							{:else}
								<p class="aq-muted aq-small">The achievable reference is on its way from the service.</p>
							{/if}
						</div>
						<table class="aq-table compact">
							<thead><tr><th>{physical ? 'Control' : 'Degree of freedom'}</th><th class="num">Reachable</th></tr></thead>
							<tbody>
								{#each p.unreachable_db ?? [] as u, i (i)}
									<tr><td>{labels[i] ?? `Control ${i + 1}`}</td><td class="num">{db(u, 2)}</td></tr>
								{/each}
							</tbody>
						</table>
						<p class="aq-muted aq-small">Reachable is the achievable reference's rms against the reference's over the band; 0 dB is all of it.</p>
					</section>
				{/if}
			</div>

			<div class="stack">
				<section class="aq-panel" aria-labelledby="pl-title" data-testid="predict-law">
					<div class="aq-head">
						<h2 id="pl-title">The Law</h2>
						<span class="aq-sub num">Prediction {p.iteration}, {p.law.name} {p.law.version}</span>
					</div>
					<div class="sel">
						<label for="pl-law" class="lbl">Law</label>
						<select id="pl-law" class="field" value={mimo.setup.law.id} disabled={!canEdit} onchange={(e) => mimo.chooseLaw(e.currentTarget.value)} data-testid="predict-law-choice">
							{#each mimo.mimoLaws as l (l.id)}<option value={l.id}>{l.name} {l.version}{l.builtin ? '' : ', imported'}</option>{/each}
						</select>
					</div>
					{#if law && descriptors.length}
						<div class="params">
							<ParamForm params={descriptors} values={mimo.setup.law.params as ParamValues} showAllErrors disabled={!canEdit} idPrefix="plaw" testidPrefix="predict-param" />
						</div>
					{/if}
					<Findings items={Object.entries(mimo.fieldErrors).filter(([k]) => k.startsWith('law.')).flatMap(([, v]) => v).map((message) => ({ level: 'error' as const, message }))} />
					<div class="aq-actions acts">
						<button class="btn" disabled={!canEdit || iterating} onclick={() => mimo.iterate(true)} data-testid="predict-again">{iterating ? 'Predicting' : 'Predict Again'}</button>
						<span class="aq-muted aq-small">With the law and parameters shown; the predicted response is fed back as if measured, so a closed-loop law shows where it settles. The law shown here is the one that engages.</span>
					</div>
				</section>

				<section class="aq-panel engage" aria-labelledby="pe-title" data-testid="predict-engage">
					<div class="aq-head"><h2 id="pe-title">Engage</h2></div>
					{#if !review}
						<p class="aq-small" data-testid="predict-engaged">{s?.state === 'finished' ? 'The run ended. This is the prediction it engaged on.' : 'The run engaged on this prediction; it is shown as it was. Stop and Abort are in the run bar.'}{#if s?.overrides?.length} Acknowledged: {s.overrides.map((o) => `"${o.text}" (${o.by})`).join('; ')}.{/if}</p>
					{:else if gateFailed}
						<p class="aq-small">A gate failed, so going on is an override: your decision, in your own words, kept with the run. If the prediction then asks a drive for more than its ceiling, that needs its own acknowledgement.</p>
						<div class="aq-actions">
							<button class="btn primary" disabled={!mimo.isController || mimo.busy !== null} onclick={() => (overrideOpen = true)} data-testid="predict-override">Override the Gate</button>
						</div>
					{:else if predictionExceeded}
						<p class="aq-small">The prediction asks a drive for more than its ceiling. Change the law or its parameters and predict again, or engage through an override: the ceilings still hold every drive during the run, and the controls that drive serves will fall short of the reference.</p>
						<div class="aq-actions">
							<button class="btn primary" disabled={!mimo.isController || mimo.busy !== null} onclick={() => (overrideOpen = true)} data-testid="predict-override">Override and Engage</button>
						</div>
					{:else}
						<p class="aq-small">The level schedule starts: soft start, the ladder, then full level. Stop and Abort stay in the run bar.</p>
						<div class="aq-actions">
							<button class="btn primary" disabled={!mimo.actions.proceed.enabled} title={mimo.actions.proceed.reason ?? undefined} onclick={() => mimo.proceed()} data-testid="predict-engage-go">{mimo.busy === 'Proceed' ? 'Engaging' : 'Engage'}</button>
							{#if mimo.actions.proceed.reason}<span class="aq-small why">{mimo.actions.proceed.reason}</span>{/if}
						</div>
					{/if}
				</section>
			</div>
		</div>
	{/if}
</div>

<OverrideDialog
	acq={mimo.acq}
	bind:open={overrideOpen}
	variant={predictionExceeded
		? {
				title: 'Engage Past the Drive Ceilings',
				lead: `The prediction asks ${p ? p.drive_rms_v.filter((v, k) => v > p.ceiling_vrms[k]).length : 0} of ${p?.drive_rms_v.length ?? 0} drives for more than their ceilings. The ceilings hold each drive during the run, so the controls those drives serve will fall short of the reference.`,
				verb: 'Engaging',
				confirm: 'Engage With This Acknowledgement',
				send: (text) => mimo.override(text, 'prediction_exceeded'),
				refusal: mimo.refusal && mimo.refusal.verb === 'Override' ? `${mimo.refusal.title}: ${mimo.refusal.message}` : null,
				busy: mimo.busy !== null
			}
		: {
		title: 'Go On Past a Failed Gate',
		lead: `Identification did not pass its gates (smallest multiple coherence ${gates ? formatFixed(gates.coherence_min, 3) : '-'} against ${gates ? formatFixed(gates.coherence_floor, 2) : '-'}, largest condition number ${gates ? formatNumber(gates.condition_max, 4) : '-'} against ${gates ? formatNumber(gates.condition_limit, 4) : '-'}).`,
		verb: 'Going on',
		confirm: 'Go On With This Acknowledgement',
		send: (text) => mimo.override(text, 'gate_failed'),
		refusal: mimo.refusal && mimo.refusal.verb === 'Override' ? `${mimo.refusal.title}: ${mimo.refusal.message}` : null,
		busy: mimo.busy !== null
	}}
/>

<style>
	.excl {
		display: flex;
		gap: var(--sp-2);
		align-items: end;
		margin-top: var(--sp-2);
	}

	.excl .field {
		flex: 1;
	}

	.achgrid {
		height: 420px;
		margin: var(--sp-2) 0;
	}

	.aq-table.compact td,
	.aq-table.compact th {
		padding-top: 2px;
		padding-bottom: 2px;
	}

	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1.4fr) minmax(320px, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.facts {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2) var(--sp-3);
		margin: 0;
	}

	.facts dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.facts dd {
		margin: 0;
		font-size: var(--text-md);
	}

	.verdict {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-size: var(--text-sm);
		white-space: nowrap;
	}

	.verdict.fail {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.verdict.warn {
		font-weight: var(--weight-medium);
		color: var(--amber-2);
	}

	.warnline {
		margin-top: var(--sp-3);
		font-weight: var(--weight-medium);
		color: var(--amber-1);
	}

	.barcol {
		width: 40%;
		min-width: 180px;
	}

	.bar {
		position: relative;
		display: inline-block;
		width: calc(100% - 92px);
		height: 10px;
		vertical-align: middle;
		background: var(--bg-3);
		border-radius: 2px;
	}

	.bar .fill {
		position: absolute;
		inset: 0 auto 0 0;
		background: var(--text-3);
		border-radius: 2px;
	}

	/* Over the ceiling: solid and dark, plus the word beside it. */
	.bar .fill.over {
		background: var(--text-1);
	}

	.bar .tick {
		position: absolute;
		top: -3px;
		bottom: -3px;
		width: 2px;
		background: var(--text-1);
	}

	.word {
		display: inline-block;
		width: 88px;
		margin-left: 4px;
		font-size: var(--text-xs);
		text-align: right;
		color: var(--text-2);
	}

	.word.over {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.ebar {
		position: relative;
		display: block;
		height: 12px;
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: 2px;
	}

	.ebar .band {
		position: absolute;
		top: 0;
		bottom: 0;
	}

	.ebar .band.abort {
		background: var(--bg-4);
	}

	.ebar .band.alarm {
		background: var(--text-4);
		opacity: 0.6;
	}

	.ebar .zero {
		position: absolute;
		top: 0;
		bottom: 0;
		left: 50%;
		width: 1px;
		background: var(--text-3);
	}

	.ebar .mark {
		position: absolute;
		top: -3px;
		width: 0;
		height: 0;
		margin-left: -5px;
		border-left: 5px solid transparent;
		border-right: 5px solid transparent;
		border-top: 8px solid var(--text-1);
	}

	.note {
		margin-top: var(--sp-3);
		padding: var(--sp-2) var(--sp-3);
		color: var(--text-2);
		background: var(--bg-2);
		border-left: 3px solid var(--line-3);
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin-bottom: var(--sp-3);
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.params {
		margin-bottom: var(--sp-3);
	}

	.acts {
		align-items: center;
	}

	.acts span {
		flex: 1;
		min-width: 200px;
	}

	.engage {
		border: 1.5px solid var(--blue-line);
	}

	.engage p {
		margin: 0 0 var(--sp-3);
		color: var(--text-2);
	}

	.why {
		font-weight: var(--weight-medium);
		color: var(--amber-1);
	}

	@media (max-width: 1180px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
