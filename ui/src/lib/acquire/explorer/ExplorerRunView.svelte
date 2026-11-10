<script lang="ts">
	/**
	 * The Explorer run as it happens, in the run view's frame: the page bar, the page on screen, and
	 * beside it the run's state, the level card, the steering panel and the station. The pages are
	 * the service's views drawn as they come; on the Survey page the shell marks the tone's
	 * frequency and the survey's highest point from the run's own numbers. The window on another
	 * monitor shows the same frame, compact, without the steering.
	 */
	import type { Snippet } from 'svelte';
	import type { SpectrumView, View } from '$lib/protocol/types';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import { formatNumber } from '$lib/viz/format';
	import StateChip from '../StateChip.svelte';
	import PulsePageBar from '../pulse/PulsePageBar.svelte';
	import ExplorerLevelCard from './ExplorerLevelCard.svelte';
	import ExplorerSteer from './ExplorerSteer.svelte';
	import { explorerPlotId, EXPLORER_PAGE_HELP } from './explorerLayout';
	import type { ExplorerStore } from './explorerStore.svelte';
	import type { ExplorerViewsState } from './explorerViews.svelte';
	import { EXPLORER_OUTCOME_WORDS, EXPLORER_STATE_WORDS, hz } from './explorerWords';

	interface Props {
		explorer: ExplorerStore;
		views: ExplorerViewsState;
		/** The station panel, from the screen. */
		station?: Snippet;
		/** The page bar's right end (the way back to the setup). */
		end?: Snippet;
		onpopout?: (pageId: string) => boolean;
		compact?: boolean;
	}
	let { explorer, views, station, end, onpopout, compact = false }: Props = $props();

	const status = $derived(explorer.now);
	const live = $derived(explorer.frameOfRun);
	const page = $derived(views.page);
	let width = $state(0);

	$effect(() => {
		if (width > 0) views.setWidth(page.id, width);
	});

	const one = (q: string): View[] => views.plotViews(explorerPlotId(page, q));
	const chipRun = $derived(explorer.run);
	const surveyView = $derived((one('survey').find((v) => v.view === 'spectrum') as SpectrumView | undefined) ?? null);
	const peak = $derived(live?.peak ?? null);
	const unit = $derived(surveyView?.y.unit ?? live?.channels.find((c) => c.index === live?.control?.index)?.unit ?? '');
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
	data-testid="explorer-run-view"
	data-state={status?.state ?? ''}
	data-phase={status?.phase ?? ''}
	data-hz={status?.frequency_hz ?? ''}
	data-vpk={status?.drive_vpk ?? ''}
	data-t={live?.t_s ?? ''}
	data-sweep={status?.sweep ? `${status.sweep.direction}${status.sweep.halted ? ' halted' : ''}` : ''}
>
	<PulsePageBar {views} prefix="explorer" {end} {onpopout} />

	{#if status?.stall}
		<p class="aq-notice warn" role="alert" data-testid="explorer-stall">
			<span class="grow">{status.stall.message} Abort stays open.</span>
		</p>
	{/if}

	<div class="grid">
		<div class="pageview" data-testid="explorer-page-view" data-page={page.id} data-kind={page.kind} bind:clientWidth={width}>
			<p class="aq-small aq-muted help">{EXPLORER_PAGE_HELP[page.kind]}</p>
			{#key page.id}
				{#if page.kind === 'survey'}
					{#if surveyView && surveyView.series.length}
						<div class="plot" data-testid="explorer-survey-plot" data-points={surveyView.series[0].x.length}>
							<SpectrumPane view={surveyView} live minHeight={300} marker={status?.frequency_hz ?? null} guides={peak ? [peak.hz] : []} />
						</div>
						<p class="aq-small marks" data-testid="explorer-survey-marks">
							The line at {hz(status?.frequency_hz)} is the tone{#if peak}; the thin line at {hz(peak.hz)} is the highest point, {formatNumber(peak.level, 4)} {unit}{/if}.
						</p>
					{:else}
						<p class="aq-muted aq-small empty">The survey line is drawn once the tone has run at a level for a moment at one frequency.</p>
					{/if}
				{:else if page.kind === 'spectra'}
					{@render draw(one('spectrum'), 180, 'The spectra are drawn after the first analysis pass.')}
				{:else if page.kind === 'time'}
					{@render draw(one('time'), 200, 'The samples are drawn after the first analysis pass.')}
				{:else}
					{@render draw(one('readings'), 80, 'The readings are shown after the first analysis pass.')}
				{/if}
			{/key}
		</div>

		<aside class="side" aria-label="Run numbers">
			<section class="card head">
				<StateChip run={chipRun} words={EXPLORER_OUTCOME_WORDS} stateWords={EXPLORER_STATE_WORDS} />
				{#if status?.grid}<span class="grid-words num" data-testid="explorer-run-grid">{formatNumber(status.grid.fs_hz, 6)} Hz{status.grid.actual && status.grid.fs_hz !== status.grid.requested_fs_hz ? ` (asked ${formatNumber(status.grid.requested_fs_hz, 6)})` : ''}</span>{/if}
				{#if status?.source}<span class="src">{status.source === 'SIM' ? 'Simulator' : status.source}</span>{/if}
			</section>
			<section class="card"><ExplorerLevelCard {status} {live} {compact} /></section>
			{#if !compact}
				<section class="card"><ExplorerSteer {explorer} /></section>
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

	.grow {
		flex: 1;
	}

	@media (max-width: 1100px) {
		.grid {
			grid-template-columns: 1fr;
		}
	}
</style>
