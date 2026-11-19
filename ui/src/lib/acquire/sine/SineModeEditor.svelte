<script lang="ts">
	/**
	 * What the test does with the frequency: a sweep across a band at a rate, up and down, until a
	 * count of one-way sweeps or a test time; a dwell at one frequency for some seconds or cycles; or
	 * a step test that holds each step across a band. The frequency, its direction and the tone's
	 * phase are the control service's, never the control law's. The rate the station runs at is
	 * here too. Each kind keeps its own settings, so a change of kind loses nothing.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { SineStore } from './sineStore.svelte';

	interface Props {
		sine: SineStore;
	}
	let { sine }: Props = $props();

	const t = $derived(sine.setup.test);
	const acq = $derived(sine.setup.acquisition);
	const locked = $derived(sine.locked);
	const E = $derived(sine.fieldErrors);
	const W = $derived(sine.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(sine.check && sine.checkedKey === sine.key ? sine.check.derived : null);
	const KINDS = [
		{ id: 'sweep' as const, label: 'Sweep', help: 'The frequency moves across the band at a rate, up and down.' },
		{ id: 'dwell' as const, label: 'Dwell', help: 'The tone holds one frequency.' },
		{ id: 'step' as const, label: 'Step', help: 'The tone holds each step across the band in turn.' }
	];
</script>

<div class="cols">
	<section class="aq-panel" aria-labelledby="nm-title" data-testid="sine-test">
		<div class="aq-head"><h2 id="nm-title">Test</h2><span class="aq-sub">What the frequency does.</span></div>
		<div class="sel">
			<span class="lbl" id="nm-kind-l">Kind</span>
			<div class="aq-seg" role="radiogroup" aria-labelledby="nm-kind-l">
				{#each KINDS as k (k.id)}
					<button type="button" role="radio" aria-checked={t.kind === k.id} disabled={locked} onclick={() => (t.kind = k.id)} data-testid="sine-kind-{k.id}">{k.label}</button>
				{/each}
			</div>
			<p class="help">{KINDS.find((k) => k.id === t.kind)?.help ?? ''}</p>
		</div>

		{#if t.kind === 'rstd'}
			<p class="aq-notice aq-small" data-testid="sine-rstd">This setup came with a resonance search, track and dwell, which this screen does not edit. It is sent as it came; choose a sweep, a dwell or a step to set the test here instead.</p>
		{:else if t.kind === 'dwell'}
			<div class="aq-fields spaced">
				<NumberField id="nm-df" label="Frequency" unit="Hz" step={10} min={0} bind:value={t.dwell.freq_hz} errors={errs('test.freq_hz')} disabled={locked} testid="sine-dwell-hz" />
				<div class="sel">
					<span class="lbl" id="nm-dby-l">For</span>
					<div class="aq-seg sm" role="radiogroup" aria-labelledby="nm-dby-l">
						<button type="button" role="radio" aria-checked={t.dwell.by === 'seconds'} disabled={locked} onclick={() => (t.dwell.by = 'seconds')} data-testid="sine-dwell-by-seconds">Seconds</button>
						<button type="button" role="radio" aria-checked={t.dwell.by === 'cycles'} disabled={locked} onclick={() => (t.dwell.by = 'cycles')} data-testid="sine-dwell-by-cycles">Cycles</button>
					</div>
				</div>
				{#if t.dwell.by === 'seconds'}
					<NumberField id="nm-ds" label="Test time" unit="s" step={10} min={0} bind:value={t.dwell.seconds} errors={errs('test.end.seconds')} disabled={locked} testid="sine-dwell-seconds" />
				{:else}
					<NumberField id="nm-dc" label="Cycles" step={1000} min={0} bind:value={t.dwell.cycles} errors={errs('test.end.cycles')} disabled={locked} testid="sine-dwell-cycles" />
				{/if}
			</div>
		{:else}
			<div class="aq-fields spaced">
				<NumberField id="nm-lo" label="From" unit="Hz" step={10} min={0} value={t.band_hz[0]} onchange={(v) => (t.band_hz[0] = v ?? NaN)} errors={errs('test.band_hz.0')} disabled={locked} testid="sine-band-lo" />
				<NumberField id="nm-hi" label="To" unit="Hz" step={100} min={0} value={t.band_hz[1]} onchange={(v) => (t.band_hz[1] = v ?? NaN)} errors={errs('test.band_hz.1')} disabled={locked} testid="sine-band-hi" help="Under 0.45 times the rate the station runs at." />
			</div>
			{#if t.kind === 'sweep'}
				{@const s = t.sweep}
				<div class="aq-group sep">
					<h3>Sweep</h3>
					<div class="aq-fields">
						<div class="sel">
							<span class="lbl" id="nm-law-l">Rate</span>
							<div class="aq-seg sm" role="radiogroup" aria-labelledby="nm-law-l">
								<button type="button" role="radio" aria-checked={s.law === 'log'} disabled={locked} onclick={() => (s.law = 'log')} data-testid="sine-rate-log">Octaves a minute</button>
								<button type="button" role="radio" aria-checked={s.law === 'linear'} disabled={locked} onclick={() => (s.law = 'linear')} data-testid="sine-rate-linear">Hertz a second</button>
							</div>
						</div>
						{#if s.law === 'log'}
							<NumberField id="nm-oct" label="Sweep rate" unit="oct/min" step={0.5} min={0} bind:value={s.oct_per_min} errors={errs('test.rate')} warnings={W['control.tracking'] ?? []} disabled={locked} testid="sine-rate" />
						{:else}
							<NumberField id="nm-hzs" label="Sweep rate" unit="Hz/s" step={1} min={0} bind:value={s.hz_per_s} errors={errs('test.rate')} warnings={W['control.tracking'] ?? []} disabled={locked} testid="sine-rate" />
						{/if}
						<NumberField id="nm-start" label="Start at" unit="Hz" nullable="The band's low edge" fallback={Number.isFinite(t.band_hz[0]) ? t.band_hz[0] : 20} step={10} min={0} bind:value={s.start_hz} errors={errs('test.start_hz')} disabled={locked} testid="sine-start" />
						<div class="sel">
							<span class="lbl" id="nm-dir-l">First leg</span>
							<div class="aq-seg sm" role="radiogroup" aria-labelledby="nm-dir-l">
								<button type="button" role="radio" aria-checked={s.direction === 'up'} disabled={locked} onclick={() => (s.direction = 'up')} data-testid="sine-dir-up">Up</button>
								<button type="button" role="radio" aria-checked={s.direction === 'down'} disabled={locked} onclick={() => (s.direction = 'down')} data-testid="sine-dir-down">Down</button>
							</div>
						</div>
					</div>
				</div>
				<div class="aq-group">
					<h3>End</h3>
					<p class="aq-lead aq-small">Counted over the whole run from engage, at every level: a sweep from one edge of the band to the other is one sweep.</p>
					<div class="aq-fields">
						<div class="sel wide">
							<span class="lbl" id="nm-end-l">The run ends after</span>
							<div class="aq-seg sm" role="radiogroup" aria-labelledby="nm-end-l">
								<button type="button" role="radio" aria-checked={s.end_by === 'sweeps'} disabled={locked} onclick={() => (s.end_by = 'sweeps')} data-testid="sine-end-sweeps">A count of sweeps</button>
								<button type="button" role="radio" aria-checked={s.end_by === 'duration'} disabled={locked} onclick={() => (s.end_by = 'duration')} data-testid="sine-end-duration">A test time</button>
								<button type="button" role="radio" aria-checked={s.end_by === 'both'} disabled={locked} onclick={() => (s.end_by = 'both')} data-testid="sine-end-both">Whichever comes first</button>
							</div>
						</div>
						{#if s.end_by !== 'duration'}
							<NumberField id="nm-sw" label="Sweeps" integer step={1} min={1} bind:value={s.sweeps} errors={errs('test.end.sweeps')} disabled={locked} testid="sine-sweeps" />
						{/if}
						{#if s.end_by !== 'sweeps'}
							<NumberField id="nm-dur" label="Test time" unit="s" step={60} min={0} bind:value={s.duration_s} errors={errs('test.end.duration_s')} disabled={locked} testid="sine-duration" />
						{/if}
					</div>
				</div>
			{:else}
				{@const st = t.step}
				<div class="aq-group sep">
					<h3>Steps</h3>
					<div class="aq-fields">
						<NumberField id="nm-spo" label="Steps an octave" step={1} min={0} bind:value={st.steps_per_octave} errors={errs('test.steps_per_octave')} disabled={locked} testid="sine-steps-per-octave" />
						<div class="sel">
							<span class="lbl" id="nm-sp-l">Spacing</span>
							<div class="aq-seg sm" role="radiogroup" aria-labelledby="nm-sp-l">
								<button type="button" role="radio" aria-checked={st.spacing === 'log'} disabled={locked} onclick={() => (st.spacing = 'log')} data-testid="sine-spacing-log">Even in octaves</button>
								<button type="button" role="radio" aria-checked={st.spacing === 'linear'} disabled={locked} onclick={() => (st.spacing = 'linear')} data-testid="sine-spacing-linear">Even in hertz</button>
							</div>
						</div>
						<div class="sel wide">
							<span class="lbl" id="nm-sby-l">Each step holds</span>
							<div class="aq-seg sm" role="radiogroup" aria-labelledby="nm-sby-l">
								<button type="button" role="radio" aria-checked={st.by === 'computed'} disabled={locked} onclick={() => (st.by = 'computed')} data-testid="sine-step-computed">As long as it needs</button>
								<button type="button" role="radio" aria-checked={st.by === 'seconds'} disabled={locked} onclick={() => (st.by = 'seconds')} data-testid="sine-step-seconds">Seconds</button>
								<button type="button" role="radio" aria-checked={st.by === 'cycles'} disabled={locked} onclick={() => (st.by = 'cycles')} data-testid="sine-step-cycles">Cycles</button>
							</div>
						</div>
						{#if st.by === 'seconds'}
							<NumberField id="nm-ss" label="Seconds a step" unit="s" step={1} min={0} bind:value={st.seconds} errors={errs('test.dwell.seconds')} disabled={locked} testid="sine-step-seconds-n" />
						{:else if st.by === 'cycles'}
							<NumberField id="nm-sc" label="Cycles a step" step={100} min={0} bind:value={st.cycles} errors={errs('test.dwell.cycles')} disabled={locked} testid="sine-step-cycles-n" />
						{/if}
					</div>
				</div>
			{/if}
		{/if}
		<Findings items={[...errs('test'), ...errs('test.kind'), ...errs('test.end')].map((message) => ({ level: 'error' as const, message }))} testid="sine-test-findings" />
	</section>

	<section class="aq-panel" aria-labelledby="nm-rate" data-testid="sine-acquisition">
		<div class="aq-head"><h2 id="nm-rate">Station Rate</h2><span class="aq-sub">The samples a second the station runs at.</span></div>
		<div class="aq-fields">
			<NumberField id="nm-fs" label="Rate" unit="Hz" nullable="The usual rate, 16384 Hz" fallback={16384} step={1024} min={1000} max={204800} bind:value={acq.fs_hz} errors={errs('acquisition.fs_hz')} disabled={locked} testid="sine-fs" help="Asked of the station, 1000 to 204800 Hz; the run says the rate the station adopted." />
		</div>
		{#if dry}
			<dl class="facts" data-testid="sine-test-derived">
				<div><dt>Rate</dt><dd class="num">{formatNumber(dry.grid.fs_hz, 6)} Hz{dry.grid.actual ? '' : ', until the station arms'}</dd></div>
				<div><dt>Band</dt><dd class="num">{formatNumber(dry.band_hz[0], 5)} to {formatNumber(dry.band_hz[1], 5)} Hz</dd></div>
				<div><dt>Test time</dt><dd class="num" data-testid="sine-test-s">{dry.test_s === null ? 'Set by hand in manual' : `${formatNumber(dry.test_s, 4)} s`}</dd></div>
				<div><dt>Identification</dt><dd class="num" data-testid="sine-identification-s">{formatNumber(dry.identification_s, 4)} s</dd></div>
			</dl>
			{#if dry.tracking.message}<p class="aq-notice aq-small" class:warn={dry.tracking.verdict !== 'ok'} data-testid="sine-tracking">{dry.tracking.message}</p>{/if}
		{:else if sine.checking}
			<p class="aq-muted aq-small">Checking the setup.</p>
		{/if}
	</section>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1.5fr) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.sel.wide {
		grid-column: 1 / -1;
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

	.facts {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2) var(--sp-3);
		margin: var(--sp-3) 0 var(--sp-2);
	}

	.facts dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.facts dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
	/* A group after fields or a table is set off as one after another group is. */
	.sep {
		margin-top: var(--sp-5);
		padding-top: var(--sp-4);
		border-top: 1px solid var(--line-1);
	}
</style>
