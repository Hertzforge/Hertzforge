<script lang="ts">
	/**
	 * The classical shock run as it happens, on the test screen's frame: the pages of plots, and the
	 * dock with the Run card (the shared verbs and the shock's own: Fire, the burst, the level moves),
	 * the level card with the law, the pulse table and the station. The Pulse and SRS plots lay the
	 * pulses picked in the pulse table over the latest one. The window on another monitor shows the
	 * same, compact.
	 */
	import type { Snippet } from 'svelte';
	import LawLine from '../pulse/LawLine.svelte';
	import PulseRunView from '../pulse/PulseRunView.svelte';
	import type { PlotPanel } from '../screen/screen.svelte';
	import type { CardKind } from '../screen/types';
	import ShockLevelCard from './ShockLevelCard.svelte';
	import ShockPulseTable from './ShockPulseTable.svelte';
	import ShockRunVerbs from './ShockRunVerbs.svelte';
	import type { ShockStore } from './shockStore.svelte';
	import type { ShockViewsState } from './shockViews.svelte';

	interface Props {
		shock: ShockStore;
		views: ShockViewsState;
		/** The station panel. */
		station?: Snippet;
		onpopout?: (pageId: string) => boolean;
		onoverride?: () => void;
		abortKeys: string;
		compact?: boolean;
	}
	let { shock, views, station, onpopout, onoverride, abortKeys, compact = false }: Props = $props();

	const status = $derived(shock.now);
	const state = $derived(shock.run?.state ?? null);
	const streaming = $derived(state === 'running' || state === 'review' || state === 'stopping' || state === 'finished');
	const channels = $derived(shock.rows.filter((r) => r.enabled && r.role !== 'drive').map((r) => ({ index: r.index, name: r.description || r.role })));

	function empty(p: PlotPanel): string {
		if (!streaming) return 'Appears when a run starts.';
		switch (p.plot.quantity) {
			case 'pulse':
				return 'The pulse is drawn once one has fired.';
			case 'srs':
				return "The pulse's SRS is drawn once one has fired.";
			case 'velocity':
			case 'displacement':
				return 'Drawn once a pulse has fired.';
			case 'identification':
				return 'The identified system is drawn once identification is done.';
			case 'spectrum':
				return 'The spectra are drawn once a pulse has fired.';
			case 'law':
				return "The law's values are shown after its first call.";
			default:
				return 'Appears with the first frame.';
		}
	}
</script>

{#snippet verbs()}<ShockRunVerbs {shock} />{/snippet}

{#snippet cards(kind: CardKind)}
	{#if kind === 'level'}
		<LawLine law={status?.law} testid="shock-run-law" />
		<ShockLevelCard {status} planned={shock.planned} {compact} />
	{:else if kind === 'pulses'}
		<ShockPulseTable pulses={shock.pulses} picked={shock.overlay} ontoggle={(i) => void shock.toggleOverlay(i)} />
	{:else if kind === 'station' && station}
		{@render station()}
	{/if}
{/snippet}

<PulseRunView
	host={shock}
	screen={views}
	prefix="shock"
	attrs={{ 'data-state': status?.state ?? '', 'data-phase': status?.phase ?? '', 'data-pulses': status?.pulses.fired ?? 0 }}
	{verbs}
	review="The identified system and its coherence are drawn; nothing fires until you proceed."
	{cards}
	{channels}
	{empty}
	live={streaming}
	{onpopout}
	{onoverride}
	{abortKeys}
	{compact}
/>
