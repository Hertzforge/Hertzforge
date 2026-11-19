<script lang="ts">
	/**
	 * How the system is identified before the first pulse: band-limited random at a stated drive in
	 * volts, its peak held under the drive ceiling; the band and block length of the estimate; the
	 * coherence it must reach; and whether the run holds for the operator to read it.
	 */
	import NumberField from '../NumberField.svelte';
	import type { ShockStore } from './shockStore.svelte';

	interface Props {
		shock: ShockStore;
	}
	let { shock }: Props = $props();

	const id = $derived(shock.setup.identification);
	const locked = $derived(shock.locked);
	const E = $derived(shock.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const NFFT = [512, 1024, 2048, 4096, 8192, 16384];
</script>

<section class="aq-panel narrow" aria-labelledby="si-title">
	<div class="aq-head"><h2 id="si-title">Identification</h2><span class="aq-sub">Before any pulse, at a low random drive.</span></div>
	<p class="aq-lead aq-small">The control service drives band-limited random at the level below, measures how the system answers, and inverts that answer to build the first drive. Nothing is driven at test level until identification is done.</p>
	<div class="aq-fields">
		<NumberField id="si-v" label="Drive" unit="V rms" step={0.01} min={0} bind:value={id.drive_vrms} errors={errs('identification.drive_vrms')} disabled={locked} testid="shock-id-vrms" help="Its peak is held under the drive ceiling. Usual: 0.05 V rms." />
		<NumberField id="si-t" label="Length" unit="s" step={1} min={0} bind:value={id.duration_s} errors={errs('identification.duration_s')} disabled={locked} testid="shock-id-duration" help="Longer is steadier. Usual: 4 s." />
		<div class="sel">
			<label for="si-n" class="lbl">Block length</label>
			<select id="si-n" class="field num" bind:value={id.nfft} disabled={locked} data-testid="shock-id-nfft">
				{#each NFFT as n (n)}<option value={n}>{n} points</option>{/each}
			</select>
			<p class="help">The estimate is averaged over blocks this long.</p>
		</div>
		<NumberField id="si-lo" label="Band from" unit="Hz" step={1} min={0} value={id.band_hz[0]} onchange={(v) => (id.band_hz[0] = v ?? NaN)} errors={errs('identification.band_hz')} disabled={locked} testid="shock-id-lo" />
		<NumberField id="si-hi" label="Band to" unit="Hz" step={100} min={0} value={id.band_hz[1]} onchange={(v) => (id.band_hz[1] = v ?? NaN)} disabled={locked} testid="shock-id-hi" help="At most 0.4 times the sample rate." />
		<NumberField id="si-coh" label="Coherence it must reach" step={0.05} min={0} max={1} bind:value={id.min_mean_coherence} errors={errs('identification.min_mean_coherence')} disabled={locked} testid="shock-id-coh" help="The mean over the band. Under it the run holds before any pulse. Usual: 0.8." />
	</div>
	<label class="aq-switch tog" for="si-rev">
		<input id="si-rev" type="checkbox" role="switch" checked={id.review} disabled={locked} onchange={(e) => (id.review = e.currentTarget.checked)} data-testid="shock-id-review" />
		<span class="track" aria-hidden="true"><span class="thumb"></span></span>
		<span>Hold for review after identification</span>
		<span class="word">{id.review ? 'On' : 'Off'}</span>
	</label>
	<p class="help">On: the run holds with the identified system on screen, and nothing fires until you proceed. Off: the first pulse follows identification by itself.</p>
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

	.tog {
		margin-top: var(--sp-3);
	}
</style>
