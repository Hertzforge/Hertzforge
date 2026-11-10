<script lang="ts">
	/**
	 * The control dock: the screen's cards (the run's verbs, its level, its numbers, its log), on
	 * the right by default, at the bottom, or folded to a thin rail. Drag its edge to size it. Each
	 * card folds to its title; drag a card by its title to reorder (its menu moves it a place from
	 * the keyboard), hide it, or place it on the page as a panel. The dock's menu shows and hides
	 * the cards and moves the dock.
	 */
	import type { Snippet } from 'svelte';
	import { cardWords, DOCK_SIZE } from './layout';
	import MenuItem from './MenuItem.svelte';
	import Popover from './Popover.svelte';
	import type { ScreenState } from './screen.svelte';
	import type { CardKind } from './types';

	interface Props {
		screen: ScreenState;
		card: Snippet<[CardKind]>;
		/** Cards never hidden (the run's verbs): in the dock or on a page, always somewhere. */
		fixed?: CardKind[];
	}
	let { screen, card, fixed = ['run'] }: Props = $props();

	const dock = $derived(screen.dock);
	const side = $derived(dock.side === 'collapsed' ? 'collapsed' : dock.side);
	const shown = $derived(dock.cards.filter((c) => c.shown));
	const bounds = $derived(DOCK_SIZE[dock.restore]);

	// ---- the edge ---------------------------------------------------------------------------
	let draft = $state<number | null>(null);
	const size = $derived(draft ?? dock.size);

	function edgeDown(e: PointerEvent) {
		if (e.button !== 0) return;
		e.preventDefault();
		const el = e.currentTarget as HTMLElement;
		el.setPointerCapture(e.pointerId);
		const from = dock.side === 'bottom' ? e.clientY : e.clientX;
		const start = dock.size;
		const move = (ev: PointerEvent) => {
			const by = from - (dock.side === 'bottom' ? ev.clientY : ev.clientX);
			draft = Math.min(bounds.max, Math.max(bounds.min, start + by));
		};
		const up = () => {
			if (draft !== null) screen.setDockSize(draft);
			draft = null;
			el.removeEventListener('pointermove', move);
			el.removeEventListener('pointerup', up);
			el.removeEventListener('pointercancel', up);
		};
		el.addEventListener('pointermove', move);
		el.addEventListener('pointerup', up);
		el.addEventListener('pointercancel', up);
	}

	function edgeKey(e: KeyboardEvent) {
		const grow: Record<string, number> = dock.side === 'bottom' ? { ArrowUp: 16, ArrowDown: -16 } : { ArrowLeft: 16, ArrowRight: -16 };
		const by = grow[e.key];
		if (by === undefined) return;
		e.preventDefault();
		screen.setDockSize(dock.size + (e.shiftKey ? by * 4 : by));
	}

	// ---- reordering -------------------------------------------------------------------------
	let cardsEl = $state<HTMLElement | null>(null);
	let reorder = $state<{ kind: CardKind; pointer: number; x0: number; y0: number; active: boolean; index: number } | null>(null);

	function grab(e: PointerEvent, kind: CardKind) {
		if (e.button !== 0 || (e.target as HTMLElement).closest('.more')) return;
		reorder = { kind, pointer: e.pointerId, x0: e.clientX, y0: e.clientY, active: false, index: shown.findIndex((c) => c.kind === kind) };
		window.addEventListener('pointermove', drag);
		window.addEventListener('pointerup', drop);
		window.addEventListener('pointercancel', stopReorder);
		window.addEventListener('keydown', escape, true);
	}

	function drag(e: PointerEvent) {
		const r = reorder;
		if (!r || e.pointerId !== r.pointer || !cardsEl) return;
		if (!r.active && Math.hypot(e.clientX - r.x0, e.clientY - r.y0) < 5) return;
		r.active = true;
		const across = dock.side === 'bottom';
		const at = across ? e.clientX : e.clientY;
		const els = [...cardsEl.querySelectorAll<HTMLElement>(':scope > section')];
		let index = els.length;
		for (let i = 0; i < els.length; i++) {
			const b = els[i].getBoundingClientRect();
			if (at < (across ? b.left + b.width / 2 : b.top + b.height / 2)) {
				index = i;
				break;
			}
		}
		r.index = index;
	}

	function drop(e: PointerEvent) {
		const r = reorder;
		if (!r || e.pointerId !== r.pointer) return;
		if (r.active) {
			const swallow = (ev: Event) => {
				ev.stopPropagation();
				ev.preventDefault();
			};
			window.addEventListener('click', swallow, { capture: true, once: true });
			setTimeout(() => window.removeEventListener('click', swallow, { capture: true }), 0);
			const from = shown.findIndex((c) => c.kind === r.kind);
			const to = r.index > from ? r.index - 1 : r.index;
			moveTo(r.kind, to);
		}
		stopReorder();
	}

	function escape(e: KeyboardEvent) {
		if (e.key === 'Escape' && reorder) {
			e.preventDefault();
			e.stopPropagation();
			stopReorder();
		}
	}

	function stopReorder() {
		reorder = null;
		window.removeEventListener('pointermove', drag);
		window.removeEventListener('pointerup', drop);
		window.removeEventListener('pointercancel', stopReorder);
		window.removeEventListener('keydown', escape, true);
	}

	$effect(() => stopReorder);

	/** Move a card to place `to` among the cards the dock shows. */
	function moveTo(kind: CardKind, to: number) {
		const target = shown[Math.max(0, Math.min(shown.length - 1, to))];
		if (!target) return;
		screen.moveCard(kind, dock.cards.findIndex((c) => c.kind === target.kind));
	}

	// ---- menus ------------------------------------------------------------------------------
	let dockMenu = $state(false);
	let dockBtn = $state<HTMLElement | null>(null);
	let cardMenu = $state<CardKind | null>(null);
	const moreBtns: Record<string, HTMLElement | null> = $state({});

	function placeOnPage(kind: CardKind) {
		cardMenu = null;
		screen.cardToPage(kind);
	}

	function openFromRail(kind: CardKind) {
		screen.setDockSide(dock.restore);
		screen.setCardOpen(kind, true);
	}

	const before = $derived(dock.side === 'bottom' ? 'Left' : 'Up');
	const after = $derived(dock.side === 'bottom' ? 'Right' : 'Down');
