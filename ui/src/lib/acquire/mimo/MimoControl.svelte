<script lang="ts">
	/**
	 * How the test is controlled: the control law and its parameters (drawn from the law's own
	 * manifest, so nothing here is specific to a law), the FRF during the run, how each loop
	 * estimates the spectra, how identification runs, the gates it must pass, and the caps and
	 * budgets the core holds the law to. Every field starts at the service's default with its unit
	 * and says in a line what it does. The law is fixed once the run starts.
	 */
	import LawPanel from '../law/LawPanel.svelte';
	import NumberField from '../NumberField.svelte';
	import { N_PTS_OPTIONS } from '../setup';
	import { defaultMimoSetup } from './mimoSetup';
	import type { MimoState } from './mimoStore.svelte';

	interface Props {
		mimo: MimoState;
	}
	let { mimo }: Props = $props();

	const d = $derived(mimo.setup);
	const locked = $derived(mimo.locked);
	const E = $derived(mimo.fieldErrors);
	const W = $derived(mimo.fieldWarnings);
	const law = $derived(mimo.law);
	const laws = $derived(mimo.mimoLaws);
	const admitted = $derived(mimo.check && mimo.checkedKey === mimo.key ? mimo.check.law : null);
	const shape = $derived(mimo.check && mimo.checkedKey === mimo.key ? mimo.check.shape.shape : null);
	const DEF = defaultMimoSetup();

	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];

	const SHAPE_WORDS: Record<string, string> = {
		square: 'as many controls as drives',
		more_controls_than_drives: 'more controls than drives',
		fewer_controls_than_drives: 'fewer controls than drives'
	};

	type Section = 'estimation' | 'identification' | 'control';
	interface Spec {
		section: Section;
		key: string;
		label: string;
		unit?: string;
		integer?: boolean;
		step?: number;
		min?: number;
		max?: number;
		help: string;
	}

	const ESTIMATION: Spec[] = [
		{ section: 'estimation', key: 'frames_per_loop', label: 'Frames per loop', integer: true, step: 1, min: 1, max: 1024, help: 'Welch frames averaged in one control loop. More is steadier and makes each loop longer.' },
		{ section: 'estimation', key: 'overlap', label: 'Overlap', unit: 'of a frame', step: 0.25, min: 0, max: 0.75, help: 'From 0 to 0.75. Half a frame is usual with the Hann window.' },
		{ section: 'estimation', key: 'memory_loops', label: 'Averaging memory', unit: 'loops', integer: true, step: 1, min: 1, max: 64, help: 'How many loops the estimate remembers. 1 uses only the latest loop.' }
	];
	const IDENT: Spec[] = [
		{ section: 'identification', key: 'level_db', label: 'Identification level', unit: 'dB', step: 1, min: -60, max: 0, help: 'Of the control response, relative to the reference. Uncorrelated random on every drive steps up until the response is there.' },
		{ section: 'identification', key: 'frames', label: 'Identification frames', integer: true, step: 8, min: 1, max: 4096, help: 'At least twice the number of drives; ten times is better.' },
		{ section: 'identification', key: 'noise_frames', label: 'Noise-floor frames', integer: true, step: 8, min: 0, max: 4096, help: 'Measured with the drives off, before identification.' }
	];
	const GATES: Spec[] = [
		{ section: 'identification', key: 'coherence_floor', label: 'Coherence floor', step: 0.05, min: 0, max: 1, help: "Each control's multiple coherence over the band must reach this, or the run holds at the gate." },
		{ section: 'identification', key: 'condition_limit', label: 'Condition limit', step: 100, min: 1, help: "The FRF's condition number on any line must stay under this, or the run holds at the gate." }
	];
	const CAPS: Spec[] = [
		{ section: 'control', key: 'cap_up_db', label: 'Rise cap', unit: 'dB per loop', step: 0.5, min: 0, max: 6, help: "The most each drive's PSD may rise in one loop, whatever the law asks." },
		{ section: 'control', key: 'cap_down_db', label: 'Fall cap', unit: 'dB per loop', step: 0.5, min: 0, help: "The most each drive's PSD may fall in one loop." },
		{ section: 'control', key: 'law_budget', label: 'Law time budget', unit: 'of a loop', step: 0.05, min: 0, max: 1, help: 'A law call that takes longer is a miss; the previous drive is kept.' },
		{ section: 'control', key: 'law_misses', label: 'Misses before a stop', integer: true, step: 1, min: 1, max: 100, help: 'Unusable law calls in a row (late, failed or refused by the output check) that stop the run with its ramp.' },
		{ section: 'control', key: 'max_loops_out', label: 'Diverging loops to abort', integer: true, step: 1, min: 1, help: 'Loops in a row with a growing error on one control that trip the divergence abort.' },
		{ section: 'control', key: 'level_ramp_s', label: 'Level ramp', unit: 's', step: 0.5, min: 0, max: 60, help: 'Ramp time of every level change.' },
		{ section: 'control', key: 'shutdown_ramp_s', label: 'Shutdown ramp', unit: 's', step: 0.5, min: 0, max: 60, help: 'Ramp time of a controlled stop. An abort does not ramp.' }
	];

	function bag(s: Section): Record<string, number> {
		return d[s] as unknown as Record<string, number>;
	}

	function usual() {
		if (locked) return;
		const base = defaultMimoSetup();
		d.estimation = { ...base.estimation, fs_hz: d.estimation.fs_hz, n_pts: d.estimation.n_pts };
		d.identification = base.identification;
		d.control = base.control;
		d.frf = base.frf;
		d.frf_blend = base.frf_blend;
	}

	const defOf = (s: Section, key: string) => (DEF[s] as unknown as Record<string, number>)[key];
