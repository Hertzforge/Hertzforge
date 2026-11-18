<script lang="ts">
	/**
	 * The MIMO Sine run as it happens, on the test screen's frame: the pages of plots, and the dock
	 * with the Run card (the shared verbs and MIMO Sine's own: Hold and Resume, and on a sweep
	 * Jog and Go To), the run in numbers, the law's card and the station. While the run
	 * holds for review, the Run card gives the identification in a line, the sweep-rate warning for
	 * every resonance the sweep passes too fast for, and a place for the operator's words. The plots
	 * are the service's views as they come: against frequency on the sweep grid in a sweep or a
	 * stepped sine, against time in a dwell. The window on another monitor shows the same, compact.
	 */
	import type { Snippet } from 'svelte';
	import PulseRunView from '../pulse/PulseRunView.svelte';
	import type { RunOpen } from '../runOpen.svelte';
	import type { PlotPanel } from '../screen/screen.svelte';
	import type { CardKind } from '../screen/types';
	import MsineLawCard from './MsineLawCard.svelte';
	import MsineLevelCard from './MsineLevelCard.svelte';
	import MsineRunVerbs from './MsineRunVerbs.svelte';
	import type { MimoSineStore } from './msineStore.svelte';
	import type { MsineViewsState } from './msineViews.svelte';
	import { identificationWords, resonanceWords } from './msineWords';

	interface Props {
		ms: MimoSineStore;
		views: MsineViewsState;
		station?: Snippet;
		onpopout?: (pageId: string) => boolean;
		onoverride?: () => void;
		abortKeys: string;
		compact?: boolean;
		opener?: RunOpen | null;
	}
	let { ms, views, station, onpopout, onoverride, abortKeys, compact = false, opener = null }: Props = $props();

	const status = $derived(ms.now);
	const live = $derived(ms.frameOfRun);
	const state = $derived(ms.run?.state ?? null);
	const streaming = $derived(state === 'running' || state === 'review' || state === 'stopping' || state === 'finished');
	const limits = $derived(!!ms.runConfig?.limits?.channels?.length);
	const channels = $derived([
		...ms.runControls.map((c) => ({ index: c, name: `Control ${c}` })),
		...(ms.runConfig?.limits?.channels ?? []).map((l) => ({ index: l.channel, name: `Limit ${l.channel}` }))
	]);
	const flagged = $derived(ms.flaggedResonances);

	function empty(p: PlotPanel): string {
		if (!streaming) return 'Appears when a run starts.';
		switch (p.plot.quantity) {
			case 'identification':
				return 'The identification is drawn once it is done.';
			case 'limit':
				return limits ? 'Each limit channel is drawn once the tone engages.' : 'This run has no limit channels.';
			case 'phase':
				return ms.runConfig?.phase_specified === false ? 'This run has amplitudes only: no reference phase.' : 'The phases are drawn once the tone engages.';
			case 'law':
				return "The law's values are shown after its first call.";
			default:
				return 'Drawn once the tone engages.';
		}
	}
</script>

{#snippet verbs()}<MsineRunVerbs {ms} />{/snippet}

{#snippet notes()}
	{#if status?.state === 'review' && !compact}
		<div class="review" data-testid="msine-review">
			{#if status.identification}<p class="aq-small" data-testid="msine-ident-words">{identificationWords(status.identification)}</p>{/if}
			{#if flagged.length}
				<ul class="flags" data-testid="msine-rate-warnings">
					{#each flagged as r (r.freq_hz)}<li class="aq-small">{resonanceWords(r)}</li>{/each}
				</ul>
			{:else if status.test === 'sweep'}
				<p class="aq-small aq-muted">The sweep spends at least two time constants inside every resonance the identification found.</p>
			{/if}
			<label class="lbl" for="msine-ack">Your words for the log (optional)</label>
			<textarea id="msine-ack" class="field" rows="2" bind:value={ms.acknowledgement} disabled={!ms.isController} data-testid="msine-ack"></textarea>
		</div>
	{/if}
{/snippet}

{#snippet cards(kind: CardKind)}
	{#if kind === 'level'}
		<MsineLevelCard {status} {live} report={ms.report} {compact} />
	{:else if kind === 'law'}
		<MsineLawCard law={status?.law ?? null} live={live?.law ?? null} entry={ms.runLaw} />
	{:else if kind === 'station' && station}
		{@render station()}
	{/if}
{/snippet}

<PulseRunView
	host={ms}
	screen={views}
	prefix="msine"
	attrs={{
		'data-state': status?.state ?? '',
		'data-phase': status?.phase ?? '',
		'data-hz': live?.frequency_hz ?? status?.frequency_hz ?? '',
		'data-held': status?.held ? 'true' : 'false',
		'data-notch': status?.notch ? 'true' : 'false',
		'data-level': live?.level_db ?? ''
	}}
	{verbs}
	review="The identification and its coherence are drawn; nothing drives until you proceed."
	{notes}
	{cards}
	{channels}
	{empty}
	live={streaming}
	{onpopout}
	{onoverride}
	{abortKeys}
	{compact}
	record={status?.record ?? null}
	{opener}
/>

<style>
	.review {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.review p {
		margin: 0;
	}

	.flags {
		margin: 0;
		padding-left: var(--sp-3);
		color: var(--amber-1);
	}

	.lbl {
		font-size: var(--text-xs);
		color: var(--text-2);
	}
</style>
