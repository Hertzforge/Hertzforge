<script lang="ts">
	/**
	 * The test's title, its target kind (a classical pulse at every control, or an SRS at every
	 * control), and each control's own target: the pulse's shape, amplitude, duration, polarity and
	 * shoulders under the chosen standard; or the SRS breakpoints (typed here, or opened from a
	 * breakpoint table file) with the synthesis that builds a waveform for it, on one band grid and
	 * tolerance for every control. One control is edited at a time; the first control's target can
	 * be copied to the rest. Beside it, the control service draws the control's target at full level
	 * from the dry run: a computation, never a measurement.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import type { View } from '$lib/protocol/types';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import PointsTable from '../srs/PointsTable.svelte';
	import { COMPENSATIONS, sameTargetForAll, SHAPES, STANDARDS } from './msSetup';
	import type { MimoShockStore } from './msStore.svelte';

	interface Props {
		ms: MimoShockStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	let at = $state(0);
	const n = $derived(d.controls.length);
	const i = $derived(Math.min(at, Math.max(0, n - 1)));
	const ch = $derived(d.controls[i]);
	const p = $derived(d.pulses[i]);
	const sc = $derived(d.srs.controls[i]);
	const views = $derived.by((): View[] => {
		const all = ms.check && ms.checkedKey === ms.key ? ms.check.views : [];
		return all.filter((v) => v.view === 'spectrum' && (v.title === `Target ${ch}` || v.title === `Target SRS ${ch}`));
	});
	/** Errors on a control's target, for the control's tab mark. */
	const controlErrors = (k: number) => Object.entries(E).filter(([key]) => key.startsWith(`pulses.${k}.`) || key.startsWith(`srs.controls.${k}.`)).reduce((s, [, v]) => s + v.length, 0);
</script>

