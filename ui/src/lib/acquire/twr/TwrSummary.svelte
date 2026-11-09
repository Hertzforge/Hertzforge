<script lang="ts">
	/**
	 * The TWR setup in a few lines, beside every section: the reference, the ladder and the playback,
	 * the law and the drive ceiling; the service's verdict on the setup (the record, the call budget,
	 * the plays, feasibility); and what is left to fix, each a click from its section.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { runChannels, TWR_SECTIONS, type TwrSetupSection } from './twrSetup';
	import type { TwrStore } from './twrStore.svelte';

	interface Props {
		twr: TwrStore;
		onopen: (s: TwrSetupSection) => void;
	}
	let { twr, onopen }: Props = $props();

	const d = $derived(twr.setup);
	const dry = $derived(twr.check && twr.checkedKey === twr.key ? twr.check.derived : null);
	const todo = $derived(TWR_SECTIONS.filter((s) => twr.counts[s.id].errors > 0));
	const ch = $derived(runChannels(twr.rows));
	const facts = $derived(d.facts && d.facts.sha256 === d.reference.sha256 ? d.facts : null);
	const num = (v: number, unit: string) => (Number.isFinite(v) ? `${formatNumber(v, 4)} ${unit}` : '-');
	const playback = $derived(d.run.schedule === 'once' ? 'once' : d.run.schedule === 'loop' ? `${d.run.count} times` : `for ${num(d.run.duration_s, 's')} of section time`);
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="twr-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Reference</dt><dd>{facts ? `${facts.name}, ${formatNumber(facts.duration_s, 4)} s at ${formatNumber(facts.fs_hz, 6)} Hz` : d.reference.sha256 ? 'A stored reference' : 'None yet'}</dd></div>
		<div><dt>Schedule</dt><dd>{d.schedule.mode === 'auto' ? 'Automatic' : 'Manual'}, {d.schedule.levels_db.length} {d.schedule.levels_db.length === 1 ? 'level' : 'levels'} from {db(d.schedule.levels_db[0])}, gate {num(d.schedule.gate_pct, '%')}</dd></div>
		<div><dt>Playback</dt><dd>{playback}{d.adaptation.during_playback ? ', correcting' : ''}</dd></div>
		<div><dt>Control law</dt><dd>{twr.law ? `${twr.law.name} ${twr.law.version}` : '-'}</dd></div>
		<div><dt>Drive</dt><dd>Ceiling {num(d.limits.drive_vpk_ceiling, 'V pk')}{d.limits.drive_clip_vpk !== null ? `, clip ${num(d.limits.drive_clip_vpk, 'V')}` : ''}</dd></div>
		<div><dt>Channels</dt><dd>Control {ch.control ? ch.control.index : 'none'}, drive {ch.drive ? ch.drive.index : 'none'}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<dl data-testid="twr-summary-derived">
			<div><dt>Record</dt><dd class="num">{dry.grid.record_len} samples at {formatNumber(dry.grid.fs_hz, 6)} Hz, {formatNumber(dry.grid.record_s, 4)} s{dry.grid.actual ? '' : ', until the station arms'}</dd></div>
			<div><dt>Reference</dt><dd>{dry.reference.name}{dry.reference.resampled ? ', resampled to the record' : ''}</dd></div>
			<div><dt>Playback plays</dt><dd class="num">{dry.plays_target}</dd></div>
			<div><dt>A call's budget</dt><dd class="num">{formatNumber(dry.call_budget_s, 4)} s</dd></div>
			<div><dt>Feasibility</dt><dd>{dry.feasibility.message}</dd></div>
		</dl>
	{:else if twr.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	<div class="ready" data-testid="twr-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>To Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)}>{s.label}: {twr.counts[s.id].errors} {twr.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
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
