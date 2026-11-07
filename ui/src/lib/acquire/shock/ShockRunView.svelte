<script lang="ts">
	/**
	 * The classical shock run as it happens, in the run view's frame: the page bar, the page on
	 * screen, and beside it the run's state, the level card, the law and the station. The pages are
	 * the service's views drawn as they come: the Pulse and SRS pages lay the pulses picked in the
	 * pulse table over the latest one. During the review hold a banner says what the run waits for.
	 * The window on another monitor shows the same frame, compact.
	 */
	import type { Snippet } from 'svelte';
	import type { SpectrumView, View } from '$lib/protocol/types';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import StateChip from '../StateChip.svelte';
	import ShockLevelCard from './ShockLevelCard.svelte';
	import ShockPulseTable from './ShockPulseTable.svelte';
	import { MAX_SHOCK_PAGES, SHOCK_PAGE_HELP, SHOCK_PAGE_KINDS, SHOCK_PAGE_WORDS, SHOCK_RATES, pulseViewOf, shockPlotId, withOverlay, type ShockPageKind } from './shockLayout';
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
	const index = $derived(views.pages.findIndex((p) => p.id === page.id));
	let nameText = $state('');
	let removing = $state(false);
	let blocked = $state(false);
	let nameInput = $state<HTMLInputElement | null>(null);
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
	function overlaid(kind: 'pulse' | 'srs'): View[] {
		let got = one(kind);
		if (!got.length) {
			const base = pulseViewOf(shock.lastViews, kind);
			const drive = kind === 'pulse' ? shock.lastViews.find((v) => v.view === 'spectrum' && /^drive\b/i.test(v.title)) : undefined;
			got = [base, drive].filter((v): v is SpectrumView => !!v);
		}
		const target = pulseViewOf(got, kind);
		return got.map((v) => (v === target ? (withOverlay(target, shock.overlaid, kind) ?? v) : v));
	}

	function startRename() {
		nameText = page.name;
		views.renaming = page.id;
		removing = false;
		queueMicrotask(() => nameInput?.select());
	}

	function commitRename() {
		if (views.renaming && views.renamePage(views.renaming, nameText)) views.renaming = null;
	}

	function onNameKey(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			commitRename();
		} else if (e.key === 'Escape') {
			e.preventDefault();
			views.renaming = null;
		}
	}

	function step(by: number) {
		const n = views.pages.length;
		views.selectPage(views.pages[(index + by + n) % n].id);
	}

	function add(kind: string) {
		if (!kind) return;
		removing = false;
		views.addPage(kind as ShockPageKind);
	}

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
	<div class="pagebar" role="toolbar" aria-label="Display pages" data-testid="shock-page-bar">
		<div class="pages">
			<span class="eyebrow" id="shock-pagebar-label">Page</span>
			{#if views.renaming === page.id}
				<input class="field name" aria-label="Page name" data-testid="shock-page-name" bind:this={nameInput} bind:value={nameText} maxlength="40" onkeydown={onNameKey} onblur={commitRename} />
			{:else}
				<button class="btn ghost icon sm" aria-label="Previous page" title="Previous page" disabled={views.pages.length < 2} onclick={() => step(-1)}>
					<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</button>
				<select class="field select" aria-labelledby="shock-pagebar-label" data-testid="shock-page-select" value={page.id} onchange={(e) => views.selectPage(e.currentTarget.value)}>
					{#each views.pages as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
				</select>
				<button class="btn ghost icon sm" aria-label="Next page" title="Next page" disabled={views.pages.length < 2} onclick={() => step(1)}>
					<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</button>
				<button class="btn ghost sm" data-testid="shock-page-rename" onclick={startRename}>Rename</button>
			{/if}
			<select class="field addsel" aria-label="Add a page" data-testid="shock-page-add" disabled={!views.canAddPage} title={views.canAddPage ? 'Add a page' : `At most ${MAX_SHOCK_PAGES} pages.`} value="" onchange={(e) => (add(e.currentTarget.value), (e.currentTarget.value = ''))}>
				<option value="">Add Page</option>
				{#each SHOCK_PAGE_KINDS as k (k)}<option value={k}>{SHOCK_PAGE_WORDS[k]}</option>{/each}
			</select>
			{#if removing}
				<span class="confirm" role="alert">
					Remove "{page.name}"?
					<button class="btn sm" data-testid="shock-page-remove-yes" onclick={() => ((removing = false), views.removePage(page.id))}>Remove</button>
					<button class="btn ghost sm" onclick={() => (removing = false)}>Keep</button>
				</span>
			{:else}
				<button class="btn ghost sm" data-testid="shock-page-remove" disabled={!views.canRemovePage} title={views.canRemovePage ? 'Remove this page' : 'The last page stays.'} onclick={() => (removing = true)}>Remove Page</button>
			{/if}
		</div>
		<div class="tools">
			{#if onpopout}
				<button class="btn ghost sm" data-testid="shock-page-popout" onclick={() => (blocked = !onpopout(page.id))} title="Open this page in a window of its own, to place on another monitor. Stop and Abort are in every window.">
					<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="1.5" y="3.5" width="9" height="9" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M7 1.5h5.5V7M12.5 1.5L7 7" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
					New Window
				</button>
			{/if}
			<label class="rate">
				<span class="eyebrow">Updates</span>
				<select class="field num" data-testid="shock-page-rate" value={String(views.layout.rate)} onchange={(e) => views.setRate(Number(e.currentTarget.value))}>
					{#each SHOCK_RATES as r (r)}<option value={String(r)}>{r} per s</option>{/each}
				</select>
			</label>
		</div>
		{#if end}<div class="end">{@render end()}</div>{/if}
		{#if blocked}<p class="blocked" role="alert">The browser did not open the window. Allow pop-up windows for this page, then try again.</p>{/if}
		{#if views.error}<p class="blocked" role="alert" data-testid="shock-display-error">{views.error}</p>{/if}
	</div>

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

	.pagebar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-4);
		padding: 6px var(--sp-2);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.pages,
	.tools {
		display: flex;
		flex-wrap: nowrap;
		align-items: center;
		gap: 4px;
	}

	.select {
		min-width: 12em;
	}

	.addsel {
		width: auto;
	}

	.name {
		width: 14em;
	}

	.end {
		margin-left: auto;
	}

	.eyebrow {
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.rate {
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}

	.confirm {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
	}

	.blocked {
		flex-basis: 100%;
		margin: 0;
		font-size: var(--text-xs);
		color: var(--amber-1);
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
