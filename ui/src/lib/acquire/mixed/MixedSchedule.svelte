<script lang="ts">
	/**
	 * The level schedule (Random's ladder, moving every part by the same dB; its dwells and full
	 * level count in parts time, from when the floor has equalized) and the identification before
	 * it: Random's broadband identification, the coherence it must reach, and whether the run holds
	 * for review before anything plays at test level.
	 */
	import NumberField from '../NumberField.svelte';
	import ScheduleEditor from '../ScheduleEditor.svelte';
	import type { MixedStore } from './mixedStore.svelte';

	interface Props {
		mixed: MixedStore;
	}
	let { mixed }: Props = $props();

	const c = $derived(mixed.setup.control);
	const p = $derived(c.pretest);
	const locked = $derived(mixed.locked);
	const E = $derived(mixed.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(mixed.derived);
</script>

<div class="cols">
	<div class="stack">
		<ScheduleEditor acq={mixed.scheduleHost} />
		<p class="aq-small aq-muted note">Every part moves by the same dB. The floor plays alone at the first level until it has equalized; the dwells and full level count from then, and stop on a hold.{#if dry && dry.test_s !== null} The ladder is {dry.test_s} s of parts time.{/if}</p>
	</div>
	<section class="aq-panel" aria-labelledby="ms-id" data-testid="mixed-identification">
		<div class="aq-head"><h2 id="ms-id">Identification</h2><span class="aq-sub">A broadband noise burst gives the floor law its plant and each tone its feed-forward at its frequency.</span></div>
		<div class="aq-fields">
			<NumberField id="ms-lvl" label="Steps up to" unit="dB" step={1} value={p.level_db} onchange={(v) => (p.level_db = v ?? NaN)} errors={errs('control.pretest.level_db')} disabled={locked} testid="mixed-pretest-level" help="Of the control response against the floor's reference. Usual: -20 dB." />
			<NumberField id="ms-avg" label="Averages" integer step={1} min={1} value={p.averages} onchange={(v) => (p.averages = v ?? NaN)} errors={errs('control.pretest.averages')} disabled={locked} testid="mixed-pretest-averages" help="Loops the identification measures over. Usual: 4." />
			<NumberField id="ms-start" label="Starts from" unit="V rms" step={0.01} min={0} value={p.start_vrms} onchange={(v) => (p.start_vrms = v ?? NaN)} errors={errs('control.pretest.start_vrms')} disabled={locked} testid="mixed-pretest-start" help="Usual: 0.01 V rms." />
			<NumberField id="ms-max" label="At most" unit="V rms" nullable="No cap" fallback={1} step={0.1} min={0} value={p.max_vrms} onchange={(v) => (p.max_vrms = v)} errors={errs('control.pretest.max_vrms')} disabled={locked} testid="mixed-pretest-max" />
			<NumberField id="ms-coh" label="Coherence floor" step={0.05} min={0} max={1} value={c.coherence_floor} onchange={(v) => (c.coherence_floor = v ?? NaN)} errors={errs('control.coherence_floor')} disabled={locked} testid="mixed-coherence-floor" help="The band coherence identification must reach. Below it the run holds and never drives at test level. Usual: 0.5." />
		</div>
		<label class="check"><input type="checkbox" bind:checked={p.shaped} disabled={locked} data-testid="mixed-pretest-shaped" /> Shape the burst like the floor's reference</label>
		<label class="check"><input type="checkbox" bind:checked={c.review_after_sysid} disabled={locked} data-testid="mixed-review" /> Hold for review after identification: nothing plays until you proceed</label>
		{#if dry}<p class="aq-small aq-muted" data-testid="mixed-identification-derived">Identification measures for {dry.identification_s} s.</p>{/if}
	</section>
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
		gap: var(--sp-2);
		min-width: 0;
	}

	.note {
		margin: 0;
	}

	.check {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: var(--sp-2);
		font-size: var(--text-sm);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
