<script lang="ts">
	/**
	 * How the run controls each part. The floor's law (from the single-output Random world: it is
	 * called each loop on the floor's clean lines) and the tones' law (from the Sine world: one
	 * choice, one instance per tone, called each tick), each with its parameters from the law's own
	 * manifest; how the control channels combine; the tracking width every tone is read through and
	 * the width around a standing tone left out of the floor's average; the tick and the part ramp;
	 * the floor's loop; where the identification is trusted for a tone; and the guards the core holds
	 * each law to. Every field starts at the service's usual value and says in a line what it does.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import LawPanel from '../law/LawPanel.svelte';
	import { rowName } from '../mimo/mimoSetup';
	import NumberField from '../NumberField.svelte';
	import { defaultMixedSetup } from './mixedSetup';
	import type { MixedStore } from './mixedStore.svelte';

	interface Props {
		mixed: MixedStore;
	}
	let { mixed }: Props = $props();

	const d = $derived(mixed.setup);
	const c = $derived(d.control);
	const locked = $derived(mixed.locked);
	const E = $derived(mixed.fieldErrors);
	const W = $derived(mixed.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(mixed.derived);
	const controlRows = $derived(mixed.rows.filter((r) => r.enabled && r.role === 'control'));
	const DEF = defaultMixedSetup();
	let guardsOpen = $state(false);

	type NumKey = 'control_dof' | 'damping' | 'soft_start_db' | 'soft_start_relax_db' | 'level_ramp_s' | 'shutdown_ramp_s' | 'part_ramp_s' | 'floor_settle_s' | 'trust_coherence' | 'trust_h_floor_frac';
	interface Spec {
		key: NumKey;
		label: string;
		unit?: string;
		step: number;
		min?: number;
		max?: number;
		help: string;
	}
	const FLOOR: Spec[] = [
		{ key: 'control_dof', label: 'Control degrees of freedom', step: 10, min: 2, help: "Of the floor's estimate, on its clean lines. Higher is steadier and slower to adapt." },
		{ key: 'damping', label: 'Correction damping', step: 1, min: 0, max: 10, help: '0 corrects fully every loop; 10 spreads a correction over about ten loops.' },
		{ key: 'floor_settle_s', label: 'Floor alone for at most', unit: 's', step: 5, min: 0, max: 600, help: 'The floor plays alone at the first level until it has equalized; at this time the parts come in anyway, and the log says so.' }
	];
	const RAMPS: Spec[] = [
		{ key: 'part_ramp_s', label: 'Part ramp', unit: 's', step: 0.5, min: 0, max: 60, help: 'How long one part takes to come in or go out: its window, its end, or a switch during the run. Tones come in from their feed-forward amplitude.' },
		{ key: 'soft_start_db', label: 'Soft start', unit: 'dB', step: 1, min: 0, help: 'The floor engages this far below the first level and ramps up to it.' },
		{ key: 'soft_start_relax_db', label: 'Soft start band relief', unit: 'dB', step: 1, min: 0, help: "The floor's bands widen by this until the first level is reached." },
		{ key: 'level_ramp_s', label: 'Level ramp', unit: 's', step: 0.5, min: 0, help: 'Every part moves together over this on a level change.' },
		{ key: 'shutdown_ramp_s', label: 'Shutdown ramp', unit: 's', step: 0.5, min: 0, help: 'A controlled stop takes every part 40 dB down over this, then to zero. An abort does not ramp.' }
	];
	const TRUST: Spec[] = [
		{ key: 'trust_coherence', label: 'Coherence at least', step: 0.05, min: 0, max: 1, help: "Where the identification is trusted for a tone's feed-forward and its law. Usual: 0.5." },
		{ key: 'trust_h_floor_frac', label: 'Gain at least, share of its peak', step: 0.01, min: 0, max: 1, help: 'Usual: 0.05.' }
	];
	const FLOOR_GUARDS = [
		{ key: 'max_change_db', label: 'Largest change of a line a loop', unit: 'dB', step: 1, min: 0, help: 'The most any drive line may move in one call of the floor law.' },
		{ key: 'law_budget', label: 'Time a call may take, share of a loop', unit: '', step: 0.05, min: 0, max: 1, help: "A call that takes longer is not used; the floor's drive in force holds." },
		{ key: 'law_misses', label: 'Unusable calls before a stop', unit: '', step: 1, min: 1, max: 100, integer: true, help: 'Calls in a row that come late, fail, or give a drive the checks refuse; then the run takes its controlled stop.' }
	] as const;
	const TONE_GUARDS = [
		{ key: 'max_step_db', label: 'Largest change a tick', unit: 'dB', step: 0.5, min: 1, max: 3, help: "The most a tone's drive may move from one tick to the next." },
		{ key: 'max_rise_db_per_s', label: 'Largest rise a second', unit: 'dB/s', step: 10, min: 0, max: 10000, help: "The most a tone's drive may climb in a second." },
		{ key: 'divergence_s', label: 'Held back for at most', unit: 's', step: 0.5, min: 0, max: 60, help: 'Seconds the change limits may hold a tone law back without a break before the run stops.' },
		{ key: 'law_budget', label: 'Time a call may take, share of a tick', unit: '', step: 0.05, min: 0, max: 1, help: "A call that takes longer is not used; the tone's drive in force holds." },
		{ key: 'law_misses', label: 'Unusable calls before a stop', unit: '', step: 1, min: 1, max: 100, integer: true, help: 'Calls in a row of any one tone.' },
		{ key: 'fault_debounce_ticks', label: 'Ticks a tone may read far under', unit: '', step: 10, min: 0, integer: true, help: 'Before the run stops for a lost control point. 0 turns the check off.' }
	] as const;
	const guardFindings = $derived(Object.keys(E).some((k) => k.startsWith('guards.')));

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
		d.guards = structuredClone(DEF.guards);
	}
</script>

{#snippet field(s: Spec)}
	<NumberField id="mc-{s.key}" label={s.label} unit={s.unit} step={s.step} min={s.min} max={s.max} value={c[s.key]} onchange={(v) => (c[s.key] = v ?? NaN)} errors={errs(`control.${s.key}`)} help="{s.help} Usual: {DEF.control[s.key]}." disabled={locked} testid="mixed-{s.key}" />
{/snippet}

<div class="cols">
	<div class="stack">
		<LawPanel
			laws={mixed.floorLaws}
			law={mixed.floorLaw}
			chosenId={d.lawFloor.id}
			values={d.lawFloor.params}
			subtitle="The floor's law, from the single-output Random laws. It is called each loop on the floor alone, on the lines no tone or narrowband covers."
			{locked}
			connected={mixed.connected}
			busy={mixed.busy}
			problems={mixed.lawProblems}
			errors={mixed.floorLawErrors}
			onchoose={(id) => mixed.chooseLaw('floor', id)}
			onimport={(path) => mixed.importLaw('floor', path)}
		>
			{#snippet facts()}
				{#if dry}<div><dt>On hardware</dt><dd data-testid="mixed-floor-law-hw">{dry.law.floor.hardware ? 'May drive hardware in this role' : 'Runs on the simulator only in this role: it needs a passed mixed check as the floor law'}</dd></div>{/if}
			{/snippet}
		</LawPanel>
		<LawPanel
			laws={mixed.toneLaws}
			law={mixed.toneLaw}
			chosenId={d.lawTones.id}
			values={d.lawTones.params}
			subtitle="The tones' law, from the Sine laws: one choice, one instance per tone, called each tick. A parameter marked live can be changed while the run goes."
			{locked}
			connected={mixed.connected}
			busy={mixed.busy}
			problems={mixed.lawProblems}
			errors={mixed.toneLawErrors}
			live={mixed.toneLaw?.live ?? []}
			onchoose={(id) => mixed.chooseLaw('tones', id)}
			onimport={(path) => mixed.importLaw('tones', path)}
		>
			{#snippet facts()}
				{#if dry}<div><dt>On hardware</dt><dd data-testid="mixed-tone-law-hw">{dry.law.tones.hardware ? 'May drive hardware in this role' : 'Runs on the simulator only in this role: it needs a passed check in its own world and a passed mixed check as the tone law'}</dd></div>{/if}
			{/snippet}
		</LawPanel>
	</div>

	<div class="stack">
		<section class="aq-panel" aria-labelledby="mc-loop" data-testid="mixed-loop">
			<div class="aq-head"><h2 id="mc-loop">Control Point</h2><span class="aq-sub">What each part is controlled on.</span></div>
			<div class="aq-fields">
				<div class="sel">
					<label for="mc-comp" class="lbl">Control channels combine by</label>
					<select id="mc-comp" class="field" bind:value={c.composite} disabled={locked} data-testid="mixed-composite">
						<option value="weighted_avg">Weighted average</option>
						<option value="min">The lowest</option>
						<option value="max">The highest</option>
					</select>
				</div>
				{#if c.composite === 'weighted_avg'}
					{#each controlRows as r (r.index)}
						<NumberField id="mc-w-{r.index}" label="Weight, channel {r.index} {rowName(r)}" step={0.1} min={0} value={weightOf(r.index)} onchange={(v) => setWeight(r.index, v ?? NaN)} disabled={locked} testid="mixed-weight-{r.index}" />
					{/each}
				{/if}
			</div>
			<Findings items={errs('control.weights').map((message) => ({ level: 'error' as const, message }))} />
			{#if !controlRows.length}<p class="aq-small aq-muted">The channel table has no enabled control row yet.</p>{/if}

			<div class="aq-group sep">
				<h3>Tones and Their Lines</h3>
				<div class="aq-fields">
					<NumberField id="mc-track" label="Tracking width" unit="Hz" step={0.1} min={0} max={10} value={c.tracking_hz} onchange={(v) => (c.tracking_hz = v ?? NaN)} errors={errs('control.tracking_hz')} warnings={W['control.tracking_hz'] ?? []} disabled={locked} testid="mixed-tracking" help="Every tone is read through a filter this wide that follows it. Tones stay at least two widths apart. Usual: 0.5 Hz." />
					<NumberField id="mc-excl" label="Left out around a standing tone" unit="Hz each side" step={0.5} min={0} max={50} bind:value={d.exclusion_hz} errors={errs('exclusion_hz')} disabled={locked} testid="mixed-exclusion" help="These lines are not read for the floor while a tone stands there; the floor law is given the error of the clean lines either side. Usual: 2 Hz." />
					<NumberField id="mc-tick" label="Each tone law decides every" unit="ms" step={1} min={2} max={1000} bind:value={d.acquisition.tick_ms} errors={errs('acquisition.tick_ms')} disabled={locked} testid="mixed-tick" help="From 2 ms for a built-in law and 8 ms for a law file. Usual: 10 ms." />
				</div>
				{#if dry}
					<p class="aq-small aq-muted" data-testid="mixed-tick-derived">A tick is {dry.grid.tick_frames} samples, {formatNumber(dry.grid.tick_s * 1000, 4)} ms; a tone law call may take {formatNumber(dry.call_budget_s.tones * 1000, 3)} ms and a floor law call {formatNumber(dry.call_budget_s.floor * 1000, 4)} ms.</p>
				{/if}
			</div>

			<div class="aq-group">
				<h3>The Floor's Loop</h3>
				<div class="aq-fields">
					{#each FLOOR as s (s.key)}{@render field(s)}{/each}
				</div>
			</div>

			<div class="aq-group">
				<h3>Ramps</h3>
				<div class="aq-fields">
					{#each RAMPS as s (s.key)}{@render field(s)}{/each}
				</div>
			</div>

			<div class="aq-group">
				<h3>Where the Identification Is Trusted for a Tone</h3>
				<div class="aq-fields">
					{#each TRUST as s (s.key)}{@render field(s)}{/each}
				</div>
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="mc-guards" data-testid="mixed-guards-panel">
			<div class="aq-head">
				<h2 id="mc-guards">Guards</h2>
				<span class="aq-sub">The core holds each law to these, whichever law it is.</span>
				<span class="aq-spacer"></span>
				<button class="btn ghost sm" aria-expanded={guardsOpen} onclick={() => (guardsOpen = !guardsOpen)} data-testid="mixed-guards-open">{guardsOpen ? 'Hide' : 'Show'}</button>
				<button class="btn ghost sm" onclick={usual} disabled={locked}>Usual Values</button>
			</div>
			{#if guardsOpen || guardFindings}
				<h3>The Floor Law</h3>
				<div class="aq-fields">
					{#each FLOOR_GUARDS as s (s.key)}
						<NumberField id="mg-f-{s.key}" label={s.label} unit={s.unit || undefined} step={s.step} min={s.min} max={'max' in s ? s.max : undefined} integer={'integer' in s} value={d.guards.floor[s.key]} onchange={(v) => (d.guards.floor[s.key] = v ?? NaN)} errors={errs(`guards.floor.${s.key}`)} help="{s.help} Usual: {DEF.guards.floor[s.key]}." disabled={locked} testid="mixed-guard-floor-{s.key}" />
					{/each}
				</div>
				<h3>Each Tone's Law</h3>
				<div class="aq-fields">
					{#each TONE_GUARDS as s (s.key)}
						<NumberField id="mg-t-{s.key}" label={s.label} unit={s.unit || undefined} step={s.step} min={s.min} max={'max' in s ? s.max : undefined} integer={'integer' in s} value={d.guards.tones[s.key]} onchange={(v) => (d.guards.tones[s.key] = v ?? NaN)} errors={errs(`guards.tones.${s.key}`)} help="{s.help} Usual: {DEF.guards.tones[s.key]}." disabled={locked} testid="mixed-guard-tones-{s.key}" />
					{/each}
				</div>
			{:else}
				<p class="aq-small aq-muted" data-testid="mixed-guards-summary">Floor: at most {formatNumber(d.guards.floor.max_change_db, 3)} dB a line a loop. Tones: at most {formatNumber(d.guards.tones.max_step_db, 3)} dB a tick and {formatNumber(d.guards.tones.max_rise_db_per_s, 4)} dB a second up. {d.guards.floor.law_misses} and {d.guards.tones.law_misses} unusable calls in a row stop the run.</p>
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

	h3 {
		margin: var(--sp-2) 0;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.sep {
		margin-top: var(--sp-5);
		padding-top: var(--sp-4);
		border-top: 1px solid var(--line-1);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
