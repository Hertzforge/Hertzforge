<script lang="ts">
	/**
	 * The MIMO Shock run as it happens, on the test screen's frame: the pages of plots, and the dock
	 * with the Run card (the shared verbs, Fire in the manual mode, and the acknowledgement the run
	 * waits for when a control cannot reach its target), the level card with the law and the ladder,
	 * the verdict grid, the latest pulse in detail, and the station. While the scaled abort lines are
	 * off, a standing notice says so above every page. The window on another monitor shows the same,
	 * compact.
	 */
	import type { Snippet } from 'svelte';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import type { BodyPanel, PlotBodies } from '../screen/bodies';
	import LawLine from '../pulse/LawLine.svelte';
	import PulseRunView from '../pulse/PulseRunView.svelte';
	import type { PlotPanel } from '../screen/screen.svelte';
	import type { CardKind } from '../screen/types';
	import MsLevelCard from './MsLevelCard.svelte';
	import MsPulseDetail from './MsPulseDetail.svelte';
	import MsVerdictGrid from './MsVerdictGrid.svelte';
	import type { MimoShockStore } from './msStore.svelte';
	import type { MsViewsState } from './msViews.svelte';
	import { unreachableControls } from './msWords';

	interface Props {
		ms: MimoShockStore;
		views: MsViewsState;
		station?: Snippet;
		onpopout?: (pageId: string) => boolean;
		abortKeys: string;
		compact?: boolean;
	}
	let { ms, views, station, onpopout, abortKeys, compact = false }: Props = $props();

	const status = $derived(ms.now);
	const state = $derived(ms.run?.state ?? null);
	const streaming = $derived(state === 'running' || state === 'review' || state === 'stopping' || state === 'finished');
	const channels = $derived(ms.runControls.map((c) => ({ index: c, name: `Control ${c}` })));
	const unscaled = $derived(ms.unscaledNotice);
	const shown = $derived(ms.shownPulse);
	const fire = $derived(ms.fireAction);
	const holdWords = $derived.by(() => {
		const bad = unreachableControls(status?.review ?? null);
		return status?.review?.hold?.message ?? (bad.length ? `Controls ${bad.join(', ')} cannot reach their targets.` : '');
	});

	/** The quantities the service sends as tables, drawn as tables in their panels. */
	const bodies: PlotBodies = { verdicts: tableBody, ladder: tableBody, review: tableBody, pulses: tableBody };

	function empty(p: PlotPanel): string {
		if (!streaming) return 'Appears when a run starts.';
		switch (p.plot.quantity) {
			case 'pulse':
			case 'srs':
			case 'drives':
			case 'verdicts':
				return 'Drawn once a pulse has fired.';
			case 'identification':
			case 'review':
				return 'Drawn once identification is done.';
			case 'law':
				return "The law's values are shown after its first call.";
			default:
				return 'Appears with the first frame.';
		}
	}
</script>

{#snippet tableBody(panel: BodyPanel)}
	{@const tables = views.views(panel.id).filter((v) => v.view === 'table')}
	<div class="tables" data-testid="ms-table-{panel.plot.quantity}">
		{#if tables.length}
			{#each tables as v, i (i)}<ViewRenderer view={v} showTitle={tables.length > 1} />{/each}
		{:else}
			<p class="aq-small aq-muted empty">{empty(panel as unknown as PlotPanel)}</p>
		{/if}
	</div>
{/snippet}

{#snippet verbs()}
	{#if fire.visible}
		<button class="btn primary" data-testid="act-fire" aria-disabled={!fire.enabled} class:off={!fire.enabled} title={fire.reason ?? 'Fire the next pulse at the level in force.'} onclick={() => fire.enabled && ms.fire()}>Fire</button>
	{/if}
{/snippet}

{#snippet notes()}
	{#if ms.mustAcknowledge}
		<div class="ack" role="alert" data-testid="ms-ack">
			<p class="head">
				<svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
				<strong>Holding: A Target Cannot Be Reached</strong>
			</p>
			<p class="msg" data-testid="ms-ack-message">{holdWords}</p>
			{#if !compact}
				<label class="lbl" for="ms-ack-text">Why the run should go on, in your own words (kept in the log)</label>
				<textarea id="ms-ack-text" class="field" rows="2" bind:value={ms.acknowledgement} disabled={!ms.isController} data-testid="ms-ack-text"></textarea>
				<button class="btn primary" aria-disabled={!ms.actions.proceed.enabled} class:off={!ms.actions.proceed.enabled} title={ms.actions.proceed.reason ?? undefined} onclick={() => ms.actions.proceed.enabled && ms.proceed()} data-testid="ms-ack-proceed">Proceed With This Acknowledgement</button>
			{/if}
		</div>
	{/if}
{/snippet}

{#snippet cards(kind: CardKind)}
	{#if kind === 'level'}
		<LawLine law={status?.law} testid="ms-run-law" />
		<MsLevelCard {status} rows={ms.pulses} {compact} />
	{:else if kind === 'verdicts'}
		<MsVerdictGrid rows={ms.pulses} controls={ms.runControls} shown={ms.picked} onpick={(i) => ms.pick(i)} />
	{:else if kind === 'pulse'}
		<MsPulseDetail row={shown} windowS={ms.windowS} latest={ms.picked === null} onlatest={() => ms.pick(null)} />
	{:else if kind === 'station' && station}
		{@render station()}
	{/if}
{/snippet}

<div class="msrun">
	{#if unscaled}
		<p class="notice" role="status" data-testid="ms-unscaled-notice">
			<svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /><path d="M7 5.4v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /><circle cx="7" cy="10.1" r="0.85" fill="currentColor" /></svg>
			{unscaled}
		</p>
	{/if}
	<div class="fill">
		<PulseRunView
			host={ms}
			screen={views}
			prefix="ms"
			attrs={{ 'data-state': status?.state ?? '', 'data-phase': status?.phase ?? '', 'data-pulses': status?.pulses.fired ?? 0 }}
			{verbs}
			review={ms.mustAcknowledge ? null : 'Identification is done and each control\'s reachable floor is on the level card; nothing fires until you proceed.'}
			{notes}
			{cards}
			{bodies}
			{channels}
			{empty}
			live={streaming}
			{onpopout}
			{abortKeys}
			{compact}
		/>
	</div>
</div>

<style>
	.msrun {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
	}

	.fill {
		flex: 1;
		min-height: 0;
	}

	.tables {
		height: 100%;
		overflow: auto;
		padding: 4px var(--sp-2);
	}

	.empty {
		display: grid;
		place-items: center;
		height: 100%;
		margin: 0;
	}

	.notice {
		display: flex;
		align-items: center;
		gap: 6px;
		margin: 0 0 4px;
		padding: 4px var(--sp-3);
		font-size: var(--text-sm);
		color: var(--text-1);
		background: var(--amber-weak);
		border: 1px solid var(--amber-2);
		border-radius: var(--radius-md);
	}

	.ack {
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding: var(--sp-2);
		border: 2px solid var(--amber-2);
		border-radius: var(--radius-md);
	}

	.ack p {
		margin: 0;
	}

	.ack .head {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
	}

	.msg {
		font-size: var(--text-sm);
	}

	.lbl {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.btn.off {
		opacity: 0.55;
		cursor: default;
	}
</style>
