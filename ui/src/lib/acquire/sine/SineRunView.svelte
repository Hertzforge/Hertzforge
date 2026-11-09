<script lang="ts">
	/**
	 * The Sine run as it happens, in the run view's frame: the page bar, the page on screen, and
	 * beside it the run's state, the level card, the law's parameters and the station. The pages are
	 * the service's views drawn as they come: against frequency on the sweep grid in a sweep or a
	 * step test, against time in a dwell. During the review hold a banner says what the run waits
	 * for. The window on another monitor shows the same frame, compact.
	 */
	import type { Snippet } from 'svelte';
	import type { View } from '$lib/protocol/types';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import StateChip from '../StateChip.svelte';
	import PulsePageBar from '../pulse/PulsePageBar.svelte';
	import SineLevelCard from './SineLevelCard.svelte';
	import SineLiveParams from './SineLiveParams.svelte';
	import { sinePlotId, SINE_PAGE_HELP } from './sineLayout';
	import type { SineStore } from './sineStore.svelte';
	import type { SineViewsState } from './sineViews.svelte';
	import { SINE_OUTCOME_WORDS } from './sineWords';

	interface Props {
		sine: SineStore;
		views: SineViewsState;
		/** The station panel, from the screen. */
		station?: Snippet;
		/** The page bar's right end (the way back to the setup). */
		end?: Snippet;
		onpopout?: (pageId: string) => boolean;
		compact?: boolean;
	}
	let { sine, views, station, end, onpopout, compact = false }: Props = $props();

	const status = $derived(sine.now);
	const live = $derived(sine.frameOfRun);
	const page = $derived(views.page);
	let width = $state(0);

	$effect(() => {
		if (width > 0) views.setWidth(page.id, width);
	});

	const one = (q: string): View[] => views.plotViews(sinePlotId(page, q));
	const chipRun = $derived(sine.run);
	const limits = $derived(!!sine.runConfig?.limits.channels?.length);
	const dwell = $derived(status?.test === 'dwell' || status?.test === 'rstd');
	const against = $derived(dwell ? 'against time, the last minute' : 'against frequency');
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
	data-testid="sine-run-view"
	data-state={status?.state ?? ''}
	data-phase={status?.phase ?? ''}
	data-hz={live?.frequency_hz ?? status?.frequency_hz ?? ''}
	data-t={live?.t_s ?? ''}
	data-frame-held={live ? String(live.held) : ''}
	data-held={status?.held ? 'true' : 'false'}
	data-direction={status?.direction ?? ''}
	data-level={live?.level_db ?? ''}
>
	<PulsePageBar {views} prefix="sine" {end} {onpopout} />

	{#if status?.state === 'review'}
		<p class="aq-notice review" role="status" data-testid="sine-review">
			<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
			<span class="grow">Holding for review. The identified gain and its coherence are on the Identification page; nothing drives until you proceed.</span>
		</p>
	{/if}

	<div class="grid">
		<div class="pageview" data-testid="sine-page-view" data-page={page.id} data-kind={page.kind} bind:clientWidth={width}>
			<p class="aq-small aq-muted help">{SINE_PAGE_HELP[page.kind]}{page.kind !== 'identification' && page.kind !== 'law' && page.kind !== 'time' ? ` Drawn ${against}.` : ''}</p>
			{#key page.id}
				{#if page.kind === 'control'}
					{@render draw([...one('control'), ...one('drive')], 240, 'The control amplitude is drawn once the tone engages.')}
				{:else if page.kind === 'limits'}
					{@render draw(one('response'), 200, limits ? 'Each limit channel is drawn once the tone engages.' : 'This run has no limit channels.')}
				{:else if page.kind === 'responses'}
					{@render draw([...one('response'), ...one('phase')], 180, 'Every input is drawn once the tone engages.')}
				{:else if page.kind === 'identification'}
					{@render draw(one('identification'), 200, 'The identified gain is drawn once identification is done.')}
				{:else if page.kind === 'phase'}
					{@render draw(one('phase'), 180, 'The phases are drawn once the tone engages.')}
				{:else if page.kind === 'distortion'}
					{@render draw(one('thd'), 200, 'The distortion is drawn once the tone engages.')}
				{:else if page.kind === 'time'}
					{@render draw(one('time'), 160, 'The samples are drawn once the tone engages.')}
				{:else}
					{@render draw([...one('law'), ...one('readings')], 80, "The law's values are shown after its first call.")}
				{/if}
			{/key}
		</div>

		<aside class="side" aria-label="Run numbers">
			<section class="card head">
				<StateChip run={chipRun} words={SINE_OUTCOME_WORDS} />
				{#if status?.law}<span class="law" data-testid="sine-run-law">{status.law.name} <span class="v">{status.law.version}</span>{#if status.law.misses > 0}<span class="miss">, {status.law.misses} {status.law.misses === 1 ? 'miss' : 'misses'} in a row</span>{/if}</span>{/if}
				{#if status?.source}<span class="src">{status.source === 'SIM' ? 'Simulator' : status.source}</span>{/if}
			</section>
			<section class="card"><SineLevelCard {status} {live} config={sine.runConfig} {compact} /></section>
			{#if !compact}
				<section class="card"><SineLiveParams {sine} /></section>
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
