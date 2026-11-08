<script lang="ts">
	/**
	 * The classical shock run as it happens, in the run view's frame: the page bar, the page on
	 * screen, and beside it the run's state, the level card, the law and the station. The pages are
	 * the service's views drawn as they come: the Pulse and SRS pages lay the pulses picked in the
	 * pulse table over the latest one. During the review hold a banner says what the run waits for.
	 * The window on another monitor shows the same frame, compact.
	 */
	import type { Snippet } from 'svelte';
	import type { View } from '$lib/protocol/types';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import StateChip from '../StateChip.svelte';
	import PulsePageBar from '../pulse/PulsePageBar.svelte';
	import { overlaidViews } from '../pulse/pulseLayout';
	import ShockLevelCard from './ShockLevelCard.svelte';
	import ShockPulseTable from './ShockPulseTable.svelte';
	import { SHOCK_PAGE_HELP, shockPlotId } from './shockLayout';
	import type { ShockStore } from './shockStore.svelte';
	import type { ShockViewsState } from './shockViews.svelte';
	import { SHOCK_OUTCOME_WORDS } from './shockWords';

	interface Props {
		shock: ShockStore;
		views: ShockViewsState;
		/** The station panel, from the screen. */
		station?: Snippet;
		/** The page bar's right end (the way back to the setup). */
		end?: Snippet;
		onpopout?: (pageId: string) => boolean;
		compact?: boolean;
	}
	let { shock, views, station, end, onpopout, compact = false }: Props = $props();

	const status = $derived(shock.now);
	const page = $derived(views.page);
	let width = $state(0);

	$effect(() => {
		if (width > 0) views.setWidth(page.id, width);
	});

	const one = (q: string): View[] => views.plotViews(shockPlotId(page, q));

	/**
	 * The page's views with the picked pulses laid over its Pulse (or SRS) view; the others (the
	 * Drive view beside the Pulse) as the service sent them. Before this window's first frame, the
	 * last pulse event's views stand in.
	 */
	const overlaid = (kind: 'pulse' | 'srs'): View[] => overlaidViews(one(kind), shock.lastViews, shock.overlaid, kind);

	const chipRun = $derived(shock.run);
</script>

{#snippet draw(list: View[], min: number, empty: string)}
	{#if list.length}
		{#each list as v, i (i)}<div class="plot"><ViewRenderer view={v} live minHeight={min} /></div>{/each}
	{:else}
		<p class="aq-muted aq-small empty">{empty}</p>
	{/if}
{/snippet}

<div class="live" class:compact data-testid="shock-run-view" data-state={status?.state ?? ''} data-phase={status?.phase ?? ''} data-pulses={status?.pulses.fired ?? 0}>
	<PulsePageBar {views} prefix="shock" {end} {onpopout} />

	{#if status?.state === 'review'}
		<p class="aq-notice review" role="status" data-testid="shock-review">
			<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
			<span class="grow">Holding for review. The identified system and its coherence are on the Identification page; nothing fires until you proceed.</span>
		</p>
	{/if}

	<div class="grid">
		<div class="pageview" data-testid="shock-page-view" data-page={page.id} data-kind={page.kind} bind:clientWidth={width}>
			<p class="aq-small aq-muted help">{SHOCK_PAGE_HELP[page.kind]}</p>
			{#key page.id}
				{#if page.kind === 'pulse'}
					{@render draw(overlaid('pulse'), 260, 'The pulse is drawn once one has fired.')}
				{:else if page.kind === 'srs'}
					{@render draw(overlaid('srs'), 320, "The pulse's SRS is drawn once one has fired.")}
				{:else if page.kind === 'kinematics'}
					{@render draw([...one('velocity'), ...one('displacement')], 200, 'Velocity and displacement are drawn once a pulse has fired.')}
				{:else if page.kind === 'identification'}
					{@render draw([...one('identification'), ...one('spectrum')], 190, 'The identified system is drawn once identification is done.')}
				{:else if page.kind === 'pulses'}
					<div class="pulses">
						<ShockPulseTable pulses={shock.pulses} picked={shock.overlay} ontoggle={(i) => void shock.toggleOverlay(i)} />
						{@render draw(one('readings'), 80, '')}
					</div>
				{:else}
					{@render draw([...one('law'), ...one('readings')], 80, "The law's values are shown after its first call.")}
				{/if}
			{/key}
			{#if shock.overlay.length && (page.kind === 'pulse' || page.kind === 'srs')}
				<p class="aq-small aq-muted" data-testid="shock-overlay-note">Laid over: pulses {shock.overlay.map((i) => i + 1).join(', ')}. Pick them on the Pulses page.</p>
			{/if}
		</div>

		<aside class="side" aria-label="Run numbers">
			<section class="card head">
				<StateChip run={chipRun} words={SHOCK_OUTCOME_WORDS} />
				{#if status?.law}<span class="law" data-testid="shock-run-law">{status.law.name} <span class="v">{status.law.version}</span>{#if status.law.misses > 0}<span class="miss">, {status.law.misses} {status.law.misses === 1 ? 'miss' : 'misses'} in a row</span>{/if}</span>{/if}
				{#if status?.source}<span class="src">{status.source === 'SIM' ? 'Simulator' : status.source}</span>{/if}
			</section>
			<section class="card"><ShockLevelCard {status} planned={shock.planned} {compact} /></section>
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

	.pulses {
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