</script>

{#snippet fields(specs: Spec[])}
	<div class="aq-fields">
		{#each specs as s (s.key)}
			<NumberField id="mc-{s.key}" label={s.label} unit={s.unit} integer={s.integer} step={s.step} min={s.min} max={s.max} value={bag(s.section)[s.key]} onchange={(v) => (bag(s.section)[s.key] = v ?? NaN)} errors={errs(`${s.section}.${s.key}`)} warnings={warns(`${s.section}.${s.key}`)} help="{s.help} Usual: {defOf(s.section, s.key)}{s.unit && !s.unit.startsWith('of') ? ` ${s.unit}` : ''}." disabled={locked} testid="mimo-{s.key}" />
		{/each}
	</div>
{/snippet}

<div class="cols">
	<LawPanel
		{laws}
		{law}
		chosenId={d.law.id}
		values={d.law.params}
		subtitle="Fixed once the run starts; it can be changed at the prediction, before engaging."
		{locked}
		connected={mimo.connected}
		busy={mimo.busy}
		problems={mimo.lawProblems}
		errors={E}
		channelLabels={mimo.labels}
		onchoose={(id) => mimo.chooseLaw(id)}
		onimport={(path) => mimo.importLaw(path)}
	>
		{#snippet facts(l)}
			<div class="wide"><dt>Supports</dt><dd>{(l.shapes ?? []).map((s) => SHAPE_WORDS[s] ?? s).join('; ') || '-'}</dd></div>
		{/snippet}
		{#snippet notice()}
			{#if admitted && !admitted.admitted}
				<p class="aq-notice warn aq-small" role="alert">This law does not admit this test{shape ? ` (${SHAPE_WORDS[shape]})` : ''}; the check above says why.</p>
			{/if}
		{/snippet}
	</LawPanel>

	<section class="aq-panel" aria-labelledby="loop-title" data-testid="loop-panel">
		<div class="aq-head">
			<h2 id="loop-title">The Loop</h2>
			<span class="aq-sub">Every field starts at the service's usual value.</span>
			<span class="aq-spacer"></span>
			<button class="btn ghost sm" onclick={usual} disabled={locked}>Usual Values</button>
		</div>

		<div class="aq-group">
			<h3>FRF During the Run</h3>
			<div class="frf">
				<div class="aq-seg" role="radiogroup" aria-label="FRF during the run">
					<button type="button" role="radio" aria-checked={d.frf === 'hold'} disabled={locked} onclick={() => (d.frf = 'hold')} data-testid="frf-hold">Hold</button>
					<button type="button" role="radio" aria-checked={d.frf === 'update'} disabled={locked} onclick={() => (d.frf = 'update')} data-testid="frf-update">Update</button>
				</div>
				<p class="aq-muted aq-small">
					{d.frf === 'hold' ? 'The law uses the FRF from identification for the whole run.' : "The FRF is estimated again every loop from the drive and the response, and blended with identification's. It is sound only where the drive excites the plant."}
				</p>
			</div>
			{#if d.frf === 'update'}
				<div class="aq-fields blend">
					<NumberField id="mc-blend" label="Blend weight" step={0.1} min={0} max={1} bind:value={d.frf_blend} errors={errs('control.frf_blend')} help="Weight of each loop's estimate against identification's, above 0 and at most 1. Usual: 0.5." disabled={locked} testid="mimo-frf-blend" />
				</div>
			{/if}
		</div>

		<div class="aq-group">
			<h3>Estimation</h3>
			<div class="aq-fields">
				<NumberField id="mc-fs" label="Sample rate" unit="Hz" step={1024} min={256} max={204800} bind:value={d.estimation.fs_hz} errors={errs('estimation.fs_hz')} help="The station may run at a nearby rate; the run reports the one it uses." disabled={locked} testid="mimo-fs" />
				<div class="sel">
					<label for="mc-npts" class="lbl">Frame length</label>
					<select id="mc-npts" class="field num" bind:value={d.estimation.n_pts} disabled={locked}>
						{#each N_PTS_OPTIONS as n (n)}<option value={n}>{n} points</option>{/each}
					</select>
					<p class="help">Sets the line spacing: the sample rate over the frame length.</p>
					{#each errs('estimation.n_pts') as m (m)}<p class="err">{m}</p>{/each}
				</div>
				<div class="sel">
					<label for="mc-win" class="lbl">Window</label>
					<select id="mc-win" class="field" bind:value={d.estimation.window} disabled={locked}>
						<option value="hann">Hann</option>
						<option value="rectangular">Rectangular</option>
					</select>
				</div>
			</div>
			<div class="spaced">{@render fields(ESTIMATION)}</div>
		</div>

		<div class="aq-group">
			<h3>Identification</h3>
			{@render fields(IDENT)}
			<div class="tog">
				<label class="aq-switch" for="mc-review">
					<input id="mc-review" type="checkbox" role="switch" checked={d.identification.review} disabled={locked} onchange={(e) => (d.identification.review = e.currentTarget.checked)} data-testid="mimo-review" />
					<span class="track" aria-hidden="true"><span class="thumb"></span></span>
					<span>Hold for the prediction after identification</span>
					<span class="word">{d.identification.review ? 'On' : 'Off'}</span>
				</label>
				<p class="help">Nothing drives at test level until you have read the prediction and engaged. Off: the run engages by itself when the gates pass.</p>
			</div>
		</div>

		<div class="aq-group">
			<h3>Gates</h3>
			<p class="aq-lead">Checked once identification is done. A failed gate holds the run until you override it in your own words, or change the setup.</p>
			{@render fields(GATES)}
		</div>

		<div class="aq-group">
			<h3>Caps and Budgets</h3>
			<p class="aq-lead">The core holds every law to these, whichever law it is.</p>
			{@render fields(CAPS)}
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

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.frf {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
	}

	.frf p {
		flex: 1;
		min-width: 240px;
		margin: 0;
	}

	.blend {
		margin-top: var(--sp-3);
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.help {
		margin: 2px 0 0;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.err {
		font-size: var(--text-xs);
		color: var(--amber-1);
	}

	.spaced {
		margin-top: var(--sp-3);
	}

	.tog {
		margin-top: var(--sp-3);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
