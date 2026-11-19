<script lang="ts">
	/**
	 * How the run closes the loop between pulses: the control law and its parameters (from the SRS
	 * world of the law catalog, drawn from the law's own manifest) and the guards the core holds
	 * every law to, whichever law it is. A law proposes one gain for each component after each
	 * pulse; the core caps the gains, synthesizes the drive from the table and checks it before it
	 * plays. Every field starts at the service's usual value and says in a line what it does.
	 */
	import { formatNumber } from '$lib/viz/format';
	import LawPanel from '../law/LawPanel.svelte';
	import NumberField from '../NumberField.svelte';
	import { defaultSrsSetup } from './srsSetup';
	import type { SrsStore } from './srsStore.svelte';

	interface Props {
		srs: SrsStore;
	}
	let { srs }: Props = $props();

	const d = $derived(srs.setup);
	const c = $derived(d.control);
	const locked = $derived(srs.locked);
	const E = $derived(srs.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(srs.check && srs.checkedKey === srs.key ? srs.check.derived : null);
	const DEF = defaultSrsSetup().control;

	interface Spec {
		key: keyof typeof DEF;
		label: string;
		unit?: string;
		integer?: boolean;
		step: number;
		min?: number;
		max?: number;
		help: string;
	}

	const GUARDS: Spec[] = [
		{ key: 'law_budget', label: 'Time budget, share of the time between pulses', step: 0.05, min: 0, max: 1, help: 'A call that takes longer is a miss; the gains in force are kept for the next pulse.' },
		{ key: 'law_misses', label: 'Misses before a stop', integer: true, step: 1, min: 1, help: 'Unusable calls in a row (late, failed, or refused by the checks below) that stop the run.' },
		{ key: 'max_pulses', label: 'Pulses in the run at most', integer: true, step: 10, min: 1, help: 'The run ends as not converged past this.' },
		{ key: 'max_gain_rise_db', label: 'Gain rise cap, a pulse', unit: 'dB', step: 1, min: 0, max: 20, help: "The most any component's gain may rise from one pulse to the next; a law asking for more is not used." }
	];

	function usual() {
		if (locked) return;
		d.control = defaultSrsSetup().control;
	}
</script>

<div class="cols">
	<LawPanel
		laws={srs.srsLaws}
		law={srs.law}
		chosenId={d.law.id}
		values={d.law.params}
		subtitle="Fixed for the run: the law and its parameters do not change once it starts."
		{locked}
		connected={srs.connected}
		busy={srs.busy}
		problems={srs.lawProblems}
		errors={E}
		onchoose={(id) => srs.chooseLaw(id)}
		onimport={(path) => srs.importLaw(path)}
	/>

	<section class="aq-panel" aria-labelledby="rc-guards" data-testid="srs-guards-panel">
		<div class="aq-head">
			<h2 id="rc-guards">Guards</h2>
			<span class="aq-sub">Every field starts at the service's usual value.</span>
			<span class="aq-spacer"></span>
			<button class="btn ghost sm" onclick={usual} disabled={locked}>Usual Values</button>
		</div>
		<p class="aq-lead">The core holds every law to these, whichever law it is. A law gives a gain for each component, finite and above zero; the core caps it where a limit channel needs room, synthesizes the drive and checks it in volts before it plays.</p>
		<div class="aq-fields">
			{#each GUARDS as s (s.key)}
				<NumberField id="rc-{s.key}" label={s.label} unit={s.unit} integer={s.integer} step={s.step} min={s.min} max={s.max} value={c[s.key]} onchange={(v) => (c[s.key] = v ?? NaN)} errors={errs(`control.${s.key}`)} help="{s.help} Usual: {DEF[s.key]}." disabled={locked} testid="srs-{s.key}" />
			{/each}
		</div>
		{#if dry}
			<p class="aq-small aq-muted" data-testid="srs-call-budget">A call's budget for this setup: <span class="num">{formatNumber(dry.call_budget_s * 1000, 3)} ms</span>.</p>
		{/if}
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
