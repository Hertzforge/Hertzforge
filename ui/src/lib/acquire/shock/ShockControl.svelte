<script lang="ts">
	/**
	 * How the run closes the loop between pulses: the control law and its parameters (from the
	 * classical world of the law catalog, drawn from the law's own manifest), what the correction
	 * trusts, and the guards the core holds every law to, whichever law it is. Every field starts at
	 * the service's usual value and says in a line what it does.
	 */
	import { formatNumber } from '$lib/viz/format';
	import LawPanel from '../law/LawPanel.svelte';
	import NumberField from '../NumberField.svelte';
	import { defaultShockSetup } from './shockSetup';
	import type { ShockStore } from './shockStore.svelte';

	interface Props {
		shock: ShockStore;
	}
	let { shock }: Props = $props();

	const d = $derived(shock.setup);
	const c = $derived(d.control);
	const locked = $derived(shock.locked);
	const E = $derived(shock.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(shock.check && shock.checkedKey === shock.key ? shock.check.derived : null);
	const DEF = defaultShockSetup().control;

	interface Spec {
		key: Exclude<keyof typeof DEF, 'band_hz'>;
		label: string;
		unit?: string;
		integer?: boolean;
		step: number;
		min?: number;
		max?: number;
		help: string;
	}

	const TRUST: Spec[] = [
		{ key: 'coherence_floor', label: 'Coherence floor', step: 0.05, min: 0, max: 1, help: 'A line is corrected only where identification measured it with at least this coherence.' },
		{ key: 'tikhonov_floor', label: 'Inverse floor, share of the peak gain', step: 0.01, min: 0, max: 1, help: 'Keeps the inverse from growing where the system barely answers.' },
		{ key: 'ref_floor_db', label: 'Reference floor, under its peak', unit: 'dB', step: 5, min: 0, help: 'Lines of the reference below this are left alone.' },
		{ key: 'min_band_energy', label: 'Share of the reference the trusted lines cover', step: 0.05, min: 0, max: 1, help: 'Under it, identification holds the run before any pulse.' }
	];
	const GUARDS: Spec[] = [
		{ key: 'law_budget', label: 'Time budget, share of the time between pulses', step: 0.05, min: 0, max: 1, help: 'A call that takes longer is a miss; the drive in force fires again.' },
		{ key: 'law_misses', label: 'Misses before a stop', integer: true, step: 1, min: 1, help: 'Unusable calls in a row (late, failed, or refused by the check below) that stop the run.' },
		{ key: 'max_pulses', label: 'Pulses in the run at most', integer: true, step: 10, min: 1, help: 'The run ends as not converged past this.' },
		{ key: 'max_peak_rise_db', label: 'Drive peak rise cap, a pulse', unit: 'dB', step: 1, min: 0, help: "The most the drive's peak may rise from one pulse to the next at full level." },
		{ key: 'max_bin_rise_db', label: 'Line rise cap, a pulse', unit: 'dB', step: 1, min: 0, help: 'The most any line of the drive may rise from one pulse to the next at full level.' },
		{ key: 'first_drive_ratio', label: 'First drive bound, times the reference', step: 0.25, min: 1, help: "The first drive's predicted response peak at most this times the reference's." },
		{ key: 'divergence_pulses', label: 'Pulses with a growing error', integer: true, step: 1, min: 1, help: 'Pulses in a row with the error rising more than 1 dB that end the run.' }
	];

	function usual() {
		if (locked) return;
		d.control = defaultShockSetup().control;
	}
</script>

{#snippet fields(specs: Spec[])}
	<div class="aq-fields">
		{#each specs as s (s.key)}
			<NumberField id="sk-{s.key}" label={s.label} unit={s.unit} integer={s.integer} step={s.step} min={s.min} max={s.max} value={c[s.key]} onchange={(v) => (c[s.key] = v ?? NaN)} errors={errs(`control.${s.key}`)} help="{s.help} Usual: {DEF[s.key]}." disabled={locked} testid="shock-{s.key}" />
		{/each}
	</div>
{/snippet}

<div class="cols">
	<LawPanel
		laws={shock.classicalLaws}
		law={shock.law}
		chosenId={d.law.id}
		values={d.law.params}
		subtitle="Fixed for the run: the law and its parameters do not change once it starts."
		{locked}
		connected={shock.connected}
		busy={shock.busy}
		problems={shock.lawProblems}
		errors={E}
		onchoose={(id) => shock.chooseLaw(id)}
		onimport={(path) => shock.importLaw(path)}
	/>

	<section class="aq-panel" aria-labelledby="sk-loop" data-testid="shock-loop-panel">
		<div class="aq-head">
			<h2 id="sk-loop">The Correction</h2>
			<span class="aq-sub">Every field starts at the service's usual value.</span>
			<span class="aq-spacer"></span>
			<button class="btn ghost sm" onclick={usual} disabled={locked}>Usual Values</button>
		</div>
		<div class="aq-group">
			<h3>What It Trusts</h3>
			<div class="aq-fields">
				<NumberField id="sk-blo" label="Band from" unit="Hz" step={1} min={0} value={c.band_hz[0]} onchange={(v) => (c.band_hz[0] = v ?? NaN)} errors={errs('control.band_hz')} disabled={locked} testid="shock-band-lo" />
				<NumberField id="sk-bhi" label="Band to" unit="Hz" step={100} min={0} value={c.band_hz[1]} onchange={(v) => (c.band_hz[1] = v ?? NaN)} disabled={locked} testid="shock-band-hi" help="The lines the correction may change." />
			</div>
			<div class="spaced">{@render fields(TRUST)}</div>
		</div>
		<div class="aq-group">
			<h3>Guards</h3>
			<p class="aq-lead">The core holds every law to these, whichever law it is, and checks each drive before it plays.</p>
			{@render fields(GUARDS)}
			{#if dry}
				<p class="aq-small aq-muted" data-testid="shock-call-budget">A call's budget for this setup: <span class="num">{formatNumber(dry.call_budget_s * 1000, 3)} ms</span>.</p>
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

	.spaced {
		margin-top: var(--sp-3);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
