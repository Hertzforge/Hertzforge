<script lang="ts">
	/**
	 * The Recorder run at a glance: what it is doing in plain words (monitoring, armed and waiting,
	 * recording and how far), the last recording saved, how far the disk is behind, a write fault
	 * and what it left, the rate the station runs at beside the rate asked, the station time, the
	 * recordings, fires and marks, the time left above the free-space floor, each recorded row's
	 * level with its overload indicator, and once the run ends, how and where its recordings are.
	 * Every number is the service's; shape and word carry each state, color only follows.
	 */
	import type { RecorderEnd, RecorderLiveHead, RecorderRecording, RecorderState } from '$lib/protocol/recorder';
	import { formatNumber } from '$lib/viz/format';
	import { clock } from '../words';
	import { backlogWords, endWords, faultWords, rateWords, recorderCardStatus, RECORDER_STOP_WORDS, savedWords, secs, spanWords, type RecorderCardSetup } from './recorderWords';

	interface Props {
		/** The state with the live frame laid over it. */
		status: RecorderState | null;
		/** The last frame's numbers, when they are this run's. */
		live: RecorderLiveHead | null;
		setup: RecorderCardSetup | null;
		lastSaved: RecorderRecording | null;
		/** The recording that holds what a write fault left: the one with the fault, else the last. */
		faulted: RecorderRecording | null;
		folder: string | null;
		compact?: boolean;
	}
	let { status, live, setup, lastSaved, faulted, folder, compact = false }: Props = $props();

	const end = $derived((status?.end ?? null) as RecorderEnd | null);
	const st = $derived(recorderCardStatus(status, setup, end));
	const grid = $derived(status?.grid ?? null);
	const rec = $derived(status?.recording ?? null);
	const backlog = $derived(backlogWords(rec?.backlog_s));
	const fault = $derived(status?.disk?.fault ?? (end?.outcome === 'write_failed' ? end.message : null));
	const n = (v: number, sig = 4) => formatNumber(v, sig);
</script>

<section class="card" class:compact aria-label="Recorder" data-testid="recorder-card">
	<div class="statusline {st.tone}" role="status" data-testid="recorder-status">
		<span class="mk" aria-hidden="true">
			<svg viewBox="0 0 14 14" width="14" height="14">
				{#if st.mark === 'control'}
					<circle cx="7" cy="7" r="5.5" fill="currentColor" />
				{:else if st.mark === 'hold'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><circle cx="7" cy="7" r="1.6" fill="currentColor" />
				{:else if st.mark === 'stop'}
					<path d="M2 3.5L7 11l5-7.5z" fill="currentColor" />
				{:else if st.mark === 'ended'}
					<rect x="2.5" y="2.5" width="9" height="9" rx="1.3" fill="currentColor" />
				{:else if st.mark === 'armed'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M7 2a5 5 0 010 10z" fill="currentColor" />
				{:else}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.2 2" />
				{/if}
			</svg>
		</span>
		<span class="words">{st.text}</span>
	</div>

	{#if lastSaved && status?.state !== 'finished'}<p class="line" data-testid="recorder-saved">{savedWords(lastSaved)}</p>{/if}
	{#if backlog}<p class="line warn" data-testid="recorder-backlog">{backlog}</p>{/if}
	{#if fault}<p class="line fail" role="alert" data-testid="recorder-fault">{faultWords(fault, faulted)}</p>{/if}

	<dl class="nums">
		<div>
			<dt>Rate</dt>
			<dd class="num" data-testid="recorder-rate" data-fs={grid?.fs_hz ?? ''} data-asked={grid?.requested_fs_hz ?? ''}>{rateWords(grid?.fs_hz)}{#if grid?.actual && grid.fs_hz !== grid.requested_fs_hz}<span class="sub"> (asked {rateWords(grid.requested_fs_hz)})</span>{/if}</dd>
		</div>
		<div><dt>Station time</dt><dd class="num" data-testid="recorder-time">{clock(status?.t_s)}</dd></div>
		<div><dt>Recordings</dt><dd class="num" data-testid="recorder-saved-count">{status?.recordings ?? 0}</dd></div>
		<div><dt>Marks</dt><dd class="num" data-testid="recorder-marks">{status?.marks ?? 0}</dd></div>
		{#if status?.trigger}
			<div><dt>Fires</dt><dd class="num" data-testid="recorder-fires">{status.trigger.fires}{#if status.trigger.left !== null}<span class="sub">, {status.trigger.left} left</span>{/if}</dd></div>
		{/if}
		<div><dt>Above the floor</dt><dd class="num" data-testid="recorder-above-floor">{spanWords(status?.disk?.seconds_above_floor)}</dd></div>
		{#if !compact}
			<div><dt>Group delay</dt><dd class="num">{grid ? `${n(grid.group_delay_frames, 4)} frames` : '-'}</dd></div>
			<div><dt>Backlog</dt><dd class="num">{rec ? secs(rec.backlog_s, 2) : '-'}</dd></div>
		{/if}
	</dl>

	{#if live?.rows.length}
		<table class="rows" data-testid="recorder-rows">
			<thead><tr><th>Row</th><th>rms</th><th>Peak</th><th>Marks</th><th><span class="visually-hidden">Overload</span></th></tr></thead>
			<tbody>
				{#each live.rows as r (r.index)}
					<tr data-testid="recorder-row-{r.index}" data-overload={r.overload}>
						<td class="num">{r.index}</td>
						<td class="num">{r.rms === null ? '-' : `${n(r.rms)} ${r.unit}`}</td>
						<td class="num">{r.peak === null ? '-' : `${n(r.peak)} ${r.unit}`}</td>
						<td class="num">{r.marks}</td>
						<td>
							{#if r.overload}
								<span class="over" title="Reached 0.9 of the row's range; holds for the run">
									<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="currentColor" /></svg>
									Overload
								</span>
							{/if}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}

	{#if status?.stop && status.state === 'stopping'}<p class="aq-small aq-muted" data-testid="recorder-stop-cause">{RECORDER_STOP_WORDS[status.stop]}.</p>{/if}

	{#if end}
		<div class="end" data-testid="recorder-end" data-outcome={end.outcome}>
			<p class="aq-small" data-testid="recorder-end-where">{endWords(end, folder)}</p>
			<p class="aq-small aq-muted">{end.message}</p>
		</div>
	{/if}
</section>

<style>
	.card {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.statusline {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.statusline .mk {
		display: inline-flex;
	}

	.statusline.pass .mk {
		color: var(--blue-2);
	}

	.statusline.warn .mk,
	.statusline.fail .mk {
		color: var(--amber-1);
	}

	.statusline.info .mk {
		color: var(--text-3);
	}

	.words {
		flex: 1;
		min-width: 0;
	}

	.line {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.line.warn,
	.line.fail {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.sub {
		color: var(--text-3);
	}

	.nums {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
		margin: 0;
	}

	.nums dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.nums dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.rows {
		width: 100%;
		font-size: var(--text-xs);
		border-collapse: collapse;
	}

	.rows th {
		font-weight: var(--weight-medium);
		color: var(--text-3);
		text-align: left;
	}

	.rows td,
	.rows th {
		padding: 2px 4px;
	}

	.over {
		display: inline-flex;
		align-items: center;
		gap: 3px;
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.end {
		padding-top: var(--sp-2);
		border-top: 1px solid var(--line-1);
	}

	.end p {
		margin: 0 0 4px;
	}

	p {
		margin: 0;
	}
</style>
