<script lang="ts">
	/**
	 * One panel on a page: a title line, and under it a plot or a card. The title is the handle to
	 * drag it by; double-click it to fill the page, and again to restore. Its tools show on hover or
	 * focus: the plot's options, fill the page, and remove (a card goes back to the dock). The grip
	 * at the left opens the panel's menu, which moves and resizes it a cell at a time; with the grip
	 * focused, the arrow keys move it and Shift with an arrow resizes it.
	 */
	import type { Snippet } from 'svelte';
	import PlotBody from '../plots/PlotBody.svelte';
	import PlotOptions from '../plots/PlotOptions.svelte';
	import { customBody, type PlotBodies, type PlotExtras } from './bodies';
	import { cardWords } from './layout';
	import MenuItem from './MenuItem.svelte';
	import Popover from './Popover.svelte';
	import type { ScreenState } from './screen.svelte';
	import type { CardKind, Cell, Panel } from './types';

	interface Props {
		screen: ScreenState;
		panel: Panel;
		/** Where it is drawn, in cells. */
		at: Cell;
		/** It fills the page. */
		maximized: boolean;
		/** Another panel fills the page: this one is not drawn. */
		hidden: boolean;
		/** Being dragged: the pointer's travel, CSS pixels. */
		lifted: { dx: number; dy: number } | null;
		card: Snippet<[CardKind]>;
		/** The test's channels, for a plot's channel choice. */
		channels: { index: number; name: string }[];
		/** What the plot says while it has nothing to draw. */
		empty: string;
		/** Live data: ranges held steady across frames. */
		live: boolean;
		/** A drag starts on the title (move) or the corner (resize). */
		onstart: (e: PointerEvent, mode: 'move' | 'resize') => void;
		/** A test type's own bodies for some of its plot types, drawn in place of the plain plot body. */
		bodies?: PlotBodies;
	}
	let { screen, panel, at, maximized, hidden, lifted, card, channels, empty, live, onstart, bodies }: Props = $props();

	const custom = $derived(customBody(bodies, panel));

	const title = $derived(panel.kind === 'plot' ? screen.titleOf(panel) : cardWords(panel.card));
	const type = $derived(panel.kind === 'plot' ? screen.typeOf(panel.plot.quantity) : null);
	const optionsOpen = $derived(screen.options === panel.id);
	let menuOpen = $state(false);
	let optBtn = $state<HTMLElement | null>(null);
	let grip = $state<HTMLElement | null>(null);
	let body = $state<{ copyImage(): Promise<void>; saveImage(name: string): Promise<void>; saveCsv(name: string): Promise<void>; traces(): { name: string; role: string }[] } | null>(null);

	function onexport(what: 'copy' | 'image' | 'csv') {
		if (!body) return;
		if (what === 'copy') void body.copyImage();
		else if (what === 'image') void body.saveImage(title);
		else void body.saveCsv(title);
	}

	function ondblclick(e: MouseEvent) {
		if ((e.target as HTMLElement).closest('button')) return;
		screen.toggleMaximize(panel.id);
	}

	/** The grip's keys: arrows move a cell, Shift and an arrow resizes. */
	function onGripKey(e: KeyboardEvent) {
		const dir: Record<string, [number, number]> = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
		const d = dir[e.key];
		if (!d || e.altKey || e.metaKey || e.ctrlKey) return;
		e.preventDefault();
		if (e.shiftKey) screen.growPanel(panel.id, d[0], d[1]);
		else screen.stepPanel(panel.id, d[0], d[1]);
		queueMicrotask(() => grip?.focus());
	}

	function act(fn: () => unknown) {
		fn();
		menuOpen = false;
		queueMicrotask(() => grip?.focus());
	}

	const moves = [
		{ label: 'Move Left', d: [-1, 0], keys: 'Left' },
		{ label: 'Move Right', d: [1, 0], keys: 'Right' },
		{ label: 'Move Up', d: [0, -1], keys: 'Up' },
		{ label: 'Move Down', d: [0, 1], keys: 'Down' }
	] as const;
	const sizes = [
		{ label: 'Wider', d: [1, 0], keys: 'Shift Right' },
		{ label: 'Narrower', d: [-1, 0], keys: 'Shift Left' },
		{ label: 'Taller', d: [0, 1], keys: 'Shift Down' },
		{ label: 'Shorter', d: [0, -1], keys: 'Shift Up' }
	] as const;
</script>

