<script lang="ts">
	/**
	 * The Mixed Mode setup in a few lines, beside every section: the parts, the schedule, the two
	 * laws and the drive; then what the control service derived from the draft (each part's mean
	 * square and motion, the totals, the predicted peak of the sum, the lines left out of the floor's
	 * average, the closest tones, the grid and the tick, the test and identification times, whether
	 * each law may drive hardware, feasibility); and what is left to fix, each a click from its
	 * section. Every derived number is the service's, from its dry run of the draft as it stands.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { runChannels } from '../sine/sineSetup';
	import { MIXED_SECTIONS, type MixedSection } from './mixedSetup';
	import type { MixedStore } from './mixedStore.svelte';
	import { kinematicsWords, motionWords, PART_KIND_WORDS } from './mixedWords';

	interface Props {
		mixed: MixedStore;
		onopen: (s: MixedSection) => void;
	}
	let { mixed, onopen }: Props = $props();

	const d = $derived(mixed.setup);
	const dry = $derived(mixed.derived);
	const todo = $derived(MIXED_SECTIONS.filter((s) => mixed.counts[s.id].errors > 0));
	const ch = $derived(runChannels(mixed.rows));
	const num = (v: number | null | undefined, unit: string, sig = 4) => (v !== null && v !== undefined && Number.isFinite(v) ? `${formatNumber(v, sig)} ${unit}` : '-');
	const kind = $derived(d.tones.length && d.bands.length ? 'Sine and random on random' : d.tones.length ? 'Sine on random' : d.bands.length ? 'Random on random' : 'The floor alone');
	const name = (id: string) => dry?.parts.find((p) => p.id === id)?.name ?? id;
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="mixed-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Test</dt><dd data-testid="mixed-summary-kind">{kind}</dd></div>
		<div><dt>Parts</dt><dd>The floor, {d.tones.length} {d.tones.length === 1 ? 'tone' : 'tones'}, {d.bands.length} {d.bands.length === 1 ? 'narrowband' : 'narrowbands'}</dd></div>
		<div><dt>Schedule</dt><dd>{d.schedule.mode === 'auto' ? 'Automatic' : 'Manual'}, {d.schedule.levels.length} {d.schedule.levels.length === 1 ? 'level' : 'levels'} from {db(d.schedule.levels[0]?.db)}</dd></div>
		<div><dt>Laws</dt><dd>Floor: {mixed.floorLaw ? `${mixed.floorLaw.name} ${mixed.floorLaw.version}` : '-'}; tones: {mixed.toneLaw ? `${mixed.toneLaw.name} ${mixed.toneLaw.version}` : '-'}</dd></div>
		<div><dt>Drive</dt><dd>Ceiling {num(d.limits.drive_vpk_ceiling, 'V pk')}, abort {num(d.limits.drive_vpk_abort, 'V pk')}</dd></div>
		<div><dt>Channels</dt><dd>Control {ch.control ? ch.control.index : 'none'}, drive {ch.drive ? ch.drive.index : 'none'}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<table class="parts" data-testid="mixed-summary-parts">
			<thead>
				<tr><th>Part</th><th>Moves over</th><th class="num">Mean square</th></tr>
			</thead>
			<tbody>
				{#each dry.parts as p (p.id)}
					<tr data-part={p.id}>
						<td><span class="kind">{PART_KIND_WORDS[p.kind]}</span> {p.name}</td>
						<td class="num">{p.kind === 'floor' ? `${formatNumber(dry.band_hz[0], 4)} to ${formatNumber(dry.band_hz[1], 4)} Hz` : motionWords(p.freq_hz)}{p.width_hz ? `, ${motionWords(p.width_hz).replace(' Hz', '')} Hz wide` : ''}</td>
						<td class="num">{num(p.ms_g2, 'g²')}</td>
					</tr>
				{/each}
			</tbody>
		</table>
		<dl data-testid="mixed-summary-derived">
			<div><dt>Total</dt><dd class="num">{num(dry.reference.total_ms_g2, 'g²')}, {num(dry.reference.total_rms_g, 'g rms')}; random part {num(dry.reference.random_rms_g, 'g rms')}</dd></div>
			<div><dt>Predicted peak of the sum</dt><dd class="num">{kinematicsWords(dry.kinematics)}</dd></div>
			<div><dt>Left out of the floor's average</dt><dd>{dry.exclusion.length ? dry.exclusion.map((e) => `${name(e.part)}: ${formatNumber(e.lo_hz, 4)} to ${formatNumber(e.hi_hz, 4)} Hz, ${e.lines} lines`).join('; ') : 'No line for the whole run'}</dd></div>
			<div><dt>Closest tones</dt><dd>{dry.closest_tones ? `${name(dry.closest_tones.parts[0])} and ${name(dry.closest_tones.parts[1])}, ${num(dry.closest_tones.spacing_hz, 'Hz')} apart (at least ${num(dry.closest_tones.min_hz, 'Hz')})` : 'Fewer than two tones'}</dd></div>
			<div><dt>Tracking</dt><dd>{dry.tracking.every((t) => t.verdict === 'ok') ? 'Every tone is tracked' : dry.tracking.filter((t) => t.verdict !== 'ok').map((t) => `${name(t.part)}: ${t.message ?? t.verdict}`).join('; ')}</dd></div>
			<div><dt>Lines and tick</dt><dd class="num">{formatNumber(dry.grid.lines, 6)} lines {formatNumber(dry.grid.df_hz, 4)} Hz apart, a loop {num(dry.grid.loop_s, 's')}; a tick {formatNumber(dry.grid.tick_s * 1000, 4)} ms{dry.grid.actual ? '' : ', until the station arms'}</dd></div>
			<div><dt>Floor equalizes in</dt><dd class="num" data-testid="mixed-floor-equalize">{num(dry.floor_equalize_s, 's')} on this grid, before the other parts come in</dd></div>
			<div><dt>Test time</dt><dd class="num">{dry.test_s === null ? 'Not set in advance' : num(dry.test_s, 's')} after the floor equalizes; identification {num(dry.identification_s, 's')}</dd></div>
			<div><dt>Law call budget</dt><dd class="num">Floor {num(dry.call_budget_s.floor * 1000, 'ms')}, each tone {num(dry.call_budget_s.tones * 1000, 'ms')}</dd></div>
			<div><dt>On hardware</dt><dd>Floor law {dry.law.floor.hardware ? 'may drive hardware' : 'runs on the simulator only'}; tone law {dry.law.tones.hardware ? 'may drive hardware' : 'runs on the simulator only'}</dd></div>
			<div><dt>Feasibility</dt><dd>{dry.feasibility.message}</dd></div>
		</dl>
	{:else if mixed.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	<div class="ready" data-testid="mixed-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>Left to Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)} data-testid="mixed-todo-{s.id}">{s.label}: {mixed.counts[s.id].errors} {mixed.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
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

	table.parts {
		width: 100%;
		margin-bottom: var(--sp-2);
		border-collapse: collapse;
		font-size: var(--text-xs);
	}

	.parts th {
		font-weight: var(--weight-medium);
		color: var(--text-3);
		text-align: left;
	}

	.parts td,
	.parts th {
		padding: 2px 4px;
		border-bottom: 1px solid var(--line-1);
	}

	.parts .num {
		text-align: right;
	}

	.kind {
		color: var(--text-3);
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
