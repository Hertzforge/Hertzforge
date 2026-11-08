<script lang="ts">
	/**
	 * The classical shock setup in a few lines, beside every section: the pulse, the standard, the
	 * schedule and the law; the service's verdict on the setup (the grid, the time between pulses,
	 * the pulse's kinematics, feasibility); and what is left to fix, each a click from its section.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { runChannels, SHAPES, SHOCK_SECTIONS, STANDARDS, type ShockSection } from './shockSetup';
	import type { ShockStore } from './shockStore.svelte';

	interface Props {
		shock: ShockStore;
		onopen: (s: ShockSection) => void;
	}
	let { shock, onopen }: Props = $props();

	const d = $derived(shock.setup);
	const dry = $derived(shock.check && shock.checkedKey === shock.key ? shock.check.derived : null);
	const todo = $derived(SHOCK_SECTIONS.filter((s) => shock.counts[s.id].errors > 0));
	const ch = $derived(runChannels(shock.rows));
	const shape = $derived(SHAPES.find((s) => s.id === d.pulse.shape)?.label ?? d.pulse.shape);
	const std = $derived(STANDARDS.find((s) => s.id === d.standard.name)?.label ?? d.standard.name);
	const total = $derived(d.schedule.levels.reduce((n, l) => n + (Number.isFinite(l.pulses) ? l.pulses : 0), 0));
	const num = (v: number, unit: string) => (Number.isFinite(v) ? `${formatNumber(v, 4)} ${unit}` : '-');
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="shock-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Pulse</dt><dd>{shape}, {num(d.pulse.amplitude_g, 'g')}, {num(d.pulse.duration_ms, 'ms')}, {d.pulse.polarity}</dd></div>
		<div><dt>Shoulders</dt><dd>{d.pulse.compensation.shape.replaceAll('_', ' ')}, {formatNumber(d.pulse.compensation.pre_frac, 3)} times the duration{d.pulse.compensation.symmetric ? '' : ` before, ${formatNumber(d.pulse.compensation.post_frac, 3)} after`}</dd></div>
		<div><dt>Standard</dt><dd>{std}</dd></div>
		<div><dt>Schedule</dt><dd>{d.schedule.mode === 'auto' ? 'Automatic' : 'Manual'}, {d.schedule.levels.length} {d.schedule.levels.length === 1 ? 'level' : 'levels'} from {db(d.schedule.levels[0]?.db)}, {total} pulses planned{d.schedule.advance === 'on_pass' ? ', advancing on passing pulses' : ''}</dd></div>
		<div><dt>Control law</dt><dd>{shock.law ? `${shock.law.name} ${shock.law.version}` : '-'}</dd></div>
		<div><dt>Drive</dt><dd>Ceiling {num(d.limits.drive_vpk_ceiling, 'V pk')}, abort {num(d.limits.drive_vpk_abort, 'V pk')}</dd></div>
		<div><dt>Channels</dt><dd>Control {ch.control ? ch.control.index : 'none'}, drive {ch.drive ? ch.drive.index : 'none'}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<dl data-testid="shock-summary-derived">
			<div><dt>Grid</dt><dd class="num">{formatNumber(dry.grid.fs_hz, 6)} Hz, {dry.grid.record_len} samples a pulse{dry.grid.actual ? '' : ', until the station arms'}</dd></div>
			<div><dt>Time between pulses</dt><dd class="num">{formatNumber(dry.spacing_s, 3)} s</dd></div>
			<div><dt>Velocity change</dt><dd class="num">{formatNumber(dry.reference.velocity_change_ips, 4)} in/s</dd></div>
			<div><dt>Displacement</dt><dd class="num">{formatNumber(dry.reference.displacement_pk_pk_in, 4)} in pk-pk</dd></div>
			<div><dt>Feasibility</dt><dd>{dry.feasibility.message}</dd></div>
		</dl>
	{:else if shock.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	<div class="ready" data-testid="shock-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>To Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)}>{s.label}: {shock.counts[s.id].errors} {shock.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
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
