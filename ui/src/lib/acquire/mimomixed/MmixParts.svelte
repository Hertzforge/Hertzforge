<script lang="ts">
	/**
	 * The parts on the floor: the tones table, the narrowbands table, and the two settings every
	 * part shares. The exclusion is the band around each tone the floor's law leaves alone; the
	 * tracking bandwidth sets how fast each tone's tracker follows, and so how fast a tone can sweep
	 * through a resonance before the tracker loses it. The tracker's delay is said in seconds.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import MmixPartsTable from './MmixPartsTable.svelte';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const d = $derived(mm.setup);
	const locked = $derived(mm.locked);
	const E = $derived(mm.fieldErrors);
	const delay = $derived(Number.isFinite(d.tracking_hz) && d.tracking_hz > 0 ? 0.625 / d.tracking_hz : null);
</script>

<div class="stack">
	<MmixPartsTable {mm} table="tones" />
	<MmixPartsTable {mm} table="bands" />
	<section class="aq-panel" aria-labelledby="mmix-track" data-testid="mmix-tracking">
		<div class="aq-head"><h2 id="mmix-track">Tracking</h2><span class="aq-sub">Shared by every tone.</span></div>
		<div class="aq-fields narrow">
			<NumberField id="mmix-excl" label="Exclusion around each tone" unit="Hz" min={0} step={0.5} bind:value={d.exclusion_hz} errors={E.exclusion_hz} disabled={locked} testid="mmix-exclusion" help="The floor's law is not judged on lines this close to a tone." />
			<NumberField id="mmix-trk" label="Tracking bandwidth" unit="Hz" min={0.05} step={0.1} bind:value={d.tracking_hz} errors={E.tracking_hz} disabled={locked} testid="mmix-tracking-hz" />
		</div>
		<p class="aq-small" data-testid="mmix-tracker-delay">
			{#if delay !== null}
				At {d.tracking_hz} Hz the tracker reads each tone about {delay.toFixed(2)} s late. A tone that sweeps through a resonance faster than the tracker can follow loses its amplitude reading there and the run aborts: sweeps of a few Hz/s through a resonance are beyond it at the default 0.5 Hz.
			{:else}
				Give the tracking bandwidth to see the tracker's delay.
			{/if}
		</p>
		<Findings items={[...(E['tones'] ?? []), ...(E['bands'] ?? [])].map((message) => ({ level: 'error' as const, message }))} />
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	p {
		margin: var(--sp-2) 0 0;
	}
</style>
