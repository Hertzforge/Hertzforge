<script lang="ts">
	/**
	 * What the service says of the setup as it stands: every refusal by its name, with the part,
	 * control or drive it is on, and every warning the same way; then the prediction and the budget
	 * forecast. The prediction needs the identified model, so before a run has identified this says
	 * when it comes: at the review hold, before anything drives at level.
	 */
	import { REFUSAL_WORDS, refusalWhere } from './mmixSetup';
	import type { MimoMixedStore } from './mmixStore.svelte';
	import MmixPrediction from './MmixPrediction.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const f = $derived(mm.findings);
	const names = $derived({ parts: Object.fromEntries([...mm.setup.tones, ...mm.setup.bands].map((p) => [p.id, p.name])), controls: mm.controlLabels, drives: mm.driveLabels });
	const prediction = $derived(mm.status?.prediction ?? null);
	const dry = $derived(mm.check && mm.checkedKey === mm.key ? mm.check.derived : null);
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="mmix-refusals" data-testid="mmix-refusals">
		<div class="aq-head"><h2 id="mmix-refusals">Refusals and Warnings</h2><span class="aq-sub">{mm.checking ? 'Checking the setup.' : 'The service checks the setup as you edit it.'}</span></div>
		{#if f.errors.length || f.warnings.length}
			<ul class="items">
				{#each f.errors as e, i (i)}
					{@const where = refusalWhere(e, names)}
					<li class="err" data-testid="mmix-refusal-{e.reason}" data-part={e.part ?? ''} data-control={e.control ?? ''} data-drive={e.drive ?? ''}>
						<span class="mark" aria-hidden="true">✕</span>
						<span><strong>{REFUSAL_WORDS[e.reason] ?? e.reason}</strong>{where ? ` (${where})` : ''}: {e.message}</span>
					</li>
				{/each}
				{#each f.warnings as w, i (i)}
					{@const where = refusalWhere(w, names)}
					<li class="warn" data-testid="mmix-warning-{w.reason}" data-part={w.part ?? ''}>
						<span class="mark" aria-hidden="true">!</span>
						<span><strong>{REFUSAL_WORDS[w.reason] ?? w.reason}</strong>{where ? ` (${where})` : ''}: {w.message}</span>
					</li>
				{/each}
			</ul>
		{:else if dry}
			<p class="aq-small" data-testid="mmix-no-refusals">The service accepts the setup. The test's band is {dry.band_hz[0]} to {dry.band_hz[1]} Hz; identification takes about {dry.identification_s} s; the tracker's delay is about {dry.tracker_delay_s} s.</p>
		{:else}
			<p class="aq-small aq-muted">{mm.pending ?? 'Checked once the fields are complete.'}</p>
		{/if}
	</section>

	<section class="aq-panel" aria-labelledby="mmix-pred" data-testid="mmix-forecast">
		<div class="aq-head"><h2 id="mmix-pred">Prediction and Budget</h2><span class="aq-sub">Each drive's peak need, the floor's and the tones' together, against its ceiling.</span></div>
		{#if prediction}
			<MmixPrediction {prediction} drives={mm.runDriveLabels} controls={mm.runControlLabels} />
		{:else}
			<p class="aq-small aq-muted" data-testid="mmix-forecast-later">The prediction needs the model of the structure, so it comes after identification: the run holds for review with it, before anything drives at level. A forecast that cannot fit holds the run until you proceed with your words or abort.</p>
		{/if}
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.items {
		margin: 0;
		padding: 0;
		list-style: none;
		display: flex;
		flex-direction: column;
		gap: 6px;
		font-size: var(--text-sm);
	}

	.items li {
		display: flex;
		gap: var(--sp-2);
	}

	.err .mark {
		color: var(--amber-1);
	}

	.warn .mark {
		color: var(--amber-1);
	}
</style>
