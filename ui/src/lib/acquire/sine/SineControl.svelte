<script lang="ts">
	/**
	 * How the run closes the loop each control tick: the control law and its parameters (from the
	 * Sine world of the law catalog, drawn from the law's own manifest, the parameters changeable
	 * during the run marked), how the control channels combine, the tracking filter that reads each
	 * input at the tone's frequency, the control tick, where the identification is trusted, and the
	 * guards the core holds every law to, whichever law it is. A law decides one amplitude a tick;
	 * the core keeps the frequency, the sweep, the phase, the schedule and every abort. Every field
	 * starts at the service's usual value and says in a line what it does.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import LawPanel from '../law/LawPanel.svelte';
	import { rowName } from '../mimo/mimoSetup';
	import NumberField from '../NumberField.svelte';
	import { defaultSineSetup } from './sineSetup';
	import type { SineStore } from './sineStore.svelte';

	interface Props {
		sine: SineStore;
	}
	let { sine }: Props = $props();

	const d = $derived(sine.setup);
	const c = $derived(d.control);
	const id = $derived(d.identification);
	const locked = $derived(sine.locked);
	const E = $derived(sine.fieldErrors);
	const W = $derived(sine.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(sine.check && sine.checkedKey === sine.key ? sine.check.derived : null);
	const controlRows = $derived(sine.rows.filter((r) => r.enabled && r.role === 'control'));
	const DEF = defaultSineSetup().control;
	let advanced = $state(false);

	type GuardKey = 'law_budget' | 'law_misses' | 'max_step_db' | 'max_rise_db_per_s' | 'divergence_s' | 'fault_debounce_ticks';
	const GUARDS_KEYS: GuardKey[] = ['law_budget', 'law_misses', 'max_step_db', 'max_rise_db_per_s', 'divergence_s', 'fault_debounce_ticks'];
	const guardFindings = $derived(GUARDS_KEYS.some((k) => (E[`control.${k}`] ?? []).length > 0));
	interface Spec {
		key: GuardKey;
		label: string;
		unit?: string;
		integer?: boolean;
		step: number;
		min?: number;
		max?: number;
		help: string;
	}

	const GUARDS: Spec[] = [
		{ key: 'max_step_db', label: 'Largest change a tick', unit: 'dB', step: 0.5, min: 1, max: 3, help: 'The most the drive may move from one tick to the next, either way; a law asking for more is held to it.' },
		{ key: 'max_rise_db_per_s', label: 'Largest rise a second', unit: 'dB/s', step: 10, min: 0, max: 10000, help: 'The most the drive may climb in a second, whatever the law asks.' },
		{ key: 'divergence_s', label: 'Held back for at most', unit: 's', step: 0.5, min: 0, max: 60, help: 'Seconds the change limits may hold the law back without a break before the run stops.' },
		{ key: 'law_budget', label: 'Time a call may take, share of a tick', step: 0.05, min: 0, max: 1, help: 'A call that takes longer is not used; the drive in force holds.' },
		{ key: 'law_misses', label: 'Unusable calls before a stop', integer: true, step: 1, min: 1, max: 100, help: 'Calls in a row that come late, fail, or give a drive the checks refuse; then the run takes its controlled stop.' },
		{ key: 'fault_debounce_ticks', label: 'Ticks the control point may read far under', integer: true, step: 10, min: 0, help: 'Before the run stops for a lost control point (a fallen accelerometer, a broken cable). 0 turns the check off.' }
	];

	function weightOf(ch: number): number {
		return c.weights.find((w) => w.channel === ch)?.weight ?? 1;
	}

	function setWeight(ch: number, v: number) {
		const w = c.weights.find((x) => x.channel === ch);
		if (w) w.weight = v;
		else c.weights.push({ channel: ch, weight: v });
	}

	function usual() {
		if (locked) return;
		const base = defaultSineSetup();
		for (const s of GUARDS) c[s.key] = base.control[s.key];
	}
</script>

<div class="cols">
	<LawPanel
		laws={sine.sineLaws}
		law={sine.law}
		chosenId={d.law.id}
		values={d.law.params}
		subtitle="The law is fixed for the run; a parameter marked live can be changed while it runs."
		{locked}
		connected={sine.connected}
		busy={sine.busy}
		problems={sine.lawProblems}
		errors={E}
		live={sine.law?.live ?? []}
		onchoose={(lawId) => sine.chooseLaw(lawId)}
		onimport={(path) => sine.importLaw(path)}
	/>

	<div class="stack">
		<section class="aq-panel" aria-labelledby="nc-loop" data-testid="sine-loop">
			<div class="aq-head"><h2 id="nc-loop">Control Point</h2><span class="aq-sub">What the law is told it is controlling.</span></div>
			<div class="aq-fields">
				<div class="sel">
					<label for="nc-comp" class="lbl">Control channels combine by</label>
					<select id="nc-comp" class="field" bind:value={c.composite} disabled={locked} data-testid="sine-composite">
						<option value="weighted_avg">Weighted average</option>
						<option value="min">The lowest</option>
						<option value="max">The highest</option>
					</select>
				</div>
				{#if c.composite === 'weighted_avg'}
					{#each controlRows as r (r.index)}
						<NumberField id="nc-w-{r.index}" label="Weight, channel {r.index} {rowName(r)}" step={0.1} min={0} value={weightOf(r.index)} onchange={(v) => setWeight(r.index, v ?? NaN)} disabled={locked} testid="sine-weight-{r.index}" />
					{/each}
				{/if}
			</div>
			<Findings items={errs('control.weights').map((message) => ({ level: 'error' as const, message }))} />
			{#if !controlRows.length}<p class="aq-small aq-muted">The channel table has no enabled control row yet.</p>{/if}

			<div class="aq-group sep">
				<h3>Tracking Filter</h3>
				<p class="aq-lead aq-small">Each input is read at the tone's frequency through a narrow filter that follows the tone. Narrow is steady and slow to follow a fast sweep; wide follows fast and lets more noise in.</p>
				<div class="aq-fields">
					<div class="sel">
						<span class="lbl" id="nc-tr-l">Bandwidth</span>
						<div class="aq-seg sm" role="radiogroup" aria-labelledby="nc-tr-l">
							<button type="button" role="radio" aria-checked={c.tracking.kind === 'proportional'} disabled={locked} onclick={() => (c.tracking.kind = 'proportional')} data-testid="sine-tracking-proportional">A share of the frequency</button>
							<button type="button" role="radio" aria-checked={c.tracking.kind === 'fixed'} disabled={locked} onclick={() => (c.tracking.kind = 'fixed')} data-testid="sine-tracking-fixed">Fixed</button>
						</div>
					</div>
					{#if c.tracking.kind === 'proportional'}
						<NumberField id="nc-pct" label="Share of the frequency" unit="%" step={1} min={0} max={100} bind:value={c.tracking.pct} errors={errs('control.tracking')} warnings={W['control.tracking'] ?? []} disabled={locked} testid="sine-tracking-pct" />
						<NumberField id="nc-max" label="At most" unit="Hz" step={10} min={0} bind:value={c.tracking.max_hz} disabled={locked} testid="sine-tracking-max" />
					{:else}
						<NumberField id="nc-hz" label="Bandwidth" unit="Hz" step={1} min={0} bind:value={c.tracking.hz} errors={errs('control.tracking')} warnings={W['control.tracking'] ?? []} disabled={locked} testid="sine-tracking-hz" />
					{/if}
				</div>
				<p class="aq-small aq-muted">Usual: 10% of the frequency, at most 100 Hz. A sweep too fast for the filter is warned of or refused.</p>
			</div>

			<div class="aq-group">
				<h3>Control Tick</h3>
				<div class="aq-fields">
					<NumberField id="nc-tick" label="The law decides every" unit="ms" step={1} min={2} max={1000} bind:value={d.acquisition.tick_ms} errors={errs('acquisition.tick_ms')} disabled={locked} testid="sine-tick" help="From 2 ms for a built-in law and 8 ms for a law file, to 1000 ms. Usual: 10 ms." />
				</div>
				{#if dry}
					<p class="aq-small aq-muted" data-testid="sine-tick-derived">At {formatNumber(dry.grid.fs_hz, 6)} Hz a tick is {dry.grid.tick_frames} samples, {formatNumber(dry.grid.tick_s * 1000, 4)} ms; one call may take {formatNumber(dry.call_budget_s * 1000, 3)} ms.</p>
				{/if}
			</div>

			<div class="aq-group">
				<h3>Where the Identification Is Trusted</h3>
				<p class="aq-lead aq-small">The law is told the identification's prediction of the drive only where it is trusted: where the coherence reaches the figure below and the gain is not too far under its peak.</p>
				<div class="aq-fields">
					<NumberField id="nc-tc" label="Coherence at least" step={0.05} min={0} max={1} bind:value={id.trust_coherence} errors={errs('identification.trust_coherence')} disabled={locked} testid="sine-trust-coherence" help="Usual: 0.5." />
					<NumberField id="nc-tf" label="Gain at least, share of its peak" step={0.01} min={0} max={1} bind:value={id.trust_h_floor_frac} errors={errs('identification.trust_h_floor_frac')} disabled={locked} testid="sine-trust-floor" help="Usual: 0.05." />
				</div>
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="nc-guards" data-testid="sine-guards-panel">
			<div class="aq-head">
				<h2 id="nc-guards">Guards</h2>
				<span class="aq-sub">The core holds every law to these, whichever law it is.</span>
				<span class="aq-spacer"></span>
				<button class="btn ghost sm" aria-expanded={advanced} onclick={() => (advanced = !advanced)} data-testid="sine-guards-open">{advanced ? 'Hide' : 'Show'}</button>
				<button class="btn ghost sm" onclick={usual} disabled={locked}>Usual Values</button>
			</div>
			<p class="aq-lead aq-small lead">A law gives one amplitude a tick, a number at or above zero. The core then limits how far it may move, keeps it under the limit channels and the drive ceiling, and stops the run when the law keeps failing.</p>
			{#if advanced || guardFindings}
				<div class="aq-fields">
					{#each GUARDS as s (s.key)}
						<NumberField id="nc-{s.key}" label={s.label} unit={s.unit} integer={s.integer} step={s.step} min={s.min} max={s.max} value={c[s.key]} onchange={(v) => (c[s.key] = v ?? NaN)} errors={errs(`control.${s.key}`)} help="{s.help} Usual: {DEF[s.key]}." disabled={locked} testid="sine-{s.key}" />
					{/each}
				</div>
			{:else}
				<p class="aq-small aq-muted" data-testid="sine-guards-summary">At most {formatNumber(c.max_step_db, 3)} dB a tick and {formatNumber(c.max_rise_db_per_s, 4)} dB a second up; {c.law_misses} unusable calls in a row stop the run.</p>
			{/if}
		</section>
	</div>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
	/* A group after fields or a table is set off as one after another group is. */
	.sep {
		margin-top: var(--sp-5);
		padding-top: var(--sp-4);
		border-top: 1px solid var(--line-1);
	}

	.lead {
		margin: 0 0 var(--sp-3);
	}
</style>
