<script lang="ts">
	/**
	 * The MIMO page bar: which view is on screen, picked from a drop-down (or stepped through), and
	 * the page's tools: rename, add a page of any of the four views, remove a page, open the page
	 * in a window of its own for another monitor, and how often the display updates. The same bar
	 * as the single-output run's, for the MIMO pages.
	 */
	import type { Snippet } from 'svelte';
	import { MIMO_PAGE_KINDS, MIMO_PAGE_WORDS, MIMO_RATES, type MimoPageKind } from '$lib/acquire/display/mimoLayout';
	import type { MimoViewsState } from './mimoViews.svelte';

	interface Props {
		views: MimoViewsState;
		onpopout?: (pageId: string) => boolean;
		end?: Snippet;
	}
	let { views, onpopout, end }: Props = $props();

	const page = $derived(views.page);
	const index = $derived(views.pages.findIndex((p) => p.id === page.id));
	let nameText = $state('');
	let removing = $state(false);
	let blocked = $state(false);
	let nameInput = $state<HTMLInputElement | null>(null);

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
		views.addPage(kind as MimoPageKind);
	}

	function popout() {
		blocked = onpopout ? !onpopout(page.id) : false;
	}
</script>

<div class="pagebar" role="toolbar" aria-label="Display pages" data-testid="mimo-page-bar">
	<div class="pages">
		<span class="eyebrow" id="mimo-pagebar-label">Page</span>
		{#if views.renaming === page.id}
			<input class="field name" aria-label="Page name" data-testid="mimo-page-name" bind:this={nameInput} bind:value={nameText} maxlength="40" onkeydown={onNameKey} onblur={commitRename} />
		{:else}
			<button class="btn ghost icon sm" aria-label="Previous page" title="Previous page" disabled={views.pages.length < 2} onclick={() => step(-1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<select class="field select" aria-labelledby="mimo-pagebar-label" data-testid="mimo-page-select" value={page.id} onchange={(e) => views.selectPage(e.currentTarget.value)}>
				{#each views.pages as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
			</select>
			<button class="btn ghost icon sm" aria-label="Next page" title="Next page" disabled={views.pages.length < 2} onclick={() => step(1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<button class="btn ghost sm" data-testid="mimo-page-rename" onclick={startRename}>Rename</button>
		{/if}
		<select class="field addsel" aria-label="Add a page" data-testid="mimo-page-add" disabled={!views.canAddPage} title={views.canAddPage ? 'Add a page' : 'At most 12 pages.'} value="" onchange={(e) => (add(e.currentTarget.value), (e.currentTarget.value = ''))}>
			<option value="">Add Page</option>
			{#each MIMO_PAGE_KINDS as k (k)}<option value={k}>{MIMO_PAGE_WORDS[k]}</option>{/each}
		</select>
		{#if removing}
			<span class="confirm" role="alert" data-testid="mimo-page-remove-confirm">
				Remove "{page.name}"?
				<button class="btn sm" data-testid="mimo-page-remove-yes" onclick={() => ((removing = false), views.removePage(page.id))}>Remove</button>
				<button class="btn ghost sm" onclick={() => (removing = false)}>Keep</button>
			</span>
		{:else}
			<button class="btn ghost sm" data-testid="mimo-page-remove" disabled={!views.canRemovePage} title={views.canRemovePage ? 'Remove this page' : 'The last page stays.'} onclick={() => (removing = true)}>Remove Page</button>
		{/if}
	</div>

	<div class="plots">
		{#if onpopout}
			<button class="btn ghost sm" data-testid="mimo-page-popout" onclick={popout} title="Open this page in a window of its own, to place on another monitor. Stop and Abort are in every window.">
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="1.5" y="3.5" width="9" height="9" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M7 1.5h5.5V7M12.5 1.5L7 7" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
				New Window
			</button>
		{/if}
		<label class="rate">
			<span class="eyebrow">Updates</span>
			<select class="field num" data-testid="mimo-page-rate" value={String(views.layout.rate)} onchange={(e) => views.setRate(Number(e.currentTarget.value))}>
				{#each MIMO_RATES as r (r)}<option value={String(r)}>{r} per s</option>{/each}
			</select>
		</label>
	</div>

	{#if end}<div class="end">{@render end()}</div>{/if}

	{#if blocked}
		<p class="blocked" role="alert">The browser did not open the window. Allow pop-up windows for this page, then try again.</p>
	{/if}
	{#if views.error}
		<p class="blocked" role="alert" data-testid="mimo-display-error">{views.error}</p>
	{/if}
</div>

<style>
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
	.plots {
		display: flex;
		flex-wrap: nowrap;
		align-items: center;
		gap: 4px;
	}

	.eyebrow {
		margin-right: 4px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.06em;
		color: var(--text-3);
		text-transform: uppercase;
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

	.confirm {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
	}

	.rate {
		display: inline-flex;
		align-items: center;
		gap: 2px;
		margin-left: var(--sp-2);
	}

	.end {
		margin-left: auto;
	}

	.blocked {
		flex-basis: 100%;
		font-size: var(--text-sm);
		color: var(--amber-1);
	}
</style>
