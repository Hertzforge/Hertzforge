<script lang="ts">
	/**
	 * The Recorder setup in a few lines, beside every section: the rate, the trigger, the analysis,
	 * the rows it records and the drive rows it leaves alone; what the service derived for it on the
	 * selected station (the block, the batch and the group delay, no outputs, the analysis grid,
	 * the pre-window and lengths in frames, the bytes a second, the floor and the time left above
	 * it); and what is left to fix, each a click from its section.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { RECORDER_SECTIONS, recorderRows, type RecorderSection } from './recorderSetup';
	import type { RecorderStore } from './recorderStore.svelte';
	import { bytesWords, spanWords } from './recorderWords';

	interface Props {
		recorder: RecorderStore;
		onopen: (s: RecorderSection) => void;
	}
	let { recorder, onopen }: Props = $props();

	const d = $derived(recorder.setup);
	const t = $derived(recorder.setup.trigger);
	const dry = $derived(recorder.check && recorder.checkedKey === recorder.key ? recorder.check.derived : null);
	const todo = $derived(RECORDER_SECTIONS.filter((s) => recorder.counts[s.id].errors > 0));
	const ch = $derived(recorderRows(recorder.rows));
	const n = (v: number, sig = 4) => formatNumber(v, sig);
	const num = (v: number | null, unit: string, sig = 4) => (v !== null && Number.isFinite(v) ? `${n(v, sig)} ${unit}` : '-');
	const trigger = $derived.by(() => {
		if (t.mode === 'start') return `From the start, ${t.record_s === null ? 'until cut' : num(t.record_s, 's')}`;
		if (t.mode === 'manual') return `Manual, pre-window ${num(t.pre_s, 's')}, ${t.record_s === null ? 'until cut' : num(t.record_s, 's')}`;
		const unit = ch.recorded.find((r) => r.index === t.row)?.units ?? '';
		return `Level, row ${t.row ?? '-'} ${t.slope} through ${num(t.level, unit).trim()}, pre-window ${num(t.pre_s, 's')}, ${num(t.record_s, 's')}, ${t.count === null ? 'until the run ends' : `${t.count} a time`}`;
	});
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="recorder-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Rate asked</dt><dd>{num(d.acquisition.fs_hz, 'Hz', 6)}</dd></div>
		<div><dt>Trigger</dt><dd data-testid="recorder-summary-trigger">{trigger}</dd></div>
		<div><dt>Analysis</dt><dd>Block {d.analysis.block}, {d.analysis.averages} averages, decimation {d.analysis.decimation}{d.analysis.frf ? `, FRF ${d.analysis.frf.response} over ${d.analysis.frf.reference}` : ''}</dd></div>
		<div><dt>Rows</dt><dd data-testid="recorder-summary-rows">Records {ch.recorded.length ? ch.recorded.map((r) => r.index).join(', ') : 'none'}{ch.drives.length ? `; drive ${ch.drives.map((r) => r.index).join(', ')} not opened` : ''}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<dl data-testid="recorder-summary-derived">
			<div><dt>Rate and block</dt><dd class="num" data-testid="recorder-derived-grid">{n(dry.grid.fs_hz, 6)} Hz, {dry.grid.block_frames} frames a block, {dry.grid.batch_frames} a batch{dry.grid.actual ? '' : ', until the station opens'}</dd></div>
			<div><dt>Group delay</dt><dd class="num" data-testid="recorder-derived-delay">{n(dry.grid.group_delay_frames, 4)} frames</dd></div>
			<div><dt>Outputs</dt><dd data-testid="recorder-derived-outputs">{dry.outputs === 'none' ? 'None: no output is opened' : dry.outputs}</dd></div>
			<div><dt>Recorded rows</dt><dd class="num">{dry.rows.join(', ')}</dd></div>
			<div><dt>Analysis</dt><dd class="num" data-testid="recorder-derived-analysis">{n(dry.analysis.df_hz, 4)} Hz lines, {dry.analysis.lines} lines to {n(dry.analysis.span_hz, 5)} Hz; {n(dry.analysis.overlap_pct, 3)}% overlap; the first full average {n(dry.analysis.averages_s, 4)} s after start</dd></div>
			<div><dt>Trigger</dt><dd class="num" data-testid="recorder-derived-trigger">Pre-window {dry.trigger.pre_frames} frames; {dry.trigger.record_frames === null ? 'until cut' : `${dry.trigger.record_frames} frames after the trigger`}; holdoff {dry.trigger.holdoff_frames} frames</dd></div>
			<div><dt>Disk</dt><dd class="num" data-testid="recorder-derived-disk">{bytesWords(dry.disk.bytes_per_s)} a second; floor {bytesWords(dry.disk.floor_bytes)}; {dry.disk.free_bytes === null ? 'free space not readable' : `${bytesWords(dry.disk.free_bytes)} free, ${spanWords(dry.disk.seconds_above_floor)} of recording above the floor`}</dd></div>
		</dl>
	{:else if recorder.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	<div class="ready" data-testid="recorder-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>Left to Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)} data-testid="recorder-todo-{s.id}">{s.label}: {recorder.counts[s.id].errors} {recorder.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
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
