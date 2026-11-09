<script lang="ts" generics="K extends string">
	/**
	 * The page bar of a pulse-by-pulse run view (classical shock, SRS shock): the page on screen and
	 * the way to the others, renaming, adding and removing pages, opening a page in a window of its
	 * own, and how often the window asks for frames. The pages are the test type's own.
	 */
	import type { Snippet } from 'svelte';
	import type { PulseViewsState } from './pulseViews.svelte';

	interface Props {
		views: PulseViewsState<K, string>;
		/** The test ids' prefix (`shock` gives `shock-page-select`). */
		prefix: string;
		/** The page bar's right end (the way back to the setup). */
		end?: Snippet;
		onpopout?: (pageId: string) => boolean;
	}
	let { views, prefix, end, onpopout }: Props = $props();

	const set = $derived(views.wire.pages);
	const page = $derived(views.page);
	const index = $derived(views.pages.findIndex((p) => p.id === page.id));
	let nameText = $state('');
	let removing = $state(false);
	let blocked = $state(false);
	let nameInput = $state<HTMLInputElement | null>(null);
	const labelId = $derived(`${prefix}-pagebar-label`);

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
		views.addPage(kind as K);
	}
</script>

<div class="pagebar" role="toolbar" aria-label="Display pages" data-testid="{prefix}-page-bar">
	<div class="pages">
		<span class="eyebrow" id={labelId}>Page</span>
		{#if views.renaming === page.id}
			<input class="field name" aria-label="Page name" data-testid="{prefix}-page-name" bind:this={nameInput} bind:value={nameText} maxlength="40" onkeydown={onNameKey} onblur={commitRename} />
		{:else}
			<button class="btn ghost icon sm" aria-label="Previous page" title="Previous page" disabled={views.pages.length < 2} onclick={() => step(-1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<select class="field select" aria-labelledby={labelId} data-testid="{prefix}-page-select" value={page.id} onchange={(e) => views.selectPage(e.currentTarget.value)}>
				{#each views.pages as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
			</select>
			<button class="btn ghost icon sm" aria-label="Next page" title="Next page" disabled={views.pages.length < 2} onclick={() => step(1)}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			<button class="btn ghost sm" data-testid="{prefix}-page-rename" onclick={startRename}>Rename</button>
		{/if}
		<select class="field addsel" aria-label="Add a page" data-testid="{prefix}-page-add" disabled={!views.canAddPage} title={views.canAddPage ? 'Add a page' : `At most ${set.maxPages} pages.`} value="" onchange={(e) => (add(e.currentTarget.value), (e.currentTarget.value = ''))}>
			<option value="">Add Page</option>
			{#each set.kinds as k (k)}<option value={k}>{set.words[k]}</option>{/each}
		</select>
		{#if removing}
			<span class="confirm" role="alert">
				Remove "{page.name}"?
				<button class="btn sm" data-testid="{prefix}-page-remove-yes" onclick={() => ((removing = false), views.removePage(page.id))}>Remove</button>
				<button class="btn ghost sm" onclick={() => (removing = false)}>Keep</button>
			</span>
		{:else}
			<button class="btn ghost sm" data-testid="{prefix}-page-remove" disabled={!views.canRemovePage} title={views.canRemovePage ? 'Remove this page' : 'The last page stays.'} onclick={() => (removing = true)}>Remove Page</button>
		{/if}
	</div>
	<div class="tools">
		{#if onpopout}
			<button class="btn ghost sm" data-testid="{prefix}-page-popout" onclick={() => (blocked = !onpopout(page.id))} title="Open this page in a window of its own, to place on another monitor. Stop and Abort are in every window.">
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="1.5" y="3.5" width="9" height="9" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M7 1.5h5.5V7M12.5 1.5L7 7" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
				New Window
			</button>
		{/if}
		<label class="rate">
			<span class="eyebrow">Updates</span>
			<select class="field num" data-testid="{prefix}-page-rate" value={String(views.layout.rate)} onchange={(e) => views.setRate(Number(e.currentTarget.value))}>
				{#each set.rates as r (r)}<option value={String(r)}>{r} per s</option>{/each}
			</select>
		</label>
	</div>
	{#if end}<div class="end">{@render end()}</div>{/if}
	{#if blocked}<p class="blocked" role="alert">The browser did not open the window. Allow pop-up windows for this page, then try again.</p>{/if}
	{#if views.error}<p class="blocked" role="alert" data-testid="{prefix}-display-error">{views.error}</p>{/if}
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
</style>
