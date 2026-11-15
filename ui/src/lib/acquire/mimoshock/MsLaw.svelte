<script lang="ts">
	/**
	 * How the run builds the drives between pulses: the control law, chosen from the catalog's MIMO
	 * Shock laws that accept the target kind, with the parameters its own manifest declares, and its
	 * check record; then the guards the core holds every law to, whichever law it is. Nothing here
	 * names a law: the panel lists what the control service lists.
	 */
	import LawPanel from '../law/LawPanel.svelte';
	import NumberField from '../NumberField.svelte';
	import { defaultMsSetup } from './msSetup';
	import type { MimoShockStore } from './msStore.svelte';

	interface Props {
		ms: MimoShockStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const c = $derived(d.control);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const DEF = defaultMsSetup().control;
	const kindWords = $derived(d.kind === 'waveform' ? 'a classical pulse at every control' : 'an SRS at every control');

	type Key = 'law_budget' | 'law_misses' | 'max_pulses' | 'max_growth_db' | 'max_band_growth_db' | 'divergence_pulses' | 'divergence_drive_ratio' | 'first_drive_ratio';
	const GUARDS: { key: Key; label: string; unit?: string; integer?: boolean; step: number; min?: number; max?: number; help: string }[] = [
		{ key: 'max_growth_db', label: "A drive's peak growth a pulse", unit: 'dB', step: 1, min: 0, max: 40, help: "The most a drive's peak may grow from one pulse to the next beyond the level step. An answer over it is not used." },
		{ key: 'max_band_growth_db', label: "A band's growth a pulse", unit: 'dB', step: 1, min: 0, max: 40, help: 'The same for any third-octave band of a drive.' },
		{ key: 'divergence_pulses', label: 'Pulses with a growing error', integer: true, step: 1, min: 1, max: 100, help: 'Pulses in a row at one level with the error rising that stop the run.' },
		{ key: 'divergence_drive_ratio', label: "A drive's growth at one level", step: 0.1, min: 1, max: 10, help: "A drive whose rms passes this many times its rms on the level's first pulse stops the run." },
		{ key: 'first_drive_ratio', label: 'First drives bound, times the target', step: 0.25, min: 1, max: 10, help: "The first drives' predicted peak at every control is at most this times its target's." },
		{ key: 'law_budget', label: 'Time a call, share of the time between pulses', step: 0.05, min: 0, max: 1, help: 'A call that takes longer is a miss; the drives in force play again.' },
		{ key: 'law_misses', label: 'Misses before a stop', integer: true, step: 1, min: 1, help: 'Unusable answers in a row (late, failed, or refused by these guards) that stop the run.' },
		{ key: 'max_pulses', label: 'Pulses in the run at most', integer: true, step: 10, min: 1, help: 'The run stops past this.' }
	];

	function usual() {
		if (locked) return;
		for (const g of GUARDS) c[g.key] = DEF[g.key];
	}
</script>

<div class="stack">
	<LawPanel
		laws={ms.kindLaws}
		law={ms.law}
		chosenId={d.law.id}
		values={d.law.params}
		subtitle="Fixed for the run. Listed: the laws that accept {kindWords}."
		{locked}
		connected={ms.connected}
		busy={ms.busy}
		problems={ms.lawProblems}
		errors={E}
		channelLabels={d.controls.map((x) => `Control ${x}`)}
		onchoose={(id) => ms.chooseLaw(id)}
		onimport={(path) => ms.importLaw(path)}
	>
		{#snippet facts(entry)}
			{@const t = (entry as { targets?: string[] }).targets}
			{#if t}<div><dt>Targets</dt><dd data-testid="ms-law-targets">{t.map((k) => (k === 'srs' ? 'SRS' : 'Classical pulse')).join(', ')}</dd></div>{/if}
		{/snippet}
	</LawPanel>

	<section class="aq-panel" aria-labelledby="ms-gd-title" data-testid="ms-guards">
		<div class="aq-head">
			<h2 id="ms-gd-title">Guards on Any Law</h2>
			<span class="aq-sub">Held whatever the law answers: an answer over a guard is not played.</span>
			<span class="aq-spacer"></span>
			<button class="btn sm ghost" disabled={locked} onclick={usual}>Usual Values</button>
		</div>
		<div class="aq-fields">
			{#each GUARDS as g (g.key)}
				<NumberField id="ms-{g.key}" label={g.label} unit={g.unit} integer={g.integer} step={g.step} min={g.min} max={g.max} value={c[g.key]} onchange={(v) => (c[g.key] = v ?? NaN)} errors={errs(`control.${g.key}`)} help="{g.help} Usual: {DEF[g.key]}." disabled={locked} testid="ms-{g.key}" />
			{/each}
		</div>
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}
</style>
