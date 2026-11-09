<script lang="ts">
	/**
	 * The Sine setup in a few lines, beside every section: the profile, the test, the schedule, the
	 * law and the drive; the service's verdict on the setup (the rate and the tick, the test time,
	 * the identification's length, the stop ramp, the tracking check, feasibility); and what is left
	 * to fix, each a click from its section.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { runChannels, SINE_SECTIONS, type SineSection } from './sineSetup';
	import type { SineStore } from './sineStore.svelte';
	import { SINE_TEST_WORDS } from './sineWords';

	interface Props {
		sine: SineStore;
		onopen: (s: SineSection) => void;
	}
	let { sine, onopen }: Props = $props();

	const d = $derived(sine.setup);
	const dry = $derived(sine.check && sine.checkedKey === sine.key ? sine.check.derived : null);
	const todo = $derived(SINE_SECTIONS.filter((s) => sine.counts[s.id].errors > 0));
	const ch = $derived(runChannels(sine.rows));
	const num = (v: number, unit: string, sig = 4) => (Number.isFinite(v) ? `${formatNumber(v, sig)} ${unit}` : '-');
	const test = $derived.by(() => {
		const t = d.test;
		if (t.kind === 'dwell') return `Dwell at ${num(t.dwell.freq_hz, 'Hz')} for ${t.dwell.by === 'seconds' ? num(t.dwell.seconds, 's') : `${num(t.dwell.cycles, 'cycles')}`}`;
		const band = `${num(t.band_hz[0], 'Hz')} to ${num(t.band_hz[1], 'Hz')}`;
		if (t.kind === 'step') return `Step, ${band}, ${formatNumber(t.step.steps_per_octave, 3)} steps an octave`;
		if (t.kind === 'rstd') return SINE_TEST_WORDS.rstd;
		const s = t.sweep;
		const rate = s.law === 'log' ? num(s.oct_per_min, 'oct/min', 3) : num(s.hz_per_s, 'Hz/s', 3);
		const end = s.end_by === 'sweeps' ? `${s.sweeps} sweeps` : s.end_by === 'duration' ? num(s.duration_s, 's') : `${s.sweeps} sweeps or ${num(s.duration_s, 's')}`;
		return `Sweep, ${band} at ${rate}, ${end}`;
	});
	const rows = $derived(d.profile.rows.length);
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="sine-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Profile</dt><dd>{d.profile.label.trim() || 'Unnamed'}, {rows} {rows === 1 ? 'row' : 'rows'}</dd></div>
		<div><dt>Test</dt><dd>{test}</dd></div>
		<div><dt>Schedule</dt><dd>{d.schedule.mode === 'auto' ? 'Automatic' : 'Manual'}, {d.schedule.levels.length} {d.schedule.levels.length === 1 ? 'level' : 'levels'} from {db(d.schedule.levels[0])}</dd></div>
		<div><dt>Identification</dt><dd>At {db(d.identification.level_db)}{d.identification.review ? ', then a review hold' : ''}</dd></div>
		<div><dt>Control law</dt><dd>{sine.law ? `${sine.law.name} ${sine.law.version}` : '-'}</dd></div>
		<div><dt>Drive</dt><dd>Ceiling {num(d.limits.drive_vpk_ceiling, 'V pk')}, abort {num(d.limits.drive_vpk_abort, 'V pk')}</dd></div>
		<div><dt>Limit channels</dt><dd>{d.limits.channels.length ? d.limits.channels.map((c) => c.channel).join(', ') : 'None'}</dd></div>
		<div><dt>Channels</dt><dd>Control {ch.control ? ch.control.index : 'none'}, drive {ch.drive ? ch.drive.index : 'none'}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<dl data-testid="sine-summary-derived">
			<div><dt>Rate and tick</dt><dd class="num">{formatNumber(dry.grid.fs_hz, 6)} Hz, {formatNumber(dry.grid.tick_s * 1000, 4)} ms{dry.grid.actual ? '' : ', until the station arms'}</dd></div>
			<div><dt>Test time</dt><dd class="num">{dry.test_s === null ? 'Not set in advance' : `${formatNumber(dry.test_s, 4)} s`}</dd></div>
			<div><dt>Identification</dt><dd class="num">{formatNumber(dry.identification_s, 4)} s</dd></div>
			<div><dt>Stop ramp</dt><dd class="num">{formatNumber(dry.stop_ramp_db_per_s, 4)} dB/s, {formatNumber(dry.safe_stop_s, 3)} s from the ceiling</dd></div>
			<div><dt>Peaks</dt><dd class="num">{formatNumber(dry.profile.peak_accel_g, 4)} g, {formatNumber(dry.profile.peak_velocity_ips, 4)} in/s, {formatNumber(dry.profile.peak_disp_pp_in, 4)} in pk-pk</dd></div>
			<div><dt>Tracking</dt><dd>{dry.tracking.verdict === 'ok' ? 'The filter follows the sweep' : (dry.tracking.message ?? dry.tracking.verdict)}</dd></div>
			<div><dt>Feasibility</dt><dd>{dry.feasibility.message}</dd></div>
		</dl>
	{:else if sine.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	<div class="ready" data-testid="sine-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>Left to Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)} data-testid="sine-todo-{s.id}">{s.label}: {sine.counts[s.id].errors} {sine.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
				{/each}
			</ul>
		{:else if dry}
			<p class="aq-small ok">Nothing is missing: the setup checks clean.</p>
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
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
	}

	h3 {
		margin: var(--sp-3) 0 var(--sp-1);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
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
		color: var(--text-1);
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.ok {
		margin: var(--sp-3) 0 0;
		color: var(--blue-1);
	}
</style>
