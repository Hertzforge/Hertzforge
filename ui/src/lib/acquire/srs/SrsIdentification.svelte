<script lang="ts">
	/**
	 * How the system is identified before the first pulse: a quiet burst (the synthesized pulse, or
	 * a short broadband pulse) at a level under the full-level synthesis, its peak held under a cap
	 * in volts; the block the gain is estimated in and its floor; the coherence it must reach; and
	 * whether the run holds for the operator to read it. The control law's first gains come from the
	 * gain it measures at each component.
	 */
	import NumberField from '../NumberField.svelte';
	import { EXCITATIONS, NFFTS } from './srsSetup';
	import type { SrsStore } from './srsStore.svelte';

	interface Props {
		srs: SrsStore;
	}
	let { srs }: Props = $props();

	const id = $derived(srs.setup.identification);
	const ceiling = $derived(srs.setup.limits.drive_vpk_ceiling);
	const locked = $derived(srs.locked);
	const E = $derived(srs.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
</script>

<section class="aq-panel narrow" aria-labelledby="ri-title" data-testid="srs-identification">
	<div class="aq-head"><h2 id="ri-title">Identification</h2><span class="aq-sub">Before any pulse, at a low drive.</span></div>
	<p class="aq-lead aq-small">The control service plays a quiet burst, measures how the control channel (and every limit channel without a supplied gain) answers, and seeds the control law from that gain at each component. Nothing is driven at test level until identification is done.</p>
	<div class="sel wide">
		<span class="lbl" id="ri-exc-l">Burst</span>
		<div class="aq-seg" role="radiogroup" aria-labelledby="ri-exc-l">
			{#each EXCITATIONS as x (x.id)}
				<button type="button" role="radio" aria-checked={id.excitation === x.id} disabled={locked} onclick={() => (id.excitation = x.id)} data-testid="srs-id-{x.id}">{x.label}</button>
			{/each}
		</div>
		<p class="help">{EXCITATIONS.find((x) => x.id === id.excitation)?.help}</p>
	</div>
	<div class="aq-fields spaced">
		<NumberField id="ri-lv" label="Level under full" unit="dB" step={1} max={0} bind:value={id.level_db} errors={errs('identification.level_db')} disabled={locked} testid="srs-id-level" help="Below 0 dB and at least -60. Usual: -20 dB." />
		<NumberField
			id="ri-cap"
			label="Burst peak at most"
			unit="V pk"
			nullable="A tenth of the drive ceiling"
			fallback={Number.isFinite(ceiling) ? ceiling / 10 : 0.05}
			step={0.01}
			min={0}
			bind:value={id.max_vpk}
			errors={errs('identification.max_vpk')}
			disabled={locked}
			testid="srs-id-max-vpk"
			help="A burst over it is scaled down to it. At most the drive ceiling."
		/>
		<div class="sel">
			<label for="ri-n" class="lbl">Block length</label>
			<select id="ri-n" class="field num" bind:value={id.nfft} disabled={locked} data-testid="srs-id-nfft">
				{#if !NFFTS.includes(id.nfft)}<option value={id.nfft}>{id.nfft} points</option>{/if}
				{#each NFFTS as n (n)}<option value={n}>{n} points</option>{/each}
			</select>
			<p class="help">The gain is estimated in blocks this long. Usual: 1024 points.</p>
		</div>
		<NumberField id="ri-fl" label="Gain floor, share of its peak" step={0.01} min={0} max={1} bind:value={id.h_floor_frac} errors={errs('identification.h_floor_frac')} disabled={locked} testid="srs-id-floor" help="The gain is floored here before the seed divides by it, so a band the system barely answers is not driven hard. Usual: 0.05." />
		<NumberField id="ri-coh" label="Coherence it must reach" step={0.05} min={0} max={1} bind:value={id.min_mean_coherence} errors={errs('identification.min_mean_coherence')} disabled={locked} testid="srs-id-coh" help="The mean over the band. Under it the run holds before any pulse. Usual: 0.8." />
	</div>
	<label class="aq-switch tog" for="ri-rev">
		<input id="ri-rev" type="checkbox" role="switch" checked={id.review} disabled={locked} onchange={(e) => (id.review = e.currentTarget.checked)} data-testid="srs-id-review" />
		<span class="track" aria-hidden="true"><span class="thumb"></span></span>
		<span>Hold for review after identification</span>
		<span class="word">{id.review ? 'On' : 'Off'}</span>
	</label>
	<p class="help">On: the run holds with the identified gain and its coherence on screen, and nothing fires until you proceed. Off: the first pulse follows identification by itself.</p>
</section>

<style>
	.narrow {
		max-width: 860px;
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

	.help {
		margin: 2px 0 0;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.spaced {
		margin-top: var(--sp-3);
	}

	.tog {
		margin-top: var(--sp-3);
	}
</style>
