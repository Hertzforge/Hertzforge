<script lang="ts">
	/**
	 * The SRS shock setup in a few lines, beside every section: the target and its grid, the
	 * synthesis, the envelope, the schedule and the law; the service's verdict on the setup (the
	 * grid, the time between pulses, the synthesized pulse's kinematics, feasibility); and what is
	 * left to fix, each a click from its section.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { ENGINES, runChannels, SRS_SECTIONS, type SrsSection } from './srsSetup';
	import type { SrsStore } from './srsStore.svelte';

	interface Props {
		srs: SrsStore;
		onopen: (s: SrsSection) => void;
	}
	let { srs, onopen }: Props = $props();

	const d = $derived(srs.setup);
	const dry = $derived(srs.check && srs.checkedKey === srs.key ? srs.check.derived : null);
	const todo = $derived(SRS_SECTIONS.filter((s) => srs.counts[s.id].errors > 0));
	const ch = $derived(runChannels(srs.rows));
	const engine = $derived(ENGINES.find((e) => e.id === d.synthesis.engine)?.label ?? d.synthesis.engine);
	const total = $derived(d.schedule.levels.reduce((n, l) => n + (Number.isFinite(l.pulses) ? l.pulses : 0), 0));
	const num = (v: number, unit: string) => (Number.isFinite(v) ? `${formatNumber(v, 4)} ${unit}` : '-');
	const target = $derived(d.target.kind === 'flat' ? `${num(d.target.accel_g, 'g')} in every band` : `${d.target.points.length} breakpoints`);
	const grid = $derived(`${num(d.bands.f_lo_hz, 'Hz')} to ${num(d.bands.f_hi_hz, 'Hz')}, ${d.bands.bands_per_octave === 1 ? 'whole octaves' : `1/${d.bands.bands_per_octave} octave`}, Q ${formatNumber(d.bands.q, 3)}`);
	const env = $derived(d.envelope.per_band ? 'A pair for each band' : `Alarm ${formatNumber(d.envelope.alarm_db[0], 3)} under, ${formatNumber(d.envelope.alarm_db[1], 3)} over; abort ${formatNumber(d.envelope.abort_db[0], 3)} under, ${formatNumber(d.envelope.abort_db[1], 3)} over (dB)`);
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="srs-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Target</dt><dd>{target}</dd></div>
		<div><dt>Band grid</dt><dd>{grid}</dd></div>
		<div><dt>Synthesis</dt><dd>{engine}, {d.synthesis.table ? `a table of ${(d.synthesis.engine === 'wavsyn' ? d.synthesis.wavelets : d.synthesis.sines).length}` : 'one per band'}</dd></div>
		<div><dt>Envelope</dt><dd>{env}</dd></div>
		<div><dt>Schedule</dt><dd>{d.schedule.mode === 'auto' ? 'Automatic' : 'Manual'}, {d.schedule.levels.length} {d.schedule.levels.length === 1 ? 'level' : 'levels'} from {db(d.schedule.levels[0]?.db)}, {total} pulses planned{d.schedule.advance === 'on_pass' ? ', advancing on passing pulses' : ''}</dd></div>
		<div><dt>Control law</dt><dd>{srs.law ? `${srs.law.name} ${srs.law.version}` : '-'}</dd></div>
		<div><dt>Drive</dt><dd>Ceiling {num(d.limits.drive_vpk_ceiling, 'V pk')}, abort {num(d.limits.drive_vpk_abort, 'V pk')}</dd></div>
		<div><dt>Limit channels</dt><dd>{d.limit_channels.enabled && d.limit_channels.channels.length ? d.limit_channels.channels.map((c) => c.channel).join(', ') : 'None'}</dd></div>
		<div><dt>Channels</dt><dd>Control {ch.control ? ch.control.index : 'none'}, drive {ch.drive ? ch.drive.index : 'none'}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<dl data-testid="srs-summary-derived">
			<div><dt>Grid</dt><dd class="num">{formatNumber(dry.grid.fs_hz, 6)} Hz, {dry.grid.record_len} samples a pulse{dry.grid.actual ? '' : ', until the station arms'}</dd></div>
			<div><dt>Bands</dt><dd class="num">{dry.bands_hz.length}</dd></div>
			<div><dt>Offline fit</dt><dd class="num">{dry.synthesis.converged ? 'Within tolerance' : 'Not within tolerance'}, {dry.synthesis.components} components</dd></div>
			<div><dt>Time between pulses</dt><dd class="num">{formatNumber(dry.spacing_s, 3)} s</dd></div>
			<div><dt>Peak at full level</dt><dd class="num">{formatNumber(dry.reference.peak_g, 4)} g</dd></div>
			<div><dt>Displacement</dt><dd class="num">{formatNumber(dry.reference.displacement_pk_pk_in, 4)} in pk-pk</dd></div>
			<div><dt>Feasibility</dt><dd>{dry.feasibility.message}</dd></div>
		</dl>
	{:else if srs.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	<div class="ready" data-testid="srs-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>To Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)}>{s.label}: {srs.counts[s.id].errors} {srs.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
				{/each}
			</ul>
		{:else if dry}
			<p class="aq-small ok">
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
				Nothing missing.
			</p>
		{/if}
	</div>
</aside>

<style>
	.summary {
		position: sticky;
		top: 0;
	}

	h2 {
		margin-bottom: var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	h3 {
		margin: var(--sp-3) 0 var(--sp-2);
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--text-2);
	}

	dl {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin: 0;
	}

	dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.ok {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		margin: var(--sp-3) 0 0;
		color: var(--blue-1);
	}
</style>
