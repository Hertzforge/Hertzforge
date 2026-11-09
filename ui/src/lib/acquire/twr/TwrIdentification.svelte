<script lang="ts">
	/**
	 * How the system is identified before the first play: band-limited random (or a log chirp) at a
	 * stated drive in volts, its peak held under the drive ceiling and the clip; its length, the block
	 * it is estimated in and its band; the mean coherence it must reach, under which the run holds
	 * until a new run; and whether the run holds for the operator to read it.
	 */
	import { formatNumber } from '$lib/viz/format';
	import NumberField from '../NumberField.svelte';
	import { EXCITATIONS, NFFT_OPTIONS } from './twrSetup';
	import type { TwrStore } from './twrStore.svelte';

	interface Props {
		twr: TwrStore;
	}
	let { twr }: Props = $props();

	const id = $derived(twr.setup.identification);
	const locked = $derived(twr.locked);
	const E = $derived(twr.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(twr.check && twr.checkedKey === twr.key ? twr.check.derived : null);
</script>

<section class="aq-panel narrow" aria-labelledby="ti-title" data-testid="twr-identification">
	<div class="aq-head"><h2 id="ti-title">Identification</h2><span class="aq-sub">Before any play, at a low drive.</span></div>
	<p class="aq-lead aq-small">The control service drives the excitation below, measures how the system answers and its lag behind the drive, and inverts that answer to build the first drive. Nothing is driven at test level until identification is done.</p>
	<div class="sel">
		<span class="lbl" id="ti-ex-l">Excitation</span>
		<div class="aq-seg" role="radiogroup" aria-labelledby="ti-ex-l">
			{#each EXCITATIONS as x (x.id)}
				<button type="button" role="radio" aria-checked={id.excitation === x.id} disabled={locked} onclick={() => (id.excitation = x.id)} data-testid="twr-id-{x.id}">{x.label}</button>
			{/each}
		</div>
	</div>
	<div class="aq-fields spaced">
		<NumberField id="ti-v" label="Drive" unit="V rms" step={0.01} min={0} bind:value={id.drive_vrms} errors={errs('identification.drive_vrms')} disabled={locked} testid="twr-id-vrms" help="Its peak is held under the drive ceiling and the clip. Usual: 0.05 V rms." />
		<NumberField id="ti-t" label="Length" unit="s" step={1} min={0} bind:value={id.duration_s} errors={errs('identification.duration_s')} disabled={locked} testid="twr-id-duration" help="At least two blocks. Usual: 4 s." />
		<div class="sel">
			<label for="ti-n" class="lbl">Block length</label>
			<select id="ti-n" class="field num" bind:value={id.nfft} disabled={locked} data-testid="twr-id-nfft">
				{#each NFFT_OPTIONS as k (k)}<option value={k}>{k} points</option>{/each}
			</select>
			<p class="help">The estimate is averaged over blocks this long.</p>
		</div>
		<NumberField id="ti-lo" label="Band from" unit="Hz" step={1} min={0} value={id.band_hz[0]} onchange={(v) => (id.band_hz[0] = v ?? NaN)} errors={errs('identification.band_hz')} disabled={locked} testid="twr-id-lo" />
		<NumberField id="ti-hi" label="Band to" unit="Hz" step={100} min={0} value={id.band_hz[1]} onchange={(v) => (id.band_hz[1] = v ?? NaN)} disabled={locked} testid="twr-id-hi" help="Below half the rate." />
		<NumberField id="ti-coh" label="Mean coherence it must reach" step={0.05} min={0} max={1} bind:value={id.min_mean_coherence} errors={errs('identification.min_mean_coherence')} disabled={locked} testid="twr-id-coh" help="The mean over the band. Under it the run holds, and only a new run releases it. Usual: 0.8." />
	</div>
	{#if dry}<p class="aq-small aq-muted" data-testid="twr-id-length">Identification runs for {formatNumber(dry.identification_s, 3)} s.</p>{/if}
	<label class="aq-switch tog" for="ti-rev">
		<input id="ti-rev" type="checkbox" role="switch" checked={id.review} disabled={locked} onchange={(e) => (id.review = e.currentTarget.checked)} data-testid="twr-id-review" />
		<span class="track" aria-hidden="true"><span class="thumb"></span></span>
		<span>Hold for review after identification</span>
		<span class="word">{id.review ? 'On' : 'Off'}</span>
	</label>
	<p class="help">On: the run holds with the identified system on screen, and nothing plays until you proceed. Off: the law is seeded and the first play follows identification by itself.</p>
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

	.sel > .aq-seg {
		align-self: flex-start;
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
