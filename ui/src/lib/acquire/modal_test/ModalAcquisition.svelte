<script lang="ts">
	/**
	 * The mode (hammer impact, or open-loop shakers), and the acquisition: the sample rate, the block,
	 * the band, the FRF estimator and the averages at each point. The frequency resolution, the
	 * lines in the band and the estimator the service uses with several references are the service's,
	 * shown from its check.
	 */
	import NumberField from '../NumberField.svelte';
	import Findings from '../Findings.svelte';
	import { MODAL_ESTIMATORS } from '$lib/protocol/modal_test';
	import { MODAL_BLOCKS } from './modalTestSetup';
	import type { ModalTestStore } from './modalTestStore.svelte';
	import { num } from './modalTestWords';

	interface Props {
		modal: ModalTestStore;
	}
	let { modal }: Props = $props();

	const d = $derived(modal.setup);
	const locked = $derived(modal.locked);
	const E = $derived(modal.fieldErrors);
	const W = $derived(modal.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];
	const c = $derived(modal.checkOf);
	const MODES = [
		{ id: 'impact', label: 'Impact', help: 'A hammer with a force gauge is the reference; each hit is checked and counted.' },
		{ id: 'shaker', label: 'Shaker', help: 'One or more shakers driven open loop; each frame is averaged.' }
	] as const;
	const ESTIMATOR_HELP: Record<string, string> = {
		H1: 'H1: least squares against noise on the responses.',
		H2: 'H2: against noise on the force.',
		Hv: 'Hv: against noise on both.'
	};
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="mt-mode" data-testid="mt-mode">
		<div class="aq-head"><h2 id="mt-mode">Mode</h2></div>
		<div class="aq-seg" role="radiogroup" aria-label="Mode">
			{#each MODES as m (m.id)}
				<button type="button" role="radio" aria-checked={d.mode === m.id} disabled={locked} onclick={() => modal.chooseMode(m.id)} data-testid="mt-mode-{m.id}">{m.label}</button>
			{/each}
		</div>
		<p class="help">{MODES.find((m) => m.id === d.mode)?.help}</p>
		{#if modal.rovingNote}<p class="mode-note" role="status" data-testid="mt-roving-cleared">{modal.rovingNote}</p>{/if}
	</section>

	<section class="aq-panel" aria-labelledby="mt-acq" data-testid="mt-acquisition">
		<div class="aq-head"><h2 id="mt-acq">Acquisition</h2></div>
		<div class="aq-fields">
			<NumberField id="mt-fs" label="Sample rate" unit="Hz" step={256} min={256} max={204800} bind:value={d.fs_hz} errors={errs('fs_hz')} warnings={warns('fs_hz')} disabled={locked} testid="mt-fs" help="256 Hz to 204 800 Hz; the station programs the nearest it can, and the check says which." />
			<div class="sel">
				<label for="mt-block" class="lbl">Block</label>
				<select id="mt-block" class="field" bind:value={d.block} disabled={locked} data-testid="mt-block">
					{#each MODAL_BLOCKS as b (b)}<option value={b}>{b} samples</option>{/each}
				</select>
				<Findings items={errs('block').map((message) => ({ level: 'error' as const, message }))} />
			</div>
			<NumberField id="mt-lo" label="Band from" unit="Hz" nullable="Default" value={Number.isFinite(d.band.lo_hz) ? d.band.lo_hz : null} onchange={(v) => (d.band.lo_hz = v ?? NaN)} errors={errs('band.lo_hz')} disabled={locked} testid="mt-band-lo" help="Empty: from 0 Hz." />
			<NumberField id="mt-hi" label="Band to" unit="Hz" nullable="Default" value={Number.isFinite(d.band.hi_hz) ? d.band.hi_hz : null} onchange={(v) => (d.band.hi_hz = v ?? NaN)} errors={errs('band.hi_hz')} disabled={locked} testid="mt-band-hi" help="Empty: the sample rate over 2.56." />
			<div class="sel">
				<label for="mt-est" class="lbl">Estimator</label>
				<select id="mt-est" class="field" bind:value={d.estimator} disabled={locked} data-testid="mt-estimator">
					{#each MODAL_ESTIMATORS as e (e)}<option value={e}>{e}</option>{/each}
				</select>
				<p class="help">{ESTIMATOR_HELP[d.estimator]} With several references the service uses the matrix H1.</p>
			</div>
			<NumberField id="mt-avg" label="Averages" integer step={1} min={1} max={10000} bind:value={d.averages} errors={errs('averages')} warnings={warns('averages')} disabled={locked} testid="mt-averages" help={d.mode === 'impact' ? 'Accepted hits at each point.' : 'Frames at each point.'} />
			<NumberField id="mt-decay" label="Warn when a response has not died away by the end of the frame" unit="%" step={0.5} min={0.01} max={100} nullable="Default" value={Number.isFinite(d.decay_warn_pct) ? d.decay_warn_pct : null} onchange={(v) => (d.decay_warn_pct = v ?? NaN)} errors={errs('decay_warn_pct')} warnings={warns('decay_warn_pct')} disabled={locked} testid="mt-decay-warn" help="The level at the frame's end, percent of the response's loudest. Empty: the control service's 2%. A warning only; the hit or frame is kept." />
		</div>
		{#if c}
			<p class="aq-small derived" data-testid="mt-derived">From the control service: {num(c.fs_hz, 'Hz')}, a {num(c.frame_s, 's')} block, {num(c.df_hz, 'Hz')} between lines, {c.band.lines} lines from {num(c.band.lo_hz, 'Hz')} to {num(c.band.hi_hz, 'Hz')}, estimator {c.estimator}, {c.averages} {c.averages === 1 ? 'average' : 'averages'}.</p>
		{/if}
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		max-width: 860px;
	}

	.help {
		font-size: var(--text-xs);
		color: var(--text-3);
		margin: var(--sp-1) 0 0;
	}

	.derived {
		margin: var(--sp-2) 0 0;
	}

	.mode-note {
		font-size: var(--text-sm);
		color: var(--amber-2);
		background: var(--amber-weak);
		border-left: 2px solid var(--amber-2);
		padding: var(--sp-1) var(--sp-2);
		margin: var(--sp-2) 0 0;
	}
</style>
