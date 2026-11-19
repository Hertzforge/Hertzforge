<script lang="ts">
	/**
	 * The page tabs under the plots, as a spreadsheet's sheets: one tab a page (double-click to
	 * rename), `+` for a new page, and at the right `+ Plot`, Arrange Evenly and the page's menu
	 * (rename, remove, grid, update rate, linked time axes, a window of its own for another monitor).
	 */
	import AddPlotMenu from '../plots/AddPlotMenu.svelte';
	import { GRID_SIZES, RATES } from './layout';
	import MenuItem from './MenuItem.svelte';
	import Popover from './Popover.svelte';
	import type { ScreenState } from './screen.svelte';

	interface Props {
		screen: ScreenState;
		/** Open a page in a window of its own; false when the browser refused to open it. */
		onpopout?: (pageId: string) => boolean;
	}
	let { screen, onpopout }: Props = $props();

	const page = $derived(screen.page);
	const hasTime = $derived(screen.hooks.types.some((t) => t.time));
	let nameText = $state('');
	let nameInput = $state<HTMLInputElement | null>(null);
	let addBtn = $state<HTMLElement | null>(null);
	let menuBtn = $state<HTMLElement | null>(null);
	let adding = $state(false);
	let menuOpen = $state(false);
	let removing = $state(false);
	let blocked = $state(false);
	let tablist = $state<HTMLElement | null>(null);

	function startRename(id: string) {
		const p = screen.pages.find((x) => x.id === id);
		if (!p) return;
		screen.selectPage(id);
		nameText = p.name;
		screen.renaming = id;
		queueMicrotask(() => nameInput?.select());
	}

	function commitRename() {
		const id = screen.renaming;
		if (!id) return;
		if (screen.renamePage(id, nameText)) screen.renaming = null;
	}

	function onNameKey(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			commitRename();
			focusTab(page.id);
		} else if (e.key === 'Escape') {
			e.preventDefault();
			screen.renaming = null;
			focusTab(page.id);
		}
	}

	function focusTab(id: string) {
		queueMicrotask(() => tablist?.querySelector<HTMLElement>(`[data-page="${id}"]`)?.focus());
	}

	function addPage() {
		const id = screen.addPage();
		if (id) startRename(id);
	}

	function onTabKey(e: KeyboardEvent) {
		const ids = screen.pages.map((p) => p.id);
		const i = ids.indexOf(page.id);
		let next = -1;
		if (e.key === 'ArrowRight') next = (i + 1) % ids.length;
		else if (e.key === 'ArrowLeft') next = (i - 1 + ids.length) % ids.length;
		else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = ids.length - 1;
		else if (e.key === 'F2') {
			e.preventDefault();
			startRename(page.id);
			return;
		}
		if (next < 0) return;
		e.preventDefault();
		screen.selectPage(ids[next]);
		focusTab(ids[next]);
	}

	function popout() {
		menuOpen = false;
		blocked = onpopout ? !onpopout(page.id) : false;
	}

	function closeMenu() {
		menuOpen = false;
		removing = false;
	}
</script>