</script>

{#if side === 'collapsed'}
	<aside class="rail" aria-label="Controls" data-testid="dock" data-side="collapsed">
		<button class="btn ghost icon sm" data-testid="dock-open" aria-label="Open the controls" title="Open the controls" onclick={() => screen.setDockSide(dock.restore)}>
			<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
		</button>
		{#each shown as c (c.kind)}
			<button class="railcard" data-testid="rail-{c.kind}" title="Open {cardWords(c.kind)}" onclick={() => openFromRail(c.kind)}>{cardWords(c.kind)}</button>
		{/each}
	</aside>
{:else}
	<aside class="dock {side}" style:--size="{size}px" aria-label="Controls" data-testid="dock" data-side={side}>
		<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (a window splitter: focusable, and the arrow keys size it) -->
		<div
			class="edge"
			role="separator"
			tabindex="0"
			aria-label="Size the controls"
			aria-orientation={side === 'bottom' ? 'horizontal' : 'vertical'}
			aria-valuenow={size}
			aria-valuemin={bounds.min}
			aria-valuemax={bounds.max}
			title="Drag to size"
			onpointerdown={edgeDown}
			onkeydown={edgeKey}
		></div>
		<div class="bar">
			<span class="eyebrow">Controls</span>
			<button class="btn ghost icon sm" bind:this={dockBtn} data-testid="dock-menu" aria-haspopup="menu" aria-expanded={dockMenu} aria-label="Controls menu" title="Cards and where the controls sit" onclick={() => (dockMenu = !dockMenu)}>
				<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><g fill="currentColor"><circle cx="3" cy="7" r="1.3" /><circle cx="7" cy="7" r="1.3" /><circle cx="11" cy="7" r="1.3" /></g></svg>
			</button>
			<button class="btn ghost icon sm" data-testid="dock-fold" aria-label="Fold the controls to a rail" title="Fold to a rail" onclick={() => screen.setDockSide('collapsed')}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
					{#if side === 'bottom'}<path d="M2.5 4.5L6 8l3.5-3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />{:else}<path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />{/if}
				</svg>
			</button>
		</div>
		<div class="cards" bind:this={cardsEl} class:sorting={reorder?.active}>
			{#each shown as c, i (c.kind)}
				<section class="card" class:open={c.open} class:moving={reorder?.active && reorder.kind === c.kind} class:insert={reorder?.active && reorder.index === i} data-testid="dock-card-{c.kind}" data-card={c.kind} aria-label={cardWords(c.kind)}>
					<!-- svelte-ignore a11y_no_static_element_interactions (the title is a pointer handle; the card's menu moves it from the keyboard) -->
					<header onpointerdown={(e) => grab(e, c.kind)}>
						<button class="fold" aria-expanded={c.open} data-testid="dock-fold-{c.kind}" title={c.open ? 'Fold' : 'Unfold'} onclick={() => screen.setCardOpen(c.kind, !c.open)}>
							<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d={c.open ? 'M2.5 4.5L6 8l3.5-3.5' : 'M4.5 2.5L8 6l-3.5 3.5'} fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
							<h3>{cardWords(c.kind)}</h3>
						</button>
						<button class="btn ghost icon sm more" bind:this={moreBtns[c.kind]} aria-haspopup="menu" aria-expanded={cardMenu === c.kind} aria-label="{cardWords(c.kind)} menu" data-testid="dock-card-menu-{c.kind}" onclick={() => (cardMenu = cardMenu === c.kind ? null : c.kind)}>
							<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><g fill="currentColor"><circle cx="3" cy="7" r="1.2" /><circle cx="7" cy="7" r="1.2" /><circle cx="11" cy="7" r="1.2" /></g></svg>
						</button>
					</header>
					{#if c.open}<div class="cbody">{@render card(c.kind)}</div>{/if}
				</section>
			{/each}
			{#if reorder?.active && reorder.index >= shown.length}<div class="endmark" aria-hidden="true"></div>{/if}
		</div>
	</aside>
{/if}

{#if dockMenu}
	<Popover anchor={dockBtn} label="Controls menu" menu align="end" testid="dock-menu-pop" onclose={() => (dockMenu = false)}>
		<div class="ws-mh">Cards</div>
		{#each dock.cards as c (c.kind)}
			{@const place = screen.cardPlace(c.kind)}
			{@const locked = fixed.includes(c.kind) && c.shown}
			<MenuItem
				kind="check"
				label={place ? `${cardWords(c.kind)} (on ${place.page.name})` : cardWords(c.kind)}
				checked={c.shown}
				disabled={locked}
				reason="Always in reach."
				testid="dock-show-{c.kind}"
				onclick={() => screen.showCard(c.kind, !c.shown)}
			/>
		{/each}
		<hr class="ws-sep" />
		<div class="ws-mh">Controls</div>
		<MenuItem kind="radio" label="On the Right" checked={dock.side === 'right'} testid="dock-right" onclick={() => ((dockMenu = false), screen.setDockSide('right'))} />
		<MenuItem kind="radio" label="At the Bottom" checked={dock.side === 'bottom'} testid="dock-bottom" onclick={() => ((dockMenu = false), screen.setDockSide('bottom'))} />
		<MenuItem label="Fold to a Rail" testid="dock-collapse" onclick={() => ((dockMenu = false), screen.setDockSide('collapsed'))} />
	</Popover>
{/if}

{#if cardMenu}
	{@const kind = cardMenu}
	{@const i = shown.findIndex((c) => c.kind === kind)}
	<Popover anchor={moreBtns[kind] ?? null} label="{cardWords(kind)} menu" menu align="end" testid="dock-card-menu-pop" onclose={() => (cardMenu = null)}>
		<MenuItem label="Move {before}" disabled={i <= 0} onclick={() => moveTo(kind, i - 1)} />
		<MenuItem label="Move {after}" disabled={i < 0 || i >= shown.length - 1} onclick={() => moveTo(kind, i + 1)} />
		<MenuItem label="Place on the Page" testid="dock-place-{kind}" disabled={screen.page.panels.length >= 16} reason="This page is full." onclick={() => placeOnPage(kind)} />
		{#if !fixed.includes(kind)}
			<MenuItem label="Hide" testid="dock-hide-{kind}" onclick={() => ((cardMenu = null), screen.showCard(kind, false))} />
		{/if}
	</Popover>
{/if}

<style>
	.dock {
		position: relative;
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
	}

	.dock.right {
		width: var(--size);
		padding-left: 6px;
	}

	.dock.bottom {
		height: var(--size);
		padding-top: 6px;
	}

	.edge {
		position: absolute;
		z-index: 3;
		touch-action: none;
	}

	.right .edge {
		top: 0;
		bottom: 0;
		left: 0;
		width: 6px;
		cursor: col-resize;
	}

	.bottom .edge {
		top: 0;
		left: 0;
		right: 0;
		height: 6px;
		cursor: row-resize;
	}

	.edge:hover,
	.edge:focus-visible {
		background: var(--blue-line);
		outline: none;
	}

	.bar {
		display: flex;
		align-items: center;
		gap: 2px;
		height: 24px;
		padding-left: 4px;
	}

	.bar .eyebrow {
		flex: 1;
	}

	.cards {
		display: flex;
		flex: 1;
		flex-direction: column;
		gap: 6px;
		min-height: 0;
		overflow-y: auto;
		padding-bottom: 2px;
	}

	.bottom .cards {
		flex-direction: row;
		align-items: stretch;
		overflow-x: auto;
		overflow-y: hidden;
	}

	.card {
		flex: none;
		display: flex;
		flex-direction: column;
		min-height: 0;
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.bottom .card {
		width: 300px;
	}

	.bottom .card.open {
		overflow: hidden;
	}

	.card.moving {
		opacity: 0.5;
	}

	/* Where the dragged card will land: a line before this card. */
	.card.insert {
		box-shadow: 0 -3px 0 0 var(--blue-2);
	}

	.bottom .card.insert {
		box-shadow: -3px 0 0 0 var(--blue-2);
	}

	.endmark {
		flex: none;
		height: 3px;
		background: var(--blue-2);
		border-radius: 2px;
	}

	.bottom .endmark {
		width: 3px;
		height: auto;
	}

	header {
		display: flex;
		align-items: center;
		gap: 2px;
		padding-right: 2px;
		cursor: grab;
		user-select: none;
		touch-action: none;
	}

	.sorting,
	.sorting header {
		cursor: grabbing;
	}

	.fold {
		display: flex;
		flex: 1;
		align-items: center;
		gap: 6px;
		min-width: 0;
		height: 28px;
		padding: 0 6px 0 8px;
		color: var(--text-3);
		background: transparent;
		border: 0;
		border-radius: var(--radius-md);
		cursor: inherit;
	}

	.fold:hover {
		color: var(--text-1);
	}

	.fold h3 {
		overflow: hidden;
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--text-2);
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.more {
		opacity: 0;
	}

	.card:hover .more,
	.card:focus-within .more,
	.more[aria-expanded='true'] {
		opacity: 1;
	}

	@media (hover: none) {
		.more {
			opacity: 1;
		}
	}

	.cbody {
		min-height: 0;
		padding: 0 10px 10px;
	}

	.bottom .cbody {
		flex: 1;
		overflow-y: auto;
	}

	.rail {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 4px;
		width: 30px;
		padding: 2px 0 0 4px;
	}

	.railcard {
		padding: 8px 2px;
		font-size: var(--text-xs);
		color: var(--text-2);
		writing-mode: vertical-rl;
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
	}

	.railcard:hover {
		color: var(--text-1);
		background: var(--bg-2);
		border-color: var(--line-1);
	}
</style>
