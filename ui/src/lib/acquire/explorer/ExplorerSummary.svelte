<script lang="ts">
	/**
	 * The Explorer setup in a few lines, beside every section: the band and the start, the drive
	 * ceiling and the slews, the FFT size, the run's rows; what the service derived for it on the
	 * selected station (the block and lookahead, the pipeline, the rise and stop lengths, the
	 * fastest sweep, the display and the survey); and what is left to fix, each a click from its
	 * section.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { EXPLORER_SECTIONS, explorerChannels, type ExplorerSection } from './explorerSetup';
	import type { ExplorerStore } from './explorerStore.svelte';

	interface Props {
		explorer: ExplorerStore;
		onopen: (s: ExplorerSection) => void;
	}
	let { explorer, onopen }: Props = $props();

	const d = $derived(explorer.setup);
	const dry = $derived(explorer.check && explorer.checkedKey === explorer.key ? explorer.check.derived : null);
	const todo = $derived(EXPLORER_SECTIONS.filter((s) => explorer.counts[s.id].errors > 0));
	const ch = $derived(explorerChannels(explorer.rows));
	const n = (v: number, sig = 4) => formatNumber(v, sig);
	const num = (v: number, unit: string, sig = 4) => (Number.isFinite(v) ? `${n(v, sig)} ${unit}` : '-');
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="explorer-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Band</dt><dd>{num(d.band_hz[0], 'Hz')} to {num(d.band_hz[1], 'Hz')}, starting at {num(d.start_hz, 'Hz')}</dd></div>
		<div><dt>Rate</dt><dd>{d.acquisition.fs_hz === null ? 'Usual (8192 Hz)' : num(d.acquisition.fs_hz, 'Hz', 6)}</dd></div>
		<div><dt>Drive</dt><dd>Ceiling {num(d.max_drive_vpk, 'V pk')}, {num(d.slew.drive_db_per_s, 'dB/s')}</dd></div>
		<div><dt>Sweep</dt><dd>At most {num(d.slew.freq_oct_per_s, 'oct/s')}</dd></div>
		<div><dt>FFT size</dt><dd>{d.display.nfft}</dd></div>
		<div><dt>Channels</dt><dd>Control {ch.control ? ch.control.index : 'none'}, drive {ch.drive ? ch.drive.index : 'none'}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<dl data-testid="explorer-summary-derived">
			<div><dt>Rate and block</dt><dd class="num" data-testid="explorer-derived-grid">{n(dry.grid.fs_hz, 6)} Hz, {dry.grid.block_frames} frames a block, lookahead {dry.grid.lookahead_frames}{dry.grid.actual ? '' : ', until the station arms'}</dd></div>
			<div><dt>Band</dt><dd class="num">{n(dry.band_hz[0], 5)} to {n(dry.band_hz[1], 5)} Hz</dd></div>
			<div><dt>Pipeline</dt><dd class="num" data-testid="explorer-derived-pipeline">{dry.pipeline_frames} frames, {n(dry.pipeline_s * 1000, 4)} ms</dd></div>
			<div><dt>Rise from 0 V</dt><dd class="num">{n(dry.rise_s, 4)} s</dd></div>
			<div><dt>Stop</dt><dd class="num" data-testid="explorer-derived-stop">{n(dry.stop_s, 4)} s; the service waits {n(dry.stop_wait_s, 3)} s</dd></div>
			<div><dt>Fastest sweep</dt><dd class="num">{n(dry.sweep_max_oct_per_min, 4)} oct/min</dd></div>
			<div><dt>Display</dt><dd class="num">{n(dry.display.df_hz, 4)} Hz lines, {n(dry.display.window_s, 4)} s a window</dd></div>
			<div><dt>Survey</dt><dd class="num" data-testid="explorer-derived-survey">Points at least {n(dry.survey.tolerance_hz, 4)} Hz apart; the first {n(dry.survey.first_point_s[0] * 1000, 4)} ms after a change at the bottom of the band, {n(dry.survey.first_point_s[1] * 1000, 4)} ms at the top</dd></div>
		</dl>
	{:else if explorer.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	<div class="ready" data-testid="explorer-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>Left to Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)} data-testid="explorer-todo-{s.id}">{s.label}: {explorer.counts[s.id].errors} {explorer.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
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
