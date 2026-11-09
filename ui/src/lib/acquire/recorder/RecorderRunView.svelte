<script lang="ts">
	/**
	 * The Recorder run as it happens, in the run view's frame: the page bar, the page on screen, and
	 * beside it the run's state, the card, the last verb's answer and the station (with the
	 * simulator's source when the run is on it). The pages are the service's views drawn as they
	 * come; on the Time page the shell lays the limit marks that fall in view, at their station
	 * time, and the Readings page lists every mark. The window on another monitor shows the same
	 * frame, compact.
	 */
	import type { Snippet } from 'svelte';
	import type { SpectrumView, View } from '$lib/protocol/types';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import { formatNumber } from '$lib/viz/format';
	import StateChip from '../StateChip.svelte';
	import PulsePageBar from '../pulse/PulsePageBar.svelte';
	import RecorderCard from './RecorderCard.svelte';
	import RecorderRecordings from './RecorderRecordings.svelte';
	import RecorderSimSource from './RecorderSimSource.svelte';
	import { recorderPlotId, RECORDER_PAGE_HELP } from './recorderLayout';
	import type { RecorderStore } from './recorderStore.svelte';
	import type { RecorderViewsState } from './recorderViews.svelte';
	import { markWords, RECORDER_OUTCOME_WORDS, RECORDER_STATE_WORDS } from './recorderWords';

	interface Props {
		recorder: RecorderStore;
		views: RecorderViewsState;
		/** The station panel, from the screen. */
		station?: Snippet;
		/** The page bar's right end (the way back to the setup). */
		end?: Snippet;
		onpopout?: (pageId: string) => boolean;
		compact?: boolean;
	}
	let { recorder, views, station, end, onpopout, compact = false }: Props = $props();

	const status = $derived(recorder.now);
	const live = $derived(recorder.frameOfRun);
	const page = $derived(views.page);
	const fs = $derived(status?.grid?.fs_hz ?? null);
	let width = $state(0);
	/** The x range the Time page draws now, station seconds. */
	let timeShown = $state<[number, number] | null>(null);

	$effect(() => {
		if (width > 0) views.setWidth(page.id, width);
	});

	const one = (q: string): View[] => views.plotViews(recorderPlotId(page, q));
	const chipRun = $derived(recorder.run);
	const saved = $derived(recorder.recordings.filter((r) => r.complete));
	const faulted = $derived(saved.find((r) => r.fault) ?? saved.at(-1) ?? null);
	const onSim = $derived(status?.station?.kind === 'sim');
	const hasPair = $derived(!!recorder.runConfig?.analysis?.frf);
	const unitOf = (row: number) => live?.rows.find((r) => r.index === row)?.unit ?? '';

	/** The time view's span in station seconds, and the limit marks inside it. */
	function guidesFor(v: SpectrumView): number[] {
		if (!fs || !v.series.length) return [];
		const x = v.series[0].x;
		if (!x.length) return [];
		const lo = x[0];
		const hi = x[x.length - 1];
		return recorder.marks.filter((m) => m.kind === 'limit').map((m) => m.station_frame / fs).filter((t) => t >= lo && t <= hi);
	}
</script>

