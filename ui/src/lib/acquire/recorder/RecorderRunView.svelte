<script lang="ts">
	/**
	 * The Recorder run as it happens, on the test screen's frame: the pages of plots, and the dock
	 * with the Run card (the shared verbs and the Recorder's own: Arm, Record, Cut, with the service's
	 * answer to the last of them), the recording card, every mark, the recordings and the station
	 * (with the simulator's source when the run is on it). A time plot carries the limit marks that
	 * fall in view as thin lines, at their station time. The window on another monitor shows the
	 * same, compact.
	 */
	import type { Snippet } from 'svelte';
	import type { SpectrumView } from '$lib/protocol/types';
	import PulseRunView from '../pulse/PulseRunView.svelte';
	import type { BodyPanel, PlotExtras } from '../screen/bodies';
	import type { PlotPanel } from '../screen/screen.svelte';
	import type { CardKind } from '../screen/types';
	import RecorderCard from './RecorderCard.svelte';
	import RecorderRecordings from './RecorderRecordings.svelte';
	import RecorderRunVerbs from './RecorderRunVerbs.svelte';
	import RecorderSimSource from './RecorderSimSource.svelte';
	import type { RecorderStore } from './recorderStore.svelte';
	import type { RecorderViewsState } from './recorderViews.svelte';
	import { markWords } from './recorderWords';

	interface Props {
		recorder: RecorderStore;
		views: RecorderViewsState;
		/** The station panel. */
		station?: Snippet;
		onpopout?: (pageId: string) => boolean;
		abortKeys: string;
		compact?: boolean;
	}
	let { recorder, views, station, onpopout, abortKeys, compact = false }: Props = $props();

	const status = $derived(recorder.now);
	const live = $derived(recorder.frameOfRun);
	const runState = $derived(status?.state ?? null);
	const streaming = $derived(runState === 'starting' || runState === 'running' || runState === 'stopping' || runState === 'finished');
	const fs = $derived(status?.grid?.fs_hz ?? null);
	const saved = $derived(recorder.recordings.filter((r) => r.complete));
	const faulted = $derived(saved.find((r) => r.fault) ?? saved.at(-1) ?? null);
	const onSim = $derived(status?.station?.kind === 'sim');
	const hasPair = $derived(!!recorder.runConfig?.analysis?.frf);
	const unitOf = (row: number) => live?.rows.find((r) => r.index === row)?.unit ?? '';
	const channels = $derived(recorder.rows.filter((r) => r.enabled && r.role !== 'drive').map((r) => ({ index: r.index, name: r.description || r.role })));
	const limitMarks = $derived(recorder.marks.filter((m) => m.kind === 'limit'));
	/** The x range each time plot draws now, station seconds, by panel. */
	let shown = $state<Record<string, [number, number]>>({});

	function empty(p: PlotPanel): string {
		if (!streaming) return 'Appears when a run starts.';
		switch (p.plot.quantity) {
			case 'time':
				return 'The time history is drawn after the first batch.';
			case 'spectrum':
				return 'The spectra are drawn once the first average is complete.';
			case 'frf':
				return hasPair ? 'The FRF is drawn once the first average is complete.' : "This run has no FRF pair. Choose one in the setup's Analysis section, then start a run from it.";
			default:
				return 'The readings are shown after the first batch.';
		}
	}

	/** The limit marks inside the time plot's span, at their station time. */
	function guidesFor(id: string): number[] {
		const v = views.views(id).find((x): x is SpectrumView => x.view === 'spectrum');
		if (!fs || !v?.series.length) return [];
		const x = v.series[0].x;
		if (!x.length) return [];
		const lo = x[0];
		const hi = x[x.length - 1];
		return limitMarks.map((m) => m.station_frame / fs).filter((t) => t >= lo && t <= hi);
	}
</script>

{#snippet verbs()}<RecorderRunVerbs {recorder} />{/snippet}

{#snippet notes()}
	{#if recorder.answer && !compact}
		<p class="answer aq-small" data-testid="recorder-answer" aria-live="polite">{recorder.answer.words}</p>
	{/if}
{/snippet}

{#snippet timePlot(panel: BodyPanel, draw: Snippet<[PlotExtras]>)}
	{@const g = guidesFor(panel.id)}
	{@const x = shown[panel.id]}
	<div class="timeplot" data-testid="recorder-time-plot" data-guides={g.length} data-x0={x?.[0] ?? ''} data-x1={x?.[1] ?? ''}>
		{@render draw({ guides: g, onrange: (lo, hi) => (shown[panel.id] = [lo, hi]) })}
		{#if g.length}<p class="aq-small marks">The thin lines are limit marks, at the station time each was made.</p>{/if}
	</div>
{/snippet}

{#snippet cards(kind: CardKind)}
	{#if kind === 'recording'}
		<RecorderCard {status} {live} setup={recorder.cardSetup} lastSaved={recorder.lastSaved} {faulted} folder={recorder.folder} {compact} />
	{:else if kind === 'recmarks'}
		<div class="marklist" data-testid="recorder-mark-list" data-count={recorder.marks.length}>
			{#if recorder.marks.length}
				<ul>
					{#each recorder.marks as m, i (i)}<li class="aq-small num">{markWords(m, fs, unitOf(m.row))}</li>{/each}
				</ul>
			{:else}
				<p class="aq-small aq-muted">None. A row passing one of its limits is marked here and recording goes on.</p>
			{/if}
		</div>
	{:else if kind === 'recordings'}
		<RecorderRecordings {saved} now={status?.recording ?? null} folder={recorder.folder} {fs} />
	{:else if kind === 'station'}
		{#if station}{@render station()}{/if}
		{#if onSim}<div class="src"><RecorderSimSource rows={status?.rows ?? []} /></div>{/if}
	{/if}
{/snippet}

<PulseRunView
	host={recorder}
	screen={views}
	prefix="recorder"
	attrs={{
		'data-state': status?.state ?? '',
		'data-phase': status?.phase ?? '',
		'data-mode': status?.mode ?? '',
		'data-t': live?.t_s ?? '',
		'data-frames': status?.station_frames ?? '',
		'data-recording': status?.recording?.name ?? '',
		'data-recordings': status?.recordings ?? '',
		'data-marks': status?.marks ?? '',
		'data-fires': status?.trigger?.fires ?? ''
	}}
	{verbs}
	{notes}
	{cards}
	bodies={{ time: timePlot }}
	{channels}
	{empty}
	live={streaming}
	{onpopout}
	{abortKeys}
	{compact}
/>

<style>
	.timeplot {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-height: 0;
	}

	.marks {
		flex: none;
		margin: 0;
		padding: 0 4px;
		color: var(--text-2);
	}

	.marklist ul {
		margin: 0;
		padding-left: 1.1em;
	}

	.marklist p {
		margin: 0;
	}

	.src {
		margin-top: var(--sp-3);
	}

	.answer {
		margin: 0;
		padding: 0 var(--sp-1);
		color: var(--text-2);
	}
</style>
