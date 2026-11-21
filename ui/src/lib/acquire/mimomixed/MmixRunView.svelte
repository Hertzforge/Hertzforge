<script lang="ts">
	/**
	 * The MIMO Mixed Mode run as it happens, on the test screen's frame: the pages of plots, and the
	 * dock with the Run card (the shared verbs, Hold and Resume, and past a failed gate or a forecast
	 * that cannot fit, Proceed With Your Words), the parts, the drives' shared budget, the run in
	 * numbers, the log and the station. While the run holds for review, the Run card gives the gate
	 * in a line, the prediction with each drive's need against its ceiling, and a place for the
	 * operator's words. The window on another monitor shows the same, compact.
	 */
	import { untrack, type Snippet } from 'svelte';
	import PulseRunView from '../pulse/PulseRunView.svelte';
	import type { RunOpen } from '../runOpen.svelte';
	import type { PlotPanel } from '../screen/screen.svelte';
	import type { CardKind } from '../screen/types';
	import MixedLogCard from '../mixed/MixedLogCard.svelte';
	import type { MixedLogEntry } from '$lib/protocol/mixed';
	import MmixDrivesCard from './MmixDrivesCard.svelte';
	import MmixLevelCard from './MmixLevelCard.svelte';
	import MmixPartsCard from './MmixPartsCard.svelte';
	import MmixPrediction from './MmixPrediction.svelte';
	import MmixRunVerbs from './MmixRunVerbs.svelte';
	import type { MimoMixedStore } from './mmixStore.svelte';
	import type { MmixViewsState } from './mmixViews.svelte';

	interface Props {
		mm: MimoMixedStore;
		views: MmixViewsState;
		station?: Snippet;
		onpopout?: (pageId: string) => boolean;
		abortKeys: string;
		compact?: boolean;
		opener?: RunOpen | null;
	}
	let { mm, views, station, onpopout, abortKeys, compact = false, opener = null }: Props = $props();

	const status = $derived(mm.now);
	const live = $derived(mm.frameOfRun);
	const state = $derived(mm.run?.state ?? null);
	const streaming = $derived(state === 'running' || state === 'review' || state === 'stopping' || state === 'finished');
	const channels = $derived(mm.runConfig ? mm.runConfig.channels.controls.map((c, i) => ({ index: c.channel, name: mm.runControlLabels[i] ?? `Control ${c.channel}` })) : mm.setup.controls.map((c, i) => ({ index: c.channel, name: mm.controlLabels[i] })));
	const gates = $derived(status?.gates ?? null);
	const gateHold = $derived(status?.phase === 'gate_failed' || status?.phase === 'prediction_exceeded');

	// The plots' part choice offers the run's tones and narrowbands (the setup's before a run).
	$effect(() => {
		const rows = status?.parts ?? [];
		const list = rows.length
			? rows.filter((p) => p.kind !== 'floor').map((p) => ({ id: p.id, name: p.name, kind: p.kind as 'tone' | 'narrowband' }))
			: [...mm.setup.tones.map((t) => ({ id: t.id, name: t.name, kind: 'tone' as const })), ...mm.setup.bands.map((b) => ({ id: b.id, name: b.name, kind: 'narrowband' as const }))];
		untrack(() => (views.partChoices = list));
	});

	function empty(p: PlotPanel): string {
		if (!streaming) return 'Appears when a run starts.';
		switch (p.plot.quantity) {
			case 'control':
			case 'residual':
			case 'cross':
			case 'drive':
				return 'Drawn once the floor plays.';
			case 'tone_amplitude':
			case 'tone_phase':
				return 'Each tone is drawn from when it first comes in.';
			case 'band_level':
				return 'Each narrowband is drawn from when it first comes in.';
			case 'identification':
			case 'coherence':
				return 'Drawn once identification is done.';
			case 'law':
				return "The laws' values are shown after their first calls.";
			default:
				return 'Appears with the first frame.';
		}
	}
</script>

{#snippet verbs()}<MmixRunVerbs {mm} />{/snippet}

{#snippet notes()}
	{#if status?.state === 'review' && !compact}
		<div class="review" data-testid="mmix-review" data-phase={status.phase}>
			{#if gates}
				<p class="aq-small" data-testid="mmix-gate-words">
					{gates.passed ? 'The gate passed' : 'The gate did not pass'}: the worst multiple coherence is {gates.coherence_min.toFixed(2)} against {gates.coherence_floor}; the model's condition is {Math.round(gates.condition_max)} against {gates.condition_limit}.
				</p>
			{/if}
			{#if status.prediction}<MmixPrediction prediction={status.prediction} drives={mm.runDriveLabels} controls={mm.runControlLabels} testid="mmix-review-prediction" />{/if}
			<label class="lbl" for="mmix-ack">{gateHold ? 'Your words for the log (needed to proceed)' : 'Your words for the log (optional)'}</label>
			<textarea id="mmix-ack" class="field" rows="2" bind:value={mm.acknowledgement} disabled={!mm.isController} data-testid="mmix-ack"></textarea>
		</div>
	{/if}
{/snippet}

{#snippet cards(kind: CardKind)}
	{#if kind === 'level'}
		<MmixLevelCard {mm} />
	{:else if kind === 'parts'}
		<MmixPartsCard {mm} {compact} />
	{:else if kind === 'drives'}
		<MmixDrivesCard {mm} />
	{:else if kind === 'log'}
		<MixedLogCard log={mm.log as unknown as MixedLogEntry[]} />
	{:else if kind === 'station' && station}
		{@render station()}
	{/if}
{/snippet}

<PulseRunView
	host={mm}
	screen={views}
	prefix="mmix"
	attrs={{
		'data-state': status?.state ?? '',
		'data-phase': status?.phase ?? '',
		'data-t': live?.t_s ?? '',
		'data-held': status?.held ? 'true' : 'false',
		'data-limiting': status?.limiting ? 'true' : 'false',
		'data-level': live?.level_db ?? ''
	}}
	{verbs}
	review={gateHold ? 'The run holds on its gate: proceed with your words, or abort.' : 'The identification, the gate and the prediction are drawn; nothing drives at level until you proceed.'}
	{notes}
	{cards}
	{channels}
	{empty}
	live={streaming}
	{onpopout}
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

	.lbl {
		font-size: var(--text-xs);
		color: var(--text-2);
	}
</style>
