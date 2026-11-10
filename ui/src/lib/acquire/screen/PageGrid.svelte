<script lang="ts">
	/**
	 * The page on screen: a grid of equal cells filling the space it is given, never scrolling, with
	 * each panel on its rectangle of cells. Drag a panel by its title to move it, or by its corner to
	 * resize it: the landing place is shown snapped to the cells, with the other panels where they
	 * will go, and marked No Room when there is none; Escape puts everything back. One panel can fill
	 * the page.
	 */
	import type { Snippet } from 'svelte';
	import * as grid from './grid';
	import Panel from './Panel.svelte';
	import type { PlotPanel, ScreenState } from './screen.svelte';
	import type { CardKind, Cell, Panel as PanelT } from './types';

	interface Props {
		screen: ScreenState;
		card: Snippet<[CardKind]>;
		/** The test's channels, for a plot's channel choice. */
		channels: { index: number; name: string }[];
		/** What an empty plot says. */
		empty: (panel: PlotPanel) => string;
		/** Live data: ranges held steady across frames. */
		live: boolean;
	}
	let { screen, card, channels, empty, live }: Props = $props();

	const page = $derived(screen.page);
	const plots = $derived(page.panels.filter((p) => p.kind === 'plot').length);
	const maximized = $derived(screen.maximized && page.panels.some((p) => p.id === screen.maximized) ? screen.maximized : null);

	let el = $state<HTMLElement | null>(null);

	// The page's shape, for arranging it evenly.
	$effect(() => {
		if (!el || typeof ResizeObserver === 'undefined') return;
		const ro = new ResizeObserver(([e]) => {
			const { width, height } = e.contentRect;
			if (width > 0 && height > 0) screen.aspect = width / height;
		});
		ro.observe(el);
		return () => ro.disconnect();
	});

	interface Drag {
		id: string;
		mode: 'move' | 'resize';
		pointer: number;
		x0: number;
		y0: number;
		/** Where in the panel it was taken, CSS pixels from its top-left corner. */
		grabX: number;
		grabY: number;
		/** Past the few pixels that tell a drag from a click. */
		active: boolean;
		dx: number;
		dy: number;
		/** The cells under the pointer. */
		target: Cell | null;
		/** The page as it will be, or null when there is no room. */
		preview: PanelT[] | null;
	}
	let drag = $state<Drag | null>(null);

	function metrics() {
		const r = (el as HTMLElement).getBoundingClientRect();
		const style = getComputedStyle(el as HTMLElement);
		const gx = parseFloat(style.columnGap) || 0;
		const gy = parseFloat(style.rowGap) || 0;
		return { r, gx, gy, px: (r.width - gx * (page.cols - 1)) / page.cols + gx, py: (r.height - gy * (page.rows - 1)) / page.rows + gy };
	}

	function start(e: PointerEvent, panel: PanelT, mode: 'move' | 'resize') {
		if (e.button !== 0 || !el || maximized) return;
		const t = e.target as HTMLElement;
		if (mode === 'move' && t.closest('button') && !t.closest('.grip')) return;
		const m = metrics();
		drag = {
			id: panel.id,
			mode,
			pointer: e.pointerId,
			x0: e.clientX,
			y0: e.clientY,
			grabX: e.clientX - (m.r.left + panel.at.x * m.px),
			grabY: e.clientY - (m.r.top + panel.at.y * m.py),
			active: false,
			dx: 0,
			dy: 0,
			target: null,
			preview: null
		};
		if (mode === 'resize') e.preventDefault();
		window.addEventListener('pointermove', onmove);
		window.addEventListener('pointerup', onup);
		window.addEventListener('pointercancel', cancel);
		window.addEventListener('keydown', onkey, true);
	}

	function onmove(e: PointerEvent) {
		const d = drag;
		if (!d || e.pointerId !== d.pointer || !el) return;
		const dx = e.clientX - d.x0;
		const dy = e.clientY - d.y0;
		if (!d.active && Math.hypot(dx, dy) < 5) return;
		const panel = page.panels.find((p) => p.id === d.id);
		if (!panel) return cancel();
		d.active = true;
		d.dx = dx;
		d.dy = dy;
		const m = metrics();
		const { cols, rows } = page;
		let target: Cell;
		if (d.mode === 'move') {
			const x = Math.round((e.clientX - d.grabX - m.r.left) / m.px);
			const y = Math.round((e.clientY - d.grabY - m.r.top) / m.py);
			target = grid.clampCell({ ...panel.at, x, y }, cols, rows);
		} else {
			const w = Math.round((e.clientX - (m.r.left + panel.at.x * m.px) + m.gx) / m.px);
			const h = Math.round((e.clientY - (m.r.top + panel.at.y * m.py) + m.gy) / m.py);
			target = grid.clampCell({ x: panel.at.x, y: panel.at.y, w: Math.min(w, cols - panel.at.x), h: Math.min(h, rows - panel.at.y) }, cols, rows);
		}
		if (d.target && grid.sameCell(target, d.target)) return;
		d.target = target;
		const snap = $state.snapshot(page.panels) as PanelT[];
		d.preview = d.mode === 'move' ? grid.moveTo(snap, d.id, target.x, target.y, cols, rows) : grid.resize(snap, d.id, target.w, target.h, cols, rows);
	}

	function onup(e: PointerEvent) {
		const d = drag;
		if (!d || e.pointerId !== d.pointer) return;
		if (d.active) {
			// The click that ends a drag is not a click on what it was dropped on.
			const swallow = (ev: Event) => {
				ev.stopPropagation();
				ev.preventDefault();
			};
			window.addEventListener('click', swallow, { capture: true, once: true });
			setTimeout(() => window.removeEventListener('click', swallow, { capture: true }), 0);
			if (d.preview && d.target) {
				if (d.mode === 'move') screen.movePanel(d.id, d.target.x, d.target.y);
				else screen.resizePanel(d.id, d.target.w, d.target.h);
			}
		}
		end();
	}

	function onkey(e: KeyboardEvent) {
		if (e.key === 'Escape' && drag) {
			e.preventDefault();
			e.stopPropagation();
			end();
		}
	}

	function cancel() {
		end();
	}

	function end() {
		drag = null;
		window.removeEventListener('pointermove', onmove);
		window.removeEventListener('pointerup', onup);
		window.removeEventListener('pointercancel', cancel);
		window.removeEventListener('keydown', onkey, true);
	}

	$effect(() => end);

	const FULL = $derived({ x: 0, y: 0, w: page.cols, h: page.rows });

	/** Where a panel is drawn: filling the page, at its landing place during a drag, or its own cell. */
	function shownAt(p: PanelT): Cell {
		if (maximized === p.id) return FULL;
		const d = drag;
		if (!d?.active || !d.preview || p.id === d.id) return p.at;
		return d.preview.find((q) => q.id === p.id)?.at ?? p.at;
	}

	const ghost = $derived.by(() => {
		const d = drag;
		if (!d?.active || !d.target) return null;
		const at = d.preview?.find((q) => q.id === d.id)?.at ?? d.target;
		return { at, room: d.preview !== null };
	});
