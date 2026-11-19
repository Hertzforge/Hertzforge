<script lang="ts">
	/**
	 * The standard each pulse is judged by, and what a failed pulse does; then how the pulse is
	 * measured: the rate asked of the station and each pulse's shock response spectrum, which is
	 * informational (the run is judged on the pulse, not on its SRS).
	 */
	import NumberField from '../NumberField.svelte';
	import { STANDARDS } from './shockSetup';
	import type { ShockStore } from './shockStore.svelte';

	interface Props {
		shock: ShockStore;
	}
	let { shock }: Props = $props();

	const d = $derived(shock.setup);
	const locked = $derived(shock.locked);
	const E = $derived(shock.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const std = $derived(STANDARDS.find((s) => s.id === d.standard.name) ?? STANDARDS[0]);
	const pct = (v: number) => `${Math.round(v * 100)}%`;
</script>

<div class="cols">
	<section class="aq-panel" aria-labelledby="ss-std">
		<div class="aq-head"><h2 id="ss-std">Standard and Tolerance</h2></div>
		<div class="aq-group">
			<h3>Judged By</h3>
			<div class="aq-seg" role="radiogroup" aria-label="Standard">
				{#each STANDARDS as s (s.id)}
					<button type="button" role="radio" aria-checked={d.standard.name === s.id} disabled={locked} onclick={() => (d.standard.name = s.id)} data-testid="shock-standard-{s.id}">{s.label}</button>
				{/each}
			</div>
			<p class="aq-small aq-muted">Each pulse passes when its amplitude stays within {pct(std.amplitude)} of the peak around the reference and its velocity change is within {pct(std.velocity)}.</p>
		</div>
		<div class="aq-group">
			<h3>A Failed Pulse</h3>
			<div class="aq-seg" role="radiogroup" aria-label="When a failed pulse aborts">
				<button type="button" role="radio" aria-checked={d.standard.tolerance_abort === 'full_level'} disabled={locked} onclick={() => (d.standard.tolerance_abort = 'full_level')} data-testid="shock-abort-full">Aborts at full level</button>
				<button type="button" role="radio" aria-checked={d.standard.tolerance_abort === 'every_level'} disabled={locked} onclick={() => (d.standard.tolerance_abort = 'every_level')} data-testid="shock-abort-every">Aborts at every level</button>
			</div>
			<p class="aq-small aq-muted">
				{d.standard.tolerance_abort === 'full_level'
					? 'Below full level a failed pulse raises an alarm and the law corrects the next one; at full level it aborts.'
					: 'Any failed pulse aborts the run, at any level.'}
				A pulse that comes in low never aborts by itself: the limit on pulses at one level ends a run that does not converge.
			</p>
			<div class="aq-fields spaced">
				<NumberField
					id="ss-over"
					label="Overshoot abort, share over the commanded peak"
					nullable="Twice the tolerance"
					fallback={std.amplitude * 2}
					step={0.05}
					min={0}
					bind:value={d.standard.overshoot_abort}
					errors={errs('standard.overshoot_abort')}
					disabled={locked}
					testid="shock-overshoot"
					help="A measured peak over (1 + this) times the commanded peak aborts at any level. Usual: twice the amplitude tolerance, {std.amplitude * 2} under this standard."
				/>
			</div>
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="ss-acq">
		<div class="aq-head"><h2 id="ss-acq">Measurement</h2><span class="aq-sub">The station may run at a nearby rate; the run reports the one it uses.</span></div>
		<div class="aq-fields">
			<NumberField id="ss-fs" label="Sample rate" unit="Hz" step={1024} min={1024} bind:value={d.acquisition.fs_hz} errors={errs('acquisition.fs_hz')} disabled={locked} testid="shock-fs" help="The rate asked of the station for identification and every pulse." />
		</div>
		<div class="aq-group">
			<h3>Shock Response Spectrum</h3>
			<p class="aq-lead">Computed for every pulse and shown on the SRS page. It is informational: the run is judged on the pulse.</p>
			<div class="aq-fields">
				<NumberField id="ss-flo" label="From" unit="Hz" step={5} min={0} bind:value={d.acquisition.srs.f_lo_hz} errors={errs('acquisition.srs.f_lo_hz')} disabled={locked} testid="shock-srs-lo" />
				<NumberField id="ss-fhi" label="To" unit="Hz" step={100} min={0} bind:value={d.acquisition.srs.f_hi_hz} errors={errs('acquisition.srs.f_hi_hz')} disabled={locked} testid="shock-srs-hi" />
				<div class="sel">
					<label for="ss-bpo" class="lbl">Bands per octave</label>
					<select id="ss-bpo" class="field num" bind:value={d.acquisition.srs.bands_per_octave} disabled={locked} data-testid="shock-srs-bpo">
						{#each [1, 3, 6, 12, 24] as n (n)}<option value={n}>{n}</option>{/each}
					</select>
				</div>
				<NumberField id="ss-tol" label="Band" unit="± dB" step={0.5} min={0} bind:value={d.acquisition.srs.band_tol_db} errors={errs('acquisition.srs.band_tol_db')} disabled={locked} testid="shock-srs-tol" help="The informational band drawn about the reference's SRS." />
			</div>
			<label class="aq-switch tog" for="ss-pvss">
				<input id="ss-pvss" type="checkbox" role="switch" checked={d.acquisition.pvss} disabled={locked} onchange={(e) => (d.acquisition.pvss = e.currentTarget.checked)} data-testid="shock-pvss" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Also compute the pseudo-velocity SRS</span>
				<span class="word">{d.acquisition.pvss ? 'On' : 'Off'}</span>
			</label>
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

	.spaced,
	.tog {
		margin-top: var(--sp-3);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
