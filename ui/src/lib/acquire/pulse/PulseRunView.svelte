<script lang="ts">
	/**
	 * The run of a test type on the pulse page layer, on the test screen's frame: the page's plots
	 * on their grid with the page tabs under them, and the control dock with the run's cards. The
	 * Run card holds the verbs that apply now, the shared ones and the test type's own, then a review
	 * hold or the run's end as one line with its button, then any line of the type's own (a hold, the
	 * last verb's answer). The other cards are the type's: its level, its own panels, the station.
	 * A type whose runs leave a run folder passes its record: from arm on the Run card names the
	 * folder, and a finished run's end line offers Open in Analysis and Copy Folder Path.
	 */
	import type { Snippet } from 'svelte';
	import type { ActionId, RunBarHost } from '../actions';
	import RunOpenActions from '../RunOpenActions.svelte';
	import RunRecordLine from '../RunRecordLine.svelte';
	import type { RunOpen } from '../runOpen.svelte';
	import RunVerbs from '../RunVerbs.svelte';
	import type { PlotBodies } from '../screen/bodies';
	import Dock from '../screen/Dock.svelte';
	import PageGrid from '../screen/PageGrid.svelte';
	import PageTabs from '../screen/PageTabs.svelte';
	import type { PlotPanel, ScreenState } from '../screen/screen.svelte';
	import type { CardKind } from '../screen/types';
	import type { RunRecordRef } from '$lib/protocol/types';
	import { clock, db, OUTCOME_WORDS, outcomeTone } from '../words';

	interface Props {
		host: RunBarHost;
		screen: ScreenState;
		/** The test ids' prefix: `<prefix>-run-view`, `<prefix>-page-view`, `<prefix>-review`. */
		prefix: string;
		/** The run view's data attributes. */
		attrs?: Record<string, string | number | boolean | null | undefined>;
		/** The test type's own verbs, after the shared ones. */
		verbs?: Snippet;
		/** What the review hold waits for, while the run holds for review; null otherwise. */
		review?: string | null;
		/** Lines of the type's own in the Run card, under the hold and the end. */
		notes?: Snippet;
		/** The type's cards (every one but Run). */
		cards: Snippet<[CardKind]>;
		/** The type's own bodies for some of its plot types. */
		bodies?: PlotBodies;
		/** The run's inputs, for a plot's channel choice. */
		channels: { index: number; name: string }[];
		/** What an empty plot says. */
		empty: (p: PlotPanel) => string;
		/** The run has streamed: ranges held steady across frames. */
		live: boolean;
		/** Open a page in a window of its own; false when the browser refused. */
		onpopout?: (pageId: string) => boolean;
		/** Open the acknowledgement dialog for arming past a feasibility exceedance. */
		onoverride?: () => void;
		abortKeys: string;
		/** A window on another monitor: the plots and the cards, with Stop and Abort the only verbs. */
		compact?: boolean;
		/** The run's record, from arm on, for a type whose runs leave a run folder. */
		record?: RunRecordRef | null;
		/** A finished run's way to its data on the analysis side; absent in a window that only watches. */
		opener?: RunOpen | null;
	}
	let { host, screen, prefix, attrs = {}, verbs, review = null, notes, cards, bodies, channels, empty, live, onpopout, onoverride = () => {}, abortKeys, compact = false, record: given = null, opener = null }: Props = $props();

	const run = $derived(host.run);
	const end = $derived(run?.state === 'finished' ? run.end : null);
	const tone = $derived(end ? outcomeTone(end.outcome) : null);
	const holding = $derived(run?.state === 'review' && review !== null);
	/** Verbs the Run card draws as its hold and end lines, so not twice; Apply Setup is the setup's. */
	const omit = $derived<ActionId[]>(['apply', ...(holding ? (['proceed'] as const) : []), ...(end ? (['reset'] as const) : [])]);
	const reset = $derived(host.actions.reset);
	const proceed = $derived(host.actions.proceed);
	const endWords = $derived(end ? (host.outcomeWords?.[end.outcome] ?? OUTCOME_WORDS[end.outcome] ?? end.outcome) : '');
	const endWhen = $derived.by(() => {
		if (!end || end.at_s === null) return '';
		const level = !host.levelHidden && end.level_db !== null ? `, ${db(end.level_db)}` : '';
		return `at ${clock(end.at_s)}${level}`;
	});
	const page = $derived(screen.page);
	/** The record a caller gives, or the one its run's head carries. */
	const record = $derived(given ?? run?.record ?? null);
