<script lang="ts">
	/**
	 * The Playback run as it happens, on the test screen's frame: the pages of plots (the inputs and
	 * the drives as played, on the station clock), and the dock with the Run card (Start, Stop,
	 * Abort and New Run, the run folder, and at the end its outcome with Open in Analysis), the Play
	 * card (the phase, the samples played of the file, the read-ahead held, the level and ceiling
	 * and the end's message), the drive check the run was armed on, and the station. No spectra and
	 * no analysis: the run folder carries the time histories to the analysis side.
	 */
	import type { Snippet } from 'svelte';
	import PulseRunView from '../pulse/PulseRunView.svelte';
	import type { RunOpen } from '../runOpen.svelte';
	import type { PlotPanel } from '../screen/screen.svelte';
	import type { CardKind } from '../screen/types';
	import type { PlaybackStore } from './playbackStore.svelte';
	import type { PlaybackViewsState } from './playbackViews.svelte';
	import { playbackStatus, progressWords, PLAYBACK_OUTCOME_WORDS, volts } from './playbackWords';

	interface Props {
		playback: PlaybackStore;
		views: PlaybackViewsState;
		station?: Snippet;
		onpopout?: (pageId: string) => boolean;
		abortKeys: string;
		compact?: boolean;
		opener?: RunOpen | null;
	}
	let { playback, views, station, onpopout, abortKeys, compact = false, opener = null }: Props = $props();

	const s = $derived(playback.now);
	const state = $derived(s?.state ?? null);
	const streaming = $derived(state === 'starting' || state === 'running' || state === 'stopping' || state === 'finished');
	const report = $derived(playback.served?.check && 'channels' in playback.served.check ? playback.served.check : null);
	const rows = $derived([...(s?.inputs ?? []), ...(s?.drives ?? [])]);
	const channels = $derived(rows.map((r) => ({ index: r.index, name: r.description || `Row ${r.index}` })));
	const p = $derived(s?.progress ?? null);

	$effect(() => {
		const byIndex = new Map(rows.map((r) => [r.index, r.description]));
		views.names = (i) => byIndex.get(i) || `Row ${i}`;
	});

	function empty(panel: PlotPanel): string {
		if (!streaming) return 'Appears when a run starts.';
		return panel.plot.quantity === 'drives' ? 'The drives are drawn after the first frame.' : 'The inputs are drawn after the first frame.';
	}
</script>

{#snippet cards(kind: CardKind)}
	{#if kind === 'play'}
		<div class="play" data-testid="playback-play" data-phase={s?.phase ?? ''} data-played={p?.played ?? ''} data-samples={p?.samples ?? ''}>
			<p class="status" data-testid="playback-status">{playbackStatus(state, s?.phase ?? null, s?.end ?? null)}</p>
			{#if p}
				<div class="bar" role="progressbar" aria-label="Played" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(p.fraction * 100)}><span style:width="{p.fraction * 100}%"></span></div>
				<p class="aq-small num" data-testid="playback-progress">{progressWords(p, s?.fs_hz)}</p>
			{/if}
			{#if s}
				<dl class="nums">
					<div><dt>Level</dt><dd class="num">{s.level_db} dB</dd></div>
					<div><dt>Ceiling</dt><dd class="num">{volts(s.ceiling_v)}</dd></div>
					<div><dt>Rate</dt><dd class="num">{s.fs_hz} Hz</dd></div>
					<div><dt>Drive rows</dt><dd class="num">{s.drive.map.join(', ')}</dd></div>
				</dl>
				{#if s.overloads.some((o) => o.overload)}<p class="aq-small warn">Overload on {s.overloads.filter((o) => o.overload).map((o) => `row ${o.channel}`).join(', ')}.</p>{/if}
			{/if}
			{#if s?.end}
				<p class="aq-small" data-testid="playback-end-message"><strong>{PLAYBACK_OUTCOME_WORDS[s.end.outcome]}.</strong> {s.end.message}{s.end.emergency_stop ? ' The drive went to zero at once.' : ''}{s.end.trip ? ` Row ${s.end.trip.channel}, ${s.end.trip.axis}: ${s.end.trip.value} against a limit of ${s.end.trip.limit}.` : ''}</p>
			{/if}
		</div>
	{:else if kind === 'checked'}
		{#if report}
			<ul class="chk" data-testid="playback-checked">
				{#each report.channels as c (c.channel)}<li class="aq-small num">{c.channel}, {c.name}: row {c.drive_row ?? '-'}, peak {volts(c.played_peak_v)} played; {c.pass ? 'ends at rest' : 'ends refused'}</li>{/each}
			</ul>
		{:else}
			<p class="aq-small aq-muted">The check the run was set up with is shown once the setup is applied.</p>
		{/if}
	{:else if kind === 'station'}
		{#if station}{@render station()}{/if}
	{/if}
{/snippet}

<PulseRunView
	host={playback}
	screen={views}
	prefix="playback"
	attrs={{
		'data-state': state ?? '',
		'data-phase': s?.phase ?? '',
		'data-played': p?.played ?? '',
		'data-samples': p?.samples ?? '',
		'data-outcome': s?.end?.outcome ?? ''
	}}
	{cards}
	{channels}
	{empty}
	live={streaming}
	{onpopout}
	{abortKeys}
	{compact}
	{opener}
/>

<style>
	.play {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.status {
		margin: 0;
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
	}

	.bar {
		height: 6px;
		background: var(--bg-3, var(--line-2));
		border-radius: 3px;
		overflow: hidden;
	}

	.bar span {
		display: block;
		height: 100%;
		background: var(--blue-2, var(--blue-1));
	}

	p {
		margin: 0;
	}

	.nums {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 4px var(--sp-3);
		margin: 0;
	}

	.nums dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.nums dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.warn {
		color: var(--amber-1, var(--text-1));
	}

	.chk {
		margin: 0;
		padding-left: 1.1em;
	}
</style>
