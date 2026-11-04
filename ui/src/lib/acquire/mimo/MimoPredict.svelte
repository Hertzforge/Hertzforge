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
	import { formatFixed, formatNumber } from '$lib/viz/format';
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
	const labels = $derived(s?.shape?.labels ?? mimo.labels);
	const driveLabels = $derived(s?.shape?.drive_labels ?? mimo.channels.drives.map((r) => r.description));
	const physical = $derived((s?.shape?.space ?? mimo.space.kind) === 'physical');
	const moreControls = $derived(s?.shape?.shape === 'more_controls_than_drives');
	const law = $derived(mimo.law);
	const descriptors = $derived((law?.params ?? []).map(lawParamDescriptor).filter((x) => x !== null));
	const canEdit = $derived(review && mimo.isController);
	let overrideOpen = $state(false);

	/** The tolerances a control is judged by: its own bands when the controls are physical. */
	function bands(i: number): { alarm: [number, number]; abort: [number, number] } | null {
		if (!physical) return null;
		const r = mimo.channels.controls[i];
		if (!r) return null;
		const c = controlOf(mimo.setup, r.index);
		return { alarm: c.alarm_db, abort: c.abort_db };
	}

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
	{#if !review || !p}
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
						<p class="aq-small warnline">A gate failed: some controls are not explained by the drives, or the plant is nearly singular on some lines. Change the setup (controls, drives, frame) and run again, or override in your own words.</p>
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
						{#if physical}The darker band is the alarm band, the lighter the abort band; the bar spans plus and minus {ERR_SPAN} dB.{:else}Under virtual control the bands are the control channels'; the errors here are each degree of freedom's.{/if}
					</p>
					{#if moreControls}
						<p class="aq-small note" data-testid="predict-achievable">
							There are more controls than drives, so the shakers can reach only part of the reference: what the drives can produce at the controls, through the identified plant. The error above is measured against the whole reference, so a control the drives cannot reach shows the part left over. Coherence and phase between controls follow from the drives, and may not match the reference's.
						</p>
					{/if}
				</section>
			</div>

			<div class="stack">
				<section class="aq-panel" aria-labelledby="pl-title" data-testid="predict-law">
					<div class="aq-head">
						<h2 id="pl-title">The Law</h2>
						<span class="aq-sub num">Prediction {p.iteration + 1}, {p.law.name} {p.law.version}</span>
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
					{#if gateFailed}
						<p class="aq-small">A gate failed, so engaging is an override: your decision, in your own words, kept with the run.</p>
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
	variant={{
		title: 'Engage Past a Failed Gate',
		lead: `Identification did not pass its gates (smallest multiple coherence ${gates ? formatFixed(gates.coherence_min, 3) : '-'} against ${gates ? formatFixed(gates.coherence_floor, 2) : '-'}, largest condition number ${gates ? formatNumber(gates.condition_max, 4) : '-'} against ${gates ? formatNumber(gates.condition_limit, 4) : '-'}).`,
		verb: 'Engaging',
		confirm: 'Engage With This Acknowledgement',
		send: (text) => mimo.override(text),
		refusal: mimo.refusal && mimo.refusal.verb === 'Override' ? `${mimo.refusal.title}: ${mimo.refusal.message}` : null,
		busy: mimo.busy !== null
	}}
/>

<style>
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