</script>

<div
	class="grid"
	class:dragging={drag?.active}
	bind:this={el}
	style:--cols={page.cols}
	style:--rows={page.rows}
	data-testid="page-view"
	data-page={page.id}
	data-plots={plots}
	data-grid="{page.cols}x{page.rows}"
>
	{#if drag?.active}
		{#each { length: page.cols * page.rows } as _, i (i)}
			<div class="cell" style:grid-column={(i % page.cols) + 1} style:grid-row={Math.floor(i / page.cols) + 1} aria-hidden="true"></div>
		{/each}
	{/if}
	{#each page.panels as p (p.id)}
		<Panel
			{screen}
			panel={p}
			at={shownAt(p)}
			maximized={maximized === p.id}
			hidden={maximized !== null && maximized !== p.id}
			lifted={drag?.active && drag.mode === 'move' && drag.id === p.id ? { dx: drag.dx, dy: drag.dy } : null}
			{card}
			{channels}
			empty={p.kind === 'plot' ? empty(p) : ''}
			{live}
			onstart={(e, mode) => start(e, p, mode)}
		/>
	{/each}
	{#if ghost}
		<div class="ghost" class:none={!ghost.room} style:grid-column="{ghost.at.x + 1} / span {ghost.at.w}" style:grid-row="{ghost.at.y + 1} / span {ghost.at.h}" data-testid="drop-place" aria-hidden="true">
			{#if !ghost.room}<span>No Room</span>{/if}
		</div>
	{/if}
	{#if page.panels.length === 0}
		<p class="empty" data-testid="page-empty">No plots on this page.</p>
	{/if}
</div>

<style>
	.grid {
		position: relative;
		display: grid;
		flex: 1;
		grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
		grid-template-rows: repeat(var(--rows), minmax(0, 1fr));
		gap: 6px;
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}

	.grid.dragging {
		cursor: grabbing;
		user-select: none;
	}

	/* The cells, faintly, while a panel is dragged. */
	.cell {
		border: 1px dashed var(--line-1);
		border-radius: 2px;
		pointer-events: none;
	}

	.ghost {
		z-index: 4;
		display: grid;
		place-items: center;
		background: var(--blue-weak);
		border: 2px dashed var(--blue-2);
		border-radius: var(--radius-md);
		pointer-events: none;
	}

	/* No room: a dotted outline and the words, not a colour alone. */
	.ghost.none {
		background: var(--amber-weak);
		border-style: dotted;
		border-color: var(--amber-2);
	}

	.ghost span {
		padding: 2px 8px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-1);
		background: var(--bg-2);
		border-radius: var(--radius-sm);
	}

	.empty {
		grid-column: 1 / -1;
		grid-row: 1 / -1;
		place-self: center;
		font-size: var(--text-sm);
		color: var(--text-3);
	}
</style>
