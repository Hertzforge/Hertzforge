<script lang="ts">
	/**
	 * The TWR run as it happens, in the run view's frame: the page bar, the page on screen, and beside
	 * it the run's state, the law, the level card and the station. The pages are the service's views
	 * drawn as they come: the last play's records against time over the record, the identification
	 * against frequency, each play's error against the play index. During the review hold and the
	 * coherence hold a banner says what the run waits for. The window on another monitor shows the
	 * same frame, compact.
	 */
	import type { Snippet } from 'svelte';
	import type { View } from '$lib/protocol/types';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import StateChip from '../StateChip.svelte';
	import PulsePageBar from '../pulse/PulsePageBar.svelte';
	import TwrLevelCard from './TwrLevelCard.svelte';
	import TwrPlayTable from './TwrPlayTable.svelte';
	import { twrPlotId, TWR_PAGE_HELP } from './twrLayout';
	import type { TwrStore } from './twrStore.svelte';
	import type { TwrViewsState } from './twrViews.svelte';
	import { TWR_OUTCOME_WORDS, TWR_STATE_WORDS } from './twrWords';

	interface Props {
		twr: TwrStore;
		views: TwrViewsState;
		/** The station panel, from the screen. */
		station?: Snippet;
		/** The page bar's right end (the way back to the setup). */
		end?: Snippet;
		onpopout?: (pageId: string) => boolean;
		compact?: boolean;
	}
	let { twr, views, station, end, onpopout, compact = false }: Props = $props();

	const status = $derived(twr.now);
	const page = $derived(views.page);
	let width = $state(0);

	$effect(() => {
		if (width > 0) views.setWidth(page.id, width);
	});

	const one = (q: string): View[] => views.plotViews(twrPlotId(page, q));
	const chipRun = $derived(twr.run);
	const progress = $derived(status?.play && status.play.frames > 0 ? status.play.frames_done / status.play.frames : null);
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
	data-testid="twr-run-view"
	data-state={status?.state ?? ''}
	data-phase={status?.phase ?? ''}
	data-plays={status?.plays.done ?? 0}
	data-t={status?.t_s ?? ''}
	data-level={status?.level_db ?? ''}
	data-progress={progress === null ? '' : progress.toFixed(4)}
	data-frozen={status?.frozen ? 'true' : 'false'}
>
	<PulsePageBar {views} prefix="twr" {end} {onpopout} />

	{#if status?.state === 'review'}
		<p class="aq-notice review" role="status" data-testid="twr-review">
			<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
			<span class="grow">Holding for review. The identified system, its coherence and the bins the correction trusts are on the Identification page; nothing plays until you proceed.</span>
		</p>
	{:else if status?.state === 'held'}
		<p class="aq-notice warn review" role="status" data-testid="twr-held">
			<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /><path d="M7 5.4v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
			<span class="grow">Held: the identification's mean coherence stayed under its floor, so nothing drives. New Run releases the hold; raise the identification drive in the setup first if it was too low.</span>
		</p>
	{/if}

	<div class="grid">
		<div class="pageview" data-testid="twr-page-view" data-page={page.id} data-kind={page.kind} bind:clientWidth={width}>
			<p class="aq-small aq-muted help">{TWR_PAGE_HELP[page.kind]}</p>
			{#key page.id}
				{#if page.kind === 'replication'}
					{@render draw(one('response'), 300, 'The response is drawn once a play has been judged.')}
				{:else if page.kind === 'error'}
					{@render draw(one('error'), 260, 'The error is drawn once a play has been judged.')}
				{:else if page.kind === 'spectra'}
					{@render draw(one('spectra'), 300, 'The spectra are drawn once a play has been judged.')}
				{:else if page.kind === 'identification'}
					{@render draw([...one('frf'), ...one('coherence'), ...one('mask')], 170, 'The identified system is drawn once identification is done.')}
				{:else if page.kind === 'convergence'}
					{@render draw(one('convergence'), 280, "Each play's error is drawn once a play has been judged.")}
				{:else if page.kind === 'law'}
					{@render draw([...one('law'), ...one('readings')], 80, "The law's values are shown after its first call.")}
				{:else if page.kind === 'plays'}
					<div class="plays">
						<TwrPlayTable plays={twr.plays} law={twr.runLaw} record={twr.record} busy={twr.recordBusy} ontoggle={(i) => void twr.toggleRecord(i)} />
						{@render draw(one('readings'), 80, '')}
					</div>
				{:else if page.kind === 'target'}
					{@render draw(one('target'), 260, 'The target is drawn once the run has a level.')}
				{:else if page.kind === 'drive'}
					{@render draw(one('drive'), 260, 'The drive is drawn once a play has been judged.')}
				{:else if page.kind === 'envelope'}
					{@render draw(one('envelope'), 300, 'The envelope is drawn once a play has been judged.')}
				{:else}
					{@render draw(one('scope'), 260, 'The control channel is drawn while a play is in flight.')}
				{/if}
			{/key}
		</div>

		<aside class="side" aria-label="Run numbers">
			<section class="card head">
				<StateChip run={chipRun} words={TWR_OUTCOME_WORDS} stateWords={TWR_STATE_WORDS} />
				{#if status?.law}<span class="law" data-testid="twr-run-law">{status.law.name} <span class="v">{status.law.version}</span>{#if status.law.misses > 0}<span class="miss">, {status.law.misses} {status.law.misses === 1 ? 'miss' : 'misses'} in a row</span>{/if}</span>{/if}
				{#if status?.source}<span class="src">{status.source === 'SIM' ? 'Simulator' : status.source}</span>{/if}
			</section>
			<section class="card"><TwrLevelCard {status} gatePct={twr.gatePct} {compact} /></section>
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

	.review {
		margin: 0;
	}

	.grid {
		display: grid;
		flex: 1;
		grid-template-columns: minmax(0, 1fr) 320px;
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

	.help {
		margin: 0;
	}

	.plot {
		min-width: 0;
	}

	.empty {
		margin: var(--sp-4) 0;
	}

	.plays {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-height: 0;
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

	.law {
		color: var(--text-1);
	}

	.v {
		color: var(--text-3);
	}

	.miss {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.src {
		margin-left: auto;
	}

	@media (max-width: 1100px) {
		.grid {
			grid-template-columns: 1fr;
		}
	}
</style>