{#snippet draw(extra: PlotExtras)}
	{#if panel.kind === 'plot'}
		<PlotBody bind:this={body} views={screen.views(panel.id)} channels={screen.frame(panel.id)?.channels ?? null} linkKey={screen.page.linkTime ? `${screen.hooks.testType}:${screen.page.id}` : null} {type} spec={panel.plot} {empty} {live} onspec={(spec) => screen.updatePlot(panel.id, spec)} marker={extra.marker ?? null} guides={extra.guides ?? []} onrange={extra.onrange} />
	{/if}
{/snippet}

<section
	class="panel"
	class:maximized
	class:hidden
	class:lifted={!!lifted}
	data-testid={panel.kind === 'plot' ? `plot-${panel.id}` : `card-panel-${panel.card}`}
	data-panel={panel.id}
	data-plot={panel.kind === 'plot' ? panel.id : undefined}
	data-quantity={panel.kind === 'plot' ? panel.plot.quantity : undefined}
	data-card={panel.kind === 'card' ? panel.card : undefined}
	aria-label={title}
	inert={hidden || undefined}
	style:grid-column="{at.x + 1} / span {at.w}"
	style:grid-row="{at.y + 1} / span {at.h}"
	style:transform={lifted ? `translate(${lifted.dx}px, ${lifted.dy}px)` : undefined}
>
	<!-- svelte-ignore a11y_no_static_element_interactions (the title is a pointer handle; the grip is its keyboard way) -->
	<header onpointerdown={(e) => onstart(e, 'move')} {ondblclick}>
		<button
			class="grip"
			bind:this={grip}
			aria-haspopup="menu"
			aria-expanded={menuOpen}
			aria-label="Arrange {title}"
			aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown Shift+ArrowLeft Shift+ArrowRight Shift+ArrowUp Shift+ArrowDown"
			title="Drag to move. Click for the menu; arrow keys move, Shift and an arrow resizes."
			data-testid="panel-menu"
			onclick={() => (menuOpen = !menuOpen)}
			onkeydown={onGripKey}
		>
			<svg viewBox="0 0 10 14" width="8" height="12" aria-hidden="true"><g fill="currentColor"><circle cx="3" cy="3" r="1.1" /><circle cx="7" cy="3" r="1.1" /><circle cx="3" cy="7" r="1.1" /><circle cx="7" cy="7" r="1.1" /><circle cx="3" cy="11" r="1.1" /><circle cx="7" cy="11" r="1.1" /></g></svg>
		</button>
		<h3 {title}>{title}</h3>
		<div class="tools" class:on={optionsOpen || menuOpen}>
			{#if panel.kind === 'plot'}
				<button class="btn ghost icon sm" bind:this={optBtn} data-testid="plot-options" aria-haspopup="dialog" aria-expanded={optionsOpen} aria-label="Options for {title}" title="Options" onclick={() => screen.openOptions(optionsOpen ? null : panel.id)}>
					<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><path d="M2 4h10M2 10h10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /><circle cx="5" cy="4" r="1.7" fill="var(--bg-1)" stroke="currentColor" stroke-width="1.3" /><circle cx="9.5" cy="10" r="1.7" fill="var(--bg-1)" stroke="currentColor" stroke-width="1.3" /></svg>
				</button>
			{/if}
			<button class="btn ghost icon sm" data-testid="panel-maximize" aria-pressed={maximized} aria-label={maximized ? `Restore ${title}` : `Fill the page with ${title}`} title={maximized ? 'Restore' : 'Fill the page'} onclick={() => screen.toggleMaximize(panel.id)}>
				{#if maximized}
					<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M5.5 1.5v4h-4M8.5 12.5v-4h4M5.5 5.5L1.5 1.5M8.5 8.5l4 4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
				{:else}
					<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M8.5 1.5h4v4M5.5 12.5h-4v-4M12.5 1.5L8 6M1.5 12.5L6 8" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
				{/if}
			</button>
			{#if panel.kind === 'plot'}
				<button class="btn ghost icon sm" data-testid="plot-remove" aria-label="Remove {title}" title="Remove this plot" onclick={() => screen.removePanel(panel.id)}>
					<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
				</button>
			{:else}
				<button class="btn ghost icon sm" data-testid="card-to-dock" aria-label="Put {title} back in the dock" title="Back to the dock" onclick={() => screen.cardToDock(panel.id)}>
					<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M1.5 7h8M6.5 4l3 3-3 3M12.5 1.5v11" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</button>
			{/if}
		</div>
	</header>

	<div class="body">
		{#if panel.kind === 'plot' && custom}
			{@render custom(panel, draw)}
		{:else if panel.kind === 'plot'}
			{@render draw({})}
		{:else}
			<div class="cardbody">{@render card(panel.card)}</div>
		{/if}
	</div>

	{#if !maximized}
		<!-- svelte-ignore a11y_no_static_element_interactions (a pointer handle; the panel menu resizes from the keyboard) -->
		<div class="corner" title="Drag to resize" onpointerdown={(e) => onstart(e, 'resize')}>
			<svg viewBox="0 0 10 10" width="9" height="9" aria-hidden="true"><path d="M9 3L3 9M9 6.5L6.5 9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" /></svg>
		</div>
	{/if}
</section>

{#if optionsOpen && panel.kind === 'plot'}
	<Popover anchor={optBtn} label="Options for {title}" align="end" testid="plot-options-pop" onclose={() => screen.openOptions(null)}>
		<PlotOptions spec={panel.plot} types={screen.hooks.types} {channels} traces={body?.traces() ?? []} onchange={(spec) => screen.updatePlot(panel.id, spec)} {onexport} onclose={() => screen.openOptions(null)} />
	</Popover>
{/if}

{#if menuOpen}
	<Popover anchor={grip} label="Arrange {title}" menu testid="panel-menu-pop" onclose={() => (menuOpen = false)}>
		{#each moves as m (m.label)}
			<MenuItem label={m.label} keys={m.keys} disabled={maximized || !screen.canStep(panel.id, m.d[0], m.d[1])} onclick={() => act(() => screen.stepPanel(panel.id, m.d[0], m.d[1]))} />
		{/each}
		<hr class="ws-sep" />
		{#each sizes as s (s.label)}
			<MenuItem label={s.label} keys={s.keys} disabled={maximized || !screen.canGrow(panel.id, s.d[0], s.d[1])} onclick={() => act(() => screen.growPanel(panel.id, s.d[0], s.d[1]))} />
		{/each}
		<hr class="ws-sep" />
		<MenuItem label={maximized ? 'Restore' : 'Fill the Page'} onclick={() => act(() => screen.toggleMaximize(panel.id))} />
		{#if panel.kind === 'plot'}
			<MenuItem label="Options" onclick={() => ((menuOpen = false), screen.openOptions(panel.id))} />
			<MenuItem label="Remove Plot" danger onclick={() => ((menuOpen = false), screen.removePanel(panel.id))} />
		{:else}
			<MenuItem label="Back to the Dock" onclick={() => ((menuOpen = false), screen.cardToDock(panel.id))} />
		{/if}
	</Popover>
{/if}

<style>
	.panel {
		position: relative;
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.panel.maximized {
		z-index: 2;
	}

	.panel.hidden {
		visibility: hidden;
	}

	.panel.lifted {
		z-index: 5;
		opacity: 0.85;
		box-shadow: var(--shadow-pop);
		pointer-events: none;
	}

	header {
		display: flex;
		align-items: center;
		gap: 2px;
		height: 26px;
		padding: 0 2px 0 0;
		cursor: grab;
		user-select: none;
		touch-action: none;
	}

	.grip {
		display: inline-grid;
		place-items: center;
		width: 18px;
		height: 22px;
		padding: 0;
		color: var(--text-4);
		background: transparent;
		border: 0;
		border-radius: var(--radius-sm);
		opacity: 0;
		cursor: grab;
	}

	.grip:hover,
	.grip:focus-visible,
	.grip[aria-expanded='true'] {
		color: var(--text-1);
		background: var(--bg-3);
	}

	h3 {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--text-2);
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.tools {
		display: flex;
		align-items: center;
		gap: 1px;
		flex-shrink: 0;
		opacity: 0;
	}

	.tools .btn.icon.sm {
		width: 22px;
		height: 22px;
	}

	.panel:hover .tools,
	.panel:focus-within .tools,
	.tools.on,
	.panel:hover .grip,
	.panel:focus-within .grip {
		opacity: 1;
	}

	@media (hover: none) {
		.tools,
		.grip {
			opacity: 1;
		}
	}

	.body {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-height: 0;
		padding: 0 4px 4px;
		overflow: hidden;
	}

	.body > :global(*) {
		flex: 1;
		min-height: 0;
	}

	.cardbody {
		overflow: auto;
		padding: 2px 6px 6px;
	}

	.corner {
		position: absolute;
		right: 0;
		bottom: 0;
		display: grid;
		place-items: center;
		width: 14px;
		height: 14px;
		color: var(--text-4);
		cursor: nwse-resize;
		opacity: 0;
		touch-action: none;
	}

	.panel:hover .corner {
		opacity: 1;
	}

	.corner:hover {
		color: var(--text-1);
	}
</style>