<div class="cols">
	<div class="stack">
		<section class="aq-panel" aria-labelledby="ms-t-title">
			<div class="aq-head"><h2 id="ms-t-title">Targets</h2><span class="aq-sub">What each control point should see at full level.</span></div>
			<div class="aq-fields">
				<div class="sel wide">
					<label for="ms-title" class="lbl">Test title</label>
					<input id="ms-title" class="field" bind:value={d.title} disabled={locked} aria-invalid={errs('title').length > 0} data-testid="ms-title" />
					<Findings items={errs('title').map((message) => ({ level: 'error' as const, message }))} />
				</div>
				<div class="sel wide">
					<span class="lbl" id="ms-kind-l">Target at every control</span>
					<div class="aq-seg" role="radiogroup" aria-labelledby="ms-kind-l">
						<button type="button" role="radio" aria-checked={d.kind === 'waveform'} disabled={locked} onclick={() => ms.setKind('waveform')} data-testid="ms-kind-waveform">Classical pulse</button>
						<button type="button" role="radio" aria-checked={d.kind === 'srs'} disabled={locked} onclick={() => ms.setKind('srs')} data-testid="ms-kind-srs">Shock response spectrum</button>
					</div>
					<span class="aq-small aq-muted">One kind for every control. Each control is judged on its own target; the timing between controls is free unless it is held (Timing & Verdict).</span>
					<Findings items={errs('kind').map((message) => ({ level: 'error' as const, message }))} />
				</div>
				{#if d.kind === 'waveform'}
					<div class="sel">
						<label for="ms-std" class="lbl">Standard</label>
						<select id="ms-std" class="field" bind:value={d.standard} disabled={locked} data-testid="ms-standard">
							{#each STANDARDS as s (s.id)}<option value={s.id}>{s.label}</option>{/each}
						</select>
						<span class="aq-small aq-muted">Every control's pulse is judged by its tolerance.</span>
					</div>
				{/if}
			</div>

			{#if d.kind === 'srs'}
				<div class="aq-group">
					<h3>Band Grid and Tolerance</h3>
					<div class="aq-fields">
						<NumberField id="ms-blo" label="From" unit="Hz" step={10} min={0} bind:value={d.srs.bands.f_lo_hz} errors={errs('srs.bands.f_lo_hz')} disabled={locked} testid="ms-band-lo" />
						<NumberField id="ms-bhi" label="To" unit="Hz" step={100} min={0} bind:value={d.srs.bands.f_hi_hz} errors={errs('srs.bands.f_hi_hz')} disabled={locked} testid="ms-band-hi" />
						<NumberField id="ms-bpo" label="Bands an octave" integer step={1} min={1} bind:value={d.srs.bands.bands_per_octave} errors={errs('srs.bands.bands_per_octave')} disabled={locked} testid="ms-bpo" />
						<NumberField id="ms-q" label="Quality factor" step={1} min={0} bind:value={d.srs.bands.q} errors={errs('srs.bands.q')} disabled={locked} testid="ms-q" help="Every SRS of the test is computed at it. Usual: 10." />
						<NumberField id="ms-tlo" label="Tolerance under" unit="dB" step={0.5} max={0} value={d.srs.tolerance_db[0]} onchange={(v) => (d.srs.tolerance_db = [v ?? NaN, d.srs.tolerance_db[1]])} errors={errs('srs.tolerance_db')} disabled={locked} testid="ms-tol-lo" />
						<NumberField id="ms-thi" label="Tolerance over" unit="dB" step={0.5} min={0} value={d.srs.tolerance_db[1]} onchange={(v) => (d.srs.tolerance_db = [d.srs.tolerance_db[0], v ?? NaN])} disabled={locked} testid="ms-tol-hi" />
					</div>
					<Findings items={[...errs('srs.bands'), ...errs('srs.controls')].map((message) => ({ level: 'error' as const, message }))} />
				</div>
			{/if}
		</section>

		{#if n}
			<section class="aq-panel" aria-labelledby="ms-tc-title" data-testid="ms-control-target">
				<div class="aq-head">
					<h2 id="ms-tc-title">Control {ch}</h2>
					<span class="aq-sub">Its own {d.kind === 'waveform' ? 'pulse' : 'SRS'} at full level.</span>
					<span class="aq-spacer"></span>
					{#if n > 1}<button class="btn sm" disabled={locked} onclick={() => sameTargetForAll(d)} data-testid="ms-same-target">Copy Control {d.controls[0]} to Every Control</button>{/if}
				</div>
				<div class="ctabs" role="tablist" aria-label="Controls">
					{#each d.controls as c, k (c)}
						{@const bad = controlErrors(k)}
						<button role="tab" aria-selected={k === i} class:on={k === i} onclick={() => (at = k)} data-testid="ms-target-tab-{c}">
							Control {c}
							{#if bad}<span class="bad" title="{bad} {bad === 1 ? 'error' : 'errors'}"><svg viewBox="0 0 14 14" width="10" height="10" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /></svg><span class="num">{bad}</span><span class="visually-hidden">{bad === 1 ? 'error' : 'errors'}</span></span>{/if}
						</button>
					{/each}
				</div>
				<Findings items={errs('pulses').map((message) => ({ level: 'error' as const, message }))} />

				{#if d.kind === 'waveform' && p}
					<div class="aq-fields">
						<div class="sel">
							<label for="ms-shape" class="lbl">Shape</label>
							<select id="ms-shape" class="field" bind:value={p.shape} disabled={locked} data-testid="ms-shape">
								{#each SHAPES as s (s.id)}<option value={s.id}>{s.label}</option>{/each}
							</select>
						</div>
						<NumberField id="ms-amp" label="Amplitude" unit="g" step={1} min={0} bind:value={p.amplitude_g} errors={errs(`pulses.${i}.amplitude_g`)} disabled={locked} testid="ms-amplitude" />
						<NumberField id="ms-dur" label="Duration" unit="ms" step={0.5} min={0} bind:value={p.duration_ms} errors={errs(`pulses.${i}.duration_ms`)} disabled={locked} testid="ms-duration" />
						<div class="sel">
							<span class="lbl" id="ms-pol-l">Polarity</span>
							<div class="aq-seg sm" role="radiogroup" aria-labelledby="ms-pol-l">
								<button type="button" role="radio" aria-checked={p.polarity === 'positive'} disabled={locked} onclick={() => (p.polarity = 'positive')}>Positive</button>
								<button type="button" role="radio" aria-checked={p.polarity === 'negative'} disabled={locked} onclick={() => (p.polarity = 'negative')} data-testid="ms-negative">Negative</button>
							</div>
						</div>
						{#if p.shape === 'trapezoid'}
							<NumberField id="ms-rise" label="Rise, share of the duration" step={0.05} min={0} max={1} bind:value={p.rise_frac} errors={errs(`pulses.${i}.rise_frac`)} disabled={locked} />
							<NumberField id="ms-fall" label="Fall, share of the duration" step={0.05} min={0} max={1} bind:value={p.fall_frac} errors={errs(`pulses.${i}.fall_frac`)} disabled={locked} />
						{/if}
						<div class="sel">
							<label for="ms-comp" class="lbl">Shoulders</label>
							<select id="ms-comp" class="field" bind:value={p.compensation.shape} disabled={locked}>
								{#each COMPENSATIONS as s (s.id)}<option value={s.id}>{s.label}</option>{/each}
							</select>
						</div>
						<NumberField id="ms-pre" label="Shoulder length, times the duration" step={0.5} min={0} bind:value={p.compensation.pre_frac} errors={errs(`pulses.${i}.compensation.pre_frac`)} disabled={locked} />
						<label class="aq-switch">
							<input type="checkbox" role="switch" aria-checked={p.compensation.symmetric} bind:checked={p.compensation.symmetric} disabled={locked} />
							<span class="track" aria-hidden="true"><span class="thumb"></span></span>
							<span class="word">{p.compensation.symmetric ? 'Same shoulder after' : 'Its own shoulder after'}</span>
						</label>
						{#if !p.compensation.symmetric}
							<NumberField id="ms-post" label="After, times the duration" step={0.5} min={0} bind:value={p.compensation.post_frac} errors={errs(`pulses.${i}.compensation.post_frac`)} disabled={locked} />
						{/if}
					</div>
				{:else if d.kind === 'srs' && sc}
					<p class="aq-lead aq-small">Points of the target, frequency rising, joined by straight lines on log-log axes, covering the band grid.</p>
					<PointsTable
						rows={sc.points}
						columns={[
							{ key: 'freq_hz', label: 'Frequency', unit: 'Hz', step: 10 },
							{ key: 'accel_g', label: 'SRS', unit: 'g', step: 10 }
						]}
						prefix="srs.controls.{i}.points"
						errors={E}
						{locked}
						min={2}
						make={(last) => ({ freq_hz: last ? Math.round(last.freq_hz * 2) : 100, accel_g: last?.accel_g ?? 10 })}
						testid="ms-srs-points"
						label="Control {ch} target breakpoints"
						file={{ client: ms.acq.tableFiles, tableKind: 'srs_target', name: `SRS Target, Control ${ch}`, column: 'accel_g' }}
					/>
					<div class="aq-group">
						<h3>Synthesis</h3>
						<p class="aq-lead aq-small">The control service builds a full-level waveform for this control's SRS, so a law that works on waveforms can run the test.</p>
						<div class="aq-fields">
							<div class="sel">
								<label for="ms-eng" class="lbl">Components</label>
								<select id="ms-eng" class="field" bind:value={sc.engine} disabled={locked} data-testid="ms-engine">
									<option value="wavsyn">Odd half-cycle wavelets</option>
									<option value="seds">Damped sines</option>
								</select>
							</div>
							<div class="sel">
								<span class="lbl" id="ms-spol-l">Polarity</span>
								<div class="aq-seg sm" role="radiogroup" aria-labelledby="ms-spol-l">
									<button type="button" role="radio" aria-checked={sc.polarity === 'positive'} disabled={locked} onclick={() => (sc.polarity = 'positive')}>Positive</button>
									<button type="button" role="radio" aria-checked={sc.polarity === 'negative'} disabled={locked} onclick={() => (sc.polarity = 'negative')}>Negative</button>
								</div>
							</div>
							<NumberField id="ms-iters" label="Iterations at most" integer step={5} min={1} bind:value={sc.max_iters} errors={errs(`srs.controls.${i}.max_iters`)} disabled={locked} />
							<NumberField id="ms-stol" label="Fit within" unit="dB" step={0.5} min={0} bind:value={sc.tol_db} errors={errs(`srs.controls.${i}.tol_db`)} disabled={locked} />
							<NumberField id="ms-sgain" label="Largest change an iteration" unit="dB" step={0.5} min={0} bind:value={sc.max_gain_db_per_iter} errors={errs(`srs.controls.${i}.max_gain_db_per_iter`)} disabled={locked} />
						</div>
					</div>
				{/if}
			</section>
		{:else}
			<p class="aq-notice warn aq-small" data-testid="ms-no-controls">Switch on control rows in the channel table (Station & Channels) to give each its target.</p>
		{/if}
	</div>

	<section class="aq-panel preview" aria-labelledby="ms-tp-title" data-testid="ms-target-preview" data-state={views.length ? 'ok' : 'none'}>
		<div class="aq-head"><h2 id="ms-tp-title">Control {ch ?? '-'} at Full Level</h2><span class="aq-sub">A computation by the control service, never a measurement.</span></div>
		{#if views.length}
			{#each views as v, k (k)}
				<div class="plot"><ViewRenderer view={v} minHeight={220} /></div>
			{/each}
		{:else if ms.checking}
			<p class="aq-muted aq-small">Checking the setup.</p>
		{:else}
			<p class="aq-muted aq-small">{ms.connected ? 'The target is drawn once the setup checks clean.' : 'The target is drawn by the control service once it is connected.'}</p>
		{/if}
	</section>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
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

	.sel.wide {
		grid-column: 1 / -1;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.ctabs {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin-bottom: var(--sp-3);
	}

	.ctabs button {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		padding: 4px 10px;
		font-size: var(--text-sm);
		color: var(--text-2);
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.ctabs button.on {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		border-color: var(--blue-2);
		box-shadow: inset 0 -2px 0 var(--blue-2);
	}

	.bad {
		display: inline-flex;
		align-items: center;
		gap: 2px;
		color: var(--amber-1);
		font-size: var(--text-2xs);
	}

	.plot + .plot {
		margin-top: var(--sp-2);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
