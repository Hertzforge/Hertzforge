<script lang="ts">
	/**
	 * The Recorder's rate and trigger, or its analysis: the rate asked of the station, what starts a
	 * recording (the run's first frame, Record, or a row crossing a level) with the level, slope and
	 * hysteresis, the pre-window, each recording's length, the count and the holdoff; and the live
	 * analysis grid with the FRF pair. What the service derived for this setup is in the summary
	 * beside every section, shown as it came and never computed here.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { AVERAGING_CHOICES, BLOCK_CHOICES, DECIMATION_CHOICES, MODE_CHOICES, recorderRows, WINDOW_CHOICES } from './recorderSetup';
	import type { RecorderStore } from './recorderStore.svelte';

	interface Props {
		recorder: RecorderStore;
		part: 'trigger' | 'analysis';
	}
	let { recorder, part }: Props = $props();

	const d = $derived(recorder.setup);
	const t = $derived(recorder.setup.trigger);
	const a = $derived(recorder.setup.analysis);
	const locked = $derived(recorder.locked);
	const E = $derived(recorder.fieldErrors);
	const W = $derived(recorder.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];
	const rows = $derived(recorderRows(recorder.rows).recorded);
	const trigRow = $derived(rows.find((r) => r.index === t.row) ?? null);
	const unit = $derived(trigRow?.units ?? '');
	const level = $derived(t.mode === 'level');
	const label = (r: { index: number; description: string; role: string }) => `Channel ${r.index}${r.description.trim() ? `: ${r.description.trim()}` : ` (${r.role})`}`;

	function setPair(on: boolean) {
		if (!on) a.frf = null;
		else a.frf = { reference: rows[0]?.index ?? NaN, response: rows[1]?.index ?? rows[0]?.index ?? NaN };
	}
</script>

<div class="stack narrow">
	{#if part === 'trigger'}
		<section class="aq-panel" aria-labelledby="rs-rate" data-testid="recorder-rate">
			<div class="aq-head"><h2 id="rs-rate">Rate</h2><span class="aq-sub">Every recorded row, at the rate the station runs at.</span></div>
			<div class="aq-fields">
				<NumberField id="rs-fs" label="Rate asked of the station" unit="Hz" step={100} min={1000} max={204800} bind:value={d.acquisition.fs_hz} errors={errs('acquisition.fs_hz')} disabled={locked} testid="recorder-fs" help="A station may program a nearby rate; the run adopts it, records at it and shows both." />
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="rs-trig" data-testid="recorder-trigger">
			<div class="aq-head"><h2 id="rs-trig">Trigger</h2><span class="aq-sub">What starts a recording. Monitoring saves nothing.</span></div>
			<div class="sel">
				<span class="lbl" id="rs-mode-l">Starts a recording</span>
				<div class="aq-seg" role="radiogroup" aria-labelledby="rs-mode-l">
					{#each MODE_CHOICES as m (m.id)}
						<button type="button" role="radio" aria-checked={t.mode === m.id} disabled={locked} onclick={() => (t.mode = m.id)} data-testid="recorder-mode-{m.id}">{m.label}</button>
					{/each}
				</div>
				<p class="help" data-testid="recorder-mode-help">{MODE_CHOICES.find((m) => m.id === t.mode)?.help}</p>
			</div>

			{#if level}
				<div class="aq-fields spaced">
					<div class="sel">
						<label for="rs-row" class="lbl">Row the level is read on</label>
						<select id="rs-row" class="field" value={t.row ?? ''} disabled={locked} onchange={(e) => (t.row = e.currentTarget.value === '' ? null : Number(e.currentTarget.value))} data-testid="recorder-row">
							<option value="" disabled>Choose a recorded row</option>
							{#each rows as r (r.index)}<option value={r.index}>{label(r)}</option>{/each}
						</select>
						<Findings items={errs('trigger.row').map((message) => ({ level: 'error' as const, message }))} />
					</div>
					<NumberField id="rs-level" label="Level" unit={unit} step={0.05} bind:value={t.level} errors={errs('trigger.level')} disabled={locked} testid="recorder-level" help="In the row's unit, within its range either side of zero." />
					<div class="sel">
						<span class="lbl" id="rs-slope-l">Slope</span>
						<div class="aq-seg" role="radiogroup" aria-labelledby="rs-slope-l">
							<button type="button" role="radio" aria-checked={t.slope === 'rising'} disabled={locked} onclick={() => (t.slope = 'rising')} data-testid="recorder-slope-rising">Rising</button>
							<button type="button" role="radio" aria-checked={t.slope === 'falling'} disabled={locked} onclick={() => (t.slope = 'falling')} data-testid="recorder-slope-falling">Falling</button>
						</div>
					</div>
					<NumberField id="rs-hyst" label="Hysteresis" unit={unit} nullable="1% of the range" fallback={trigRow ? trigRow.range_max * 0.01 : 0.05} step={0.01} min={0} bind:value={t.hysteresis} errors={errs('trigger.hysteresis')} disabled={locked} testid="recorder-hysteresis" help="Before the detector arms, the signal has been this far the other side of the level, so a signal hovering at the level fires at most once." />
				</div>
			{/if}

			<div class="aq-fields spaced">
				{#if t.mode !== 'start'}
					<NumberField id="rs-pre" label="Pre-window" unit="s" step={0.1} min={0} max={10} bind:value={t.pre_s} errors={errs('trigger.pre_s')} disabled={locked} testid="recorder-pre" help="The frames before the trigger a recording begins with, up to 10 s. They fill once the run is armed." />
				{/if}
				{#if level}
					<NumberField id="rs-len" label="Length after the trigger" unit="s" step={0.5} min={0} bind:value={t.record_s} errors={errs('trigger.record_s')} disabled={locked} testid="recorder-length" help="Each recording holds exactly this after its trigger, plus its pre-window." />
				{:else}
					<NumberField id="rs-len" label="Length after the trigger" unit="s" nullable="Until cut" fallback={10} step={0.5} min={0} bind:value={t.record_s} errors={errs('trigger.record_s')} disabled={locked} testid="recorder-length" help={t.mode === 'start' ? 'From the first frame; until cut, it runs until Cut or the run ends.' : 'Until cut, it runs until Cut or the run ends.'} />
				{/if}
				{#if level}
					<NumberField id="rs-count" label="Recordings an arm saves" nullable="Until the run ends" fallback={1} integer step={1} min={1} bind:value={t.count} errors={errs('trigger.count')} disabled={locked} testid="recorder-count" help="The trigger re-arms by itself until it has saved this many, then the run goes back to monitoring." />
					<NumberField id="rs-hold" label="Holdoff" unit="s" step={0.5} min={0} bind:value={t.holdoff_s} errors={errs('trigger.holdoff_s')} disabled={locked} testid="recorder-holdoff" help="The least station time from one trigger to the next. A re-armed trigger also waits for a full fresh pre-window." />
				{/if}
			</div>
			{#if t.mode === 'start'}<p class="aq-small aq-muted note">From the start has no pre-window: the recording's first frame is the run's first frame.</p>{/if}
		</section>
	{:else}
		<section class="aq-panel" aria-labelledby="rs-an" data-testid="recorder-analysis">
			<div class="aq-head"><h2 id="rs-an">Analysis</h2><span class="aq-sub">The live spectra and FRF. Recordings are always at the station's rate.</span></div>
			<div class="sel">
				<span class="lbl" id="rs-block-l">Block</span>
				<div class="aq-seg sm" role="radiogroup" aria-labelledby="rs-block-l">
					{#each BLOCK_CHOICES as v (v)}
						<button type="button" role="radio" aria-checked={a.block === v} disabled={locked} onclick={() => (a.block = v)} data-testid="recorder-block-{v}">{v}</button>
					{/each}
				</div>
				<p class="help">Frames at the analysis rate. The line spacing is the analysis rate over the block.</p>
				<Findings items={errs('analysis.block').map((message) => ({ level: 'error' as const, message }))} />
			</div>
			<div class="sel spaced">
				<span class="lbl" id="rs-win-l">Window</span>
				<div class="aq-seg" role="radiogroup" aria-labelledby="rs-win-l">
					{#each WINDOW_CHOICES as w (w.id)}
						<button type="button" role="radio" aria-checked={a.window === w.id} disabled={locked} onclick={() => (a.window = w.id)} data-testid="recorder-window-{w.id}">{w.label}</button>
					{/each}
				</div>
			</div>
			<div class="sel spaced">
				<span class="lbl" id="rs-avg-l">Averaging</span>
				<div class="aq-seg" role="radiogroup" aria-labelledby="rs-avg-l">
					{#each AVERAGING_CHOICES as v (v.id)}
						<button type="button" role="radio" aria-checked={a.averaging === v.id} disabled={locked} onclick={() => (a.averaging = v.id)} data-testid="recorder-averaging-{v.id}">{v.label}</button>
					{/each}
				</div>
			</div>
			<div class="aq-fields spaced">
				<NumberField id="rs-overlap" label="Overlap" unit="%" nullable="The window's own" fallback={50} step={5} min={0} max={95} bind:value={a.overlap_pct} errors={errs('analysis.overlap_pct')} disabled={locked} testid="recorder-overlap" help="Hann 50, flat top 75, rectangular 0, Blackman-Harris 66.7." />
				<NumberField id="rs-avgs" label="Averages" integer step={1} min={1} bind:value={a.averages} errors={errs('analysis.averages')} disabled={locked} testid="recorder-averages" help="Blocks in a linear average; the time constant in blocks of an exponential one." />
			</div>
			<div class="sel spaced">
				<span class="lbl" id="rs-dec-l">Decimation</span>
				<div class="aq-seg sm" role="radiogroup" aria-labelledby="rs-dec-l">
					{#each DECIMATION_CHOICES as v (v)}
						<button type="button" role="radio" aria-checked={a.decimation === v} disabled={locked} onclick={() => (a.decimation = v)} data-testid="recorder-decimation-{v}">{v}</button>
					{/each}
				</div>
				<p class="help">The analysis rate is the station's rate over it. It changes only the live analysis, never a recording.</p>
				<Findings items={errs('analysis.decimation').map((message) => ({ level: 'error' as const, message }))} />
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="rs-frf" data-testid="recorder-frf">
			<div class="aq-head"><h2 id="rs-frf">FRF</h2><span class="aq-sub">One pair of recorded rows: H1, its phase and the coherence.</span></div>
			<label class="check">
				<input type="checkbox" checked={a.frf !== null} disabled={locked || rows.length < 2} onchange={(e) => setPair(e.currentTarget.checked)} data-testid="recorder-frf-on" />
				Show a live FRF
			</label>
			{#if rows.length < 2}<p class="help">An FRF needs two recorded rows.</p>{/if}
			{#if a.frf}
				<div class="aq-fields spaced">
					<div class="sel">
						<label for="rs-ref" class="lbl">Reference</label>
						<select id="rs-ref" class="field" value={a.frf.reference} disabled={locked} onchange={(e) => a.frf && (a.frf.reference = Number(e.currentTarget.value))} data-testid="recorder-frf-reference">
							{#each rows as r (r.index)}<option value={r.index}>{label(r)}</option>{/each}
						</select>
					</div>
					<div class="sel">
						<label for="rs-resp" class="lbl">Response</label>
						<select id="rs-resp" class="field" value={a.frf.response} disabled={locked} onchange={(e) => a.frf && (a.frf.response = Number(e.currentTarget.value))} data-testid="recorder-frf-response">
							{#each rows as r (r.index)}<option value={r.index}>{label(r)}</option>{/each}
						</select>
					</div>
				</div>
			{/if}
			<Findings items={[...errs('analysis.frf').map((message) => ({ level: 'error' as const, message })), ...warns('analysis.frf').map((message) => ({ level: 'warning' as const, message }))]} />
		</section>
	{/if}
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

	.note {
		margin: var(--sp-2) 0 0;
	}

	.spaced {
		margin-top: var(--sp-3);
	}

	.check {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
		color: var(--text-1);
	}
</style>