</script>

{#snippet card(kind: CardKind)}
	{#if kind === 'run'}
		<div class="runcard">
			<!-- The verbs first: Stop and Abort stay in view however short the card is. -->
			<RunVerbs acq={host} {onoverride} {abortKeys} {compact} {verbs} layout="card" {omit} />
			{#if record}<RunRecordLine {record} ended={!!end} />{/if}
			{#if holding}
				<div class="line hold" role="status" data-testid="{prefix}-review">
					<span class="mk" aria-hidden="true">
						<svg viewBox="0 0 14 14" width="16" height="16"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
					</span>
					<div class="what">
						<p class="head"><strong>Holding for Review</strong></p>
						<p class="msg">{review}</p>
					</div>
					{#if !compact}
						<button class="btn primary" aria-disabled={!proceed.enabled} class:off={!proceed.enabled} title={proceed.reason ?? undefined} onclick={() => proceed.enabled && host.proceed()} data-testid="review-proceed">Proceed</button>
					{/if}
				</div>
			{/if}
			{#if end}
				<div class="line end {tone}" role={tone === 'fail' ? 'alert' : 'status'} data-testid="run-end" data-outcome={end.outcome}>
					<span class="mk" aria-hidden="true">
						{#if tone === 'pass'}
							<svg viewBox="0 0 14 14" width="16" height="16"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
						{:else if tone === 'info'}
							<svg viewBox="0 0 14 14" width="16" height="16"><rect x="2.5" y="2.5" width="9" height="9" rx="1.3" fill="currentColor" /></svg>
						{:else}
							<svg viewBox="0 0 14 14" width="16" height="16"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="var(--status-fail-fill)" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.6" stroke-linecap="round" /></svg>
						{/if}
					</span>
					<div class="what">
						<p class="head">
							<strong>{endWords}</strong>
							{#if endWhen}<span class="when num">{endWhen}</span>{/if}
						</p>
					</div>
					{#if reset.visible && !compact}
						<button class="btn" class:primary={reset.enabled} data-testid="act-reset" aria-disabled={!reset.enabled} class:off={!reset.enabled} title={reset.reason ?? 'A new run from the same setup.'} onclick={() => reset.enabled && host.reset()}>New Run</button>
					{/if}
					{#if record && opener && !compact}<RunOpenActions {record} {opener} />{/if}
				</div>
			{/if}
			{#if notes}{@render notes()}{/if}
		</div>
	{:else}
		{@render cards(kind)}
	{/if}
{/snippet}

<div class="live" class:compact class:bottom={screen.dock.side === 'bottom'} data-testid="{prefix}-run-view" {...attrs}>
	<div class="area">
		<div class="pageview" data-testid="{prefix}-page-view" data-page={page.id}>
			<PageGrid {screen} {card} {channels} {empty} {live} {bodies} />
		</div>
		<PageTabs {screen} {onpopout} />
	</div>
	<Dock {screen} {card} />
</div>

<style>
	.live {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		height: 100%;
		min-height: 0;
	}

	.live.bottom {
		grid-template-columns: minmax(0, 1fr);
		grid-template-rows: minmax(0, 1fr) auto;
	}

	.area {
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
	}

	.pageview {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
	}

	.runcard {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}

	/* A hold or an end: one line, its mark, its words and its button. */
	.line {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		padding: var(--sp-2);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.line .mk {
		display: inline-flex;
		margin-top: 1px;
	}

	.line .what {
		flex: 1;
		min-width: 0;
	}

	.line .btn {
		flex: none;
	}

	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: 2px 8px;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.when {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.msg {
		margin-top: 2px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.hold {
		color: var(--blue-1);
		background: var(--blue-weak);
		border-color: var(--blue-2);
	}

	.end {
		flex-wrap: wrap;
	}

	.end.pass {
		color: var(--blue-1);
		border-color: var(--blue-line);
	}

	.end.info .mk {
		color: var(--text-2);
	}

	.end.fail {
		background: var(--amber-weak);
		border: 2px solid var(--amber-2);
	}

	.btn.off {
		opacity: 0.45;
		cursor: default;
	}
</style>