{#snippet draw(list: View[], min: number, empty: string)}
	{#if list.length}
		{#each list as v, i (i)}<div class="plot"><ViewRenderer view={v} live minHeight={min} /></div>{/each}
	{:else}
		<p class="aq-muted aq-small empty">{empty}</p>
	{/if}
{/snippet}

<div
	class="live"
	class:compact
	data-testid="recorder-run-view"
	data-state={status?.state ?? ''}
	data-phase={status?.phase ?? ''}
	data-mode={status?.mode ?? ''}
	data-t={live?.t_s ?? ''}
	data-frames={status?.station_frames ?? ''}
	data-recording={status?.recording?.name ?? ''}
	data-recordings={status?.recordings ?? ''}
	data-marks={status?.marks ?? ''}
	data-fires={status?.trigger?.fires ?? ''}
>
	<PulsePageBar {views} prefix="recorder" {end} {onpopout} />

	<div class="grid">
		<div class="pageview" data-testid="recorder-page-view" data-page={page.id} data-kind={page.kind} bind:clientWidth={width}>
			<p class="aq-small aq-muted help">{RECORDER_PAGE_HELP[page.kind]}</p>
			{#key page.id}
				{#if page.kind === 'time'}
					{@const list = one('time').filter((v): v is SpectrumView => v.view === 'spectrum')}
					{#if list.length}
						{#each list as v, i (i)}
							{@const g = guidesFor(v)}
							<div class="plot" data-testid="recorder-time-plot" data-guides={g.length} data-x0={timeShown?.[0] ?? ''} data-x1={timeShown?.[1] ?? ''}><SpectrumPane view={v} live followX minHeight={300} guides={g} onRange={(lo, hi) => (timeShown = [lo, hi])} /></div>
						{/each}
						{#if recorder.marks.some((m) => m.kind === 'limit')}<p class="aq-small marks">The thin lines are limit marks, at the station time each was made.</p>{/if}
					{:else}
						<p class="aq-muted aq-small empty">The time history is drawn after the first batch.</p>
					{/if}
				{:else if page.kind === 'spectra'}
					{@render draw(one('spectrum'), 180, 'The spectra are drawn once the first average is complete.')}
				{:else if page.kind === 'frf'}
					{#if hasPair}
						{@render draw(one('frf'), 180, 'The FRF is drawn once the first average is complete.')}
					{:else}
						<p class="aq-muted aq-small empty" data-testid="recorder-frf-none">This run has no FRF pair. Choose one in the setup's Analysis section, then start a run from it.</p>
					{/if}
				{:else if page.kind === 'readings'}
					{@render draw(one('readings'), 80, 'The readings are shown after the first batch.')}
					<section class="marklist" aria-labelledby="rec-marks-h" data-testid="recorder-mark-list" data-count={recorder.marks.length}>
						<h3 id="rec-marks-h">Marks</h3>
						{#if recorder.marks.length}
							<ul>
								{#each recorder.marks as m, i (i)}<li class="aq-small num">{markWords(m, fs, unitOf(m.row))}</li>{/each}
							</ul>
						{:else}
							<p class="aq-small aq-muted">None. A row passing one of its limits is marked here and recording goes on.</p>
						{/if}
					</section>
				{:else}
					<RecorderRecordings {saved} now={status?.recording ?? null} folder={recorder.folder} {fs} />
				{/if}
			{/key}
		</div>

		<aside class="side" aria-label="Run numbers">
			<section class="card head">
				<StateChip run={chipRun} words={RECORDER_OUTCOME_WORDS} stateWords={RECORDER_STATE_WORDS} />
				{#if status?.grid}<span class="grid-words num" data-testid="recorder-run-grid">{formatNumber(status.grid.fs_hz, 6)} Hz{status.grid.actual && status.grid.fs_hz !== status.grid.requested_fs_hz ? ` (asked ${formatNumber(status.grid.requested_fs_hz, 6)})` : ''}</span>{/if}
				{#if status?.source}<span class="src">{status.source === 'SIM' ? 'Simulator' : status.source}</span>{/if}
			</section>
			<section class="card"><RecorderCard {status} {live} setup={recorder.cardSetup} lastSaved={recorder.lastSaved} {faulted} folder={recorder.folder} {compact} /></section>
			{#if recorder.answer && !compact}
				<p class="answer aq-small" data-testid="recorder-answer" aria-live="polite">{recorder.answer.words}</p>
			{/if}
			{#if onSim}
				<section class="card"><RecorderSimSource rows={status?.rows ?? []} /></section>
			{/if}
			{#if station && !compact}
				<section class="card">{@render station()}</section>
			{/if}
		</aside>
	</div>
</div>

<style>
	.live {
		display: flex;
		flex: 1;
		flex-direction: column;
		gap: var(--sp-3);
		min-height: 0;
	}

	.grid {
		display: grid;
		flex: 1;
		grid-template-columns: minmax(0, 1fr) 340px;
		gap: var(--sp-3);
		min-height: 0;
	}

	.compact .grid {
		grid-template-columns: minmax(0, 1fr) 280px;
	}

	.pageview {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
		min-height: 0;
		padding: var(--sp-3);
		overflow: auto;
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.help,
	.marks {
		margin: 0;
	}

	.marks {
		color: var(--text-2);
	}

	.plot {
		min-width: 0;
	}

	.empty {
		margin: var(--sp-4) 0;
	}

	.marklist h3 {
		margin: var(--sp-2) 0 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.marklist ul {
		margin: 0;
		padding-left: 1.1em;
	}

	.side {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
		min-height: 0;
		overflow-y: auto;
	}

	.card {
		flex: none;
		padding: var(--sp-3);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px var(--sp-3);
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.grid-words {
		color: var(--text-1);
	}

	.src {
		margin-left: auto;
	}

	.answer {
		margin: 0;
		padding: 0 var(--sp-1);
		color: var(--text-2);
	}

	@media (max-width: 1100px) {
		.grid {
			grid-template-columns: 1fr;
		}
	}
</style>
