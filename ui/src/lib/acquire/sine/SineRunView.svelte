<script lang="ts">
	/**
	 * The Sine run as it happens, on the test screen's frame: the pages of plots, and the dock with
	 * the Run card (the shared verbs and Sine's own: Hold and Resume, and on a sweep Reverse, Jog and
	 * Go To), the level card, the law's values and the station. The plots are the service's views as
	 * they come: against frequency on the sweep grid in a sweep or a step test, against time in a
	 * dwell. The window on another monitor shows the same, compact.
	 */
	import type { Snippet } from 'svelte';
	import PulseRunView from '../pulse/PulseRunView.svelte';
	import type { PlotPanel } from '../screen/screen.svelte';
	import type { CardKind } from '../screen/types';
	import SineLevelCard from './SineLevelCard.svelte';
	import SineLiveParams from './SineLiveParams.svelte';
	import SineRunVerbs from './SineRunVerbs.svelte';
	import type { SineStore } from './sineStore.svelte';
	import type { SineViewsState } from './sineViews.svelte';

	interface Props {
		sine: SineStore;
		views: SineViewsState;
		/** The station panel. */
		station?: Snippet;
		onpopout?: (pageId: string) => boolean;
		onoverride?: () => void;
		abortKeys: string;
		compact?: boolean;
	}
	let { sine, views, station, onpopout, onoverride, abortKeys, compact = false }: Props = $props();

	const status = $derived(sine.now);
	const live = $derived(sine.frameOfRun);
	const state = $derived(sine.run?.state ?? null);
	const streaming = $derived(state === 'running' || state === 'review' || state === 'stopping' || state === 'finished');
	const limits = $derived(!!sine.runConfig?.limits.channels?.length);
	const channels = $derived(sine.rows.filter((r) => r.enabled && r.role !== 'drive').map((r) => ({ index: r.index, name: r.description || r.role })));

	function empty(p: PlotPanel): string {
		if (!streaming) return 'Appears when a run starts.';
		switch (p.plot.quantity) {
			case 'control':
				return 'The control amplitude is drawn once the tone engages.';
			case 'drive':
				return 'The drive is drawn once the tone engages.';
			case 'response':
				if (p.plot.channels === 'limited' && !limits) return 'This run has no limit channels.';
				return 'Each input is drawn once the tone engages.';
			case 'phase':
				return 'The phases are drawn once the tone engages.';
			case 'thd':
				return 'The distortion is drawn once the tone engages.';
			case 'time':
				return 'The samples are drawn once the tone engages.';
			case 'identification':
				return 'The identified gain is drawn once identification is done.';
			case 'law':
				return "The law's values are shown after its first call.";
			default:
				return 'Appears with the first frame.';
		}
	}
</script>

{#snippet verbs()}<SineRunVerbs {sine} />{/snippet}

{#snippet cards(kind: CardKind)}
	{#if kind === 'level'}
		<SineLevelCard {status} {live} config={sine.runConfig} {compact} />
	{:else if kind === 'law'}
		<SineLiveParams {sine} />
	{:else if kind === 'station' && station}
		{@render station()}
	{/if}
{/snippet}

<PulseRunView
	host={sine}
	screen={views}
	prefix="sine"
	attrs={{
		'data-state': status?.state ?? '',
		'data-phase': status?.phase ?? '',
		'data-hz': live?.frequency_hz ?? status?.frequency_hz ?? '',
		'data-t': live?.t_s ?? '',
		'data-frame-held': live ? String(live.held) : '',
		'data-held': status?.held ? 'true' : 'false',
		'data-direction': status?.direction ?? '',
		'data-level': live?.level_db ?? ''
	}}
	{verbs}
	review="The identified gain and its coherence are drawn; nothing drives until you proceed."
	{cards}
	{channels}
	{empty}
	live={streaming}
	{onpopout}
	{onoverride}
	{abortKeys}
	{compact}
/>
