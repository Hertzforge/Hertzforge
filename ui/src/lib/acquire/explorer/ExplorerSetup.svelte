<script lang="ts">
	/**
	 * The tone and the drive: the band the tone may move in and where it starts, the rate asked of
	 * the station, the drive ceiling in volts peak (no default: it is the amplifier's input), the
	 * frequency and drive slews, the display's FFT size, and how far one press of the steering panel
	 * moves the tone. What the service derived for this setup is in the summary beside every
	 * section, shown as it came and never computed here.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { NFFT_CHOICES } from './explorerSetup';
	import type { ExplorerStore } from './explorerStore.svelte';
	import { octaveStep } from './explorerWords';

	interface Props {
		explorer: ExplorerStore;
	}
	let { explorer }: Props = $props();

	const d = $derived(explorer.setup);
	const locked = $derived(explorer.locked);
	const E = $derived(explorer.fieldErrors);
	const W = $derived(explorer.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];
</script>

<div class="stack narrow">
	<section class="aq-panel" aria-labelledby="xs-tone" data-testid="explorer-tone">
		<div class="aq-head"><h2 id="xs-tone">Tone</h2><span class="aq-sub">One sine you steer by hand.</span></div>
		<div class="sel">
			<label for="xs-title" class="lbl">Title</label>
			<input id="xs-title" class="field" bind:value={d.title} disabled={locked} data-testid="explorer-title" />
		</div>
		<div class="aq-fields spaced">
			<NumberField id="xs-lo" label="Band from" unit="Hz" step={1} min={0} bind:value={d.band_hz[0]} errors={errs('band_hz')} warnings={warns('band_hz')} disabled={locked} testid="explorer-band-lo" />
			<NumberField id="xs-hi" label="Band to" unit="Hz" step={10} min={0} bind:value={d.band_hz[1]} disabled={locked} testid="explorer-band-hi" help="Under 0.45 times the rate." />
			<NumberField id="xs-start" label="Start at" unit="Hz" step={1} min={0} bind:value={d.start_hz} errors={errs('start_hz')} disabled={locked} testid="explorer-start" help="Inside the band. The tone starts here at 0 V." />
			<NumberField id="xs-fs" label="Rate asked of the station" unit="Hz" nullable="Usual (8192 Hz)" fallback={12800} step={100} min={1000} max={204800} bind:value={d.acquisition.fs_hz} errors={errs('acquisition.fs_hz')} disabled={locked} testid="explorer-fs" help="A station may program another rate; the run adopts it and says so." />
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="xs-drive" data-testid="explorer-drive">
		<div class="aq-head"><h2 id="xs-drive">Drive</h2><span class="aq-sub">In volts peak at the drive output.</span></div>
		<p class="aq-lead aq-small lead">The level is set in volts peak only. Nothing measures the response back into the drive: what the article does at a given voltage is the response you read.</p>
		<div class="aq-fields">
			<NumberField id="xs-ceil" label="Drive ceiling" unit="V pk" step={0.1} min={0} max={10} bind:value={d.max_drive_vpk} errors={errs('max_drive_vpk')} disabled={locked} testid="explorer-ceiling" help="Every level is held at or under it, and a sample over it aborts the run." />
			<NumberField id="xs-dslew" label="Drive slew" unit="dB/s" step={1} min={0} max={1000} bind:value={d.slew.drive_db_per_s} errors={errs('slew.drive_db_per_s')} disabled={locked} testid="explorer-drive-slew" help="Every level change and the stop ramp move at it. Usual: 6 dB/s." />
			<NumberField id="xs-fslew" label="Frequency slew" unit="oct/s" step={0.5} min={0} max={10} bind:value={d.slew.freq_oct_per_s} errors={errs('slew.freq_oct_per_s')} disabled={locked} testid="explorer-freq-slew" help="The fastest a sweep moves the tone. Usual: 1 oct/s." />
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="xs-disp" data-testid="explorer-display">
		<div class="aq-head"><h2 id="xs-disp">Display</h2></div>
		<div class="sel">
			<span class="lbl" id="xs-nfft-l">FFT size</span>
			<div class="aq-seg" role="radiogroup" aria-labelledby="xs-nfft-l">
				{#each NFFT_CHOICES as v (v)}
					<button type="button" role="radio" aria-checked={d.display.nfft === v} disabled={locked} onclick={() => (d.display.nfft = v)} data-testid="explorer-nfft-{v}">{v}</button>
				{/each}
			</div>
			<p class="help">The spectra's line spacing is the rate over it, and half of that is how close two survey points may be: a larger FFT gives a finer survey line.</p>
			<Findings items={errs('display.nfft').map((message) => ({ level: 'error' as const, message }))} />
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="xs-steps" data-testid="explorer-steps">
		<div class="aq-head"><h2 id="xs-steps">Steering</h2><span class="aq-sub">How far one press moves the tone. Kept on this screen.</span></div>
		<div class="aq-fields">
			<NumberField id="xs-ffine" label="Fine frequency step" unit="steps/oct" step={12} min={1} bind:value={d.steps.fine_per_oct} errors={errs('steps.fine_per_oct')} testid="explorer-step-fine-oct" help="{octaveStep(1 / d.steps.fine_per_oct)} a press. Usual: 96." />
			<NumberField id="xs-fcoarse" label="Coarse frequency step" unit="steps/oct" step={1} min={1} bind:value={d.steps.coarse_per_oct} errors={errs('steps.coarse_per_oct')} testid="explorer-step-coarse-oct" help="{octaveStep(1 / d.steps.coarse_per_oct)} a press. Usual: 6." />
			<NumberField id="xs-dfine" label="Fine level step" unit="dB" step={0.5} min={0} bind:value={d.steps.fine_db} errors={errs('steps.fine_db')} testid="explorer-step-fine-db" help="Usual: 0.5 dB." />
			<NumberField id="xs-dcoarse" label="Coarse level step" unit="dB" step={1} min={0} bind:value={d.steps.coarse_db} errors={errs('steps.coarse_db')} testid="explorer-step-coarse-db" help="Usual: 3 dB." />
			<NumberField id="xs-rate" label="Sweep rate to start from" unit="oct/min" step={1} min={0} bind:value={d.steps.sweep_oct_per_min} errors={errs('steps.sweep_oct_per_min')} testid="explorer-step-sweep" />
		</div>
	</section>
</div>

<style>
	.narrow {
		max-width: 860px;
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
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

	.lead {
		margin: 0 0 var(--sp-2);
	}

	.spaced {
		margin-top: var(--sp-3);
	}

</style>
