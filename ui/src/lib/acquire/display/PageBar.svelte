<script lang="ts">
	/**
	 * The page bar: which page is on screen, picked from a drop-down (or stepped through), and the
	 * page's tools: rename, add and remove pages, add a plot, open the page in a window of its own
	 * for another monitor, and how often the display updates.
	 */
	import type { Snippet } from 'svelte';
	import type { DisplayState } from './display.svelte';
	import { RATES } from './layout';

	interface Props {
		display: DisplayState;
		/** Open a page in its own window; false when the browser refused to open it. */
		onpopout?: (pageId: string) => boolean;
		/** Extra controls at the right end (the main window's way back to the setup). */
		end?: Snippet;
	}
	let { display, onpopout, end }: Props = $props();

	const page = $derived(display.page);
	const index = $derived(display.pages.findIndex((p) => p.id === page.id));
	let nameText = $state('');
	let removing = $state(false);
	let blocked = $state(false);
	let nameInput = $state<HTMLInputElement | null>(null);

	function startRename() {
		nameText = page.name;
		display.renaming = page.id;
		removing = false;
		queueMicrotask(() => nameInput?.select());
	}

	function commitRename() {
		if (display.renaming && display.renamePage(display.renaming, nameText)) display.renaming = null;
	}

	function onNameKey(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			commitRename();
		} else if (e.key === 'Escape') {
			e.preventDefault();
			display.renaming = null;
		}
	}

	function step(by: number) {
		const n = display.pages.length;
		display.selectPage(display.pages[(index + by + n) % n].id);
	}

	function addPage() {
		removing = false;
		const id = display.addPage();
		if (id) startRename();
	}

	function popout() {
		blocked = onpopout ? !onpopout(page.id) : false;
	}
</script>

<div class="pagebar" role="toolbar" aria-label="Display pages" data-testid="page-bar">
	<div class="pages">
		<span class="eyebrow" id="pagebar-label">Page</span>
		{#if display.renaming === page.id}
			<input
				class="field name"
				aria-label="Page name"
				data-testid="page-name"
				bind:this={nameInput}
				bind:value={nameText}
				maxlength="40"
				onkeydown={onNameKey}
				onblur={commitRename}
			/>
		{:else}
			<button class="btn ghost icon sm" aria-label="Previous page" title="Previous page" disabled={display.pages.length < 2} onclick={() => step(-1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<select class="field select" aria-labelledby="pagebar-label" data-testid="page-select" value={page.id} onchange={(e) => display.selectPage(e.currentTarget.value)}>
				{#each display.pages as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
			</select>
			<button class="btn ghost icon sm" aria-label="Next page" title="Next page" disabled={display.pages.length < 2} onclick={() => step(1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<button class="btn ghost sm" data-testid="page-rename" onclick={startRename}>Rename</button>
		{/if}
		<button class="btn ghost sm" data-testid="page-add" disabled={!display.canAddPage} title={display.canAddPage ? 'Add a page' : 'At most 12 pages.'} onclick={addPage}>
			<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
			Page
		</button>
		{#if removing}
			<span class="confirm" role="alert" data-testid="page-remove-confirm">
				Remove "{page.name}" and its plots?
				<button class="btn sm" data-testid="page-remove-yes" onclick={() => ((removing = false), display.removePage(page.id))}>Remove</button>
				<button class="btn ghost sm" onclick={() => (removing = false)}>Keep</button>
			</span>
		{:else}
			<button class="btn ghost sm" data-testid="page-remove" disabled={!display.canRemovePage} title={display.canRemovePage ? 'Remove this page' : 'The last page stays.'} onclick={() => (removing = true)}>Remove Page</button>
		{/if}
	</div>

	<div class="plots">
		<button class="btn sm" data-testid="plot-add" disabled={!display.canAddPlot} title={display.canAddPlot ? 'Add a plot to this page' : 'At most 12 plots on a page.'} onclick={() => display.addPlot()}>
			<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
			Plot
		</button>
		{#if onpopout}
			<button class="btn ghost sm" data-testid="page-popout" onclick={popout} title="Open this page in a window of its own, to place on another monitor. Stop and Abort are in every window.">
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="1.5" y="3.5" width="9" height="9" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M7 1.5h5.5V7M12.5 1.5L7 7" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
				New Window
			</button>
		{/if}
		<label class="rate">
			<span class="eyebrow">Updates</span>
			<select class="field num" data-testid="page-rate" value={String(display.layout.rate)} onchange={(e) => display.setRate(Number(e.currentTarget.value))}>
				{#each RATES as r (r)}<option value={String(r)}>{r} per s</option>{/each}
			</select>
		</label>
	</div>

	{#if end}<div class="end">{@render end()}</div>{/if}

	{#if blocked}
		<p class="blocked" role="alert">The browser did not open the window. Allow pop-up windows for this page, then try again.</p>
	{/if}
	{#if display.error}
		<p class="blocked" role="alert" data-testid="display-error">{display.error}</p>
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
		text-transform: uppercase;
		color: var(--text-3);
	}

	.pagebar .field {
		width: auto;
		height: 28px;
	}

	.select {
		min-width: 12em;
		max-width: 22em;
		font-weight: var(--weight-strong);
	}

	.pagebar .name {
		width: 14em;
	}

	.confirm {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 2px 4px 2px 8px;
		font-size: var(--text-xs);
		color: var(--text-1);
		background: var(--amber-weak);
		border: 1px dashed var(--amber-2);
		border-radius: var(--radius-md);
	}

	.rate {
		display: inline-flex;
		align-items: center;
		margin-left: var(--sp-2);
	}

	.end {
		display: flex;
		align-items: center;
		gap: 4px;
		margin-left: auto;
	}

	.blocked {
		flex-basis: 100%;
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}
</style>