<div class="tabs" role="toolbar" aria-label="Pages" data-testid="page-bar">
	<div class="sheets" role="tablist" aria-label="Pages" data-testid="page-tabs" bind:this={tablist}>
		{#each screen.pages as p (p.id)}
			{#if screen.renaming === p.id}
				<input class="field name" aria-label="Page name" data-testid="page-name" bind:this={nameInput} bind:value={nameText} maxlength="40" onkeydown={onNameKey} onblur={commitRename} />
			{:else}
				<button
					role="tab"
					class="sheet"
					aria-selected={p.id === page.id}
					tabindex={p.id === page.id ? 0 : -1}
					data-page={p.id}
					data-testid="page-tab-{p.id}"
					title="Double-click to rename"
					onclick={() => screen.selectPage(p.id)}
					ondblclick={() => startRename(p.id)}
					onkeydown={onTabKey}>{p.name}</button
				>
			{/if}
		{/each}
		<button class="btn ghost icon sm" data-testid="page-add" aria-label="Add a page" title={screen.canAddPage ? 'Add a page' : 'At most 12 pages.'} disabled={!screen.canAddPage} onclick={addPage}>
			<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
		</button>
	</div>

	{#if screen.error}
		<p class="note" role="alert" data-testid="display-error">{screen.error}</p>
	{:else if blocked}
		<p class="note" role="alert">The browser did not open the window. Allow pop-up windows for this page, then try again.</p>
	{/if}

	<div class="tools">
		<button class="btn ghost sm" bind:this={addBtn} data-testid="plot-add" aria-haspopup="menu" aria-expanded={adding} disabled={!screen.canAddPlot} title={screen.canAddPlot ? 'Add a plot to this page' : 'This page is full.'} onclick={() => (adding = !adding)}>
			<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
			Plot
		</button>
		<button class="btn ghost sm" data-testid="page-arrange" disabled={page.panels.length === 0} title="Tile this page's panels in equal rows and columns" onclick={() => screen.arrangeEvenly()}>
			<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="1.5" y="1.5" width="4.6" height="4.6" rx="0.8" fill="none" stroke="currentColor" stroke-width="1.3" /><rect x="7.9" y="1.5" width="4.6" height="4.6" rx="0.8" fill="none" stroke="currentColor" stroke-width="1.3" /><rect x="1.5" y="7.9" width="4.6" height="4.6" rx="0.8" fill="none" stroke="currentColor" stroke-width="1.3" /><rect x="7.9" y="7.9" width="4.6" height="4.6" rx="0.8" fill="none" stroke="currentColor" stroke-width="1.3" /></svg>
			Arrange Evenly
		</button>
		<button class="btn ghost icon sm" bind:this={menuBtn} data-testid="page-menu" aria-haspopup="menu" aria-expanded={menuOpen} aria-label="Page menu" title="Page menu" onclick={() => (menuOpen ? closeMenu() : (menuOpen = true))}>
			<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><g fill="currentColor"><circle cx="3" cy="7" r="1.3" /><circle cx="7" cy="7" r="1.3" /><circle cx="11" cy="7" r="1.3" /></g></svg>
		</button>
	</div>
</div>

{#if adding}
	<Popover anchor={addBtn} label="Add a plot" align="end" up testid="plot-add-pop" onclose={() => (adding = false)}>
		<AddPlotMenu
			types={screen.hooks.types}
			onpick={(t) => {
				adding = false;
				screen.addPlot(t);
			}}
			onclose={() => (adding = false)}
		/>
	</Popover>
{/if}

{#if menuOpen}
	<Popover anchor={menuBtn} label="Page menu" menu align="end" up testid="page-menu-pop" onclose={closeMenu}>
		{#if removing}
			<div class="ws-confirm" role="alert" data-testid="page-remove-confirm">
				<span>Remove "{page.name}" and its plots?</span>
				<span class="ws-row">
					<button class="btn sm" role="menuitem" data-testid="page-remove-yes" onclick={() => (closeMenu(), screen.removePage(page.id))}>Remove</button>
					<button class="btn ghost sm" role="menuitem" onclick={() => (removing = false)}>Keep</button>
				</span>
			</div>
		{:else}
			<MenuItem label="Rename" keys="F2" testid="page-rename" onclick={() => (closeMenu(), startRename(page.id))} />
			<MenuItem label="Move Left" disabled={screen.pages[0]?.id === page.id} onclick={() => screen.movePage(page.id, -1)} />
			<MenuItem label="Move Right" disabled={screen.pages[screen.pages.length - 1]?.id === page.id} onclick={() => screen.movePage(page.id, 1)} />
			<MenuItem label="Remove Page" danger testid="page-remove" disabled={!screen.canRemovePage} reason="The last page stays." onclick={() => (removing = true)} />
			<hr class="ws-sep" />
			<div class="ws-mh">Grid</div>
			{#each GRID_SIZES as n (n)}
				<MenuItem kind="radio" label="{n} x {n}" checked={page.cols === n && page.rows === n} testid="page-grid-{n}" onclick={() => screen.setGrid(n)} />
			{/each}
			<hr class="ws-sep" />
			<div class="ws-mh">Updates</div>
			{#each RATES as r (r)}
				<MenuItem kind="radio" label="{r} per s" checked={screen.layout.rate === r} testid="page-rate-{r}" onclick={() => screen.setRate(r)} />
			{/each}
			{#if hasTime}
				<hr class="ws-sep" />
				<MenuItem kind="check" label="Link Time Axes" checked={page.linkTime} testid="page-link-time" onclick={() => screen.setLinkTime(!page.linkTime)} />
			{/if}
			{#if onpopout}
				<hr class="ws-sep" />
				<MenuItem label="Open in a New Window" testid="page-popout" onclick={popout} />
			{/if}
		{/if}
	</Popover>
{/if}

<style>
	.tabs {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		min-height: 30px;
		padding: 2px 0 0;
	}

	.sheets {
		display: flex;
		flex: 1;
		align-items: center;
		gap: 2px;
		min-width: 0;
		overflow-x: auto;
		scrollbar-width: none;
	}

	.sheet {
		position: relative;
		flex: none;
		height: 26px;
		max-width: 22ch;
		padding: 0 12px;
		overflow: hidden;
		font-size: var(--text-sm);
		color: var(--text-3);
		text-overflow: ellipsis;
		white-space: nowrap;
		background: transparent;
		border: 1px solid transparent;
		border-radius: 0 0 var(--radius-md) var(--radius-md);
	}

	.sheet:hover {
		color: var(--text-1);
		background: var(--bg-2);
	}

	/* The page on show: raised to the plots above it, its name bold and underlined. */
	.sheet[aria-selected='true'] {
		font-weight: var(--weight-strong);
		color: var(--text-1);
		background: var(--bg-1);
		border-color: var(--line-1);
		border-top-color: transparent;
	}

	.sheet[aria-selected='true']::after {
		content: '';
		position: absolute;
		left: 10px;
		right: 10px;
		bottom: 3px;
		height: 2px;
		background: var(--blue-2);
		border-radius: 1px;
	}

	.name {
		flex: none;
		width: 16ch;
		height: 26px;
	}

	.tools {
		display: flex;
		flex: none;
		align-items: center;
		gap: 2px;
	}

	.note {
		flex: none;
		max-width: 40%;
		overflow: hidden;
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--amber-1);
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
