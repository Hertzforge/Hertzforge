<script lang="ts">
	/**
	 * The MIMO Shock setup in a few lines, beside every section: the channels, the targets, the
	 * timing and verdict, the ladder, the law and the abort lines; the service's verdict on the setup
	 * (the grid, the time between pulses, each control's full-level peak); and what is left to fix,
	 * each a click from its section.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { MS_SECTIONS, type MsSection } from './msSetup';
	import type { MimoShockStore } from './msStore.svelte';
	import { VERDICT_WORDS } from './msWords';

	interface Props {
		ms: MimoShockStore;
		onopen: (s: MsSection) => void;
	}
	let { ms, onopen }: Props = $props();

	const d = $derived(ms.setup);
	const dry = $derived(ms.check && ms.checkedKey === ms.key ? ms.check.derived : null);
	const todo = $derived(MS_SECTIONS.filter((s) => ms.counts[s.id].errors > 0));
	const levels = $derived(d.schedule.levels_db.filter(Number.isFinite));
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="ms-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Channels</dt><dd>{d.controls.length} {d.controls.length === 1 ? 'control' : 'controls'}, {d.drives.length} {d.drives.length === 1 ? 'drive' : 'drives'}{d.limits.length ? `, ${d.limits.length} ${d.limits.length === 1 ? 'limit' : 'limits'}` : ''}</dd></div>
		<div><dt>Targets</dt><dd>{d.kind === 'waveform' ? 'A classical pulse at every control' : 'An SRS at every control'}</dd></div>
		<div><dt>Timing</dt><dd>{d.timing.mode === 'free' ? 'Free' : 'Held'}, window {formatNumber(d.timing.window_ms, 3)} ms</dd></div>
		<div><dt>Verdict</dt><dd>{VERDICT_WORDS[d.verdict].label}</dd></div>
		<div><dt>Ladder</dt><dd>{levels.length} levels from {db(levels[0])}, {d.schedule.mode === 'auto' ? 'automatic' : 'on Fire'}</dd></div>
		<div><dt>Control law</dt><dd data-testid="ms-summary-law">{ms.law ? `${ms.law.name} ${ms.law.version}` : '-'}</dd></div>
		<div><dt>Abort lines</dt><dd data-testid="ms-summary-aborts">Absolute always; scaled {d.control.scaled_aborts ? 'on' : 'off, flagged only'}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<dl data-testid="ms-summary-derived">
			<div><dt>Grid</dt><dd class="num">{formatNumber(dry.grid.fs_hz, 6)} Hz, {dry.grid.record_len} samples a pulse{dry.grid.actual ? '' : ', until the station arms'}</dd></div>
			<div><dt>Time between pulses</dt><dd class="num">{formatNumber(dry.spacing_s, 3)} s</dd></div>
			{#each dry.controls as c (c.channel)}
				<div><dt>Control {c.channel}</dt><dd class="num">{formatNumber(c.peak, 4)} peak at full level</dd></div>
			{/each}
		</dl>
	{:else if ms.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	<div class="ready" data-testid="ms-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>To Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)}>{s.label}: {ms.counts[s.id].errors} {ms.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
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

	dl div {
		display: grid;
		grid-template-columns: 110px minmax(0, 1fr);
		gap: var(--sp-2);
		font-size: var(--text-xs);
	}

	dt {
		color: var(--text-3);
	}

	dd {
		margin: 0;
		color: var(--text-1);
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.ok {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		color: var(--blue-1);
	}
</style>
