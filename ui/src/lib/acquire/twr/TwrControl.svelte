<script lang="ts">
	/**
	 * How the run closes the loop between plays: the control law and its parameters (from the TWR
	 * world of the law catalog, drawn from the law's own manifest), the gate a play meets at its level
	 * and how many correction plays a level may take, what the inverse and the correction trust, and
	 * the guards the core holds every law to, whichever law it is. Every field starts at the service's
	 * usual value and says in a line what it does.
	 */
	import { formatNumber } from '$lib/viz/format';
	import LawPanel from '../law/LawPanel.svelte';
	import NumberField from '../NumberField.svelte';
	import { defaultTwrSetup } from './twrSetup';
	import type { TwrStore } from './twrStore.svelte';

	interface Props {
		twr: TwrStore;
	}
	let { twr }: Props = $props();

	const d = $derived(twr.setup);
	const c = $derived(d.control);
	const locked = $derived(twr.locked);
	const E = $derived(twr.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(twr.check && twr.checkedKey === twr.key ? twr.check.derived : null);
	const DEF = defaultTwrSetup();

	interface Spec {
		key: keyof typeof DEF.control;
		label: string;
		unit?: string;
		integer?: boolean;
		step: number;
		min?: number;
		max?: number;
		help: string;
	}

	const GUARDS: Spec[] = [
		{ key: 'law_budget', label: "Time budget, share of the record's length", step: 0.05, min: 0, max: 1, help: 'Of the longer of the record and 4 s. A call that takes longer is a miss; the drive in force plays again.' },
		{ key: 'law_misses', label: 'Misses before a stop', integer: true, step: 1, min: 1, max: 100, help: 'Unusable calls in a row (late, failed, or refused by the checks below) that stop the run.' },
		{ key: 'max_peak_rise_db', label: 'Drive peak rise cap, a call', unit: 'dB', step: 1, min: 0, max: 40, help: "The most the drive's peak may rise from one call to the next at full level." },
		{ key: 'max_bin_rise_db', label: 'Bin rise cap, a call', unit: 'dB', step: 1, min: 0, max: 40, help: 'The most any bin of the drive may rise from one call to the next, bins far under the largest held to a floor first.' },
		{ key: 'first_drive_ratio', label: 'First drive bound, times the target', step: 0.25, min: 1, max: 10, help: "The first drive's predicted response peak at most this times the target's." },
		{ key: 'alignment_frames', label: 'Alignment drift allowed', unit: 'frames', step: 0.5, min: 0, max: 16, help: "The most a play's fitted lag may drift from the identification's before the run ends." }
	];

	function usual() {
		if (locked) return;
		d.control = { ...DEF.control };
		d.inverse = { ...DEF.inverse };
		d.correction = { ...DEF.correction, band_hz: [d.identification.band_hz[0], d.identification.band_hz[1]] };
		d.schedule.gate_pct = DEF.schedule.gate_pct;
		d.schedule.max_iterations = DEF.schedule.max_iterations;
	}
</script>

<div class="cols">
	<LawPanel
		laws={twr.twrLaws}
		law={twr.law}
		chosenId={d.law.id}
		values={d.law.params}
		subtitle="Fixed for the run: the law and its parameters do not change once it starts."
		{locked}
		connected={twr.connected}
		busy={twr.busy}
		problems={twr.lawProblems}
		errors={E}
		onchoose={(id) => twr.chooseLaw(id)}
		onimport={(path) => twr.importLaw(path)}
	/>

	<section class="aq-panel" aria-labelledby="tc-loop" data-testid="twr-loop-panel">
		<div class="aq-head">
			<h2 id="tc-loop">The Correction</h2>
			<span class="aq-sub">Every field starts at the service's usual value.</span>
			<span class="aq-spacer"></span>
			<button class="btn ghost sm" onclick={usual} disabled={locked}>Usual Values</button>
		</div>
		<div class="aq-group">
			<h3>The Gate</h3>
			<div class="aq-fields">
				<NumberField id="tc-gate" label="Gate, rms error at most" unit="%" step={1} min={0} max={100} bind:value={d.schedule.gate_pct} errors={errs('schedule.gate_pct')} disabled={locked} testid="twr-gate" help="A play at or under it gates its level. Usual: 10%." />
				<NumberField id="tc-iter" label="Correction plays at one level" integer step={1} min={1} max={100} bind:value={d.schedule.max_iterations} errors={errs('schedule.max_iterations')} disabled={locked} testid="twr-max-iterations" help="Without a gate, then the gate-fail policy applies. Usual: 8." />
			</div>
		</div>
		<div class="aq-group">
			<h3>What It Trusts</h3>
			<div class="aq-fields">
				<NumberField id="tc-blo" label="Correction band from" unit="Hz" step={1} min={0} value={d.correction.band_hz[0]} onchange={(v) => (d.correction.band_hz[0] = v ?? NaN)} errors={errs('correction.band_hz')} disabled={locked} testid="twr-band-lo" />
				<NumberField id="tc-bhi" label="Correction band to" unit="Hz" step={100} min={0} value={d.correction.band_hz[1]} onchange={(v) => (d.correction.band_hz[1] = v ?? NaN)} disabled={locked} testid="twr-band-hi" help="Inside the identification band." />
				<NumberField id="tc-coh" label="Coherence floor" step={0.05} min={0} max={1} bind:value={d.inverse.coherence_floor} errors={errs('inverse.coherence_floor')} disabled={locked} testid="twr-coherence-floor" help="A bin is trusted only where identification measured it with at least this coherence. Usual: 0.9." />
				<NumberField id="tc-tik" label="Inverse floor, share of the peak gain" step={0.01} min={0} max={1} bind:value={d.inverse.tikhonov_floor} errors={errs('inverse.tikhonov_floor')} disabled={locked} testid="twr-tikhonov" help="Keeps the inverse from growing where the system barely answers. Usual: 0.05." />
				<NumberField id="tc-ref" label="Reference floor, under its peak" unit="dB" step={5} min={0} bind:value={d.correction.ref_floor_db} errors={errs('correction.ref_floor_db')} disabled={locked} testid="twr-ref-floor" help="Bins of the reference below this are left alone. Usual: 50 dB." />
				<NumberField id="tc-cov" label="Share of the reference the trusted bins cover" step={0.05} min={0} max={1} bind:value={d.correction.min_band_energy} errors={errs('correction.min_band_energy')} disabled={locked} testid="twr-coverage-floor" help="Under it, the run ends before any play. Usual: 0.9." />
			</div>
		</div>
		<div class="aq-group">
			<h3>Guards</h3>
			<p class="aq-lead">The core holds every law to these, whichever law it is, and checks each drive before it plays.</p>
			<div class="aq-fields">
				{#each GUARDS as s (s.key)}
					<NumberField id="tc-{s.key}" label={s.label} unit={s.unit} integer={s.integer} step={s.step} min={s.min} max={s.max} value={c[s.key]} onchange={(v) => (c[s.key] = v ?? NaN)} errors={errs(`control.${s.key}`)} help="{s.help} Usual: {DEF.control[s.key]}." disabled={locked} testid="twr-{s.key}" />
				{/each}
			</div>
			{#if dry}
				<p class="aq-small aq-muted" data-testid="twr-call-budget">A call's budget for this setup: <span class="num">{formatNumber(dry.call_budget_s, 4)} s</span>.</p>
			{/if}
		</div>
	</section>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
